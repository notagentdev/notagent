//! The child-process half of `packages/coding-agent/src/core/package-manager.ts`.
//!
//! Deviation (class 1): the TypeScript suite reaches into the manager and
//! replaces `runCommand`, `runCommandCapture` and `runCommandSync` with spies.
//! Rust has no such seam, so the three methods live behind [`CommandRunner`];
//! the manager holds one, production wires [`ProcessCommandRunner`], and the
//! ported suite wires a recording fake.

use std::collections::BTreeMap;
use std::process::Stdio;

use futures::future::BoxFuture;
use tokio::io::AsyncReadExt;

/// `{ cwd?, timeoutMs?, env? }` of `runCommandCapture`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOptions {
    pub cwd: Option<String>,
    pub timeout_ms: Option<u64>,
    pub env: BTreeMap<String, String>,
}

impl CommandOptions {
    pub fn with_cwd(cwd: impl Into<String>) -> Self {
        Self {
            cwd: Some(cwd.into()),
            ..Self::default()
        }
    }
}

/// The three spawn paths of the package manager.
pub trait CommandRunner: Send + Sync {
    /// `runCommand(command, args, options)` — inherits stdio, waits for exit.
    fn run(
        &self,
        command: String,
        args: Vec<String>,
        options: CommandOptions,
    ) -> BoxFuture<'_, Result<(), String>>;

    /// `runCommandCapture(command, args, options)` — captures stdout.
    fn run_capture(
        &self,
        command: String,
        args: Vec<String>,
        options: CommandOptions,
    ) -> BoxFuture<'_, Result<String, String>>;

    /// `runCommandSync(command, args)` — blocking, captures stdout or stderr.
    fn run_sync(&self, command: &str, args: &[String]) -> Result<String, String>;
}

/// `getEnv()`
///
/// On Linux an empty `process.env` means the environment was stripped; the
/// TypeScript then reads `/proc/self/environ` instead.
fn get_env() -> BTreeMap<String, String> {
    let env: BTreeMap<String, String> = std::env::vars().collect();
    if !cfg!(target_os = "linux") || !env.is_empty() {
        return env;
    }
    let Ok(data) = std::fs::read_to_string("/proc/self/environ") else {
        return env;
    };
    let mut parsed = BTreeMap::new();
    for entry in data.split('\0') {
        if let Some(index) = entry.find('=')
            && index > 0
        {
            parsed.insert(entry[..index].to_owned(), entry[index + 1..].to_owned());
        }
    }
    parsed
}

fn command_line(command: &str, args: &[String]) -> String {
    format!("{command} {}", args.join(" "))
}

/// The real spawner.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessCommandRunner;

impl ProcessCommandRunner {
    fn base_command(
        command: &str,
        args: &[String],
        options: &CommandOptions,
    ) -> tokio::process::Command {
        let mut child = tokio::process::Command::new(command);
        child.args(args);
        if let Some(cwd) = options.cwd.as_ref() {
            child.current_dir(cwd);
        }
        child.env_clear();
        child.envs(get_env());
        child.envs(options.env.clone());
        child
    }
}

impl CommandRunner for ProcessCommandRunner {
    fn run(
        &self,
        command: String,
        args: Vec<String>,
        options: CommandOptions,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let mut child = Self::base_command(&command, &args, &options);
            // `stdio: "inherit"` — the `isStdoutTakenOver()` branch that routes
            // stdout to stderr waits on C's `core/output-guard.ts` (request B-4).
            child
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            let status = child
                .spawn()
                .map_err(|error| error.to_string())?
                .wait()
                .await
                .map_err(|error| error.to_string())?;
            if status.success() {
                return Ok(());
            }
            let code = status
                .code()
                .map_or_else(|| "null".to_owned(), |code| code.to_string());
            Err(format!(
                "{} failed with code {code}",
                command_line(&command, &args)
            ))
        })
    }

    fn run_capture(
        &self,
        command: String,
        args: Vec<String>,
        options: CommandOptions,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            let mut child = Self::base_command(&command, &args, &options);
            child
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut spawned = child.spawn().map_err(|error| error.to_string())?;

            let mut stdout = String::new();
            let mut stderr = String::new();
            let mut stdout_pipe = spawned.stdout.take();
            let mut stderr_pipe = spawned.stderr.take();

            let collect = async {
                let stdout_read = async {
                    if let Some(pipe) = stdout_pipe.as_mut() {
                        let _ = pipe.read_to_string(&mut stdout).await;
                    }
                };
                let stderr_read = async {
                    if let Some(pipe) = stderr_pipe.as_mut() {
                        let _ = pipe.read_to_string(&mut stderr).await;
                    }
                };
                let status = async { spawned.wait().await };
                let (status, _, _) = futures::future::join3(status, stdout_read, stderr_read).await;
                status
            };

            let status = match options.timeout_ms {
                Some(timeout_ms) => {
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        collect,
                    )
                    .await
                    {
                        Ok(status) => status,
                        Err(_) => {
                            return Err(format!(
                                "{} timed out after {timeout_ms}ms",
                                command_line(&command, &args)
                            ));
                        }
                    }
                }
                None => collect.await,
            }
            .map_err(|error| error.to_string())?;

            if status.success() {
                return Ok(stdout.trim().to_owned());
            }
            let exit_status = match status.code() {
                Some(code) => format!("code {code}"),
                None => "signal unknown".to_owned(),
            };
            let output = if stderr.is_empty() { &stdout } else { &stderr };
            Err(format!(
                "{} failed with {exit_status}: {output}",
                command_line(&command, &args)
            ))
        })
    }

    fn run_sync(&self, command: &str, args: &[String]) -> Result<String, String> {
        let mut child = std::process::Command::new(command);
        child.args(args);
        child.stdin(Stdio::null());
        child.env_clear();
        child.envs(get_env());
        let result = child.output();
        let failure = |detail: String| {
            Err(format!(
                "Failed to run {}: {detail}",
                command_line(command, args)
            ))
        };
        match result {
            Err(error) => failure(error.to_string()),
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                if output.status.success() {
                    let value = if stdout.is_empty() { &stderr } else { &stdout };
                    Ok(value.trim().to_owned())
                } else {
                    failure(if stderr.is_empty() { stdout } else { stderr })
                }
            }
        }
    }
}
