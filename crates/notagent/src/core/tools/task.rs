//! Port of `packages/coding-agent/src/core/tools/task.ts` (tool half).
//!
//! The `task` tool: delegates work to subagents.
//!
//! A subagent here is the same unified agent in a different mode. There is no
//! separate roster of agent types to define and keep in step — the modes the
//! user already writes are the agent types, so a mode folder authored for the
//! main agent is a delegation target on the day it is written.
//!
//! A delegated run is a background task like any other. That is not a detail of
//! the implementation: it is what lets a subagent be moved out of the turn that
//! started it, appear in the same list as a running command, be stopped the same
//! way, and announce its own result.
//!
//! Three things are enforced rather than asked for. A child's tools come from
//! its mode's shell, so guidance it ignores still cannot make it write. A child
//! may not exceed its parent's shell. And a child has neither the delegation
//! tool nor the tools that observe background work.
//!
//! `renderCall`/`renderResult` need the theme and are wired in task 13.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use notagent_agent::agent::Agent;
use notagent_agent::types::{
    AgentMessage, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_ai::uuidv7;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::core::delegation::limits::{MAX_DELEGATIONS_PER_CALL, check_delegation_request};
use crate::core::delegation::run::{
    DelegationOptions, DelegationRun, child_tool_names, run_delegation,
};
use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::modes::Mode;
use crate::core::modes::shells::ShellId;
use crate::core::tasks::manager::{RegisterTaskOptions, TaskManager};
use crate::core::tasks::subagent_task::{SubagentRunResult, SubagentTask, SubagentTaskOptions};
use crate::core::tasks::types::{BackgroundTask, ForegroundRelease};
use crate::core::tools::ToolsOptions;
use crate::core::tools::tool_definition::{ToolContext, ToolDefinition, wrap_tool_definition};

/// What the tool needs from the session it runs in.
#[derive(Clone)]
pub struct TaskToolSources {
    pub modes: Arc<dyn Fn() -> Vec<Mode> + Send + Sync>,
    /// The mode the parent is in; a child may not exceed its shell.
    pub parent_shell: Arc<dyn Fn() -> Option<ShellId> + Send + Sync>,
    /// The parent's own mode, which may narrow what it can delegate to.
    pub parent_mode: Option<Arc<dyn Fn() -> Option<Mode> + Send + Sync>>,
    /// The agent whose provider wiring a child borrows.
    pub parent: Arc<dyn Fn() -> Option<Arc<Agent>> + Send + Sync>,
    /// Where a delegated run is registered. Absent inside a subagent.
    pub manager: Option<Arc<dyn Fn() -> Option<TaskManager> + Send + Sync>>,
    /// Whether this session may detach work at all.
    pub background_allowed: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Resolves a child's tool from the session's assembled registry.
    pub resolve_tool: Option<crate::core::delegation::run::ResolveToolFn>,
    pub tool_options: Option<Arc<dyn Fn() -> Option<ToolsOptions> + Send + Sync>>,
    /// Where the transcripts of this session's children are kept.
    ///
    /// Part of the sources because TS keys its `WeakMap` on the sources object:
    /// a mode switch rebuilds the tool but keeps the sources, and a subagent
    /// started before the switch stays continuable after it.
    pub transcripts: TaskTranscriptStore,
    /// The working directory a child reads from. TS reads `process.cwd()`
    /// directly at the call; the port takes it from the session, which is the
    /// same value and testable (deviation class 1).
    pub cwd: Option<Arc<dyn Fn() -> String + Send + Sync>>,
}

impl Default for TaskToolSources {
    fn default() -> Self {
        TaskToolSources {
            modes: Arc::new(Vec::new),
            parent_shell: Arc::new(|| None),
            parent_mode: None,
            parent: Arc::new(|| None),
            manager: None,
            background_allowed: None,
            resolve_tool: None,
            tool_options: None,
            transcripts: TaskTranscriptStore::default(),
            cwd: None,
        }
    }
}

/// Transcripts of children started in this session, for continuation.
///
/// Deviation (class 1): TS keys a `WeakMap` on the sources object; the port
/// hands the same map around explicitly, so it outlives a rebuilt tool exactly
/// as the `WeakMap` entry does.
#[derive(Clone, Default)]
pub struct TaskTranscriptStore {
    shared: Arc<Mutex<HashMap<String, KeptTranscript>>>,
}

#[derive(Clone)]
struct KeptTranscript {
    mode_id: String,
    messages: Vec<AgentMessage>,
}

/// A read-only parent may only delegate read-only work.
fn exceeds_parent(parent: Option<ShellId>, child: ShellId) -> bool {
    parent == Some(ShellId::ReadOnly) && child != ShellId::ReadOnly
}

/// The modes this session may delegate to: its own allowlist, or all of them.
fn delegatable_modes(modes: Vec<Mode>, parent: Option<&Mode>) -> Vec<Mode> {
    let Some(allowed) = parent.and_then(|mode| mode.subagents.clone()) else {
        return modes;
    };
    modes
        .into_iter()
        .filter(|mode| allowed.contains(&mode.id))
        .collect()
}

fn describe_modes(modes: &[Mode]) -> String {
    if modes.is_empty() {
        return "No modes are available to delegate to.".to_owned();
    }
    modes
        .iter()
        .map(|mode| {
            let tools = child_tool_names(mode)
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<&str>>()
                .join(", ");
            format!(
                "- {} ({}): {}",
                mode.id,
                mode.shell,
                if tools.is_empty() {
                    "no tools".to_owned()
                } else {
                    tools
                }
            )
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn render_background_result(task_id: &str, session_id: &str, mode_id: &str, task: &str) -> String {
    [
        format!("task_id: {task_id}"),
        format!("session_id: {session_id}"),
        format!("mode: {mode_id}"),
        "status: running".to_owned(),
        format!("task: {task}"),
        "next_step: its answer arrives on its own in a later turn — do not wait for it or poll task_output; carry on with other work.".to_owned(),
        format!("continue_with: session_id \"{session_id}\", not task_id \"{task_id}\"."),
    ]
    .join("\n")
}

pub struct TaskToolDefinition {
    sources: TaskToolSources,
    /// Deviation (class 1): the TS getter builds a fresh string on every read,
    /// but a Rust `&str` has to outlive the call. Every distinct description
    /// this tool has ever advertised is therefore kept — appended, never
    /// replaced — so the reference stays valid for as long as the tool. The
    /// roster of modes changes rarely, so the list stays short.
    descriptions: Mutex<Vec<Box<str>>>,
    schema_with_background: Value,
    schema_without_background: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

fn base_properties() -> serde_json::Map<String, Value> {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "tasks".to_owned(),
        json!({
            "type": "array",
            "items": { "type": "string" },
            "description": format!(
                "Independent task descriptions, run in parallel. At most {MAX_DELEGATIONS_PER_CALL}. Each must carry enough context to be worked on without seeing this conversation."
            ),
        }),
    );
    properties.insert(
        "mode".to_owned(),
        json!({
            "type": "string",
            "description": "Mode the subagents run in. Its shell decides what they may do.",
        }),
    );
    properties.insert(
        "session_id".to_owned(),
        json!({
            "type": "string",
            "description": "Continue a previous subagent instead of starting fresh, keeping everything it already learned. Only valid with a single task.",
        }),
    );
    properties
}

fn task_schema(with_background: bool) -> Value {
    let mut properties = base_properties();
    if with_background {
        properties.insert(
            "run_in_background".to_owned(),
            json!({
                "type": "boolean",
                "description": "Return immediately instead of waiting. Prefer leaving this off: a foreground subagent hands its answer straight back. Only worth it when you have other work to do meanwhile and do not need the answer to continue.",
            }),
        );
    }
    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": ["tasks", "mode"],
    })
}

pub fn create_task_tool_definition(sources: Option<TaskToolSources>) -> TaskToolDefinition {
    TaskToolDefinition {
        sources: sources.unwrap_or_default(),
        descriptions: Mutex::new(Vec::new()),
        schema_with_background: task_schema(true),
        schema_without_background: task_schema(false),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

/// `createTaskTool` — the tool as the agent loop takes it.
pub fn create_task_tool(
    sources: Option<TaskToolSources>,
) -> Arc<dyn notagent_agent::types::AgentTool> {
    wrap_tool_definition(Arc::new(create_task_tool_definition(sources)), None)
}

impl TaskToolDefinition {
    fn manager(&self) -> Option<TaskManager> {
        self.sources.manager.as_ref().and_then(|manager| manager())
    }

    fn background_allowed(&self) -> bool {
        self.sources
            .background_allowed
            .as_ref()
            .is_some_and(|allowed| allowed())
            && self.manager().is_some()
    }

    fn modes(&self) -> Vec<Mode> {
        let parent_mode = self
            .sources
            .parent_mode
            .as_ref()
            .and_then(|parent_mode| parent_mode());
        delegatable_modes((self.sources.modes)(), parent_mode.as_ref())
    }

    fn cwd(&self) -> String {
        match &self.sources.cwd {
            Some(cwd) => cwd(),
            None => std::env::current_dir()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }

    fn build_description(&self) -> String {
        let mut lines = vec![
            "Delegates independent work to subagents and returns what each one found.".to_owned(),
            String::new(),
            "A subagent starts with no view of this conversation, so each task must carry the context it needs. It returns one answer; its intermediate steps are not shown to you or to the user, so summarise anything the user should see.".to_owned(),
            String::new(),
            "Use this for work that is genuinely separable and would otherwise crowd this conversation: sweeping many files for one answer, or several independent investigations at once. Do not use it to read a known file, to search for a known symbol, or for anything you can finish in a step or two — going direct is faster and cheaper.".to_owned(),
            String::new(),
            "Available modes, with the tools a subagent gets in each:".to_owned(),
            describe_modes(&self.modes()),
            String::new(),
            "Tasks in one call run in parallel. Asking for the same task twice in one call is refused.".to_owned(),
            String::new(),
            "A subagent cannot delegate further, and cannot start background work of its own.".to_owned(),
        ];
        if self.background_allowed() {
            lines.push(String::new());
            lines.push("With `run_in_background` the subagent is detached from this turn and its answer arrives on its own later. Default to leaving it off — a foreground subagent hands the answer straight back, which is what you want whenever your next step depends on it. Never detach one and then immediately wait for it.".to_owned());
        }
        lines.join("\n")
    }
}

/// One launched child, before its answer is in.
struct Launched {
    task: String,
    session_id: String,
    task_id: Option<String>,
    run: oneshot::Receiver<DelegationRun>,
}

impl ToolDefinition for TaskToolDefinition {
    fn name(&self) -> &str {
        "task"
    }

    fn label(&self) -> &str {
        "task"
    }

    /// Read afresh each time the tool is advertised: the roster of modes and
    /// whether this session can detach work both change during a session, and a
    /// description fixed at construction would describe an older one.
    fn description(&self) -> &str {
        let description = self.build_description();
        let mut cache = self.descriptions.lock().expect("poisoned");
        let index = match cache.iter().position(|kept| **kept == *description) {
            Some(index) => index,
            None => {
                cache.push(description.into_boxed_str());
                cache.len() - 1
            }
        };
        let pointer: *const str = &*cache[index];
        // Safety: entries are only ever appended, and a `Box<str>` keeps its
        // contents in place when the vector grows, so the target lives as long
        // as `self` — which is the lifetime this reference carries.
        unsafe { &*pointer }
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("Delegate independent work to subagents")
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        vec![
            "Delegate with the task tool when work is separable and would otherwise crowd this conversation; give each subagent the context it needs, since it cannot see this one.".to_owned(),
        ]
    }

    /// The flag is absent from the schema, not merely refused, when this session
    /// cannot observe background work: a parameter a model can see is a
    /// parameter it will eventually use.
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

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let modes = self.modes();
            let requested_mode = params
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let Some(mode) = modes.iter().find(|mode| mode.id == requested_mode).cloned() else {
                let known = modes
                    .iter()
                    .map(|mode| mode.id.clone())
                    .collect::<Vec<String>>()
                    .join(", ");
                // A mode excluded by the parent's allowlist reads as unknown
                // here on purpose: from the model's side it is not a target, and
                // naming the exclusion would invite an argument about it.
                return Err(ToolExecutionError::new(format!(
                    "Unknown mode \"{requested_mode}\". Available: {}",
                    if known.is_empty() {
                        "(none loaded)".to_owned()
                    } else {
                        known
                    }
                )));
            };

            let parent_shell = (self.sources.parent_shell)();
            if exceeds_parent(parent_shell, mode.shell) {
                return Err(ToolExecutionError::new(format!(
                    "Mode \"{}\" uses the {} shell, which exceeds the read-only shell this session runs in. A read-only session cannot delegate work that changes the workspace.",
                    mode.id, mode.shell
                )));
            }

            let Some(parent) = (self.sources.parent)() else {
                return Err(ToolExecutionError::new(
                    "Delegation is not available in this session.",
                ));
            };

            let tasks: Vec<String> = params
                .get("tasks")
                .and_then(Value::as_array)
                .map(|tasks| {
                    tasks
                        .iter()
                        .map(|task| task.as_str().unwrap_or_default().to_owned())
                        .collect()
                })
                .unwrap_or_default();
            let background = params.get("run_in_background").and_then(Value::as_bool) == Some(true);
            let requested_session_id = params
                .get("session_id")
                .and_then(Value::as_str)
                .filter(|session_id| !session_id.is_empty())
                .map(str::to_owned);

            if background && !self.background_allowed() {
                return Err(ToolExecutionError::new(
                    "Background delegation is not available here, because the tools that list, read and stop background tasks are not active. Run the subagent in the foreground instead.",
                ));
            }
            if background && requested_session_id.is_some() {
                // Continuing a subagent means you want its answer; detaching
                // means you do not. Asking for both describes no coherent
                // request.
                return Err(ToolExecutionError::new(
                    "Continuing an existing subagent cannot be combined with run_in_background. Continue it in the foreground, or start a new subagent in the background.",
                ));
            }
            if requested_session_id.is_some() && tasks.len() > 1 {
                return Err(ToolExecutionError::new(
                    "Continuing a subagent takes a single task; start separate subagents for separate work.",
                ));
            }

            let mut resumed: Option<KeptTranscript> = None;
            if let Some(session_id) = &requested_session_id {
                let kept = self
                    .sources
                    .transcripts
                    .shared
                    .lock()
                    .expect("poisoned")
                    .get(session_id)
                    .cloned();
                let Some(kept) = kept else {
                    return Err(ToolExecutionError::new(format!(
                        "Unknown subagent session \"{session_id}\". Start a new subagent instead."
                    )));
                };
                if kept.mode_id != mode.id {
                    return Err(ToolExecutionError::new(format!(
                        "Subagent \"{session_id}\" runs in mode \"{}\"; continue it in that mode or start a new one.",
                        kept.mode_id
                    )));
                }
                resumed = Some(kept);
            }

            // A refusal is something the model must read and adapt to, so it
            // arrives as a tool error carrying the reason.
            check_delegation_request(&mode.id, &tasks)
                .map_err(|error| ToolExecutionError::new(error.message()))?;

            let manager = self.manager();
            let cwd = self.cwd();
            let tool_options = self
                .sources
                .tool_options
                .as_ref()
                .and_then(|tool_options| tool_options());
            let mut launched: Vec<Launched> = Vec::new();
            for task in &tasks {
                let session_id = requested_session_id.clone().unwrap_or_else(uuidv7);
                // The child answers to its own controller, so the manager can
                // stop it without the turn's signal reaching in — which is what
                // makes a detached subagent outlive the turn that asked for it.
                let controller = CancellationToken::new();
                let tokens = Arc::new(std::sync::atomic::AtomicU64::new(0));
                let token_sink = Arc::clone(&tokens);
                let (run_sender, run_receiver) = oneshot::channel::<DelegationRun>();
                let (task_sender, task_receiver) = oneshot::channel::<SubagentRunResult>();
                let delegation = DelegationOptions {
                    parent: Arc::clone(&parent),
                    mode: mode.clone(),
                    cwd: cwd.clone(),
                    task: task.clone(),
                    history: resumed.as_ref().map(|resumed| resumed.messages.clone()),
                    session_id: Some(session_id.clone()),
                    signal: Some(controller.clone()),
                    tool_options: tool_options.clone(),
                    resolve_tool: self.sources.resolve_tool.clone(),
                    timeout_ms: None,
                    on_tokens: Some(Arc::new(move |spent| {
                        token_sink.store(spent, std::sync::atomic::Ordering::SeqCst);
                    })),
                };
                // Real parallelism: every child is its own tokio task, and the
                // tool only holds the two ends of its result.
                tokio::spawn(async move {
                    let result = run_delegation(delegation).await;
                    let _ = task_sender.send(SubagentRunResult {
                        text: result.text.clone(),
                        failed: result.failed,
                    });
                    let _ = run_sender.send(result);
                });

                let task_id = match &manager {
                    Some(manager) => {
                        let description = truncate_description(task);
                        let cancel_controller = controller.clone();
                        let subagent: Arc<dyn BackgroundTask> =
                            Arc::new(SubagentTask::new(SubagentTaskOptions {
                                description,
                                tokens: Some(Arc::new(move || {
                                    tokens.load(std::sync::atomic::Ordering::SeqCst)
                                })),
                                session_id: session_id.clone(),
                                mode_id: mode.id.clone(),
                                run: task_receiver,
                                cancel: Arc::new(move || cancel_controller.cancel()),
                            }));
                        manager
                            .register(
                                subagent,
                                RegisterTaskOptions {
                                    detached: background,
                                    signal: if background { None } else { signal.clone() },
                                    ..RegisterTaskOptions::default()
                                },
                            )
                            .ok()
                    }
                    None => None,
                };
                launched.push(Launched {
                    task: task.clone(),
                    session_id,
                    task_id,
                    run: run_receiver,
                });
            }

            if background {
                let mut results = Vec::new();
                let mut text_blocks = Vec::new();
                for entry in launched {
                    // The transcript is still recorded, so a later call can
                    // continue a backgrounded subagent by its session id.
                    self.keep_later(entry.run, mode.id.clone());
                    text_blocks.push(render_background_result(
                        entry.task_id.as_deref().unwrap_or(""),
                        &entry.session_id,
                        &mode.id,
                        &entry.task,
                    ));
                    results.push(json!({
                        "sessionId": entry.session_id,
                        "failed": false,
                        "taskId": entry.task_id,
                        "background": true,
                    }));
                }
                return Ok(AgentToolResult {
                    content: vec![TextOrImageContent::Text(TextContent::new(
                        text_blocks.join("\n\n"),
                    ))],
                    details: Some(json!({ "mode": mode.id, "results": results })),
                    usage: None,
                    added_tool_names: None,
                    terminate: None,
                });
            }

            // Foreground: wait for each child either to answer or to be moved to
            // the background from under us, which the user can do at any moment.
            let settled = futures::future::join_all(launched.into_iter().map(|entry| {
                let manager = manager.clone();
                let mode_id = mode.id.clone();
                async move {
                    if let (Some(manager), Some(task_id)) = (&manager, &entry.task_id) {
                        let release = manager.wait_for_foreground_release(task_id).await;
                        if matches!(
                            release,
                            Some(ForegroundRelease::Detached | ForegroundRelease::TimeoutDetached)
                        ) {
                            return SettledEntry {
                                detached_as: entry.task_id.clone(),
                                session_id: entry.session_id,
                                task: entry.task,
                                run: None,
                                pending: Some((entry.run, mode_id)),
                            };
                        }
                    }
                    let run = entry.run.await.ok();
                    SettledEntry {
                        detached_as: None,
                        session_id: entry.session_id,
                        task: entry.task,
                        run,
                        pending: None,
                    }
                }
            }))
            .await;

            let mut results = Vec::new();
            let mut text_blocks = Vec::new();
            let count = settled.len();
            for (index, entry) in settled.into_iter().enumerate() {
                if let Some((run, mode_id)) = entry.pending {
                    self.keep_later(run, mode_id);
                }
                match &entry.run {
                    Some(run) => {
                        self.keep(&run.session_id, &mode.id, run.transcript.clone());
                        let label = if count > 1 {
                            format!("Task {}", index + 1)
                        } else {
                            "Result".to_owned()
                        };
                        text_blocks.push(format!(
                            "<subagent mode=\"{}\" session=\"{}\" task=\"{label}\">\n{}\n</subagent>",
                            mode.id, run.session_id, run.text
                        ));
                        results.push(json!({
                            "sessionId": entry.session_id,
                            "failed": run.failed,
                            "background": false,
                        }));
                    }
                    None => {
                        text_blocks.push(render_background_result(
                            entry.detached_as.as_deref().unwrap_or(""),
                            &entry.session_id,
                            &mode.id,
                            &entry.task,
                        ));
                        results.push(json!({
                            "sessionId": entry.session_id,
                            "failed": false,
                            "taskId": entry.detached_as,
                            "background": entry.detached_as.is_some(),
                        }));
                    }
                }
            }

            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(
                    text_blocks.join("\n\n"),
                ))],
                details: Some(json!({ "mode": mode.id, "results": results })),
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

struct SettledEntry {
    detached_as: Option<String>,
    session_id: String,
    task: String,
    run: Option<DelegationRun>,
    /// A child that outlived the call still records its transcript when it ends.
    pending: Option<(oneshot::Receiver<DelegationRun>, String)>,
}

impl TaskToolDefinition {
    fn keep(&self, session_id: &str, mode_id: &str, messages: Vec<AgentMessage>) {
        self.sources
            .transcripts
            .shared
            .lock()
            .expect("poisoned")
            .insert(
                session_id.to_owned(),
                KeptTranscript {
                    mode_id: mode_id.to_owned(),
                    messages,
                },
            );
    }

    /// `void run.then((result) => kept.set(…))` — the record is written when the
    /// detached child finally answers.
    fn keep_later(&self, run: oneshot::Receiver<DelegationRun>, mode_id: String) {
        let kept = self.sources.transcripts.handle();
        tokio::spawn(async move {
            if let Ok(result) = run.await {
                kept.lock().expect("poisoned").insert(
                    result.session_id.clone(),
                    KeptTranscript {
                        mode_id,
                        messages: result.transcript,
                    },
                );
            }
        });
    }
}

impl TaskTranscriptStore {
    fn handle(&self) -> Arc<Mutex<HashMap<String, KeptTranscript>>> {
        Arc::clone(&self.shared)
    }
}

fn truncate_description(task: &str) -> String {
    let characters: Vec<char> = task.chars().collect();
    if characters.len() > 60 {
        format!("{}…", characters[..60].iter().collect::<String>())
    } else {
        task.to_owned()
    }
}
