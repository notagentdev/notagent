use std::cell::RefCell;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, ComponentRef, Container, Line, component_ref};
use notagent_tui::utils::truncate_to_width;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::bash_filter::{self, PreparedInvocation};
use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::tools::output_accumulator::{
    OutputAccumulator, OutputAccumulatorOptions, OutputSnapshot,
};
use crate::core::tools::render_utils::{get_text_output, invalid_arg_text, str_arg};
use crate::core::tools::tool_definition::{
    RenderFuture, SystemPromptContribution, ToolContext, ToolDefinition, ToolRenderContext,
    ToolRenderResult, ToolRenderResultOptions, tool_render_state, wrap_tool_definition,
};
use crate::core::tools::truncate::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncatedBy, format_size, truncation_from_details,
};
use crate::modes::interactive::components::keybinding_hints::key_hint;
use crate::modes::interactive::components::visual_truncate::truncate_to_visual_lines;
use crate::modes::interactive::theme::theme::{
    BlockStyle, Theme, ThemeColor, block_style, format_elapsed, format_elapsed_live,
    format_elapsed_precise, theme,
};
use crate::utils::shell::{
    CommandTransport, ShellConfig, get_shell_config, get_shell_env, kill_process_tree,
    terminate_process_tree, track_detached_child_pid, untrack_detached_child_pid,
};

/// The task machinery these live in is `core/tasks/`; the shell tool speaks
/// their language, so they are re-exported here where its API is read.
pub use crate::core::tasks::manager::RegisterTaskOptions;
pub use crate::core::tasks::shell_task::{ShellTaskOutputSink, ShellTaskSpec};
pub use crate::core::tasks::types::{ForegroundRelease, TaskStatus};

const MAX_TIMEOUT_MS: f64 = 2_147_483_647.0;
const MAX_TIMEOUT_SECONDS: f64 = MAX_TIMEOUT_MS / 1000.0;

/// Deadlines, foreground and background.
/// The foreground default is ten minutes (user decision 2026-08-18): builds
/// and test runs the conversation waits on routinely take that long, and
/// reaching the deadline moves the command to the background rather than
/// killing it. Where that behaviour is switched off, the description says the
/// command is killed instead. A background command is a build, a watcher or a
/// server, where a day is the outer bound of anything sane.
pub const DEFAULT_TIMEOUT_S: f64 = 10.0 * 60.0;
pub const MAX_TIMEOUT_S: f64 = 60.0 * 60.0;
pub const DEFAULT_BACKGROUND_TIMEOUT_S: f64 = 10.0 * 60.0;
pub const MAX_BACKGROUND_TIMEOUT_S: f64 = 24.0 * 60.0 * 60.0;

/// How long a command told to stop is given before it is killed.
/// Short on purpose: this is the escalation inside a single command, while the
/// task manager's own grace window covers work that has no process at all.
const ABORT_ESCALATION_MS: u64 = 2_000;

/// Re-armed on every chunk that arrives after the shell exited, so a detached
/// descendant still writing keeps us reading while a quiet inherited handle
const EXIT_STDIO_GRACE_MS: u64 = 100;

const BASH_UPDATE_THROTTLE_MS: u64 = 100;

pub const BASH_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Execute bash commands (ls, grep, find, etc.)",
        guidelines: &[
            "You can inspect NOTAGENT_* environment variables for current model and session details.",
        ],
    };

/// of a plain `Error` (`"aborted"`, `"timeout:<seconds>"`, anything else).
#[derive(Debug, Clone, PartialEq)]
pub enum BashExecError {
    Aborted,
    Timeout(f64),
    Other(String),
}

impl std::fmt::Display for BashExecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BashExecError::Aborted => formatter.write_str("aborted"),
            BashExecError::Timeout(seconds) => write!(formatter, "timeout:{seconds}"),
            BashExecError::Other(message) => formatter.write_str(message),
        }
    }
}

/// Receives raw output bytes as they arrive.
pub type BashOutputSink = Arc<dyn Fn(&[u8]) + Send + Sync>;
/// Reports the process id once the command is running.
pub type BashSpawnSink = Arc<dyn Fn(u32) + Send + Sync>;

#[derive(Clone, Default)]
pub struct BashExecOptions {
    pub on_data: Option<BashOutputSink>,
    pub signal: Option<CancellationToken>,
    pub timeout: Option<f64>,
    pub env: Option<BTreeMap<String, String>>,
    /// Background tasks need the pid to show what they started and to force a
    /// stop the polite one did not achieve. An implementation with no local
    /// process — an SSH backend, say — simply never calls it.
    pub on_spawn: Option<BashSpawnSink>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BashExecResult {
    pub exit_code: Option<i32>,
}

/// Pluggable operations for the bash tool.
/// Override these to delegate command execution to remote systems (for example
/// SSH).
pub trait BashOperations: Send + Sync {
    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a str,
        options: BashExecOptions,
    ) -> BoxFuture<'a, Result<BashExecResult, BashExecError>>;
}

fn resolve_timeout_ms(timeout: Option<f64>) -> Result<Option<f64>, BashExecError> {
    let Some(timeout) = timeout else {
        return Ok(None);
    };
    if !timeout.is_finite() || timeout <= 0.0 {
        return Err(BashExecError::Other(
            "Invalid timeout: must be a finite number of seconds".to_owned(),
        ));
    }
    let timeout_ms = timeout * 1000.0;
    if timeout_ms > MAX_TIMEOUT_MS {
        return Err(BashExecError::Other(format!(
            "Invalid timeout: maximum is {MAX_TIMEOUT_SECONDS} seconds"
        )));
    }
    Ok(Some(timeout_ms))
}

/// Bash operations using notagent's built-in local shell execution backend.
/// This is useful where a caller intercepts a command and still wants
/// notagent's standard local shell behaviour while wrapping or rewriting it.
pub struct LocalBashOperations {
    shell_path: Option<String>,
    /// Set only by [`testing::local_bash_operations_with_shell_config`].
    shell_config: Option<ShellConfig>,
}

pub fn create_local_bash_operations(shell_path: Option<String>) -> LocalBashOperations {
    LocalBashOperations {
        shell_path,
        shell_config: None,
    }
}

/// `vi.spyOn(shellModule, "getShellConfig")`, which has no Rust equivalent.
pub mod testing {
    use super::{LocalBashOperations, ShellConfig};

    /// Local operations that skip shell resolution and use `config` as it is.
    pub fn local_bash_operations_with_shell_config(config: ShellConfig) -> LocalBashOperations {
        LocalBashOperations {
            shell_path: None,
            shell_config: Some(config),
        }
    }
}

enum ReaderEvent {
    Chunk(Vec<u8>),
    StdoutEnd,
    StderrEnd,
}

impl BashOperations for LocalBashOperations {
    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a str,
        options: BashExecOptions,
    ) -> BoxFuture<'a, Result<BashExecResult, BashExecError>> {
        Box::pin(async move {
            let timeout_ms = resolve_timeout_ms(options.timeout)?;
            if matches!(&options.signal, Some(signal) if signal.is_cancelled()) {
                return Err(BashExecError::Aborted);
            }
            let shell_config = match &self.shell_config {
                Some(config) => config.clone(),
                None => {
                    get_shell_config(self.shell_path.as_deref()).map_err(BashExecError::Other)?
                }
            };
            if tokio::fs::metadata(cwd).await.is_err() {
                return Err(BashExecError::Other(format!(
                    "Working directory does not exist: {cwd}\nCannot execute bash commands."
                )));
            }

            let command_from_stdin =
                shell_config.command_transport == Some(CommandTransport::Stdin);
            let mut builder = tokio::process::Command::new(&shell_config.shell);
            builder.args(&shell_config.args);
            if !command_from_stdin {
                builder.arg(command);
            }
            builder
                .current_dir(cwd)
                .env_clear()
                .envs(options.env.clone().unwrap_or_else(get_shell_env))
                .stdin(if command_from_stdin {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(unix)]
            builder.process_group(0);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                builder.creation_flags(0x0800_0000);
            }

            let mut child = builder.spawn().map_err(|error| {
                BashExecError::Other(if error.kind() == std::io::ErrorKind::NotFound {
                    // Node rejects with `spawn <shell> ENOENT`; callers and the
                    format!("spawn {} ENOENT", shell_config.shell)
                } else {
                    error.to_string()
                })
            })?;

            if command_from_stdin && let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(command.as_bytes()).await;
                let _ = stdin.shutdown().await;
            }

            let pid = child.id();
            if let Some(pid) = pid {
                track_detached_child_pid(pid);
                if let Some(on_spawn) = &options.on_spawn {
                    on_spawn(pid);
                }
            }

            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<ReaderEvent>();
            spawn_reader(child.stdout.take(), sender.clone(), ReaderEvent::StdoutEnd);
            spawn_reader(child.stderr.take(), sender, ReaderEvent::StderrEnd);

            let mut stdout_ended = false;
            let mut stderr_ended = false;
            let mut readers_gone = false;
            let mut exited = false;
            let mut exit_code: Option<i32> = None;
            let mut timed_out = false;
            let mut abort_started = false;
            let mut timeout_deadline = timeout_ms
                .map(|timeout_ms| Instant::now() + Duration::from_secs_f64(timeout_ms / 1000.0));
            let mut escalation_deadline: Option<Instant> = None;
            let mut idle_deadline: Option<Instant> = None;
            let wait = child.wait();
            tokio::pin!(wait);

            loop {
                if exited && stdout_ended && stderr_ended {
                    break;
                }
                let signal_wait = async {
                    match &options.signal {
                        Some(signal) if !abort_started => signal.cancelled().await,
                        _ => std::future::pending().await,
                    }
                };
                let deadline_wait = |deadline: Option<Instant>| async move {
                    match deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                        None => std::future::pending().await,
                    }
                };

                tokio::select! {
                    biased;
                    event = receiver.recv(), if !readers_gone => match event {
                        Some(ReaderEvent::Chunk(chunk)) => {
                            if let Some(on_data) = &options.on_data {
                                on_data(&chunk);
                            }
                            // Output is still arriving after exit; defer
                            // finalizing so the tail is not cut off.
                            if exited {
                                idle_deadline =
                                    Some(Instant::now() + Duration::from_millis(EXIT_STDIO_GRACE_MS));
                            }
                        }
                        Some(ReaderEvent::StdoutEnd) => stdout_ended = true,
                        Some(ReaderEvent::StderrEnd) => stderr_ended = true,
                        // Both readers are gone: nothing more can arrive, and
                        // polling a closed channel again would starve the
                        // other branches.
                        None => {
                            readers_gone = true;
                            stdout_ended = true;
                            stderr_ended = true;
                        }
                    },
                    status = &mut wait, if !exited => {
                        exited = true;
                        exit_code = status.ok().and_then(|status| status.code());
                        idle_deadline =
                            Some(Instant::now() + Duration::from_millis(EXIT_STDIO_GRACE_MS));
                    },
                    () = deadline_wait(timeout_deadline), if timeout_deadline.is_some() => {
                        timeout_deadline = None;
                        timed_out = true;
                        if let Some(pid) = pid {
                            kill_process_tree(pid);
                        }
                    },
                    () = signal_wait => {
                        abort_started = true;
                        // Ask first, kill after. A command that handles
                        // termination gets to close what it opened; one that
                        // ignores it still dies on schedule.
                        if let Some(pid) = pid {
                            terminate_process_tree(pid);
                            escalation_deadline =
                                Some(Instant::now() + Duration::from_millis(ABORT_ESCALATION_MS));
                        }
                    },
                    () = deadline_wait(escalation_deadline), if escalation_deadline.is_some() => {
                        escalation_deadline = None;
                        if !exited && let Some(pid) = pid {
                            kill_process_tree(pid);
                        }
                    },
                    () = deadline_wait(idle_deadline), if idle_deadline.is_some() && exited => break,
                }
            }

            if let Some(pid) = pid {
                untrack_detached_child_pid(pid);
            }
            if matches!(&options.signal, Some(signal) if signal.is_cancelled()) {
                return Err(BashExecError::Aborted);
            }
            if timed_out {
                return Err(BashExecError::Timeout(options.timeout.unwrap_or_default()));
            }
            Ok(BashExecResult { exit_code })
        })
    }
}

fn spawn_reader<R>(
    reader: Option<R>,
    sender: tokio::sync::mpsc::UnboundedSender<ReaderEvent>,
    end: ReaderEvent,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let Some(mut reader) = reader else {
        let _ = sender.send(end);
        return;
    };
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buffer = vec![0u8; 8 * 1024];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if sender
                        .send(ReaderEvent::Chunk(buffer[..read].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
        let _ = sender.send(end);
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashSpawnContext {
    pub command: String,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
}

pub type BashSpawnHook = Arc<dyn Fn(BashSpawnContext) -> BashSpawnContext + Send + Sync>;

fn resolve_spawn_context(
    command: &str,
    cwd: &str,
    spawn_hook: Option<&BashSpawnHook>,
    expose_session_environment: bool,
    context: Option<&ToolContext>,
) -> BashSpawnContext {
    let mut env = get_shell_env();
    env.remove("NOTAGENT_SESSION_ID");
    env.remove("NOTAGENT_SESSION_FILE");
    env.remove("NOTAGENT_PROVIDER");
    env.remove("NOTAGENT_MODEL");
    env.remove("NOTAGENT_REASONING_LEVEL");
    if expose_session_environment && let Some(context) = context {
        if let Some(session_id) = &context.session_id {
            env.insert("NOTAGENT_SESSION_ID".to_owned(), session_id.clone());
        }
        if let Some(session_file) = &context.session_file {
            env.insert("NOTAGENT_SESSION_FILE".to_owned(), session_file.clone());
        }
        if let Some(model) = &context.model {
            env.insert("NOTAGENT_PROVIDER".to_owned(), model.provider.clone());
            env.insert("NOTAGENT_MODEL".to_owned(), model.id.clone());
        }
        if let Some(thinking_level) = &context.thinking_level {
            env.insert(
                "NOTAGENT_REASONING_LEVEL".to_owned(),
                thinking_level.clone(),
            );
        }
    }
    let base = BashSpawnContext {
        command: command.to_owned(),
        cwd: cwd.to_owned(),
        env,
    };
    match spawn_hook {
        Some(hook) => hook(base),
        None => base,
    }
}

/// The slice of `TaskInfo` the shell tool reads back after a foreground run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedTaskSnapshot {
    pub status: TaskStatus,
    pub stop_reason: Option<String>,
    /// `exitCode` of a shell task; `None` for any other kind, matching the
    pub exit_code: Option<i32>,
}

/// The part of `TaskManager` the shell tool uses. Implemented for the real
/// manager in plan task 10.
pub trait BashTaskManager: Send + Sync {
    fn register_shell_task(
        &self,
        task: ShellTaskSpec,
        options: RegisterTaskOptions,
    ) -> Result<String, String>;
    fn wait_for_foreground_release<'a>(
        &'a self,
        task_id: &'a str,
    ) -> BoxFuture<'a, Option<ForegroundRelease>>;
    fn get_task(&self, task_id: &str) -> Option<ManagedTaskSnapshot>;
}

/// Reads whether the bash filter is enabled, at call time, so
/// `/bash-filter on|off` applies to the next command rather than the next
pub type BashFilterGate = Arc<dyn Fn() -> bool + Send + Sync>;

/// What the shell tool needs in order to detach a command.
#[derive(Clone)]
pub struct BashToolSources {
    /// Where a detached command is registered. Absent inside a subagent.
    pub manager: Arc<dyn Fn() -> Option<Arc<dyn BashTaskManager>> + Send + Sync>,
    /// Whether this session may detach work at all.
    pub background_allowed: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Whether a foreground command that reaches its deadline is backgrounded.
    pub auto_background_on_timeout: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

#[derive(Default, Clone)]
pub struct BashToolOptions {
    /// Custom operations for command execution. Default: local shell.
    pub operations: Option<Arc<dyn BashOperations>>,
    /// Command prefix prepended to every command (for example shell setup).
    pub command_prefix: Option<String>,
    /// Optional explicit shell path from settings.
    pub shell_path: Option<String>,
    /// Expose current notagent session metadata as `NOTAGENT_*` environment
    /// variables. Default: true.
    pub expose_session_environment: Option<bool>,
    /// Hook to adjust command, cwd, or env before execution.
    pub spawn_hook: Option<BashSpawnHook>,
    /// Background-task wiring. Without it the tool runs commands directly.
    pub sources: Option<BashToolSources>,
    /// Whether the bash filter compacts command output; absent means off.
    pub bash_filter: Option<BashFilterGate>,
}

fn bash_base_properties() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert(
        "command".to_owned(),
        json!({ "type": "string", "description": "Bash command to execute" }),
    );
    properties.insert(
        "timeout".to_owned(),
        json!({
            "type": "number",
            "description": format!(
                "Timeout in seconds. Foreground: default {DEFAULT_TIMEOUT_S}, maximum {MAX_TIMEOUT_S}. Background: default {DEFAULT_BACKGROUND_TIMEOUT_S}, maximum {MAX_BACKGROUND_TIMEOUT_S}."
            ),
        }),
    );
    properties
}

fn bash_schema(with_background: bool) -> Value {
    let mut properties = bash_base_properties();
    if with_background {
        properties.insert(
            "run_in_background".to_owned(),
            json!({
                "type": "boolean",
                "description": "Start the command as a background task and return its id instead of waiting for it.",
            }),
        );
        properties.insert(
            "description".to_owned(),
            json!({
                "type": "string",
                "description": "Short label for the task. Required when run_in_background is set.",
            }),
        );
        properties.insert(
            "disable_timeout".to_owned(),
            json!({
                "type": "boolean",
                "description": "Run without any deadline. Only meaningful together with run_in_background.",
            }),
        );
    }
    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": ["command"],
    })
}

fn description_without_background() -> String {
    [
        base_description(),
        String::new(),
        format!(
            "Set `timeout` in seconds for anything slow; the default is {DEFAULT_TIMEOUT_S}s and the maximum is {MAX_TIMEOUT_S}s. A command that reaches its deadline is stopped."
        ),
        "Background execution is not available here. Do not set `run_in_background`.".to_owned(),
    ]
    .join("\n")
}

fn base_description() -> String {
    format!(
        "Execute a bash command in the current working directory. Returns stdout and stderr. Output is truncated to last {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). If truncated, full output is saved to a temp file.",
        DEFAULT_MAX_BYTES / 1024
    )
}

fn description_with_background(auto_background: bool) -> String {
    [
        base_description(),
        String::new(),
        format!(
            "Foreground commands default to a {DEFAULT_TIMEOUT_S}s deadline and allow up to {MAX_TIMEOUT_S}s."
        ),
        if auto_background {
            "A foreground command that reaches its deadline is moved to the background rather than stopped, and reports itself when it finishes.".to_owned()
        } else {
            "A foreground command that reaches its deadline is stopped.".to_owned()
        },
        String::new(),
        format!(
            "With `run_in_background` the command starts as a task and this returns its id immediately; `description` is required. Background commands default to {DEFAULT_BACKGROUND_TIMEOUT_S}s and allow up to {MAX_BACKGROUND_TIMEOUT_S}s, or none at all with `disable_timeout`."
        ),
        "Use it for builds, test runs, watchers and servers — anything you want to keep working alongside. Its result arrives on its own; do not follow it with task_output to wait, and use task_stop only to cancel.".to_owned(),
    ]
    .join("\n")
}

/// Clamps a requested deadline to the ceiling for the mode it runs in.
fn normalize_timeout_seconds(
    requested: Option<f64>,
    is_background: bool,
) -> Result<f64, ToolExecutionError> {
    let fallback = if is_background {
        DEFAULT_BACKGROUND_TIMEOUT_S
    } else {
        DEFAULT_TIMEOUT_S
    };
    let ceiling = if is_background {
        MAX_BACKGROUND_TIMEOUT_S
    } else {
        MAX_TIMEOUT_S
    };
    let Some(requested) = requested else {
        return Ok(fallback);
    };
    if !requested.is_finite() || requested <= 0.0 {
        return Err(ToolExecutionError::new(
            "Invalid timeout: must be a finite number of seconds",
        ));
    }
    Ok(requested.min(ceiling))
}

/// What the model is told about a command it can no longer wait for.
fn render_detached_result(task_id: &str, description: &str, reason: ForegroundRelease) -> String {
    let opening = match reason {
        ForegroundRelease::TimeoutDetached => {
            "The command reached its foreground deadline and was moved to the background."
        }
        ForegroundRelease::Detached => "The command was moved to the background.",
        ForegroundRelease::Terminal => "Started in the background.",
    };
    [
        format!("task_id: {task_id}"),
        "status: running".to_owned(),
        format!("description: {description}"),
        opening.to_owned(),
        "next_step: its result arrives on its own in a later turn — do not wait for it or poll task_output; carry on with other work.".to_owned(),
        "next_step: use task_stop only if it must be cancelled.".to_owned(),
    ]
    .join("\n")
}

struct ThrottleState {
    dirty: bool,
    last_update_at: Option<Instant>,
    timer_armed: bool,
}

/// The accumulator plus the throttled `onUpdate` emission around it.
struct OutputPipeline {
    accumulator: Mutex<OutputAccumulator>,
    on_update: Option<AgentToolUpdateCallback>,
    throttle: Mutex<ThrottleState>,
    accepting: AtomicBool,
    closed: CancellationToken,
}

impl OutputPipeline {
    fn new(on_update: Option<AgentToolUpdateCallback>) -> Arc<Self> {
        Arc::new(Self {
            accumulator: Mutex::new(OutputAccumulator::new(OutputAccumulatorOptions {
                temp_file_prefix: "notagent-bash".to_owned(),
                ..OutputAccumulatorOptions::default()
            })),
            on_update,
            throttle: Mutex::new(ThrottleState {
                dirty: false,
                last_update_at: None,
                timer_armed: false,
            }),
            accepting: AtomicBool::new(true),
            closed: CancellationToken::new(),
        })
    }

    fn handle_data(self: &Arc<Self>, data: &[u8]) {
        if !self.accepting.load(Ordering::SeqCst) {
            return;
        }
        self.accumulator
            .lock()
            .expect("output accumulator mutex")
            .append(data);
        self.schedule_update();
    }

    fn schedule_update(self: &Arc<Self>) {
        if self.on_update.is_none() {
            return;
        }
        let delay = {
            let mut throttle = self.throttle.lock().expect("throttle mutex");
            throttle.dirty = true;
            let elapsed = throttle
                .last_update_at
                .map(|last| last.elapsed())
                .unwrap_or(Duration::MAX);
            let throttle_window = Duration::from_millis(BASH_UPDATE_THROTTLE_MS);
            if elapsed >= throttle_window {
                throttle.timer_armed = false;
                None
            } else if throttle.timer_armed {
                return;
            } else {
                throttle.timer_armed = true;
                Some(throttle_window - elapsed)
            }
        };
        match delay {
            None => self.emit_update(),
            Some(delay) => {
                let pipeline = Arc::clone(self);
                tokio::spawn(async move {
                    tokio::select! {
                        () = tokio::time::sleep(delay) => {}
                        () = pipeline.closed.cancelled() => return,
                    }
                    pipeline
                        .throttle
                        .lock()
                        .expect("throttle mutex")
                        .timer_armed = false;
                    pipeline.emit_update();
                });
            }
        }
    }

    fn emit_update(&self) {
        let Some(on_update) = &self.on_update else {
            return;
        };
        {
            let mut throttle = self.throttle.lock().expect("throttle mutex");
            if !throttle.dirty {
                return;
            }
            throttle.dirty = false;
            throttle.last_update_at = Some(Instant::now());
        }
        let snapshot = self
            .accumulator
            .lock()
            .expect("output accumulator mutex")
            .snapshot(true);
        on_update(AgentToolResult {
            content: vec![TextOrImageContent::Text(TextContent::new(
                snapshot.content.clone(),
            ))],
            details: Some(details_value(&snapshot)),
            usage: None,
            added_tool_names: None,
            terminate: None,
        });
    }

    /// `finishOutput` — stop accepting, flush a last update, close the file.
    fn finish(&self) -> OutputSnapshot {
        self.accepting.store(false, Ordering::SeqCst);
        self.accumulator
            .lock()
            .expect("output accumulator mutex")
            .finish();
        self.closed.cancel();
        self.emit_update();
        let mut accumulator = self.accumulator.lock().expect("output accumulator mutex");
        let snapshot = accumulator.snapshot(true);
        accumulator.close_temp_file();
        snapshot
    }

    fn last_line_bytes(&self) -> usize {
        self.accumulator
            .lock()
            .expect("output accumulator mutex")
            .get_last_line_bytes()
    }

    /// Writes the raw output to its temp file, whatever its size, and returns
    /// the path. `None` when the file could not be created.
    fn persist_full_output(&self) -> Option<String> {
        let mut accumulator = self.accumulator.lock().expect("output accumulator mutex");
        accumulator.persist();
        accumulator.close_temp_file();
        accumulator.full_output_path()
    }
}

fn details_value(snapshot: &OutputSnapshot) -> Value {
    let mut details = Map::new();
    if snapshot.truncation.truncated {
        details.insert(
            "truncation".to_owned(),
            serde_json::to_value(&snapshot.truncation).expect("truncation is serializable"),
        );
    }
    if let Some(path) = &snapshot.full_output_path {
        details.insert("fullOutputPath".to_owned(), Value::String(path.clone()));
    }
    Value::Object(details)
}

fn format_output(
    pipeline: &OutputPipeline,
    snapshot: &OutputSnapshot,
    empty_text: &str,
) -> (String, Option<Value>) {
    let truncation = &snapshot.truncation;
    let mut text = if snapshot.content.is_empty() {
        empty_text.to_owned()
    } else {
        snapshot.content.clone()
    };
    let mut details = None;
    if truncation.truncated {
        details = Some(details_value(snapshot));
        let start_line = truncation.total_lines - truncation.output_lines + 1;
        let end_line = truncation.total_lines;
        let full_output_path = snapshot.full_output_path.clone().unwrap_or_default();
        if truncation.last_line_partial {
            let last_line_size = format_size(pipeline.last_line_bytes());
            text.push_str(&format!(
                "\n\n[Showing last {} of line {end_line} (line is {last_line_size}). Full output: {full_output_path}]",
                format_size(truncation.output_bytes)
            ));
        } else if truncation.truncated_by == Some(TruncatedBy::Lines) {
            text.push_str(&format!(
                "\n\n[Showing lines {start_line}-{end_line} of {}. Full output: {full_output_path}]",
                truncation.total_lines
            ));
        } else {
            text.push_str(&format!(
                "\n\n[Showing lines {start_line}-{end_line} of {} ({} limit). Full output: {full_output_path}]",
                truncation.total_lines,
                format_size(DEFAULT_MAX_BYTES)
            ));
        }
    }
    (text, details)
}

/// The result text once the bash filter has had its shot at the output.
/// The filter is a whole-output function, so it runs on the finished snapshot
/// and never on a chunk. Two cases send the raw output through unchanged: a
/// filter that did not claim it, and a snapshot that was truncated — a summary
/// computed from a tail states totals it cannot know ("2 passed" for a run of
/// five hundred), which is worse than an honest truncation notice.
/// one stream (`BashExecOptions::on_data` carries no stream tag, inherited from
/// and an empty stderr. A parser that cannot read the mixture claims nothing
/// and the raw output remains, which is the same outcome as the filter being
/// off.
fn format_output_with_filter(
    pipeline: &OutputPipeline,
    snapshot: &OutputSnapshot,
    prepared: Option<&PreparedInvocation>,
    cwd: &str,
    exit_code: Option<i32>,
    empty_text: &str,
) -> (String, Option<Value>) {
    let Some(prepared) = prepared else {
        return format_output(pipeline, snapshot, empty_text);
    };
    if snapshot.truncation.truncated {
        return format_output(pipeline, snapshot, empty_text);
    }
    let filtered = bash_filter::filter_with_cwd(
        prepared,
        std::path::Path::new(cwd),
        &snapshot.content,
        "",
        exit_code,
    );
    if !filtered.changed {
        return format_output(pipeline, snapshot, empty_text);
    }
    let text = if filtered.stdout.is_empty() {
        empty_text.to_owned()
    } else {
        filtered.stdout
    };
    // The compacted form is what the caller reads from here on, so the output
    // the command actually printed has to stay reachable.
    let created_temp_file = snapshot.full_output_path.is_none();
    let Some(full_output_path) = pipeline.persist_full_output() else {
        return (text, None);
    };
    let annotated =
        format!("{text}\n\n[Compacted by the bash filter. Full output: {full_output_path}]");
    // The pointer to the raw output is part of what the caller reads, so it
    // counts. On a short output it costs more than the compaction saved, and
    // then the raw output is the cheaper answer — the filter's own promise, one
    // level up.
    if bash_filter::estimate_tokens(&annotated) >= bash_filter::estimate_tokens(&snapshot.content) {
        if created_temp_file {
            let _ = std::fs::remove_file(&full_output_path);
        }
        return format_output(pipeline, snapshot, empty_text);
    }
    let mut details = Map::new();
    details.insert("fullOutputPath".to_owned(), Value::String(full_output_path));
    (annotated, Some(Value::Object(details)))
}

fn append_status(text: &str, status: &str) -> String {
    if text.is_empty() {
        status.to_owned()
    } else {
        format!("{text}\n\n{status}")
    }
}

pub struct BashToolDefinition {
    cwd: String,
    operations: Arc<dyn BashOperations>,
    /// Whether `operations` is this machine's shell. The bash filter answers
    /// `find` and the read commands from the local filesystem, which would be
    /// the wrong filesystem behind operations that run somewhere else.
    local_operations: bool,
    bash_filter: Option<BashFilterGate>,
    command_prefix: Option<String>,
    expose_session_environment: bool,
    spawn_hook: Option<BashSpawnHook>,
    sources: Option<BashToolSources>,
    description_without_background: String,
    description_auto_background: String,
    description_stopped_on_timeout: String,
    schema_with_background: Value,
    schema_without_background: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_bash_tool_definition(
    cwd: &str,
    options: Option<BashToolOptions>,
) -> BashToolDefinition {
    let options = options.unwrap_or_default();
    let local_operations = options.operations.is_none();
    let operations = options.operations.clone().unwrap_or_else(|| {
        Arc::new(create_local_bash_operations(options.shell_path.clone()))
            as Arc<dyn BashOperations>
    });
    BashToolDefinition {
        cwd: cwd.to_owned(),
        operations,
        local_operations,
        bash_filter: options.bash_filter,
        command_prefix: options.command_prefix,
        expose_session_environment: options.expose_session_environment.unwrap_or(true),
        spawn_hook: options.spawn_hook,
        sources: options.sources,
        description_without_background: description_without_background(),
        description_auto_background: description_with_background(true),
        description_stopped_on_timeout: description_with_background(false),
        schema_with_background: bash_schema(true),
        schema_without_background: bash_schema(false),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

/// `createBashTool` — the tool as the agent loop takes it.
pub fn create_bash_tool(cwd: &str, options: Option<BashToolOptions>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_bash_tool_definition(cwd, options)), None)
}

impl BashToolDefinition {
    fn manager(&self) -> Option<Arc<dyn BashTaskManager>> {
        self.sources
            .as_ref()
            .and_then(|sources| (sources.manager)())
    }

    fn background_allowed(&self) -> bool {
        match &self.sources {
            Some(sources) => (sources.background_allowed)() && self.manager().is_some(),
            None => false,
        }
    }

    fn auto_background(&self) -> bool {
        self.background_allowed()
            && self
                .sources
                .as_ref()
                .and_then(|sources| sources.auto_background_on_timeout.as_ref())
                .map(|allowed| allowed())
                .unwrap_or(true)
    }

    /// The prepared invocation for `command`, or `None` when the filter is off.
    /// Prepared from the command the model wrote, not from the prefixed form:
    /// a command prefix makes the line multi-line, and the classifier refuses
    /// those — so preparing after the prefix would switch the filter off for
    /// every session that sets one.
    fn prepare_filter(&self, command: &str) -> Option<PreparedInvocation> {
        self.bash_filter
            .as_ref()
            .is_some_and(|enabled| enabled())
            .then(|| bash_filter::prepare(command))
    }

    /// Whether this call is answered from the filesystem instead of a process.
    /// Only with the local operations: the adapter reads this machine, and
    /// operations that run elsewhere would be asked about the wrong one.
    fn uses_execution_override(&self, prepared: Option<&PreparedInvocation>) -> bool {
        self.local_operations && prepared.is_some_and(PreparedInvocation::uses_execution_override)
    }

    /// Re-runs the command as the caller wrote it when the rewritten form
    /// produced output the filter cannot read, and returns that run's result.
    /// `None` when no retry is warranted, which is the overwhelmingly common
    /// case: only a rewritten search can ask for one. The retry always takes
    /// the direct execution path, even when the first attempt went through the
    /// task manager — it corrects a search, which is short and reads nothing
    /// but the workspace, so registering a second task for it would say more
    /// than it means.
    async fn rerun_original(
        &self,
        prepared: Option<&PreparedInvocation>,
        snapshot: &OutputSnapshot,
        spawn_context: &BashSpawnContext,
        signal: Option<CancellationToken>,
        timeout: f64,
        exit_code: Option<i32>,
    ) -> Option<Result<AgentToolResult, ToolExecutionError>> {
        let prepared = prepared?;
        if !bash_filter::should_retry_original(prepared, &snapshot.content, exit_code) {
            return None;
        }
        let command = match &self.command_prefix {
            Some(prefix) => format!("{prefix}\n{}", prepared.original()),
            None => prepared.original().to_owned(),
        };
        let pipeline = OutputPipeline::new(None);
        let on_data_pipeline = Arc::clone(&pipeline);
        let result = self
            .operations
            .exec(
                &command,
                &spawn_context.cwd,
                BashExecOptions {
                    on_data: Some(Arc::new(move |data: &[u8]| {
                        on_data_pipeline.handle_data(data);
                    })),
                    signal,
                    timeout: Some(timeout),
                    env: Some(spawn_context.env.clone()),
                    on_spawn: None,
                },
            )
            .await;
        // A retry that cannot run leaves the first attempt's result standing.
        let exit_code = result.ok()?.exit_code;
        let snapshot = pipeline.finish();
        let (output_text, details) = format_output(&pipeline, &snapshot, "(no output)");
        if let Some(exit_code) = exit_code
            && exit_code != 0
        {
            return Some(Err(ToolExecutionError::new(append_status(
                &output_text,
                &format!("Command exited with code {exit_code}"),
            ))));
        }
        Some(Ok(AgentToolResult {
            content: vec![TextOrImageContent::Text(TextContent::new(output_text))],
            details,
            usage: None,
            added_tool_names: None,
            terminate: None,
        }))
    }
}

// ============================================================================
// Rendering
// ============================================================================

/// Preview rows of the output before the result is expanded.
const BASH_PREVIEW_LINES: usize = 5;

/// How often the elapsed time is refreshed while the command runs.
const BASH_ELAPSED_TICK: Duration = Duration::from_secs(1);

/// The row state of a `bash` call.
/// `setInterval` that redraws the elapsed time becomes [`Self::tick_deadline`],
/// which [`ToolDefinition::render_deadline`] reports to the render loop
#[derive(Default)]
pub struct BashRenderState {
    pub started_at: Option<Instant>,
    pub ended_at: Option<Instant>,
    /// When the elapsed line is next due; `None` while nothing ticks.
    pub tick_deadline: Option<Instant>,
    call: Option<Rc<RefCell<Text>>>,
    result: Option<Rc<RefCell<Container>>>,
}

/// The preview of a long output, truncated to the visible rows.
struct BashPreviewComponent {
    styled_output: String,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<Line>>,
    cached_skipped: Option<usize>,
}

impl Component for BashPreviewComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        if self.cached_lines.is_none() || self.cached_width != Some(width) {
            // `paddingX` defaults to 0: the preview sits inside the tool shell's box.
            let preview =
                truncate_to_visual_lines(&self.styled_output, BASH_PREVIEW_LINES, width, 0);
            self.cached_lines = Some(preview.visual_lines);
            self.cached_skipped = Some(preview.skipped_count);
            self.cached_width = Some(width);
        }
        let lines = self.cached_lines.clone().unwrap_or_default();
        // The standard style separates the output from the command with one
        // blank row; the badge style stacks them directly (reference
        // `format_bash_result`).
        let lead = block_style() != BlockStyle::Badge;
        if let Some(skipped) = self.cached_skipped.filter(|skipped| *skipped > 0) {
            let theme = theme();
            let hint = theme.fg(ThemeColor::Muted, &format!("... ({skipped} earlier lines,"))
                + " "
                + &key_hint("app.tools.expand", "to expand")
                + &theme.fg(ThemeColor::Muted, ")");
            let mut rendered = if lead {
                vec![Line::from("")]
            } else {
                Vec::new()
            };
            rendered.push(Line::from(truncate_to_width(&hint, width)));
            rendered.extend(lines);
            return rendered;
        }
        let mut rendered = if lead {
            vec![Line::from("")]
        } else {
            Vec::new()
        };
        rendered.extend(lines);
        rendered
    }

    fn invalidate(&mut self) {
        self.cached_width = None;
        self.cached_lines = None;
        self.cached_skipped = None;
    }
}

fn format_duration(elapsed: Duration) -> String {
    format!("{:.1}s", elapsed.as_secs_f64())
}

fn bash_running_times(args: &Value, elapsed: Duration) -> (String, String) {
    let elapsed_text = format_elapsed(elapsed);
    let default_timeout = if args
        .get("run_in_background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        DEFAULT_BACKGROUND_TIMEOUT_S
    } else {
        DEFAULT_TIMEOUT_S
    };
    let timeout_secs = args
        .get("timeout")
        .and_then(Value::as_f64)
        .filter(|timeout| *timeout > 0.0)
        .unwrap_or(default_timeout) as u64;
    let timeout_text = if timeout_secs >= 60 {
        if timeout_secs.is_multiple_of(60) {
            format!("{}m", timeout_secs / 60)
        } else {
            format!("{}m {}s", timeout_secs / 60, timeout_secs % 60)
        }
    } else {
        format!("{timeout_secs}s")
    };
    (elapsed_text, timeout_text)
}

/// The compact running state that follows a foreground command in a badge
/// header. Labels are redundant there: position and the slash already identify
/// elapsed time and deadline.
pub(crate) fn format_bash_badge_running_suffix(args: &Value, elapsed: Duration) -> String {
    let (elapsed_text, timeout_text) = bash_running_times(args, elapsed);
    format!("[{elapsed_text} / {timeout_text}]")
}

/// The `$ command` header: a bold prompt and syntax-coloured command. Standard
/// blocks keep the labelled running state inline; badge blocks place their
/// compact state after the header's closing parenthesis.
fn format_bash_call(args: &Value, theme: &Theme, running_for: Option<Duration>) -> String {
    let command = str_arg(args.get("command"));
    let prompt = theme.fg(ThemeColor::ToolTitle, &theme.bold("$ "));
    let command_display = match &command {
        None => invalid_arg_text(theme),
        Some(command) if command.is_empty() => theme.fg(ThemeColor::ToolOutput, "..."),
        Some(command) => render_shell_command(command),
    };
    let mut header = format!("{prompt}{command_display}");
    if let Some(elapsed) = running_for
        && block_style() != BlockStyle::Badge
    {
        let (elapsed_text, timeout_text) = bash_running_times(args, elapsed);
        header.push_str(&theme.fg(
            ThemeColor::Muted,
            &format!(" [Running: {elapsed_text} / timeout: {timeout_text}]"),
        ));
    }
    header
}

/// The command through the tree-sitter bash highlighter, in the reference's
/// manner; a command the parser rejects renders plain.
fn render_shell_command(command: &str) -> String {
    crate::modes::interactive::theme::theme::highlight_code(command, Some("bash")).join("\n")
}

fn rebuild_bash_result_component(
    component: &Rc<RefCell<Container>>,
    result: ToolRenderResult<'_>,
    options: ToolRenderResultOptions,
    theme: &Theme,
    show_images: bool,
    started_at: Option<Instant>,
    ended_at: Option<Instant>,
) {
    component.borrow_mut().clear();

    let mut output = get_text_output(Some(result.content), show_images)
        .trim()
        .to_string();
    let truncation = truncation_from_details(result.details);
    let full_output_path = result
        .details
        .and_then(|details| details.get("fullOutputPath"))
        .and_then(Value::as_str);
    // The model-facing footer names the full log; the TUI shows that as its own
    // warning line, so it is cut from the preview text.
    if !options.is_partial
        && truncation
            .as_ref()
            .is_some_and(|truncation| truncation.truncated)
        && let Some(full_output_path) = full_output_path
        && output.ends_with(']')
        && let Some(footer_start) = output.rfind("\n\n[")
        && output[footer_start..].contains(full_output_path)
    {
        output = output[..footer_start].trim_end().to_string();
    }

    if !output.is_empty() {
        let styled_output = output
            .split('\n')
            .map(|line| theme.fg(ThemeColor::ToolOutput, line))
            .collect::<Vec<_>>()
            .join("\n");

        if options.expanded {
            // The badge style stacks the output directly under the command;
            // the standard style keeps its separating blank row.
            let lead = if block_style() == BlockStyle::Badge {
                ""
            } else {
                "\n"
            };
            component.borrow_mut().add_child(component_ref(Text::new(
                format!("{lead}{styled_output}"),
                0,
                0,
            )));
        } else {
            component
                .borrow_mut()
                .add_child(component_ref(BashPreviewComponent {
                    styled_output,
                    cached_width: None,
                    cached_lines: None,
                    cached_skipped: None,
                }));
        }
    }

    let truncated = truncation
        .as_ref()
        .is_some_and(|truncation| truncation.truncated);
    if truncated || full_output_path.is_some() {
        let mut warnings: Vec<String> = Vec::new();
        if let Some(full_output_path) = full_output_path {
            warnings.push(format!("Full output: {full_output_path}"));
        }
        if let Some(truncation) = truncation
            .as_ref()
            .filter(|truncation| truncation.truncated)
        {
            if truncation.truncated_by == Some(TruncatedBy::Lines) {
                warnings.push(format!(
                    "Truncated: showing {} of {} lines",
                    truncation.output_lines, truncation.total_lines
                ));
            } else {
                warnings.push(format!(
                    "Truncated: {} lines shown ({} limit)",
                    truncation.output_lines,
                    format_size(truncation.max_bytes)
                ));
            }
        }
        let lead = if block_style() == BlockStyle::Badge {
            ""
        } else {
            "\n"
        };
        component.borrow_mut().add_child(component_ref(Text::new(
            format!(
                "{lead}{}",
                theme.fg(ThemeColor::Warning, &format!("[{}]", warnings.join(". ")))
            ),
            0,
            0,
        )));
    }

    if let Some(started_at) = started_at {
        let end_time = ended_at.unwrap_or_else(Instant::now);
        let elapsed = end_time.saturating_duration_since(started_at);
        // Badge style (v0.1.9, reference `format_bash_result`): the timing
        // reads `(1.2s)` — live it stays invisible below one second, final it
        // original's `Elapsed/Took X.Xs` line.
        let timing = if block_style() == BlockStyle::Badge {
            if options.is_partial {
                format_elapsed_live(elapsed).map(|text| format!("({text})"))
            } else {
                Some(format!("({})", format_elapsed_precise(elapsed)))
            }
        } else {
            let label = if options.is_partial {
                "Elapsed"
            } else {
                "Took"
            };
            Some(format!("{label} {}", format_duration(elapsed)))
        };
        if let Some(timing) = timing {
            let lead = if block_style() == BlockStyle::Badge {
                ""
            } else {
                "\n"
            };
            component.borrow_mut().add_child(component_ref(Text::new(
                format!("{lead}{}", theme.fg(ThemeColor::Muted, &timing)),
                0,
                0,
            )));
        }
    }
}

impl ToolDefinition for BashToolDefinition {
    fn name(&self) -> &str {
        "bash"
    }

    fn label(&self) -> &str {
        "bash"
    }

    /// Rebuilt on each read, because what this tool can do depends on which
    /// other tools are active right now. A description that promised
    /// backgrounding in a mode without the tools to observe it would be a
    /// promise the call itself refuses.
    fn description(&self) -> &str {
        if !self.background_allowed() {
            return &self.description_without_background;
        }
        if self.auto_background() {
            &self.description_auto_background
        } else {
            &self.description_stopped_on_timeout
        }
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(BASH_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        if self.expose_session_environment {
            BASH_TOOL_SYSTEM_PROMPT_CONTRIBUTION
                .guidelines
                .iter()
                .map(|guideline| (*guideline).to_owned())
                .collect()
        } else {
            Vec::new()
        }
    }

    fn parameters(&self) -> &Value {
        if self.background_allowed() {
            &self.schema_with_background
        } else {
            &self.schema_without_background
        }
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        self.constrained_sampling.as_ref()
    }

    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let mut state = tool_render_state::<BashRenderState>(&context.state);
        // The clock starts when the command does, not when the call is first
        // drawn: the arguments are still streaming before that.
        if context.execution_started && state.started_at.is_none() {
            state.started_at = Some(Instant::now());
            state.ended_at = None;
        }
        // The running indicator lives on the call line, as in the reference;
        // it disappears the moment the result records an end time.
        let running_for = state
            .started_at
            .filter(|_| state.ended_at.is_none())
            .map(|started_at| started_at.elapsed());
        let text = format_bash_call(args, theme, running_for);
        let component = state
            .call
            .get_or_insert_with(|| Rc::new(RefCell::new(Text::new("", 0, 0))));
        component.borrow_mut().set_text(text);
        Some(Rc::clone(component) as ComponentRef)
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let mut state = tool_render_state::<BashRenderState>(&context.state);
        // While the command runs, the elapsed line is refreshed once a second;
        // `render_deadline`).
        if state.started_at.is_some() && options.is_partial && state.tick_deadline.is_none() {
            state.tick_deadline = Some(Instant::now() + BASH_ELAPSED_TICK);
        }
        if !options.is_partial || context.is_error {
            state.ended_at.get_or_insert_with(Instant::now);
            state.tick_deadline = None;
        }
        let started_at = state.started_at;
        let ended_at = state.ended_at;
        let component = state
            .result
            .get_or_insert_with(|| Rc::new(RefCell::new(Container::new())))
            .clone();
        drop(state);
        rebuild_bash_result_component(
            &component,
            result,
            options,
            theme,
            context.show_images,
            started_at,
            ended_at,
        );
        component.borrow_mut().invalidate();
        Some(component as ComponentRef)
    }

    /// The elapsed line ticks once a second while the command runs.
    fn render_deadline(&self, context: &ToolRenderContext) -> Option<Instant> {
        tool_render_state::<BashRenderState>(&context.state).tick_deadline
    }

    /// Arm the next tick once the render loop has drawn the due one.
    fn pump_render<'a>(&'a self, context: &'a ToolRenderContext) -> Option<RenderFuture<'a>> {
        let mut state = tool_render_state::<BashRenderState>(&context.state);
        let deadline = state.tick_deadline?;
        if Instant::now() < deadline {
            return None;
        }
        state.tick_deadline = Some(Instant::now() + BASH_ELAPSED_TICK);
        drop(state);
        (context.invalidate)();
        Some(Box::pin(async {}))
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        on_update: Option<AgentToolUpdateCallback>,
        context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let command = params
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let starts_in_background = params
                .get("run_in_background")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if starts_in_background && !self.background_allowed() {
                return Err(ToolExecutionError::new(
                    "Background execution is not available here, because the tools that list, read and stop background tasks are not active.",
                ));
            }
            let description = params
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            if starts_in_background && description.is_empty() {
                return Err(ToolExecutionError::new(
                    "`description` is required when starting a command in the background.",
                ));
            }
            let timeout = normalize_timeout_seconds(
                params.get("timeout").and_then(Value::as_f64),
                starts_in_background,
            )?;
            // The filter is prepared from what the model wrote; the rewrite it
            // proposes is what actually runs, and the prefix wraps that.
            let prepared = self.prepare_filter(&command);
            let execution_command = prepared
                .as_ref()
                .map_or(command.as_str(), PreparedInvocation::execution);
            let resolved_command = match &self.command_prefix {
                Some(prefix) => format!("{prefix}\n{execution_command}"),
                None => execution_command.to_owned(),
            };
            let spawn_context = resolve_spawn_context(
                &resolved_command,
                &self.cwd,
                self.spawn_hook.as_ref(),
                self.expose_session_environment,
                context.as_ref(),
            );

            // A system filter reshapes output whose form it asked for, so the
            // live stream would contradict the result. Those calls stay quiet
            // until they are done; everything else streams as before.
            let buffers_live_output = prepared
                .as_ref()
                .is_some_and(PreparedInvocation::buffers_live_output);
            let pipeline = OutputPipeline::new(if buffers_live_output {
                None
            } else {
                on_update.clone()
            });
            if let Some(on_update) = &on_update {
                on_update(AgentToolResult {
                    content: Vec::new(),
                    details: None,
                    usage: None,
                    added_tool_names: None,
                    terminate: None,
                });
            }

            // `find` and the read commands are answered from the filesystem, no
            // child process involved. Never for a backgrounded call: a
            // background job has to stream and be killable, and an adapter
            // provides neither.
            if !starts_in_background && self.uses_execution_override(prepared.as_ref()) {
                let executed = {
                    let prepared = prepared
                        .clone()
                        .expect("override implies a prepared filter");
                    let cwd = std::path::PathBuf::from(&spawn_context.cwd);
                    tokio::task::spawn_blocking(move || {
                        bash_filter::execute_override(&prepared, &cwd)
                    })
                    .await
                    .map_err(|error| {
                        ToolExecutionError::new(format!("bash filter task failed: {error}"))
                    })?
                };
                let Some(executed) = executed else {
                    return Err(ToolExecutionError::new(
                        "bash filter declared an execution override but produced none",
                    ));
                };
                pipeline.handle_data(executed.stdout.as_bytes());
                pipeline.handle_data(executed.stderr.as_bytes());
                let snapshot = pipeline.finish();
                let (output_text, details) = format_output_with_filter(
                    &pipeline,
                    &snapshot,
                    prepared.as_ref(),
                    &spawn_context.cwd,
                    executed.exit_code,
                    "(no output)",
                );
                if let Some(exit_code) = executed.exit_code
                    && exit_code != 0
                {
                    return Err(ToolExecutionError::new(append_status(
                        &output_text,
                        &format!("Command exited with code {exit_code}"),
                    )));
                }
                return Ok(AgentToolResult {
                    content: vec![TextOrImageContent::Text(TextContent::new(output_text))],
                    details,
                    usage: None,
                    added_tool_names: None,
                    terminate: None,
                });
            }

            if let Some(manager) = self.manager() {
                let sink_pipeline = Arc::clone(&pipeline);
                let task = ShellTaskSpec {
                    operations: Arc::clone(&self.operations),
                    command: spawn_context.command.clone(),
                    cwd: spawn_context.cwd.clone(),
                    env: Some(spawn_context.env.clone()),
                    description: if starts_in_background {
                        description.clone()
                    } else {
                        format!("bash: {}", truncate_command_label(&command))
                    },
                    on_output: Some(Arc::new(move |chunk: &str| {
                        sink_pipeline.handle_data(chunk.as_bytes());
                    })),
                };
                return run_managed(
                    manager.as_ref(),
                    task,
                    RegisterTaskOptions {
                        detached: starts_in_background,
                        timeout_ms: if starts_in_background
                            && params
                                .get("disable_timeout")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                        {
                            None
                        } else {
                            Some((timeout * 1000.0) as u64)
                        },
                        detach_timeout_ms: Some((DEFAULT_BACKGROUND_TIMEOUT_S * 1000.0) as u64),
                        auto_background_on_timeout: self.auto_background(),
                        signal: if starts_in_background {
                            None
                        } else {
                            signal.clone()
                        },
                    },
                    starts_in_background,
                    &pipeline,
                    timeout,
                    ManagedFilter {
                        tool: self,
                        prepared: prepared.as_ref(),
                        spawn_context: &spawn_context,
                        signal: signal.clone(),
                        timeout,
                    },
                )
                .await;
            }

            let on_data_pipeline = Arc::clone(&pipeline);
            let result = self
                .operations
                .exec(
                    &spawn_context.command,
                    &spawn_context.cwd,
                    BashExecOptions {
                        on_data: Some(Arc::new(move |data: &[u8]| {
                            on_data_pipeline.handle_data(data);
                        })),
                        signal: signal.clone(),
                        timeout: Some(timeout),
                        env: Some(spawn_context.env.clone()),
                        on_spawn: None,
                    },
                )
                .await;

            let exit_code = match result {
                Ok(result) => result.exit_code,
                Err(error) => {
                    let snapshot = pipeline.finish();
                    let (text, _) = format_output_with_filter(
                        &pipeline,
                        &snapshot,
                        prepared.as_ref(),
                        &spawn_context.cwd,
                        None,
                        "",
                    );
                    return Err(match error {
                        BashExecError::Aborted => {
                            ToolExecutionError::new(append_status(&text, "Command aborted"))
                        }
                        BashExecError::Timeout(seconds) => ToolExecutionError::new(append_status(
                            &text,
                            &format!("Command timed out after {seconds} seconds"),
                        )),
                        BashExecError::Other(message) => ToolExecutionError::new(message),
                    });
                }
            };

            let snapshot = pipeline.finish();
            // A rewritten search whose output will not parse is re-run as the
            // caller wrote it: the rewrite asked for a machine-readable shape
            // and did not get one, so what is on screen is neither the caller's
            // form nor one the filter can read.
            if let Some(retried) = self
                .rerun_original(
                    prepared.as_ref(),
                    &snapshot,
                    &spawn_context,
                    signal.clone(),
                    timeout,
                    exit_code,
                )
                .await
            {
                return retried;
            }
            let (output_text, details) = format_output_with_filter(
                &pipeline,
                &snapshot,
                prepared.as_ref(),
                &spawn_context.cwd,
                exit_code,
                "(no output)",
            );
            if let Some(exit_code) = exit_code
                && exit_code != 0
            {
                return Err(ToolExecutionError::new(append_status(
                    &output_text,
                    &format!("Command exited with code {exit_code}"),
                )));
            }
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(output_text))],
                details,
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

fn truncate_command_label(command: &str) -> String {
    let characters: Vec<char> = command.chars().collect();
    if characters.len() > 60 {
        format!("{}…", characters[..60].iter().collect::<String>())
    } else {
        command.to_owned()
    }
}

/// Runs a command as a managed task.
/// The foreground case is the interesting one: it waits for the task to release
/// it, which happens either because the command ended or because it was moved to
/// the background — by the user, or by its own deadline. Both are ordinary
/// outcomes here rather than errors.
/// What a managed run needs in order to filter its output the way a direct one
/// does: the prepared invocation, where it ran, and enough of the tool to re-run
/// the caller's own command when the rewritten one produced nothing readable.
struct ManagedFilter<'a> {
    tool: &'a BashToolDefinition,
    prepared: Option<&'a PreparedInvocation>,
    spawn_context: &'a BashSpawnContext,
    signal: Option<CancellationToken>,
    timeout: f64,
}

async fn run_managed(
    manager: &dyn BashTaskManager,
    task: ShellTaskSpec,
    options: RegisterTaskOptions,
    starts_in_background: bool,
    pipeline: &OutputPipeline,
    timeout_seconds: f64,
    filter: ManagedFilter<'_>,
) -> Result<AgentToolResult, ToolExecutionError> {
    let cwd = filter.spawn_context.cwd.as_str();
    let prepared = filter.prepared;
    let description = task.description.clone();
    let task_id = manager
        .register_shell_task(task, options)
        .map_err(ToolExecutionError::new)?;

    if starts_in_background {
        return Ok(AgentToolResult {
            content: vec![TextOrImageContent::Text(TextContent::new(
                render_detached_result(&task_id, &description, ForegroundRelease::Terminal),
            ))],
            details: None,
            usage: None,
            added_tool_names: None,
            terminate: None,
        });
    }

    let release = manager.wait_for_foreground_release(&task_id).await;
    if matches!(
        release,
        Some(ForegroundRelease::Detached | ForegroundRelease::TimeoutDetached)
    ) {
        let snapshot = pipeline.finish();
        let (text, _) = format_output_with_filter(pipeline, &snapshot, prepared, cwd, None, "");
        let header = render_detached_result(
            &task_id,
            &description,
            release.expect("release is detached here"),
        );
        return Ok(AgentToolResult {
            content: vec![TextOrImageContent::Text(TextContent::new(
                if text.is_empty() {
                    header
                } else {
                    format!("{header}\n\noutput so far:\n{text}")
                },
            ))],
            details: None,
            usage: None,
            added_tool_names: None,
            terminate: None,
        });
    }

    let info = manager.get_task(&task_id);
    let exit_code = info.as_ref().and_then(|info| info.exit_code);
    let snapshot = pipeline.finish();
    if let Some(retried) = filter
        .tool
        .rerun_original(
            prepared,
            &snapshot,
            filter.spawn_context,
            filter.signal.clone(),
            filter.timeout,
            exit_code,
        )
        .await
    {
        return retried;
    }
    let (output_text, details) =
        format_output_with_filter(pipeline, &snapshot, prepared, cwd, exit_code, "(no output)");
    if let Some(info) = &info {
        if info.status == TaskStatus::TimedOut {
            return Err(ToolExecutionError::new(append_status(
                &output_text,
                &format!("Command timed out after {timeout_seconds} seconds"),
            )));
        }
        if info.status == TaskStatus::Killed {
            return Err(ToolExecutionError::new(append_status(
                &output_text,
                info.stop_reason.as_deref().unwrap_or("Command aborted"),
            )));
        }
    }
    if info
        .as_ref()
        .is_some_and(|info| info.status == TaskStatus::Failed)
        && exit_code.is_none()
    {
        return Err(ToolExecutionError::new(append_status(
            &output_text,
            info.as_ref()
                .and_then(|info| info.stop_reason.as_deref())
                .unwrap_or("Command failed"),
        )));
    }
    if let Some(exit_code) = exit_code
        && exit_code != 0
    {
        return Err(ToolExecutionError::new(append_status(
            &output_text,
            &format!("Command exited with code {exit_code}"),
        )));
    }
    Ok(AgentToolResult {
        content: vec![TextOrImageContent::Text(TextContent::new(output_text))],
        details,
        usage: None,
        added_tool_names: None,
        terminate: None,
    })
}
