use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::Hook;

/// Grace period between asking a child tree to stop and killing what remains.
const KILL_GRACE: Duration = Duration::from_millis(2_000);

/// Output kept from a hook, enough to explain itself without flooding.
const MAX_CAPTURED_CHARS: usize = 4_000;

/// Enough bytes to preserve the character cap even for four-byte UTF-8 input.
const MAX_CAPTURED_BYTES: usize = MAX_CAPTURED_CHARS * 4;

/// The exit status a hook uses to refuse a block-capable event.
pub const REFUSAL_EXIT_CODE: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookProcessOutcome {
    Exited(i32),
    TimedOut,
    Cancelled,
    SpawnFailed,
    WaitFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookOutputDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookOutput {
    pub decision: Option<HookOutputDecision>,
    pub reason: Option<String>,
    pub context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookOutputEnvelope {
    hook_output: HookOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookStdout {
    Empty,
    Plain(String),
    Structured(HookOutput),
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRunResult {
    pub hook: Hook,
    /// Retained for callers that only need success versus failure.
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub outcome: HookProcessOutcome,
    pub output: HookStdout,
    pub duration_ms: u64,
}

/// What a block-capable hook decided about one action.
/// Most hooks abstain: a formatter or logger observes an action and has no
/// opinion about whether it should happen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HookVerdict {
    #[default]
    Abstain,
    Allow {
        reason: Option<String>,
    },
    Ask {
        reason: Option<String>,
    },
    Deny {
        reason: String,
    },
}

/// A verdict together with the declaration and runs that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookVerdictOutcome {
    pub verdict: HookVerdict,
    /// The deciding hook. Absent when every hook abstained.
    pub hook: Option<Hook>,
    pub results: Vec<HookRunResult>,
    pub cancelled: bool,
}

fn truncate(text: &str) -> String {
    if text.encode_utf16().count() <= MAX_CAPTURED_CHARS {
        return text.to_string();
    }
    let mut output = String::new();
    let mut units = 0;
    for character in text.chars() {
        let width = character.len_utf16();
        if units + width + 1 > MAX_CAPTURED_CHARS {
            break;
        }
        output.push(character);
        units += width;
    }
    output.push('…');
    output
}

fn parse_stdout(text: &str) -> HookStdout {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return HookStdout::Empty;
    }
    if !trimmed.starts_with('{') {
        return HookStdout::Plain(trimmed.to_string());
    }
    match serde_json::from_str::<HookOutputEnvelope>(trimmed) {
        Ok(envelope) => HookStdout::Structured(HookOutput {
            decision: envelope.hook_output.decision,
            reason: non_empty(envelope.hook_output.reason),
            context: non_empty(envelope.hook_output.context).map(|text| truncate(&text)),
        }),
        Err(error) => HookStdout::Invalid(format!(
            "invalid structured output; expected {{\"hook_output\": {{...}}}}: {error}"
        )),
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

/// The platform shell is fixed by the hook contract rather than inherited from
/// the user's interactive shell.
fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let shell = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut spawned = Command::new(shell);
        spawned.args(["/d", "/s", "/c", command]);
        spawned
    }
    #[cfg(not(windows))]
    {
        let mut spawned = Command::new("/bin/sh");
        spawned.args(["-c", command]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            spawned.as_std_mut().process_group(0);
        }
        spawned
    }
}

#[cfg(unix)]
fn signal_process_tree(pid: u32, signal: libc::c_int) {
    // A hook is placed in a process group whose id is its initial child's pid.
    // Negative ids address that owned group instead of an unrelated sibling.
    unsafe {
        libc::kill(-(pid as libc::pid_t), signal);
    }
}

struct ProcessTree {
    pid: Option<u32>,
    #[cfg(windows)]
    job: WindowsJob,
}

impl ProcessTree {
    fn attach(child: &tokio::process::Child) -> Result<Self, String> {
        #[cfg(windows)]
        {
            return Ok(Self {
                pid: child.id(),
                job: WindowsJob::attach(child)?,
            });
        }
        #[cfg(not(windows))]
        {
            Ok(Self { pid: child.id() })
        }
    }

    fn terminate_soft(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            signal_process_tree(pid, libc::SIGTERM);
        }
    }

    fn terminate_hard(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            signal_process_tree(pid, libc::SIGKILL);
        }
        #[cfg(windows)]
        self.job.terminate();
    }

    fn preserve_descendants(&mut self) {
        #[cfg(windows)]
        self.job.preserve_descendants();
    }
}

#[cfg(windows)]
struct WindowsJob {
    handle: isize,
    terminate_on_drop: bool,
}

#[cfg(windows)]
impl WindowsJob {
    fn attach(child: &tokio::process::Child) -> Result<Self, String> {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};

        let process = child
            .raw_handle()
            .ok_or_else(|| "hook process exited before its job was assigned".to_string())?
            as HANDLE;
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err("could not create a job for the hook process".to_string());
            }
            let assigned = AssignProcessToJobObject(handle, process) != 0;
            if !assigned {
                CloseHandle(handle);
                return Err("could not assign the hook process to its job".to_string());
            }
            Ok(Self {
                handle: handle as isize,
                terminate_on_drop: true,
            })
        }
    }

    fn terminate(&self) {
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        unsafe {
            TerminateJobObject(self.handle as HANDLE, 1);
        }
    }

    fn preserve_descendants(&mut self) {
        self.terminate_on_drop = false;
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

        unsafe {
            if self.terminate_on_drop {
                use windows_sys::Win32::System::JobObjects::TerminateJobObject;
                TerminateJobObject(self.handle as HANDLE, 1);
            }
            CloseHandle(self.handle as HANDLE);
        }
    }
}

async fn stop_child_tree(child: &mut tokio::process::Child, tree: &ProcessTree) -> Option<i32> {
    tree.terminate_soft();

    match tokio::time::timeout(KILL_GRACE, child.wait()).await {
        Ok(status) => {
            // The shell may have exited before a descendant. No process in the
            // owned tree can be allowed to retain the pipes.
            tree.terminate_hard();
            status.ok().and_then(|status| status.code())
        }
        Err(_) => {
            tree.terminate_hard();
            let _ = child.kill().await;
            child.wait().await.ok().and_then(|status| status.code())
        }
    }
}

async fn readers_finished(
    stdout: &tokio::task::JoinHandle<String>,
    stderr: &tokio::task::JoinHandle<String>,
) -> bool {
    tokio::time::timeout(KILL_GRACE, async {
        while !stdout.is_finished() || !stderr.is_finished() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}

async fn settle_readers_after_exit(
    tree: &ProcessTree,
    stdout: &tokio::task::JoinHandle<String>,
    stderr: &tokio::task::JoinHandle<String>,
) {
    if readers_finished(stdout, stderr).await {
        return;
    }

    // A detached process with redirected streams is independent work. Only a
    // descendant retaining the hook's capture pipes can keep this run open.
    tree.terminate_soft();
    if !readers_finished(stdout, stderr).await {
        tree.terminate_hard();
    }
}

/// Runs one hook to completion. Execution failures are values because callers
/// are already inside an agent action and must report rather than unwind them.
pub async fn run_hook(
    hook: &Hook,
    payload: &Value,
    signal: Option<&CancellationToken>,
) -> HookRunResult {
    let started_at = Instant::now();
    let finish =
        |outcome: HookProcessOutcome, stdout: String, stderr: String, started_at: Instant| {
            let stdout = truncate(&stdout);
            let stderr = truncate(&stderr);
            HookRunResult {
                hook: hook.clone(),
                ok: matches!(outcome, HookProcessOutcome::Exited(0)),
                exit_code: match outcome {
                    HookProcessOutcome::Exited(code) => Some(code),
                    _ => None,
                },
                timed_out: matches!(outcome, HookProcessOutcome::TimedOut),
                cancelled: matches!(outcome, HookProcessOutcome::Cancelled),
                outcome,
                output: parse_stdout(&stdout),
                stdout,
                stderr,
                duration_ms: started_at.elapsed().as_millis() as u64,
            }
        };

    let mut command = shell_command(&hook.command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return finish(
                HookProcessOutcome::SpawnFailed,
                String::new(),
                error.to_string(),
                started_at,
            );
        }
    };
    let mut tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill().await;
            return finish(
                HookProcessOutcome::SpawnFailed,
                String::new(),
                error,
                started_at,
            );
        }
    };

    // Writing on a separate task ensures a hook that ignores stdin cannot hold
    // the runner on a full pipe.
    if let Some(mut stdin) = child.stdin.take() {
        let line = format!("{payload}\n");
        tokio::spawn(async move {
            let _ = stdin.write_all(line.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }

    let read_stdout = tokio::spawn(read_to_string(child.stdout.take()));
    let read_stderr = tokio::spawn(read_to_string(child.stderr.take()));
    let deadline = Duration::from_millis(hook.timeout_ms);

    enum WaitResult {
        Exited(std::io::Result<std::process::ExitStatus>),
        TimedOut,
        Cancelled,
    }

    let stop = async {
        match signal {
            Some(token) => token.cancelled().await,
            None => std::future::pending().await,
        }
    };
    let waited = tokio::select! {
        status = child.wait() => WaitResult::Exited(status),
        () = tokio::time::sleep(deadline) => WaitResult::TimedOut,
        () = stop => WaitResult::Cancelled,
    };

    let exited_normally = matches!(
        &waited,
        WaitResult::Exited(Ok(status)) if status.code().is_some()
    );
    let outcome = match waited {
        WaitResult::Exited(Ok(status)) => match status.code() {
            Some(code) => HookProcessOutcome::Exited(code),
            None => {
                tree.terminate_hard();
                HookProcessOutcome::WaitFailed
            }
        },
        WaitResult::Exited(Err(_)) => {
            tree.terminate_hard();
            HookProcessOutcome::WaitFailed
        }
        WaitResult::TimedOut => {
            let _ = stop_child_tree(&mut child, &tree).await;
            HookProcessOutcome::TimedOut
        }
        WaitResult::Cancelled => {
            let _ = stop_child_tree(&mut child, &tree).await;
            HookProcessOutcome::Cancelled
        }
    };

    if exited_normally {
        settle_readers_after_exit(&tree, &read_stdout, &read_stderr).await;
        tree.preserve_descendants();
    }

    let stdout = read_stdout.await.unwrap_or_default();
    let stderr = read_stderr.await.unwrap_or_default();
    finish(outcome, stdout, stderr, started_at)
}

async fn read_to_string<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut kept = Vec::new();
    let mut chunk = [0_u8; 8_192];
    loop {
        let read = match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        if kept.len() < MAX_CAPTURED_BYTES {
            let remaining = MAX_CAPTURED_BYTES - kept.len();
            kept.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// Runs hooks in declaration order, one after another. Hooks may touch the same
/// workspace, so parallel execution would make their effects nondeterministic.
pub async fn run_hooks(
    hooks: &[&Hook],
    payload: &Value,
    signal: Option<&CancellationToken>,
) -> Vec<HookRunResult> {
    let mut results = Vec::new();
    for hook in hooks {
        results.push(run_hook(hook, payload, signal).await);
    }
    results
}

pub fn hook_context(result: &HookRunResult) -> Option<String> {
    if !result.ok {
        return None;
    }
    match &result.output {
        HookStdout::Plain(text) => non_empty(Some(text.clone())),
        HookStdout::Structured(output) => output.context.clone(),
        HookStdout::Empty | HookStdout::Invalid(_) => None,
    }
}

/// Reads the authored decision and applies the failure rules of the event.
pub fn read_verdict(result: &HookRunResult) -> HookVerdict {
    if result.cancelled {
        return HookVerdict::Abstain;
    }
    if let HookStdout::Invalid(error) = &result.output {
        if result.hook.event.is_blocking() {
            return HookVerdict::Deny {
                reason: format!("Hook produced {error} (hook: {})", result.hook.command),
            };
        }
        return HookVerdict::Abstain;
    }
    if let HookStdout::Structured(output) = &result.output
        && let Some(decision) = output.decision
    {
        let reason = output.reason.clone();
        return match decision {
            HookOutputDecision::Allow => HookVerdict::Allow { reason },
            HookOutputDecision::Ask
                if result.hook.event == super::events::HookEvent::UserPromptSubmit =>
            {
                HookVerdict::Deny {
                    reason: "UserPromptSubmit hooks cannot request confirmation".to_string(),
                }
            }
            HookOutputDecision::Ask => HookVerdict::Ask { reason },
            HookOutputDecision::Deny => HookVerdict::Deny {
                reason: reason.unwrap_or_else(|| describe_refusal(result)),
            },
        };
    }
    if result.hook.event.is_blocking()
        && (result.timed_out || result.exit_code == Some(REFUSAL_EXIT_CODE))
    {
        return HookVerdict::Deny {
            reason: describe_refusal(result),
        };
    }
    HookVerdict::Abstain
}

/// Whether a run failed in a way that needs a user-visible diagnostic.
pub fn is_hook_fault(result: &HookRunResult) -> bool {
    if result.cancelled {
        return false;
    }
    if matches!(result.output, HookStdout::Invalid(_)) {
        return true;
    }
    if result.ok {
        return false;
    }
    if result.hook.event.is_blocking() && result.exit_code == Some(REFUSAL_EXIT_CODE) {
        return false;
    }
    true
}

/// Runs one block-capable event and combines authored decisions. The first
/// denial stops the sequence; otherwise asking is stricter than allowing.
pub async fn decide_hooks(
    hooks: &[&Hook],
    payload: &Value,
    signal: Option<&CancellationToken>,
) -> HookVerdictOutcome {
    let mut results = Vec::new();
    let mut verdict = HookVerdict::Abstain;
    let mut decided_by = None;

    for hook in hooks {
        if !hook.event.is_blocking() {
            continue;
        }
        let result = run_hook(hook, payload, signal).await;
        let stated = read_verdict(&result);
        let cancelled = result.cancelled;
        results.push(result);
        if cancelled {
            return HookVerdictOutcome {
                verdict: HookVerdict::Abstain,
                hook: None,
                results,
                cancelled: true,
            };
        }
        if matches!(stated, HookVerdict::Deny { .. }) {
            return HookVerdictOutcome {
                verdict: stated,
                hook: Some((*hook).clone()),
                results,
                cancelled: false,
            };
        }
        if matches!(stated, HookVerdict::Abstain) {
            continue;
        }
        if matches!(verdict, HookVerdict::Ask { .. }) {
            continue;
        }
        verdict = stated;
        decided_by = Some((*hook).clone());
    }

    HookVerdictOutcome {
        verdict,
        hook: decided_by,
        results,
        cancelled: false,
    }
}

/// The message shown for a refusal. Structured output and stderr take
/// precedence; plain stdout remains a useful reason for exit-code refusals.
pub fn describe_refusal(result: &HookRunResult) -> String {
    if let HookStdout::Structured(output) = &result.output
        && let Some(reason) = &output.reason
    {
        return reason.clone();
    }
    if let Some(written) = first_line(&result.stderr, "") {
        return format!("{written} (hook: {})", result.hook.command);
    }
    if matches!(&result.output, HookStdout::Plain(_))
        && let Some(written) = first_line(&result.stdout, "")
    {
        return format!("{written} (hook: {})", result.hook.command);
    }
    if result.timed_out {
        return format!(
            "Hook timed out after {}ms (hook: {})",
            result.hook.timeout_ms, result.hook.command
        );
    }
    if result.cancelled {
        return format!("Hook was cancelled (hook: {})", result.hook.command);
    }
    format!(
        "Hook exited with {} (hook: {})",
        result
            .exit_code
            .map_or_else(|| "no status".to_string(), |code| code.to_string()),
        result.hook.command
    )
}

fn first_line<'a>(stderr: &'a str, stdout: &'a str) -> Option<&'a str> {
    let text = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let first = text.split('\n').next().unwrap_or("").trim();
    (!first.is_empty()).then_some(first)
}
