//! Port of `packages/coding-agent/src/core/agent-session.ts`.
//!
//! The integration middle of the app: the one object every run mode — print,
//! rpc, interactive — talks to. It owns the agent, persists what the agent
//! produces, decides when the context has to be compacted, retries what is
//! worth retrying, and holds the operating mode that bounds which tools exist.
//!
//! The modes add their own I/O on top and nothing else.
//!
//! Deviation (class 2): the extension runner is gone
//! (`plans/facts/extension-boundary.md`). Every `emit` it carried is replaced by
//! one of three native things, or by nothing:
//!   - `tool_call` becomes [`PermissionGate`], installed as the agent's
//!     `before_tool_call` — the pre-tool gate.
//!   - the lifecycle events the user's hooks care about become
//!     [`HookDispatcher`] calls at exactly the points the runner emitted from
//!     (extension-boundary §2.2).
//!   - `session_before_compact`/`session_before_tree` cancellation and
//!     replacement, message rewriting, input transformation, resource discovery
//!     and the whole command/tool registration surface fall away with the
//!     extensions that used them.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use notagent_agent::agent::Agent;
use notagent_agent::types::{
    AgentEvent, AgentMessage, AgentState, AgentTool, BoxFuture, QueueMode, ThinkingLevel,
};
use notagent_ai::types::{
    AssistantContent, AssistantMessage, ImageContent, Model, StopReason, TextContent,
    TextOrImageContent, Usage, UserContent, UserMessage,
};
use notagent_ai::utils::retry::RetryCallbacks;
use notagent_ai::utils::text::{content_text, content_text_with};
use notagent_ai::{
    clamp_thinking_level, get_supported_thinking_levels, is_context_overflow,
    is_recoverable_length, is_retryable_assistant_error, models_are_equal,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::{get_agent_dir, get_session_tasks_dir};
use crate::core::auth_guidance::{
    format_no_api_key_found_message, format_no_model_selected_message,
};
use crate::core::bash_executor::{BashExecutorOptions, BashResult, execute_bash_with_operations};
use crate::core::compaction::{
    BranchSummaryResult, CompactionResult, CompactionSettings, GenerateBranchSummaryOptions,
    SummarizationRequest, calculate_context_tokens, collect_entries_for_branch_summary, compact,
    estimate_context_tokens, estimate_tokens, generate_branch_summary, prepare_compaction,
    should_compact,
};
use crate::core::hooks::dispatch::HookDispatcher;
use crate::core::messages::{BashExecutionMessage, CustomMessage};
use crate::core::modes::cycle::{initial_mode_id, next_mode_id};
use crate::core::modes::indicator::estimate_injected_tokens;
use crate::core::modes::{
    MODES_DIR_NAME, Mode, ModeDiagnostic, get_builtin_modes_dir, load_modes, render_mode_block,
    render_mode_injection,
};
use crate::core::prompt_templates::{PromptTemplate, expand_prompt_template};
use crate::core::resource_loader::ResourceLoader;
use crate::core::session_manager::{
    BranchSummaryEntry, SessionEntry, SessionManager, get_latest_compaction_entry,
};
use crate::core::settings_manager::SettingsManager;
use crate::core::skills::Skill;
use crate::core::source_info::{
    SourceInfo, SyntheticSourceInfoOptions, create_synthetic_source_info,
};
use crate::core::system_prompt::{BuildSystemPromptOptions, ContextFile, build_system_prompt};
use crate::core::tasks::manager::{TaskManager, TaskManagerOptions};
use crate::core::tasks::notification::{
    NOTIFICATION_PREVIEW_BYTES, NotificationOutput, TaskNotificationDelivery,
    TaskNotificationDetails, TaskNotificationHost, TaskNotificationMessage,
    TaskNotificationSendOptions, TaskNotifier, TranscriptNotification, active_task_reminder,
};
use crate::core::tasks::store::TaskStore;
use crate::core::todos::reminder::build_pending_todos_reminder;
use crate::core::todos::{Todo, TodoStore};
use crate::core::tools::bash::{BashOperations, create_local_bash_operations};
use crate::core::tools::tool_definition::{
    ToolContext, ToolDefinition, create_tool_definition_from_agent_tool, wrap_tool_definition,
};
use crate::core::tools::{ALL_TOOL_NAMES, ToolName, ToolsOptions, create_all_tool_definitions};
use crate::utils::frontmatter::strip_frontmatter;
use crate::utils::paths::{current_dir, resolve_path_default};
use crate::utils::tool_result_images::normalize_tool_result_images;

/// The three tools that make background work observable. Background execution
/// is gated on all of them, so a mode that drops one drops the capability.
const BACKGROUND_TOOL_NAMES: [&str; 3] = ["task_list", "task_output", "task_stop"];

/// Marks the hidden message a mode switch is delivered in.
///
/// The model reads it — a custom message becomes a user message on the way to
/// the provider — while the transcript leaves it out.
const MODE_BLOCK_TYPE: &str = "mode_block";

/// The thinking levels offered when no model has been chosen yet.
const THINKING_LEVELS: [ThinkingLevel; 5] = [
    ThinkingLevel::Off,
    ThinkingLevel::Minimal,
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
];

// ============================================================================
// Skill block parsing
// ============================================================================

/// A skill block parsed out of a user message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSkillBlock {
    pub name: String,
    pub location: String,
    pub content: String,
    pub user_message: Option<String>,
}

/// Parses a skill block, or `None` when the text does not carry one.
pub fn parse_skill_block(text: &str) -> Option<ParsedSkillBlock> {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"(?s)^<skill name="([^"]+)" location="([^"]+)">\n(.*?)\n</skill>(?:\n\n(.+))?$"#,
        )
        .expect("skill block pattern")
    });

    let captures = PATTERN.captures(text)?;
    Some(ParsedSkillBlock {
        name: captures.get(1)?.as_str().to_string(),
        location: captures.get(2)?.as_str().to_string(),
        content: captures.get(3)?.as_str().to_string(),
        user_message: captures
            .get(4)
            .map(|value| value.as_str().trim().to_string())
            .filter(|value| !value.is_empty()),
    })
}

// ============================================================================
// Events
// ============================================================================

/// Why a compaction ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

impl CompactionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            CompactionReason::Manual => "manual",
            CompactionReason::Threshold => "threshold",
            CompactionReason::Overflow => "overflow",
        }
    }
}

/// Which summarization a retry belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummarizationSource {
    BranchSummary,
    Compaction(CompactionReason),
}

/// Session events, on top of the core agent events the agent itself emits.
///
/// The variants carry their payloads directly, as the TypeScript union does
/// (CONVENTIONS.md §9); boxing them would change the shape of the port.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum AgentSessionEvent {
    /// A core agent event, forwarded unchanged apart from `AgentEnd`.
    Agent(AgentEvent),
    AgentEnd {
        messages: Vec<AgentMessage>,
        will_retry: bool,
    },
    AgentSettled,
    QueueUpdate {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    CompactionStart {
        reason: CompactionReason,
    },
    EntryAppended {
        entry: Box<SessionEntry>,
    },
    SessionInfoChanged {
        name: Option<String>,
    },
    ThinkingLevelChanged {
        level: ThinkingLevel,
    },
    CompactionEnd {
        reason: CompactionReason,
        result: Option<Box<CompactionResult>>,
        aborted: bool,
        will_retry: bool,
        error_message: Option<String>,
    },
    AutoRetryStart {
        attempt: u64,
        max_attempts: u64,
        delay_ms: u64,
        error_message: String,
    },
    AutoRetryEnd {
        success: bool,
        attempt: u64,
        final_error: Option<String>,
    },
    SummarizationRetryScheduled {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error_message: String,
    },
    SummarizationRetryAttemptStart {
        source: SummarizationSource,
    },
    SummarizationRetryFinished,
    BashExecutionUpdate {
        id: Option<String>,
        delta: String,
    },
}

pub type AgentSessionEventListener = Arc<dyn Fn(AgentSessionEvent) + Send + Sync>;

// ============================================================================
// Model runtime seam
// ============================================================================

/// What the model runtime resolved for one request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionAuth {
    pub api_key: Option<String>,
    pub headers: Option<Vec<(String, String)>>,
    pub base_url: Option<String>,
    pub env: Option<Vec<(String, String)>>,
}

/// The slice of `ModelRuntime` the session reads.
///
/// Deviation (class 1): `model-runtime.ts` belongs to workstream B (O-4), so the
/// boundary is a trait rather than the concrete type. Interface request C-13
/// asks B for the implementation; the tests supply their own.
pub trait SessionModelRuntime: Send + Sync {
    fn get_auth<'a>(
        &'a self,
        model: &'a Model,
    ) -> BoxFuture<'a, Result<Option<SessionAuth>, String>>;
    fn has_configured_auth(&self, provider: &str) -> bool;
    fn check_auth<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, bool>;
    fn is_using_oauth(&self, provider: &str) -> bool;
    /// `isUsingSubscription` — OAuth against a provider whose OAuth flow is a
    /// subscription. The footer reads it to mark the cost as covered
    /// (`components/footer.ts:148`).
    fn is_using_subscription(&self, provider: &str) -> bool;
    fn get_available_snapshot(&self) -> Vec<Model>;
    fn get_model(&self, provider: &str, id: &str) -> Option<Model>;
}

/// The error `getAuth` raises when a provider needs a key it does not have.
const AUTH_HEADER_NEEDS_KEY: &str = "authHeader requires a resolved API key";

// ============================================================================
// Configuration
// ============================================================================

/// One model the session can cycle to, with the thinking level it prefers.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedModel {
    pub model: Model,
    pub thinking_level: Option<ThinkingLevel>,
}

pub struct AgentSessionConfig {
    pub agent: Arc<Agent>,
    pub session_manager: SessionManager,
    pub settings_manager: Arc<SettingsManager>,
    pub cwd: String,
    /// Models to cycle through with Ctrl+P (from `--models`).
    pub scoped_models: Vec<ScopedModel>,
    pub resource_loader: Arc<dyn ResourceLoader>,
    pub model_runtime: Arc<dyn SessionModelRuntime>,
    /// Initial active built-in tool names. Unset means the active mode decides.
    pub initial_active_tool_names: Option<Vec<String>>,
    /// Allowlist of tool names. When set, only these are exposed.
    pub allowed_tool_names: Option<Vec<String>>,
    /// Denylist of tool names, applied after the allowlist.
    pub excluded_tool_names: Option<Vec<String>>,
    /// Replaces the built-in tools, for custom runtimes and for tests. They are
    /// wrapped into minimal definitions so the registry stays definition-first
    /// even when a caller hands over plain tools.
    pub base_tools_override: Option<Vec<Arc<dyn AgentTool>>>,
    /// The user's hooks, dispatched at the points the extension runner emitted.
    pub hooks: Option<Arc<HookDispatcher>>,
    /// Why this session was started, for the `SessionStart` hook.
    pub session_start_reason: String,
}

/// Options of [`AgentSession::prompt`].
#[derive(Clone, Default)]
pub struct PromptOptions {
    /// Whether skill commands and prompt templates are expanded. Default: true.
    pub expand_prompt_templates: Option<bool>,
    pub images: Vec<ImageContent>,
    /// How to queue while a run is active. Required when streaming.
    pub streaming_behavior: Option<QueueBehavior>,
    /// Observes whether the prompt got past preflight. RPC mode answers its
    /// caller from here: a queued or accepted prompt is a success even though
    /// the run itself is still going, and only a preflight rejection is the
    /// failure the caller has to hear about.
    pub preflight_result: Option<PreflightResult>,
}

/// Callback of [`PromptOptions::preflight_result`].
pub type PreflightResult = Arc<dyn Fn(bool) + Send + Sync>;

impl std::fmt::Debug for PromptOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PromptOptions")
            .field("expand_prompt_templates", &self.expand_prompt_templates)
            .field("images", &self.images.len())
            .field("streaming_behavior", &self.streaming_behavior)
            .field("preflight_result", &self.preflight_result.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueBehavior {
    Steer,
    FollowUp,
}

/// Result of [`AgentSession::cycle_model`].
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCycleResult {
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    /// Whether the cycle ran over the scoped models or over everything available.
    pub is_scoped: bool,
}

/// Context usage, as the footer prints it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextUsage {
    /// `None` right after a compaction, before the next response lands.
    pub tokens: Option<u64>,
    pub context_window: u64,
    pub percent: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionTokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

/// Statistics for `/session`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionStats {
    pub session_file: Option<String>,
    pub session_id: String,
    pub user_messages: u64,
    pub assistant_messages: u64,
    pub tool_calls: u64,
    pub tool_results: u64,
    pub total_messages: u64,
    pub tokens: SessionTokenTotals,
    pub cost: f64,
    pub context_usage: Option<ContextUsage>,
}

/// A tool as `getAllTools` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub prompt_guidelines: Vec<String>,
    pub source_info: SourceInfo,
}

#[derive(Clone)]
struct ToolDefinitionEntry {
    definition: Arc<dyn ToolDefinition>,
    source_info: SourceInfo,
}

fn estimate_messages_tokens(messages: &[AgentMessage]) -> u64 {
    messages.iter().map(estimate_tokens).sum()
}

// ============================================================================
// Mutable state
// ============================================================================

#[derive(Default)]
struct QueueState {
    /// Pending steering messages, for display. Removed once delivered.
    steering: Vec<String>,
    /// Pending follow-up messages, for display. Removed once delivered.
    follow_up: Vec<String>,
    /// Messages queued to ride along with the next user prompt ("asides").
    pending_next_turn: Vec<CustomMessage>,
}

#[derive(Default)]
struct ModeState {
    /// Modes discovered from the shipped, user and project directories.
    modes: Vec<Mode>,
    diagnostics: Vec<ModeDiagnostic>,
    active_id: Option<String>,
    /// Mode body awaiting delivery on the next user message; consumed once.
    pending_block: Option<String>,
    /// Approval level last delivered, so leaving auto can be announced.
    delivered_approval: Option<String>,
}

#[derive(Default)]
struct ToolState {
    registry: BTreeMap<String, Arc<dyn AgentTool>>,
    definitions: Vec<(String, ToolDefinitionEntry)>,
    prompt_snippets: Vec<(String, String)>,
    prompt_guidelines: Vec<(String, Vec<String>)>,
    base_definitions: Vec<(String, Arc<dyn ToolDefinition>)>,
    /// Base system prompt without per-turn modifications.
    base_system_prompt: String,
    base_system_prompt_options: BuildSystemPromptOptions,
}

/// Background work owned by this session, and the thing that announces it.
///
/// Both are built on first use rather than in the constructor: a manager needs a
/// session id to root its records under, and that id appears once the session
/// has actually been opened. They are dropped and rebuilt when the id changes,
/// so a new session never inherits the previous one's tasks.
#[derive(Default)]
struct TaskState {
    manager: Option<Arc<TaskManager>>,
    notifier: Option<Arc<TaskNotifier>>,
    session_id: Option<String>,
    /// Set after a compaction, so the next turn is told what is still running.
    active_reminder_pending: bool,
}

// ============================================================================
// AgentSession
// ============================================================================

pub struct AgentSession {
    agent: Arc<Agent>,
    session_manager: Mutex<SessionManager>,
    settings_manager: Arc<SettingsManager>,
    model_runtime: Arc<dyn SessionModelRuntime>,
    resource_loader: Arc<dyn ResourceLoader>,
    hooks: Option<Arc<HookDispatcher>>,
    session_start_reason: String,
    cwd: String,

    scoped_models: Mutex<Vec<ScopedModel>>,
    listeners: Mutex<Vec<(u64, AgentSessionEventListener)>>,
    next_listener_id: AtomicU64,
    unsubscribe_agent: Mutex<Option<Box<dyn FnOnce() + Send>>>,

    is_agent_run_active: AtomicBool,
    idle: Arc<tokio::sync::Notify>,

    queues: Mutex<QueueState>,
    modes: Mutex<ModeState>,
    tools: Mutex<ToolState>,
    tasks: Mutex<TaskState>,
    todo_store: Arc<Mutex<TodoStore>>,

    // Compaction state
    compaction_signal: Mutex<Option<CancellationToken>>,
    auto_compaction_signal: Mutex<Option<CancellationToken>>,
    overflow_recovery_attempted: AtomicBool,
    branch_summary_signal: Mutex<Option<CancellationToken>>,

    // Retry state
    retry_signal: Mutex<Option<CancellationToken>>,
    retry_attempt: AtomicU64,

    // Bash state
    bash_signals: Mutex<Vec<CancellationToken>>,
    pending_bash_messages: Mutex<Vec<BashExecutionMessage>>,

    last_assistant_message: Mutex<Option<AssistantMessage>>,
    system_prompt_override: Mutex<Option<String>>,

    initial_active_tool_names: Option<Vec<String>>,
    allowed_tool_names: Option<HashSet<String>>,
    excluded_tool_names: Option<HashSet<String>>,
    base_tools_override: Option<Vec<Arc<dyn AgentTool>>>,

    weak_self: Mutex<Weak<AgentSession>>,
}

impl AgentSession {
    pub fn new(config: AgentSessionConfig) -> Arc<Self> {
        let session = Arc::new(AgentSession {
            agent: Arc::clone(&config.agent),
            session_manager: Mutex::new(config.session_manager),
            settings_manager: config.settings_manager,
            model_runtime: config.model_runtime,
            resource_loader: config.resource_loader,
            hooks: config.hooks,
            session_start_reason: config.session_start_reason,
            cwd: config.cwd,
            scoped_models: Mutex::new(config.scoped_models),
            listeners: Mutex::new(Vec::new()),
            next_listener_id: AtomicU64::new(0),
            unsubscribe_agent: Mutex::new(None),
            is_agent_run_active: AtomicBool::new(false),
            idle: Arc::new(tokio::sync::Notify::new()),
            queues: Mutex::new(QueueState::default()),
            modes: Mutex::new(ModeState::default()),
            tools: Mutex::new(ToolState::default()),
            tasks: Mutex::new(TaskState::default()),
            todo_store: Arc::new(Mutex::new(TodoStore::new())),
            compaction_signal: Mutex::new(None),
            auto_compaction_signal: Mutex::new(None),
            overflow_recovery_attempted: AtomicBool::new(false),
            branch_summary_signal: Mutex::new(None),
            retry_signal: Mutex::new(None),
            retry_attempt: AtomicU64::new(0),
            bash_signals: Mutex::new(Vec::new()),
            pending_bash_messages: Mutex::new(Vec::new()),
            last_assistant_message: Mutex::new(None),
            system_prompt_override: Mutex::new(None),
            initial_active_tool_names: config.initial_active_tool_names,
            allowed_tool_names: config
                .allowed_tool_names
                .map(|names| names.into_iter().collect()),
            excluded_tool_names: config
                .excluded_tool_names
                .map(|names| names.into_iter().collect()),
            base_tools_override: config.base_tools_override,
            weak_self: Mutex::new(Weak::new()),
        });

        *session.weak_self.lock().expect("poisoned") = Arc::downgrade(&session);

        // Always subscribe: session persistence, hooks, auto-compaction and the
        // retry logic all hang off this one subscription.
        let weak = Arc::downgrade(&session);
        let unsubscribe = config.agent.subscribe(Arc::new(move |event, _signal| {
            let weak = weak.clone();
            Box::pin(async move {
                if let Some(session) = weak.upgrade() {
                    session.handle_agent_event(event).await;
                }
            })
        }));
        *session.unsubscribe_agent.lock().expect("poisoned") = Some(Box::new(unsubscribe));

        session.install_agent_next_turn_refresh();
        session.install_agent_after_tool_call();
        session.build_runtime(BuildRuntimeOptions {
            active_tool_names: session.initial_active_tool_names.clone(),
        });

        session
    }

    fn this(&self) -> Arc<AgentSession> {
        self.weak_self
            .lock()
            .expect("poisoned")
            .upgrade()
            .expect("session dropped")
    }

    // =========================================================================
    // Events
    // =========================================================================

    fn emit(&self, event: AgentSessionEvent) {
        let listeners: Vec<AgentSessionEventListener> = self
            .listeners
            .lock()
            .expect("poisoned")
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        for listener in listeners {
            listener(event.clone());
        }
    }

    fn emit_queue_update(&self) {
        let queues = self.queues.lock().expect("poisoned");
        let event = AgentSessionEvent::QueueUpdate {
            steering: queues.steering.clone(),
            follow_up: queues.follow_up.clone(),
        };
        drop(queues);
        self.emit(event);
    }

    /// Subscribes to session events. The returned id unsubscribes.
    pub fn subscribe(&self, listener: AgentSessionEventListener) -> ListenerHandle {
        let id = self.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.listeners
            .lock()
            .expect("poisoned")
            .push((id, listener));
        ListenerHandle {
            session: self.weak_self.lock().expect("poisoned").clone(),
            id,
        }
    }

    fn unsubscribe(&self, id: u64) {
        self.listeners
            .lock()
            .expect("poisoned")
            .retain(|(listener_id, _)| *listener_id != id);
    }

    async fn emit_agent_settled(&self) {
        self.is_agent_run_active.store(false, Ordering::SeqCst);
        if let Some(hooks) = self.hooks.as_ref() {
            hooks.agent_settled().await;
        }
        self.emit(AgentSessionEvent::AgentSettled);
        self.idle.notify_waiters();
    }

    /// Handles one agent event: queue bookkeeping, hooks, listeners, persistence.
    async fn handle_agent_event(self: &Arc<Self>, event: AgentEvent) {
        // A user message that starts is one of ours leaving a queue. Removed
        // before the event is forwarded, so the UI never shows it twice.
        if let AgentEvent::MessageStart { message } = &event
            && let AgentMessage::User(user) = message
        {
            self.overflow_recovery_attempted
                .store(false, Ordering::SeqCst);
            let text = content_text_with(&user.content, "");
            if !text.is_empty() {
                let mut queues = self.queues.lock().expect("poisoned");
                let removed = if let Some(index) =
                    queues.steering.iter().position(|entry| entry == &text)
                {
                    queues.steering.remove(index);
                    true
                } else if let Some(index) = queues.follow_up.iter().position(|entry| entry == &text)
                {
                    queues.follow_up.remove(index);
                    true
                } else {
                    false
                };
                drop(queues);
                if removed {
                    self.emit_queue_update();
                }
            }
        }

        self.dispatch_hooks(&event).await;

        match &event {
            AgentEvent::AgentEnd { messages } => {
                let will_retry = self.will_retry_after_agent_end(messages);
                self.emit(AgentSessionEvent::AgentEnd {
                    messages: messages.clone(),
                    will_retry,
                });
            }
            other => self.emit(AgentSessionEvent::Agent(other.clone())),
        }

        if let AgentEvent::MessageEnd { message } = &event {
            match message {
                AgentMessage::Custom(custom) => {
                    let mut manager = self.session_manager.lock().expect("poisoned");
                    let _ = manager.append_custom_message_entry(
                        &custom.custom_type,
                        serde_json::to_value(&custom.content).unwrap_or(Value::Null),
                        custom.display,
                        custom.details.clone(),
                    );
                }
                AgentMessage::User(_)
                | AgentMessage::Assistant(_)
                | AgentMessage::ToolResult(_) => {
                    let value = serde_json::to_value(message).unwrap_or(Value::Null);
                    let mut manager = self.session_manager.lock().expect("poisoned");
                    let _ = manager.append_message_value(value);
                }
                // bashExecution, compactionSummary and branchSummary are
                // persisted where they are produced.
                _ => {}
            }

            if let AgentMessage::Assistant(assistant) = message {
                *self.last_assistant_message.lock().expect("poisoned") = Some(assistant.clone());

                if assistant.stop_reason != StopReason::Error
                    && assistant.stop_reason != StopReason::Length
                {
                    self.overflow_recovery_attempted
                        .store(false, Ordering::SeqCst);
                }

                // Reset the retry counter as soon as a response lands, so
                // attempts do not accumulate across the calls of one turn.
                let attempt = self.retry_attempt.load(Ordering::SeqCst);
                if assistant.stop_reason != StopReason::Error && attempt > 0 {
                    self.emit(AgentSessionEvent::AutoRetryEnd {
                        success: true,
                        attempt,
                        final_error: None,
                    });
                    self.retry_attempt.store(0, Ordering::SeqCst);
                }
            }
        }
    }

    /// The hook dispatch points, at exactly the places the extension runner
    /// emitted from (`plans/facts/extension-boundary.md` §2.2).
    async fn dispatch_hooks(self: &Arc<Self>, event: &AgentEvent) {
        let Some(hooks) = self.hooks.as_ref() else {
            // Even without hooks, the end of a turn with no tool results is
            // where open todos have to be handed back.
            if let AgentEvent::TurnEnd { tool_results, .. } = event
                && tool_results.is_empty()
            {
                self.remind_about_open_todos();
            }
            return;
        };

        match event {
            AgentEvent::AgentEnd { messages } => hooks.agent_end(messages),
            // No tool results means no further tool calls, which is the point
            // the run would end. That is exactly when open todos have to be
            // handed back — a checklist the agent walks away from is worse than
            // none, because it tells the user work is tracked while it no
            // longer is.
            AgentEvent::TurnEnd { tool_results, .. } if tool_results.is_empty() => {
                self.remind_about_open_todos();
            }
            _ => {}
        }
    }

    fn will_retry_after_agent_end(&self, messages: &[AgentMessage]) -> bool {
        let settings = self.settings_manager.get_retry_settings();
        if !settings.enabled || self.retry_attempt.load(Ordering::SeqCst) >= settings.max_retries {
            return false;
        }

        for message in messages.iter().rev() {
            if let AgentMessage::Assistant(assistant) = message {
                return self.is_retryable_error(assistant);
            }
        }
        false
    }

    /// The last assistant message in agent state, aborted ones included.
    fn find_last_assistant_message(&self) -> Option<AssistantMessage> {
        self.agent
            .state()
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                AgentMessage::Assistant(assistant) => Some(assistant.clone()),
                _ => None,
            })
    }

    /// `_installAgentHooks`' `afterToolCall` (`agent-session.ts:567-597`),
    /// minus the extension runner: what is left is the `PostToolUse` hook —
    /// which the extension mapped from its `tool_result` event
    /// (`plans/facts/extension-boundary.md` §2.2) — and the image
    /// normalisation that runs after it, so images a hook could have replaced
    /// are normalised too.
    fn install_agent_after_tool_call(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.agent.update_options(|options| {
            let previous = options.after_tool_call.clone();
            options.after_tool_call = Some(Arc::new(move |context, signal| {
                let weak = weak.clone();
                let previous = previous.clone();
                Box::pin(async move {
                    let earlier = match previous {
                        Some(previous) => previous(context.clone(), signal).await,
                        None => None,
                    };
                    let Some(session) = weak.upgrade() else {
                        return earlier;
                    };
                    let content = earlier
                        .as_ref()
                        .and_then(|result| result.content.clone())
                        .or_else(|| Some(context.result.content.clone()))
                        .unwrap_or_default();
                    if let Some(hooks) = session.hooks.as_ref() {
                        let input = match &context.args {
                            serde_json::Value::Object(args) => args.clone(),
                            _ => serde_json::Map::new(),
                        };
                        hooks
                            .tool_result(
                                &context.tool_call.name,
                                &input,
                                &content,
                                context.is_error,
                            )
                            .await;
                    }
                    let normalized = normalize_tool_result_images(
                        &content,
                        session.settings_manager.get_image_auto_resize(),
                    );
                    match (earlier, normalized) {
                        (None, None) => None,
                        (earlier, normalized) => {
                            let mut result = earlier.unwrap_or_default();
                            if let Some(normalized) = normalized {
                                result.content = Some(normalized);
                            } else if result.content.is_none() {
                                result.content = Some(content);
                            }
                            Some(result)
                        }
                    }
                })
            }));
        });
    }

    fn install_agent_next_turn_refresh(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.agent.update_options(|options| {
            let previous = options.prepare_next_turn.clone();
            options.prepare_next_turn = Some(Arc::new(move |turn| {
                let weak = weak.clone();
                let previous = previous.clone();
                Box::pin(async move {
                    let mut snapshot = match previous {
                        Some(previous) => previous(turn.clone()).await,
                        None => None,
                    };
                    let Some(session) = weak.upgrade() else {
                        return snapshot;
                    };
                    let state = session.agent.state();
                    let system_prompt = session
                        .system_prompt_override
                        .lock()
                        .expect("poisoned")
                        .clone()
                        .unwrap_or_else(|| {
                            session
                                .tools
                                .lock()
                                .expect("poisoned")
                                .base_system_prompt
                                .clone()
                        });
                    let update = snapshot.get_or_insert_with(Default::default);
                    let mut context = update.context.clone().unwrap_or(turn.context);
                    context.system_prompt = system_prompt;
                    context.tools = Some(state.tools.clone());
                    update.context = Some(context);
                    update.model = Some(state.model.clone());
                    update.thinking_level = Some(state.thinking_level);
                    snapshot
                })
            }));
        });
    }
}

/// Handle returned by [`AgentSession::subscribe`]; drop it to unsubscribe.
pub struct ListenerHandle {
    session: Weak<AgentSession>,
    id: u64,
}

impl Drop for ListenerHandle {
    fn drop(&mut self) {
        if let Some(session) = self.session.upgrade() {
            session.unsubscribe(self.id);
        }
    }
}

struct BuildRuntimeOptions {
    active_tool_names: Option<Vec<String>>,
}

// ============================================================================
// Operating modes
// ============================================================================

impl AgentSession {
    /// Loads the mode folders.
    ///
    /// `known_tool_names` decides which names a mode's tool delta may use. It is
    /// the built-in set on the first pass, because the registry does not exist
    /// yet, and the assembled one on the second — which is what lets a mode name
    /// a tool that was registered outside the built-ins without being told it
    /// does not exist.
    fn load_modes(&self, known_tool_names: Option<HashSet<String>>) {
        let roots = vec![
            get_builtin_modes_dir(),
            get_agent_dir().join(MODES_DIR_NAME),
            Path::new(&self.cwd).join(".notagent").join(MODES_DIR_NAME),
        ];
        let names = known_tool_names.unwrap_or_else(|| {
            ALL_TOOL_NAMES
                .iter()
                .map(|name| name.as_str().to_string())
                .collect()
        });
        let result = load_modes(&roots, &names).unwrap_or_default();

        let mut modes = self.modes.lock().expect("poisoned");
        modes.modes = result.modes;
        modes.diagnostics = result.diagnostics;
        let active_known = modes
            .active_id
            .as_ref()
            .is_some_and(|id| modes.modes.iter().any(|mode| &mode.id == id));
        if !active_known {
            modes.active_id = initial_mode_id(&modes.modes);
        }
    }

    /// All discovered modes, in load order.
    pub fn modes(&self) -> Vec<Mode> {
        self.modes.lock().expect("poisoned").modes.clone()
    }

    /// Problems found while loading mode folders, for surfacing to the user.
    pub fn mode_diagnostics(&self) -> Vec<ModeDiagnostic> {
        self.modes.lock().expect("poisoned").diagnostics.clone()
    }

    /// The active mode, or `None` when no mode folder could be loaded.
    pub fn active_mode(&self) -> Option<Mode> {
        let modes = self.modes.lock().expect("poisoned");
        let id = modes.active_id.as_ref()?;
        modes.modes.iter().find(|mode| &mode.id == id).cloned()
    }

    /// Every tool name this session can resolve. This is the set a mode is
    /// validated against and the set a subagent's tools are resolved from, so
    /// the two can never disagree about what exists.
    fn known_tool_names(&self) -> HashSet<String> {
        let mut names: HashSet<String> = ALL_TOOL_NAMES
            .iter()
            .map(|name| name.as_str().to_string())
            .collect();
        for (name, _) in &self.tools.lock().expect("poisoned").definitions {
            names.insert(name.clone());
        }
        names
    }

    /// Tool names the active mode's shell permits. Falls back to the read-only
    /// set when no mode loaded, so a broken mode directory cannot silently grant
    /// write access.
    fn active_mode_tool_names(&self) -> Vec<String> {
        match self.active_mode() {
            Some(mode) => mode
                .tools
                .iter()
                .map(|name| name.as_str().to_string())
                .collect(),
            None => vec![
                "read".to_string(),
                "read_minified".to_string(),
                "grep".to_string(),
                "find".to_string(),
                "ls".to_string(),
            ],
        }
    }

    /// Text injected when the active mode becomes current.
    pub fn active_mode_injection(&self) -> String {
        self.active_mode()
            .map(|mode| render_mode_injection(&mode))
            .unwrap_or_default()
    }

    /// Estimated token volume of the active mode's injected text.
    pub fn active_mode_injected_tokens(&self) -> usize {
        estimate_injected_tokens(&self.active_mode_injection())
    }

    /// Switches to `mode_id` and recomputes the tool registry from its shell.
    pub fn set_mode(&self, mode_id: &str) -> Option<Mode> {
        let mode = {
            let modes = self.modes.lock().expect("poisoned");
            modes
                .modes
                .iter()
                .find(|mode| mode.id == mode_id)
                .cloned()?
        };
        let previous_approval = self
            .active_mode()
            .map(|mode| mode.approval.as_str().to_string());
        {
            let mut modes = self.modes.lock().expect("poisoned");
            modes.active_id = Some(mode.id.clone());
        }
        let tools: Vec<String> = mode
            .tools
            .iter()
            .map(|name| name.as_str().to_string())
            .collect();
        self.refresh_tool_registry(Some(tools));
        let block = self.render_mode_block(&mode, previous_approval.as_deref());
        self.modes.lock().expect("poisoned").pending_block = block;
        Some(mode)
    }

    /// Builds the block delivered on the next user message.
    ///
    /// Leaving auto is announced explicitly rather than left to be inferred from
    /// the new block: the model needs to know that approvals are back, not
    /// merely that some other mode is now in force.
    fn render_mode_block(&self, mode: &Mode, previous_approval: Option<&str>) -> Option<String> {
        let leaving_auto = previous_approval == Some("auto") && mode.approval.as_str() != "auto";
        render_mode_block(
            mode,
            leaving_auto.then_some(
                "Auto approval is no longer active. Tool use is confirmed again, so expect approval prompts and refusals.",
            ),
        )
    }

    /// Re-arms the block when a prompt-suppressing mode is no longer represented
    /// in the conversation, which happens when compaction drops it. Without this
    /// a long session silently loses track of its own autonomy level — and that
    /// matters precisely where no prompt would reveal the loss.
    fn rearm_mode_block_if_absent(&self) {
        if self.modes.lock().expect("poisoned").pending_block.is_some() {
            return;
        }
        let Some(mode) = self.active_mode() else {
            return;
        };
        // Only the stops that suppress approval prompts are worth restating. The
        // supervised default announces itself every time a prompt appears, so
        // repeating it would add noise to every session without informing anyone.
        if mode.approval.as_str() == "manual" {
            return;
        }
        let marker = format!("<mode name=\"{}\"", mode.id);
        let carries = |content: &Value| -> bool {
            match content {
                Value::String(text) => text.contains(&marker),
                Value::Array(blocks) => blocks.iter().any(|block| {
                    block.get("type").and_then(Value::as_str) == Some("text")
                        && block
                            .get("text")
                            .and_then(Value::as_str)
                            .is_some_and(|text| text.contains(&marker))
                }),
                _ => false,
            }
        };
        // Both shapes are searched: the block now rides in its own hidden
        // message, and transcripts written before that carry it inside the
        // user's own text.
        let present = self
            .session_manager
            .lock()
            .expect("poisoned")
            .get_entries()
            .iter()
            .any(|entry| match entry {
                SessionEntry::CustomMessage(entry) => {
                    entry.custom_type == MODE_BLOCK_TYPE && carries(&entry.content)
                }
                SessionEntry::Message(entry) => {
                    entry.message.get("role").and_then(Value::as_str) == Some("user")
                        && entry.message.get("content").is_some_and(carries)
                }
                _ => false,
            });
        if !present {
            let delivered = self
                .modes
                .lock()
                .expect("poisoned")
                .delivered_approval
                .clone();
            let block = self.render_mode_block(&mode, delivered.as_deref());
            self.modes.lock().expect("poisoned").pending_block = block;
        }
    }

    /// Returns the pending mode block, if any, and clears it.
    fn consume_pending_mode_block(&self) -> Option<String> {
        self.rearm_mode_block_if_absent();
        let block = self.modes.lock().expect("poisoned").pending_block.take();
        if block.is_some() {
            let approval = self
                .active_mode()
                .map(|mode| mode.approval.as_str().to_string());
            self.modes.lock().expect("poisoned").delivered_approval = approval;
        }
        block
    }

    /// Whether a mode switch is still waiting to be delivered.
    pub fn has_pending_mode_block(&self) -> bool {
        self.modes.lock().expect("poisoned").pending_block.is_some()
    }

    /// Advances the mode ring and applies the result.
    pub fn cycle_mode(&self) -> Option<Mode> {
        let (modes, current) = {
            let state = self.modes.lock().expect("poisoned");
            (
                state.modes.clone(),
                state.active_id.clone().unwrap_or_default(),
            )
        };
        let next = next_mode_id(&modes, &current)?;
        self.set_mode(&next)
    }
}

// ============================================================================
// Tools and the system prompt
// ============================================================================

impl AgentSession {
    /// Names of the tools currently set on the agent.
    pub fn get_active_tool_names(&self) -> Vec<String> {
        self.agent
            .state()
            .tools
            .iter()
            .map(|tool| tool.name().to_string())
            .collect()
    }

    /// Every configured tool with its schema, guidelines and origin.
    pub fn get_all_tools(&self) -> Vec<ToolInfo> {
        self.tools
            .lock()
            .expect("poisoned")
            .definitions
            .iter()
            .map(|(_, entry)| ToolInfo {
                name: entry.definition.name().to_string(),
                description: entry.definition.description().to_string(),
                parameters: entry.definition.parameters().clone(),
                prompt_guidelines: entry.definition.prompt_guidelines(),
                source_info: entry.source_info.clone(),
            })
            .collect()
    }

    pub fn get_tool_definition(&self, name: &str) -> Option<Arc<dyn ToolDefinition>> {
        self.tools
            .lock()
            .expect("poisoned")
            .definitions
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, entry)| Arc::clone(&entry.definition))
    }

    /// Sets the active tools by name. Unknown names are ignored, and the system
    /// prompt is rebuilt to match. Takes effect on the next turn.
    pub fn set_active_tools_by_name(&self, tool_names: &[String]) {
        let mut tools: Vec<Arc<dyn AgentTool>> = Vec::new();
        let mut valid: Vec<String> = Vec::new();
        {
            let state = self.tools.lock().expect("poisoned");
            for name in tool_names {
                if let Some(tool) = state.registry.get(name) {
                    tools.push(Arc::clone(tool));
                    valid.push(name.clone());
                }
            }
        }
        self.agent.set_tools(tools);

        let prompt = self.rebuild_system_prompt(&valid);
        self.tools.lock().expect("poisoned").base_system_prompt = prompt.clone();
        let effective = self
            .system_prompt_override
            .lock()
            .expect("poisoned")
            .clone()
            .unwrap_or(prompt);
        self.agent.set_system_prompt(effective);
    }

    fn normalize_prompt_snippet(text: Option<&str>) -> Option<String> {
        let text = text?;
        let one_line = text
            .replace(['\r', '\n'], " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (!one_line.is_empty()).then_some(one_line)
    }

    fn normalize_prompt_guidelines(guidelines: Vec<String>) -> Vec<String> {
        let mut unique: Vec<String> = Vec::new();
        for guideline in guidelines {
            let normalized = guideline.trim().to_string();
            if !normalized.is_empty() && !unique.contains(&normalized) {
                unique.push(normalized);
            }
        }
        unique
    }

    fn rebuild_system_prompt(&self, tool_names: &[String]) -> String {
        let (valid_tool_names, tool_snippets, prompt_guidelines) = {
            let state = self.tools.lock().expect("poisoned");
            let valid: Vec<String> = tool_names
                .iter()
                .filter(|name| state.registry.contains_key(*name))
                .cloned()
                .collect();
            let mut snippets: Vec<(String, String)> = Vec::new();
            let mut guidelines: Vec<String> = Vec::new();
            for name in &valid {
                if let Some((_, snippet)) =
                    state.prompt_snippets.iter().find(|(key, _)| key == name)
                {
                    snippets.push((name.clone(), snippet.clone()));
                }
                if let Some((_, tool_guidelines)) =
                    state.prompt_guidelines.iter().find(|(key, _)| key == name)
                {
                    guidelines.extend(tool_guidelines.clone());
                }
            }
            (valid, snippets, guidelines)
        };

        let loader_system_prompt = self.resource_loader.get_system_prompt();
        let loader_append = self.resource_loader.get_append_system_prompt();
        let append_system_prompt = (!loader_append.is_empty()).then(|| loader_append.join("\n\n"));
        let skills: Vec<Skill> = self.resource_loader.get_skills().0;
        let context_files: Vec<ContextFile> = self
            .resource_loader
            .get_agents_files()
            .into_iter()
            .map(|file| ContextFile {
                path: file.path,
                content: file.content,
            })
            .collect();

        let options = BuildSystemPromptOptions {
            cwd: self.cwd.clone(),
            skills,
            context_files,
            custom_prompt: loader_system_prompt,
            append_system_prompt,
            selected_tools: Some(valid_tool_names),
            tool_snippets,
            prompt_guidelines,
        };
        let prompt = build_system_prompt(&options);
        self.tools
            .lock()
            .expect("poisoned")
            .base_system_prompt_options = options;
        prompt
    }

    fn is_allowed_tool(&self, name: &str) -> bool {
        self.allowed_tool_names
            .as_ref()
            .is_none_or(|allowed| allowed.contains(name))
            && !self
                .excluded_tool_names
                .as_ref()
                .is_some_and(|excluded| excluded.contains(name))
    }

    /// Rebuilds the registry from the base definitions and re-applies the active
    /// set.
    fn refresh_tool_registry(&self, active_tool_names: Option<Vec<String>>) {
        let previous_active = self.get_active_tool_names();

        let base: Vec<(String, Arc<dyn ToolDefinition>)> = self
            .tools
            .lock()
            .expect("poisoned")
            .base_definitions
            .iter()
            .filter(|(name, _)| self.is_allowed_tool(name))
            .cloned()
            .collect();

        let definitions: Vec<(String, ToolDefinitionEntry)> = base
            .iter()
            .map(|(name, definition)| {
                (
                    name.clone(),
                    ToolDefinitionEntry {
                        definition: Arc::clone(definition),
                        source_info: create_synthetic_source_info(
                            format!("<builtin:{name}>"),
                            SyntheticSourceInfoOptions::new("builtin"),
                        ),
                    },
                )
            })
            .collect();

        let context_factory = self.tool_context_factory();
        let registry: BTreeMap<String, Arc<dyn AgentTool>> = base
            .iter()
            .map(|(name, definition)| {
                (
                    name.clone(),
                    wrap_tool_definition(Arc::clone(definition), Some(context_factory.clone())),
                )
            })
            .collect();

        {
            let mut state = self.tools.lock().expect("poisoned");
            state.prompt_snippets = definitions
                .iter()
                .filter_map(|(name, entry)| {
                    Self::normalize_prompt_snippet(entry.definition.prompt_snippet())
                        .map(|snippet| (name.clone(), snippet))
                })
                .collect();
            state.prompt_guidelines = definitions
                .iter()
                .filter_map(|(name, entry)| {
                    let guidelines =
                        Self::normalize_prompt_guidelines(entry.definition.prompt_guidelines());
                    (!guidelines.is_empty()).then_some((name.clone(), guidelines))
                })
                .collect();
            state.definitions = definitions;
            state.registry = registry;
        }

        let mut next_active: Vec<String> = active_tool_names
            .unwrap_or(previous_active)
            .into_iter()
            .filter(|name| self.is_allowed_tool(name))
            .collect();

        if let Some(allowed) = self.allowed_tool_names.as_ref() {
            let registry_names: Vec<String> = self
                .tools
                .lock()
                .expect("poisoned")
                .registry
                .keys()
                .cloned()
                .collect();
            for name in registry_names {
                if allowed.contains(&name) {
                    next_active.push(name);
                }
            }
        }

        let mut seen: HashSet<String> = HashSet::new();
        next_active.retain(|name| seen.insert(name.clone()));
        self.set_active_tools_by_name(&next_active);
    }

    /// What a tool reads about the session it runs in. In TypeScript this comes
    /// from the extension context; the four fields the built-ins actually use
    /// are the same (`plans/facts/extension-boundary.md` §2.4).
    fn tool_context_factory(&self) -> Arc<dyn Fn() -> ToolContext + Send + Sync> {
        let weak = self.weak_self.lock().expect("poisoned").clone();
        Arc::new(move || {
            let Some(session) = weak.upgrade() else {
                return ToolContext::default();
            };
            let state = session.agent.state();
            let manager = session.session_manager.lock().expect("poisoned");
            ToolContext {
                session_id: Some(manager.get_session_id().to_string()),
                session_file: manager.get_session_file().map(str::to_string),
                thinking_level: Some(thinking_level_name(state.thinking_level).to_string()),
                model: Some(state.model.clone()),
            }
        })
    }

    /// Builds the tools, the modes and the system prompt from scratch.
    fn build_runtime(self: &Arc<Self>, options: BuildRuntimeOptions) {
        let auto_resize_images = self.settings_manager.get_image_auto_resize();
        let shell_command_prefix = self.settings_manager.get_shell_command_prefix();
        let shell_path = self.settings_manager.get_shell_path();

        // Modes are loaded first because two tools describe them: the skill tool
        // resolves their names, and the delegation tool lists them as the agent
        // types it can spawn. Building the tools before the modes exist would
        // leave both advertising an empty roster.
        self.load_modes(None);

        let base_definitions: Vec<(String, Arc<dyn ToolDefinition>)> = match self
            .base_tools_override
            .as_ref()
        {
            Some(tools) => tools
                .iter()
                .map(|tool| {
                    (
                        tool.name().to_string(),
                        create_tool_definition_from_agent_tool(Arc::clone(tool)),
                    )
                })
                .collect(),
            None => {
                let tool_options =
                    self.build_tool_options(auto_resize_images, shell_command_prefix, shell_path);
                create_all_tool_definitions(&self.cwd, Some(&tool_options))
                    .into_iter()
                    .map(|(name, definition)| (name.as_str().to_string(), definition))
                    .collect()
            }
        };
        {
            let mut state = self.tools.lock().expect("poisoned");
            state.base_definitions = base_definitions;
        }

        // With overridden tools the active set is simply all of them: there is
        // no mode shell to consult, because the caller replaced the roster the
        // shell bounds.
        let default_active = match self.base_tools_override.as_ref() {
            Some(tools) => tools.iter().map(|tool| tool.name().to_string()).collect(),
            None => self.active_mode_tool_names(),
        };
        let base_active = options.active_tool_names.clone().unwrap_or(default_active);
        self.refresh_tool_registry(Some(base_active));

        // Re-read the modes now that the registry exists. The first pass could
        // only validate a tool delta against the built-ins, so a mode naming a
        // tool from outside them was reported as naming something unknown and
        // quietly had it dropped; this pass sees the same set the tools are
        // resolved from.
        let assembled = self.known_tool_names();
        if assembled.len() != ALL_TOOL_NAMES.len() {
            let before = self.active_mode_tool_names().join("\u{0}");
            self.load_modes(Some(assembled));
            // Only refresh when the reload actually changed what the active mode
            // permits. Refreshing unconditionally would recompute the active set
            // from the mode and discard whatever the caller had activated.
            if options.active_tool_names.is_none()
                && self.base_tools_override.is_none()
                && self.active_mode_tool_names().join("\u{0}") != before
            {
                self.refresh_tool_registry(Some(self.active_mode_tool_names()));
            }
        }
    }

    fn build_tool_options(
        self: &Arc<Self>,
        auto_resize_images: bool,
        shell_command_prefix: Option<String>,
        shell_path: Option<String>,
    ) -> ToolsOptions {
        use crate::core::tools::bash::{BashToolOptions, BashToolSources};
        use crate::core::tools::read::ReadToolOptions;
        use crate::core::tools::skill::{SkillToolSkill, SkillToolSources};
        use crate::core::tools::task::{TaskToolSources, TaskTranscriptStore};
        use crate::core::tools::task_tools::TaskToolsSources;
        use crate::core::tools::todo_write::TodoWriteToolSources;

        let bash_options = BashToolOptions {
            command_prefix: shell_command_prefix.clone(),
            shell_path: shell_path.clone(),
            sources: Some(BashToolSources {
                manager: {
                    let weak = Arc::downgrade(self);
                    Arc::new(move || {
                        weak.upgrade()
                            .and_then(|session| session.task_manager())
                            .map(|manager| {
                                manager as Arc<dyn crate::core::tools::bash::BashTaskManager>
                            })
                    })
                },
                background_allowed: {
                    let weak = Arc::downgrade(self);
                    Arc::new(move || {
                        weak.upgrade()
                            .is_some_and(|session| session.background_allowed())
                    })
                },
                auto_background_on_timeout: None,
            }),
            ..BashToolOptions::default()
        };

        let skill_sources = SkillToolSources {
            skills: {
                let weak = Arc::downgrade(self);
                Arc::new(move || {
                    weak.upgrade()
                        .map(|session| {
                            session
                                .resource_loader
                                .get_skills()
                                .0
                                .into_iter()
                                .map(|skill| SkillToolSkill {
                                    name: skill.name,
                                    file_path: skill.file_path,
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                })
            },
            modes: {
                let weak = Arc::downgrade(self);
                Arc::new(move || {
                    weak.upgrade()
                        .map(|session| session.modes())
                        .unwrap_or_default()
                })
            },
        };

        let task_sources = TaskToolSources {
            modes: {
                let weak = Arc::downgrade(self);
                Arc::new(move || {
                    weak.upgrade()
                        .map(|session| session.modes())
                        .unwrap_or_default()
                })
            },
            parent_shell: {
                let weak = Arc::downgrade(self);
                Arc::new(move || {
                    weak.upgrade()
                        .and_then(|session| session.active_mode())
                        .map(|mode| mode.shell)
                })
            },
            parent_mode: Some({
                let weak = Arc::downgrade(self);
                Arc::new(move || weak.upgrade().and_then(|session| session.active_mode()))
            }),
            parent: {
                let weak = Arc::downgrade(self);
                Arc::new(move || weak.upgrade().map(|session| Arc::clone(&session.agent)))
            },
            manager: Some({
                let weak = Arc::downgrade(self);
                Arc::new(move || {
                    weak.upgrade()
                        .and_then(|session| session.task_manager())
                        .map(|manager| (*manager).clone())
                })
            }),
            background_allowed: Some({
                let weak = Arc::downgrade(self);
                Arc::new(move || {
                    weak.upgrade()
                        .is_some_and(|session| session.background_allowed())
                })
            }),
            resolve_tool: Some({
                let weak = Arc::downgrade(self);
                Arc::new(move |name: ToolName| {
                    weak.upgrade()
                        .and_then(|session| session.resolve_child_tool(name.as_str()))
                })
            }),
            subagent_model: Some({
                let weak = Arc::downgrade(self);
                Arc::new(move || weak.upgrade().and_then(|session| session.subagent_model()))
            }),
            // A child's tools are built from these options, and they carry no
            // task wiring at all: that is what makes "a subagent starts no
            // background work" structural rather than a rule it could ignore.
            tool_options: Some(Arc::new(move || {
                Some(ToolsOptions {
                    read: Some(ReadToolOptions {
                        auto_resize_images,
                        ..ReadToolOptions::default()
                    }),
                    bash: Some(BashToolOptions {
                        command_prefix: shell_command_prefix.clone(),
                        shell_path: shell_path.clone(),
                        ..BashToolOptions::default()
                    }),
                    ..ToolsOptions::default()
                })
            })),
            transcripts: TaskTranscriptStore::default(),
            cwd: Some({
                let cwd = self.cwd.clone();
                Arc::new(move || cwd.clone())
            }),
        };

        ToolsOptions {
            read: Some(ReadToolOptions {
                auto_resize_images,
                ..ReadToolOptions::default()
            }),
            bash: Some(bash_options),
            skill: Some(skill_sources),
            task: Some(task_sources),
            tasks: Some(TaskToolsSources {
                manager: {
                    let weak = Arc::downgrade(self);
                    Arc::new(move || {
                        weak.upgrade()
                            .and_then(|session| session.task_manager())
                            .map(|manager| (*manager).clone())
                    })
                },
            }),
            todo_write: Some(TodoWriteToolSources {
                store: {
                    let store = Arc::clone(&self.todo_store);
                    Arc::new(move || Some(Arc::clone(&store)))
                },
            }),
            ..ToolsOptions::default()
        }
    }

    /// Builds one tool for a subagent.
    ///
    /// Resolved from the parent's assembled registry, so a tool reaches a child
    /// exactly as it reaches its parent. What a child may *ask* for is still its
    /// mode's allowlist minus the structural exclusions — this only decides
    /// where a permitted name is looked up.
    fn resolve_child_tool(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools
            .lock()
            .expect("poisoned")
            .registry
            .get(name)
            .cloned()
    }
}

fn thinking_level_name(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

// ============================================================================
// Background tasks and todos
// ============================================================================

impl AgentSession {
    /// Whether this session may detach work.
    ///
    /// All three observation tools or none: a model that can start a background
    /// task but not list, read or stop one has been handed something it cannot
    /// follow up on, which is worse than not having it. The check is on the
    /// active set rather than on configuration, so switching to a mode without
    /// them takes the capability away for as long as that mode is in force.
    fn background_allowed(&self) -> bool {
        let active: HashSet<String> = self.get_active_tool_names().into_iter().collect();
        BACKGROUND_TOOL_NAMES
            .iter()
            .all(|name| active.contains(*name))
    }

    /// The task manager for the current session, built on first use.
    ///
    /// Rebuilt when the session id changes. Tasks belong to the conversation
    /// that started them, and carrying them into a new one would announce
    /// results nobody asked for into a conversation that has never heard of them.
    pub fn task_manager(&self) -> Option<Arc<TaskManager>> {
        let session_id = self
            .session_manager
            .lock()
            .expect("poisoned")
            .get_session_id()
            .to_string();
        if session_id.is_empty() {
            return None;
        }

        {
            let tasks = self.tasks.lock().expect("poisoned");
            if let Some(manager) = tasks.manager.as_ref()
                && tasks.session_id.as_deref() == Some(session_id.as_str())
            {
                return Some(Arc::clone(manager));
            }
        }

        let previous = self.tasks.lock().expect("poisoned").manager.take();
        if let Some(previous) = previous {
            tokio::spawn(async move {
                previous.shutdown(Some("Session replaced")).await;
            });
        }

        let this = self.this();
        let weak = Arc::downgrade(&this);
        let manager = Arc::new(TaskManager::new(
            Arc::new(TaskStore::new(get_session_tasks_dir(&session_id))),
            TaskManagerOptions {
                on_terminated: Some(Arc::new(move |info, _reason| {
                    let notifier = weak.upgrade().and_then(|session| {
                        session.tasks.lock().expect("poisoned").notifier.clone()
                    });
                    let Some(notifier) = notifier else {
                        return;
                    };
                    if tokio::runtime::Handle::try_current().is_ok() {
                        tokio::spawn(async move {
                            notifier.notify(&info).await;
                        });
                    }
                })),
                ..TaskManagerOptions::default()
            },
        ));

        let notifier = Arc::new(TaskNotifier::new(Arc::new(SessionNotificationHost {
            session: Arc::downgrade(&this),
            manager: Arc::clone(&manager),
        })));

        {
            let mut tasks = self.tasks.lock().expect("poisoned");
            tasks.manager = Some(Arc::clone(&manager));
            tasks.notifier = Some(Arc::clone(&notifier));
            tasks.session_id = Some(session_id);
        }

        // Whatever the previous process left running is reported once, here,
        // rather than silently disappearing between one start and the next.
        let reconcile_manager = Arc::clone(&manager);
        let reconcile_notifier = Arc::clone(&notifier);
        tokio::spawn(async move {
            for info in reconcile_manager.reconcile().await {
                reconcile_notifier.notify(&info).await;
            }
        });

        Some(manager)
    }

    /// The post-compaction reminder, consumed once.
    ///
    /// Built at prompt time rather than when the compaction ended, because a
    /// task that settled in between should not be re-announced as still running.
    fn pending_active_task_reminder(&self) -> Option<CustomMessage> {
        {
            let mut tasks = self.tasks.lock().expect("poisoned");
            if !tasks.active_reminder_pending {
                return None;
            }
            tasks.active_reminder_pending = false;
        }
        let active = self
            .tasks
            .lock()
            .expect("poisoned")
            .manager
            .as_ref()
            .map(|manager| manager.list(true, None))
            .unwrap_or_default();
        let text = active_task_reminder(&active)?;
        Some(CustomMessage {
            custom_type: "active_tasks".to_string(),
            content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(text))]),
            display: false,
            details: None,
            timestamp: now_millis(),
        })
    }

    /// Hands the open todos back so the run continues instead of ending.
    ///
    /// Queued as a follow-up, which the loop drains precisely where it would
    /// otherwise stop. Fires once per distinct set of open items: repeating the
    /// same reminder on every attempt to stop would be a loop, while a changed
    /// set is new information.
    fn remind_about_open_todos(&self) {
        let active = self.todo_store.lock().expect("poisoned").active();
        if active.is_empty() {
            return;
        }
        let messages = self.agent.state().messages;
        let Some(reminder) = build_pending_todos_reminder(&active, &messages) else {
            return;
        };
        self.agent.follow_up(AgentMessage::Custom(CustomMessage {
            custom_type: reminder.custom_type,
            content: reminder.content,
            display: reminder.display,
            details: serde_json::to_value(&reminder.details).ok(),
            timestamp: now_millis(),
        }));
    }

    /// The session's task list, for the UI that renders it.
    pub fn todos(&self) -> Vec<Todo> {
        self.todo_store.lock().expect("poisoned").all().to_vec()
    }

    /// The store itself, for the tool that writes it.
    pub fn todo_store(&self) -> Arc<Mutex<TodoStore>> {
        Arc::clone(&self.todo_store)
    }

    /// Stops everything this session started. Called when it is disposed.
    fn stop_background_tasks(&self) {
        let manager = {
            let mut tasks = self.tasks.lock().expect("poisoned");
            tasks.notifier = None;
            tasks.session_id = None;
            tasks.manager.take()
        };
        let Some(manager) = manager else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(async move {
                manager.shutdown(None).await;
            });
        }
    }
}

/// The session, as the task notifier sees it.
struct SessionNotificationHost {
    session: Weak<AgentSession>,
    manager: Arc<TaskManager>,
}

impl TaskNotificationHost for SessionNotificationHost {
    fn is_streaming(&self) -> bool {
        self.session
            .upgrade()
            .is_some_and(|session| session.is_streaming())
    }

    fn send<'a>(
        &'a self,
        message: TaskNotificationMessage,
        options: TaskNotificationSendOptions,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let Some(session) = self.session.upgrade() else {
                return;
            };
            session
                .send_custom_message(
                    CustomMessage {
                        custom_type: message.custom_type,
                        content: UserContent::Blocks(vec![TextOrImageContent::Text(
                            TextContent::new(message.content),
                        )]),
                        display: message.display,
                        details: Some(serde_json::json!({
                            "taskId": message.details.task_id,
                            "status": message.details.status,
                            "kind": message.details.kind,
                        })),
                        timestamp: now_millis(),
                    },
                    SendCustomMessageOptions {
                        trigger_turn: options.trigger_turn,
                        deliver_as: options.deliver_as.map(|value| match value {
                            TaskNotificationDelivery::Steer => DeliverAs::Steer,
                            TaskNotificationDelivery::FollowUp => DeliverAs::FollowUp,
                        }),
                    },
                )
                .await;
        })
    }

    fn output<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Option<NotificationOutput>> {
        Box::pin(async move {
            let snapshot = self
                .manager
                .output_snapshot(task_id, NOTIFICATION_PREVIEW_BYTES as u64)
                .await;
            Some(NotificationOutput {
                output_path: snapshot.output_path,
                total_bytes: snapshot.total_bytes,
                truncated: snapshot.truncated,
                preview: snapshot.preview,
            })
        })
    }

    fn transcript(&self) -> Vec<TranscriptNotification> {
        let Some(session) = self.session.upgrade() else {
            return Vec::new();
        };
        session
            .agent
            .state()
            .messages
            .iter()
            .map(|message| match message {
                AgentMessage::Custom(custom) => TranscriptNotification {
                    role: "custom".to_string(),
                    custom_type: Some(custom.custom_type.clone()),
                    details: custom.details.as_ref().and_then(|details| {
                        Some(TaskNotificationDetails {
                            task_id: details.get("taskId")?.as_str()?.to_string(),
                            status: details.get("status")?.as_str()?.to_string(),
                            kind: details.get("kind")?.as_str()?.to_string(),
                        })
                    }),
                },
                other => TranscriptNotification {
                    role: agent_message_role(other).to_string(),
                    custom_type: None,
                    details: None,
                },
            })
            .collect()
    }
}

fn agent_message_role(message: &AgentMessage) -> &'static str {
    match message {
        AgentMessage::User(_) => "user",
        AgentMessage::Assistant(_) => "assistant",
        AgentMessage::ToolResult(_) => "toolResult",
        AgentMessage::BashExecution(_) => "bashExecution",
        AgentMessage::Custom(_) => "custom",
        AgentMessage::BranchSummary(_) => "branchSummary",
        AgentMessage::CompactionSummary(_) => "compactionSummary",
    }
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ============================================================================
// Read-only state
// ============================================================================

/// How a custom message is delivered while a run is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverAs {
    Steer,
    FollowUp,
    NextTurn,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SendCustomMessageOptions {
    pub trigger_turn: bool,
    pub deliver_as: Option<DeliverAs>,
}

impl AgentSession {
    pub fn agent(&self) -> Arc<Agent> {
        Arc::clone(&self.agent)
    }

    /// Full agent state.
    pub fn state(&self) -> AgentState {
        self.agent.state()
    }

    /// Current model. `None` when the default placeholder is still in place.
    pub fn model(&self) -> Option<Model> {
        let model = self.agent.state().model;
        (!model.id.is_empty()).then_some(model)
    }

    pub fn thinking_level(&self) -> ThinkingLevel {
        self.agent.state().thinking_level
    }

    /// Whether a run or a post-run continuation is in progress.
    pub fn is_streaming(&self) -> bool {
        self.is_agent_run_active.load(Ordering::SeqCst)
    }

    pub fn is_idle(&self) -> bool {
        !self.is_streaming()
    }

    /// The effective system prompt, per-turn modifications included.
    pub fn system_prompt(&self) -> String {
        self.agent.state().system_prompt
    }

    pub fn retry_attempt(&self) -> u64 {
        self.retry_attempt.load(Ordering::SeqCst)
    }

    /// Whether compaction or branch summarization is running.
    pub fn is_compacting(&self) -> bool {
        self.auto_compaction_signal
            .lock()
            .expect("poisoned")
            .is_some()
            || self.compaction_signal.lock().expect("poisoned").is_some()
            || self
                .branch_summary_signal
                .lock()
                .expect("poisoned")
                .is_some()
    }

    /// All messages, custom types included.
    pub fn messages(&self) -> Vec<AgentMessage> {
        self.agent.state().messages
    }

    pub fn steering_mode(&self) -> QueueMode {
        self.agent.steering_mode()
    }

    pub fn follow_up_mode(&self) -> QueueMode {
        self.agent.follow_up_mode()
    }

    pub fn session_file(&self) -> Option<String> {
        self.session_manager
            .lock()
            .expect("poisoned")
            .get_session_file()
            .map(str::to_string)
    }

    pub fn session_id(&self) -> String {
        self.session_manager
            .lock()
            .expect("poisoned")
            .get_session_id()
            .to_string()
    }

    pub fn session_name(&self) -> Option<String> {
        self.session_manager
            .lock()
            .expect("poisoned")
            .get_session_name()
    }

    pub fn scoped_models(&self) -> Vec<ScopedModel> {
        self.scoped_models.lock().expect("poisoned").clone()
    }

    pub fn set_scoped_models(&self, scoped_models: Vec<ScopedModel>) {
        *self.scoped_models.lock().expect("poisoned") = scoped_models;
    }

    /// File-based prompt templates.
    pub fn prompt_templates(&self) -> Vec<PromptTemplate> {
        self.resource_loader.get_prompts().0
    }

    /// The model runtime the session resolves models and auth against.
    pub fn model_runtime(&self) -> Arc<dyn SessionModelRuntime> {
        Arc::clone(&self.model_runtime)
    }

    /// The model delegated children run on: the `subagentModel` setting when
    /// it names a model that exists right now, otherwise `None` — the child
    /// then inherits the parent's model. Addition over the TS original (user
    /// decision 2026-08-16, v0.1.6); set and cleared via `/subagent-model`.
    pub fn subagent_model(&self) -> Option<Model> {
        let id = self.settings_manager.get_subagent_model()?;
        match self.settings_manager.get_subagent_provider() {
            Some(provider) => self.model_runtime.get_model(&provider, &id),
            // No provider stored (hand-edited settings.json): fall back to an
            // id-wide search so the setting still works.
            None => self
                .model_runtime
                .get_available_snapshot()
                .into_iter()
                .find(|model| model.id == id),
        }
    }

    pub fn resource_loader(&self) -> Arc<dyn ResourceLoader> {
        Arc::clone(&self.resource_loader)
    }

    pub fn settings_manager(&self) -> Arc<SettingsManager> {
        Arc::clone(&self.settings_manager)
    }

    /// Runs `body` with the session manager locked. The manager is `&mut` for
    /// every append, so callers that need several operations take it once.
    pub fn with_session_manager<T>(&self, body: impl FnOnce(&mut SessionManager) -> T) -> T {
        body(&mut self.session_manager.lock().expect("poisoned"))
    }

    pub fn pending_message_count(&self) -> usize {
        let queues = self.queues.lock().expect("poisoned");
        queues.steering.len() + queues.follow_up.len()
    }

    pub fn get_steering_messages(&self) -> Vec<String> {
        self.queues.lock().expect("poisoned").steering.clone()
    }

    pub fn get_follow_up_messages(&self) -> Vec<String> {
        self.queues.lock().expect("poisoned").follow_up.clone()
    }

    /// Clears both queues and returns what was in them, so an aborting user gets
    /// their text back in the editor.
    pub fn clear_queue(&self) -> (Vec<String>, Vec<String>) {
        let (steering, follow_up) = {
            let mut queues = self.queues.lock().expect("poisoned");
            (
                std::mem::take(&mut queues.steering),
                std::mem::take(&mut queues.follow_up),
            )
        };
        self.agent.clear_all_queues();
        self.emit_queue_update();
        (steering, follow_up)
    }
}

// ============================================================================
// Prompting
// ============================================================================

impl AgentSession {
    async fn run_agent_prompt(self: &Arc<Self>, messages: Vec<AgentMessage>) {
        self.is_agent_run_active.store(true, Ordering::SeqCst);
        let _ = self.agent.prompt(messages).await;
        while self.handle_post_agent_run().await {
            let _ = self.agent.continue_run().await;
        }
        *self.system_prompt_override.lock().expect("poisoned") = None;
        self.flush_pending_bash_messages();
        self.emit_agent_settled().await;
    }

    async fn handle_post_agent_run(self: &Arc<Self>) -> bool {
        let message = self.last_assistant_message.lock().expect("poisoned").take();
        let Some(message) = message else {
            return false;
        };

        if self.is_retryable_error(&message) && self.prepare_retry(&message).await {
            return true;
        }

        let attempt = self.retry_attempt.load(Ordering::SeqCst);
        if message.stop_reason == StopReason::Error && attempt > 0 {
            self.emit(AgentSessionEvent::AutoRetryEnd {
                success: false,
                attempt,
                final_error: message.error_message.clone(),
            });
            self.retry_attempt.store(0, Ordering::SeqCst);
        }

        if self.check_compaction(&message, true).await {
            return true;
        }

        // The agent loop drains both queues before emitting agent_end. Anything
        // still here was queued by an agent_end listener and needs a
        // continuation.
        self.agent.has_queued_messages()
    }

    /// Sends a prompt.
    ///
    /// Expands skill commands and prompt templates by default, queues via steer
    /// or follow-up while a run is active, and validates model and credentials
    /// before starting one.
    pub async fn prompt(
        self: &Arc<Self>,
        text: &str,
        options: PromptOptions,
    ) -> Result<(), String> {
        let expand = options.expand_prompt_templates.unwrap_or(true);
        let preflight = options.preflight_result.clone();
        let report = |success: bool| {
            if let Some(preflight) = preflight.as_ref() {
                preflight(success);
            }
        };

        if self.compaction_signal.lock().expect("poisoned").is_some() {
            report(false);
            return Err(
                "Cannot submit a prompt while compaction is in progress. Wait for compaction to finish and retry."
                    .to_string(),
            );
        }

        // Expand skill commands (/skill:name args) and prompt templates.
        let mut expanded = text.to_string();
        if expand {
            expanded = self.expand_skill_command(&expanded);
            expanded = expand_prompt_template(&expanded, &self.prompt_templates());
        }

        if self.is_streaming() {
            let Some(behavior) = options.streaming_behavior else {
                report(false);
                return Err(
                    "Agent is already processing. Specify streamingBehavior ('steer' or 'followUp') to queue the message."
                        .to_string(),
                );
            };
            match behavior {
                QueueBehavior::FollowUp => self.queue_follow_up(&expanded, &options.images),
                QueueBehavior::Steer => self.queue_steer(&expanded, &options.images),
            }
            report(true);
            return Ok(());
        }

        // Flush any pending bash output before the new prompt.
        self.flush_pending_bash_messages();

        let Some(model) = self.model() else {
            report(false);
            return Err(format_no_model_selected_message());
        };

        let has_configured_auth = self.model_runtime.has_configured_auth(&model.provider)
            || self.model_runtime.check_auth(&model.provider).await;
        if !has_configured_auth {
            report(false);
            if self.model_runtime.is_using_oauth(&model.provider) {
                return Err(format!(
                    "Authentication failed for \"{}\". Credentials may have expired or network is unavailable. Run '/login {}' to re-authenticate.",
                    model.provider, model.provider
                ));
            }
            return Err(format_no_api_key_found_message(&model.provider));
        }

        // Catches an aborted response: the user's new prompt is sent below, so
        // no continuation is started here.
        if let Some(last_assistant) = self.find_last_assistant_message() {
            self.check_compaction(&last_assistant, false).await;
        }

        let mut messages: Vec<AgentMessage> = Vec::new();

        // A pending mode switch rides in a message of its own, hidden from the
        // transcript. Consumed here and only here — tool-result continuations
        // and retries do not pass through this path. It is addressed to the
        // model, not to the user: someone who just switched mode knows what they
        // switched to. Ordering keeps the old behaviour — the block arrives
        // immediately before the prompt it applies to, and both are appended, so
        // the cached prefix is intact.
        if let Some(block) = self.consume_pending_mode_block() {
            messages.push(AgentMessage::Custom(CustomMessage {
                custom_type: MODE_BLOCK_TYPE.to_string(),
                content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(
                    block,
                ))]),
                display: false,
                details: None,
                timestamp: now_millis(),
            }));
        }

        let mut content = vec![TextOrImageContent::Text(TextContent::new(&expanded))];
        for image in &options.images {
            content.push(TextOrImageContent::Image(image.clone()));
        }
        messages.push(AgentMessage::User(UserMessage {
            content: UserContent::Blocks(content),
            timestamp: now_millis(),
        }));

        // Any pending "next turn" messages ride along as context.
        let pending = std::mem::take(&mut self.queues.lock().expect("poisoned").pending_next_turn);
        for message in pending {
            messages.push(AgentMessage::Custom(message));
        }

        if let Some(reminder) = self.pending_active_task_reminder() {
            messages.push(AgentMessage::Custom(reminder));
        }

        // The user's UserPromptSubmit hooks run here; their output is injected
        // as a hidden hook_context message (extension-boundary §2.2).
        if let Some(hooks) = self.hooks.as_ref()
            && let Some(context) = hooks.before_agent_start(&expanded).await
        {
            messages.push(AgentMessage::Custom(CustomMessage {
                custom_type: "hook_context".to_string(),
                content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(
                    context,
                ))]),
                display: false,
                details: None,
                timestamp: now_millis(),
            }));
        }

        // Reset to the base prompt in case a previous turn modified it.
        *self.system_prompt_override.lock().expect("poisoned") = None;
        let base = self
            .tools
            .lock()
            .expect("poisoned")
            .base_system_prompt
            .clone();
        self.agent.set_system_prompt(base);

        report(true);
        self.run_agent_prompt(messages).await;
        Ok(())
    }

    /// Expands `/skill:name args` to the skill's full content.
    fn expand_skill_command(&self, text: &str) -> String {
        let Some(rest) = text.strip_prefix("/skill:") else {
            return text.to_string();
        };
        let (skill_name, args) = match rest.find(' ') {
            Some(index) => (&rest[..index], rest[index + 1..].trim()),
            None => (rest, ""),
        };

        let Some(skill) = self
            .resource_loader
            .get_skills()
            .0
            .into_iter()
            .find(|skill| skill.name == skill_name)
        else {
            // Unknown skill, pass through.
            return text.to_string();
        };

        let Ok(content) = std::fs::read_to_string(&skill.file_path) else {
            return text.to_string();
        };
        let Ok(body) = strip_frontmatter(&content) else {
            return text.to_string();
        };
        let block = format!(
            "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
            skill.name,
            skill.file_path,
            skill.base_dir,
            body.trim()
        );
        if args.is_empty() {
            block
        } else {
            format!("{block}\n\n{args}")
        }
    }

    /// Queues a steering message, delivered after the current turn's tool calls
    /// finish and before the next provider call.
    pub fn steer(&self, text: &str, images: &[ImageContent]) {
        let mut expanded = self.expand_skill_command(text);
        expanded = expand_prompt_template(&expanded, &self.prompt_templates());
        self.queue_steer(&expanded, images);
    }

    /// Queues a follow-up message, delivered only once the agent has no more
    /// tool calls and no steering messages left.
    pub fn follow_up(&self, text: &str, images: &[ImageContent]) {
        let mut expanded = self.expand_skill_command(text);
        expanded = expand_prompt_template(&expanded, &self.prompt_templates());
        self.queue_follow_up(&expanded, images);
    }

    fn queue_steer(&self, text: &str, images: &[ImageContent]) {
        self.queues
            .lock()
            .expect("poisoned")
            .steering
            .push(text.to_string());
        self.emit_queue_update();
        self.agent.steer(AgentMessage::User(UserMessage {
            content: user_content(text, images),
            timestamp: now_millis(),
        }));
    }

    fn queue_follow_up(&self, text: &str, images: &[ImageContent]) {
        self.queues
            .lock()
            .expect("poisoned")
            .follow_up
            .push(text.to_string());
        self.emit_queue_update();
        self.agent.follow_up(AgentMessage::User(UserMessage {
            content: user_content(text, images),
            timestamp: now_millis(),
        }));
    }

    /// Sends a custom message.
    ///
    /// Three cases: streaming queues it, `trigger_turn` starts a run, and
    /// otherwise it is appended to state and session without starting one.
    pub async fn send_custom_message(
        self: &Arc<Self>,
        message: CustomMessage,
        options: SendCustomMessageOptions,
    ) {
        let message = CustomMessage {
            timestamp: now_millis(),
            ..message
        };

        if options.deliver_as == Some(DeliverAs::NextTurn) {
            self.queues
                .lock()
                .expect("poisoned")
                .pending_next_turn
                .push(message);
            return;
        }

        if self.is_streaming() {
            if options.deliver_as == Some(DeliverAs::FollowUp) {
                self.agent.follow_up(AgentMessage::Custom(message));
            } else {
                self.agent.steer(AgentMessage::Custom(message));
            }
            return;
        }

        if options.trigger_turn {
            self.run_agent_prompt(vec![AgentMessage::Custom(message)])
                .await;
            return;
        }

        let mut messages = self.agent.state().messages;
        messages.push(AgentMessage::Custom(message.clone()));
        self.agent.set_messages(messages);
        {
            let mut manager = self.session_manager.lock().expect("poisoned");
            let _ = manager.append_custom_message_entry(
                &message.custom_type,
                serde_json::to_value(&message.content).unwrap_or(Value::Null),
                message.display,
                message.details.clone(),
            );
        }
        self.emit(AgentSessionEvent::Agent(AgentEvent::MessageStart {
            message: AgentMessage::Custom(message.clone()),
        }));
        self.emit(AgentSessionEvent::Agent(AgentEvent::MessageEnd {
            message: AgentMessage::Custom(message),
        }));
    }

    /// Sends a user message; always starts or joins a turn.
    pub async fn send_user_message(
        self: &Arc<Self>,
        content: &[TextOrImageContent],
        deliver_as: Option<QueueBehavior>,
        expand_prompt_templates: bool,
    ) -> Result<(), String> {
        let mut text_parts: Vec<String> = Vec::new();
        let mut images: Vec<ImageContent> = Vec::new();
        for part in content {
            match part {
                TextOrImageContent::Text(text) => text_parts.push(text.text.clone()),
                TextOrImageContent::Image(image) => images.push(image.clone()),
            }
        }

        self.prompt(
            &text_parts.join("\n"),
            PromptOptions {
                expand_prompt_templates: Some(expand_prompt_templates),
                images,
                streaming_behavior: deliver_as,
                preflight_result: None,
            },
        )
        .await
    }

    /// Aborts the current operation and waits for the agent to go idle.
    pub async fn abort(&self) {
        self.abort_retry();
        self.agent.abort();
        self.wait_for_idle().await;
    }

    pub async fn wait_for_idle(&self) {
        if self.is_idle() {
            return;
        }
        // `notified()` registers the waiter on first poll, not at creation, so
        // it is enabled before the second check — otherwise a run that settles
        // in between notifies nobody and this waits forever.
        let notified = self.idle.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_idle() {
            return;
        }
        notified.await;
    }
}

fn user_content(text: &str, images: &[ImageContent]) -> UserContent {
    let mut content = vec![TextOrImageContent::Text(TextContent::new(text))];
    for image in images {
        content.push(TextOrImageContent::Image(image.clone()));
    }
    UserContent::Blocks(content)
}

// ============================================================================
// Model management
// ============================================================================

impl AgentSession {
    /// Sets the model, validating that credentials exist first.
    pub async fn set_model(&self, model: Model) -> Result<(), String> {
        if !self.model_runtime.check_auth(&model.provider).await {
            return Err(format!("No API key for {}/{}", model.provider, model.id));
        }

        let thinking_level = self.thinking_level_for_model_switch(None);
        self.agent.set_model(model.clone());
        {
            let mut manager = self.session_manager.lock().expect("poisoned");
            let _ = manager.append_model_change(&model.provider, &model.id);
        }
        self.settings_manager
            .set_default_model_and_provider(&model.provider, &model.id);

        // Re-clamp for the new model's capabilities.
        self.set_thinking_level(thinking_level);
        Ok(())
    }

    /// Cycles to the next or previous model, over the scoped set when there is
    /// one and over everything available otherwise.
    pub fn cycle_model(&self, forward: bool) -> Option<ModelCycleResult> {
        if !self.scoped_models.lock().expect("poisoned").is_empty() {
            return self.cycle_scoped_model(forward);
        }
        self.cycle_available_model(forward)
    }

    fn cycle_scoped_model(&self, forward: bool) -> Option<ModelCycleResult> {
        let available: HashSet<String> = self
            .model_runtime
            .get_available_snapshot()
            .into_iter()
            .map(|model| format!("{}\u{0}{}", model.provider, model.id))
            .collect();
        let scoped: Vec<ScopedModel> = self
            .scoped_models
            .lock()
            .expect("poisoned")
            .iter()
            .filter(|scoped| {
                available.contains(&format!(
                    "{}\u{0}{}",
                    scoped.model.provider, scoped.model.id
                ))
            })
            .cloned()
            .collect();
        if scoped.len() <= 1 {
            return None;
        }

        let current = self.model();
        let current_index = scoped
            .iter()
            .position(|entry| models_are_equal(Some(&entry.model), current.as_ref()))
            .unwrap_or(0);
        let next_index = step(current_index, scoped.len(), forward);
        let next = &scoped[next_index];
        let thinking_level = self.thinking_level_for_model_switch(next.thinking_level);

        self.agent.set_model(next.model.clone());
        {
            let mut manager = self.session_manager.lock().expect("poisoned");
            let _ = manager.append_model_change(&next.model.provider, &next.model.id);
        }
        self.settings_manager
            .set_default_model_and_provider(&next.model.provider, &next.model.id);

        // An explicit scoped thinking level overrides the session preference; an
        // absent one inherits it. `set_thinking_level` clamps either way.
        self.set_thinking_level(thinking_level);

        Some(ModelCycleResult {
            model: next.model.clone(),
            thinking_level: self.thinking_level(),
            is_scoped: true,
        })
    }

    fn cycle_available_model(&self, forward: bool) -> Option<ModelCycleResult> {
        let available = self.model_runtime.get_available_snapshot();
        if available.len() <= 1 {
            return None;
        }

        let current = self.model();
        let current_index = available
            .iter()
            .position(|model| models_are_equal(Some(model), current.as_ref()))
            .unwrap_or(0);
        let next_index = step(current_index, available.len(), forward);
        let next = &available[next_index];

        let thinking_level = self.thinking_level_for_model_switch(None);
        self.agent.set_model(next.clone());
        {
            let mut manager = self.session_manager.lock().expect("poisoned");
            let _ = manager.append_model_change(&next.provider, &next.id);
        }
        self.settings_manager
            .set_default_model_and_provider(&next.provider, &next.id);

        self.set_thinking_level(thinking_level);

        Some(ModelCycleResult {
            model: next.clone(),
            thinking_level: self.thinking_level(),
            is_scoped: false,
        })
    }
}

fn step(index: usize, length: usize, forward: bool) -> usize {
    if forward {
        (index + 1) % length
    } else {
        (index + length - 1) % length
    }
}

// ============================================================================
// Thinking level
// ============================================================================

impl AgentSession {
    /// Sets the thinking level, clamped to what the model supports. Persisted
    /// only when it actually changes.
    pub fn set_thinking_level(&self, level: ThinkingLevel) {
        let available = self.get_available_thinking_levels();
        let effective = if available.contains(&level) {
            level
        } else {
            self.clamp_thinking_level(level)
        };

        let previous = self.agent.state().thinking_level;
        let is_changing = effective != previous;

        self.agent.set_thinking_level(effective);

        if is_changing {
            {
                let mut manager = self.session_manager.lock().expect("poisoned");
                let _ = manager.append_thinking_level_change(thinking_level_name(effective));
            }
            if self.supports_thinking() || effective != ThinkingLevel::Off {
                self.settings_manager
                    .set_default_thinking_level(thinking_level_name(effective));
            }
            self.emit(AgentSessionEvent::ThinkingLevelChanged { level: effective });
        }
    }

    /// Cycles to the next thinking level, or `None` when the model cannot think.
    pub fn cycle_thinking_level(&self) -> Option<ThinkingLevel> {
        if !self.supports_thinking() {
            return None;
        }
        let levels = self.get_available_thinking_levels();
        let current = levels
            .iter()
            .position(|level| *level == self.thinking_level())
            .map(|index| (index + 1) % levels.len())
            .unwrap_or(0);
        let next = levels[current];
        self.set_thinking_level(next);
        Some(next)
    }

    /// The thinking levels the current model offers. The provider clamps
    /// further as needed.
    pub fn get_available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        let Some(model) = self.model() else {
            return THINKING_LEVELS.to_vec();
        };
        get_supported_thinking_levels(&model)
            .into_iter()
            .map(ThinkingLevel::from)
            .collect()
    }

    pub fn supports_thinking(&self) -> bool {
        self.model().is_some_and(|model| model.reasoning)
    }

    fn thinking_level_for_model_switch(&self, explicit: Option<ThinkingLevel>) -> ThinkingLevel {
        if let Some(level) = explicit {
            return level;
        }
        if !self.supports_thinking() {
            return self
                .settings_manager
                .get_default_thinking_level()
                .and_then(|level| parse_thinking_level(&level))
                .unwrap_or(ThinkingLevel::Medium);
        }
        self.thinking_level()
    }

    fn clamp_thinking_level(&self, level: ThinkingLevel) -> ThinkingLevel {
        match self.model() {
            Some(model) => {
                let clamped = clamp_thinking_level(&model, model_thinking_level(level));
                ThinkingLevel::from(clamped)
            }
            None => ThinkingLevel::Off,
        }
    }
}

pub fn parse_thinking_level(value: &str) -> Option<ThinkingLevel> {
    match value {
        "off" => Some(ThinkingLevel::Off),
        "minimal" => Some(ThinkingLevel::Minimal),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" => Some(ThinkingLevel::Xhigh),
        "max" => Some(ThinkingLevel::Max),
        _ => None,
    }
}

// ============================================================================
// Queue modes
// ============================================================================

impl AgentSession {
    fn sync_queue_modes_from_settings(&self) {
        self.agent
            .set_steering_mode(agent_queue_mode(self.settings_manager.get_steering_mode()));
        self.agent
            .set_follow_up_mode(agent_queue_mode(self.settings_manager.get_follow_up_mode()));
    }

    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.agent.set_steering_mode(mode);
        self.settings_manager
            .set_steering_mode(settings_queue_mode(mode));
    }

    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.agent.set_follow_up_mode(mode);
        self.settings_manager
            .set_follow_up_mode(settings_queue_mode(mode));
    }
}

// ============================================================================
// Compaction
// ============================================================================

impl AgentSession {
    /// Resolves the credentials a request needs, translating the two failures a
    /// user can act on into instructions.
    async fn required_request_auth(&self, model: &Model) -> Result<(Model, SessionAuth), String> {
        let result = match self.model_runtime.get_auth(model).await {
            Ok(result) => result,
            Err(error) => {
                if error.contains(AUTH_HEADER_NEEDS_KEY) {
                    return Err(format_no_api_key_found_message(&model.provider));
                }
                return Err(error);
            }
        };

        if let Some(auth) = result
            && (auth.api_key.is_some() || auth.headers.is_some())
        {
            let mut request_model = model.clone();
            if let Some(base_url) = auth.base_url.as_ref() {
                request_model.base_url = base_url.clone();
            }
            return Ok((request_model, auth));
        }

        if self.model_runtime.is_using_oauth(&model.provider) {
            return Err(format!(
                "Authentication failed for \"{}\". Credentials may have expired or network is unavailable. Run '/login {}' to re-authenticate.",
                model.provider, model.provider
            ));
        }
        Err(format_no_api_key_found_message(&model.provider))
    }

    /// Summarization runs through the session's own stream function, so a
    /// failure to resolve ambient credentials is not fatal — the request path
    /// may still carry them.
    async fn summarization_request_auth(
        &self,
        model: &Model,
    ) -> Result<(Model, SessionAuth), String> {
        if self.agent.options().stream_fn.is_none() {
            return self.required_request_auth(model).await;
        }
        Ok(match self.model_runtime.get_auth(model).await {
            Ok(Some(auth)) => {
                let mut request_model = model.clone();
                if let Some(base_url) = auth.base_url.as_ref() {
                    request_model.base_url = base_url.clone();
                }
                (request_model, auth)
            }
            _ => (model.clone(), SessionAuth::default()),
        })
    }

    fn summarization_request(
        &self,
        auth: SessionAuth,
        signal: CancellationToken,
        source: SummarizationSource,
    ) -> SummarizationRequest {
        SummarizationRequest {
            api_key: auth.api_key,
            headers: auth.headers,
            env: auth.env,
            signal: Some(signal),
            thinking_level: Some(self.thinking_level()),
            stream_fn: self.agent.options().stream_fn,
            retry: Some(self.retry_policy()),
            callbacks: Some(self.summarization_retry_callbacks(source)),
        }
    }

    fn retry_policy(&self) -> notagent_ai::utils::retry::RetryPolicy {
        let settings = self.settings_manager.get_retry_settings();
        notagent_ai::utils::retry::RetryPolicy {
            enabled: settings.enabled,
            max_retries: settings.max_retries as u32,
            base_delay_ms: settings.base_delay_ms,
        }
    }

    fn compaction_settings(&self) -> CompactionSettings {
        self.settings_manager.get_compaction_settings().into()
    }

    /// Compacts the session context by hand. Aborts the current run first.
    pub async fn compact(
        self: &Arc<Self>,
        custom_instructions: Option<&str>,
    ) -> Result<CompactionResult, String> {
        self.abort().await;
        let signal = CancellationToken::new();
        *self.compaction_signal.lock().expect("poisoned") = Some(signal.clone());
        self.emit(AgentSessionEvent::CompactionStart {
            reason: CompactionReason::Manual,
        });

        let outcome = self
            .run_compaction(CompactionReason::Manual, custom_instructions, signal, false)
            .await;

        *self.compaction_signal.lock().expect("poisoned") = None;
        match outcome {
            Ok(result) => {
                // The messages that started any running background task are
                // gone now, so the next turn is told what is still out there —
                // otherwise the model starts duplicates of work already in
                // flight.
                self.tasks.lock().expect("poisoned").active_reminder_pending = true;
                self.emit(AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Manual,
                    result: Some(Box::new(result.clone())),
                    aborted: false,
                    will_retry: false,
                    error_message: None,
                });
                Ok(result)
            }
            Err(message) => {
                let aborted = message == "Compaction cancelled";
                self.emit(AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Manual,
                    result: None,
                    aborted,
                    will_retry: false,
                    error_message: (!aborted).then(|| format!("Compaction failed: {message}")),
                });
                Err(message)
            }
        }
    }

    /// The shared body of the manual and the automatic path.
    async fn run_compaction(
        self: &Arc<Self>,
        reason: CompactionReason,
        custom_instructions: Option<&str>,
        signal: CancellationToken,
        _will_retry: bool,
    ) -> Result<CompactionResult, String> {
        let Some(model) = self.model() else {
            return Err(format_no_model_selected_message());
        };
        let (request_model, auth) = self.summarization_request_auth(&model).await?;

        let path_entries: Vec<SessionEntry> = self
            .session_manager
            .lock()
            .expect("poisoned")
            .get_branch(None)
            .into_iter()
            .cloned()
            .collect();
        let settings = self.compaction_settings();

        let Some(preparation) = prepare_compaction(&path_entries, &settings) else {
            if matches!(path_entries.last(), Some(SessionEntry::Compaction(_))) {
                return Err("Already compacted".to_string());
            }
            return Err("Nothing to compact (session too small)".to_string());
        };

        if let Some(hooks) = self.hooks.as_ref() {
            hooks
                .session_before_compact(reason.as_str(), custom_instructions)
                .await;
        }

        let request = self.summarization_request(
            auth,
            signal.clone(),
            SummarizationSource::Compaction(reason),
        );
        let result = compact(&preparation, &request_model, custom_instructions, &request).await?;

        if signal.is_cancelled() {
            return Err("Compaction cancelled".to_string());
        }

        let estimated_tokens_after = {
            let mut manager = self.session_manager.lock().expect("poisoned");
            manager
                .append_compaction(
                    &result.summary,
                    &result.first_kept_entry_id,
                    result.tokens_before as i64,
                    result.details.clone(),
                    None,
                    result.usage,
                )
                .map_err(|error| error.to_string())?;
            let context = manager.build_session_context();
            let tokens = estimate_messages_tokens(&context.messages);
            self.agent.set_messages(context.messages);
            tokens
        };

        if let Some(hooks) = self.hooks.as_ref() {
            hooks.session_compact(reason.as_str()).await;
        }

        Ok(CompactionResult {
            estimated_tokens_after: Some(estimated_tokens_after),
            ..result
        })
    }

    /// Cancels an in-progress compaction, manual or automatic.
    pub fn abort_compaction(&self) {
        if let Some(signal) = self.compaction_signal.lock().expect("poisoned").as_ref() {
            signal.cancel();
        }
        if let Some(signal) = self
            .auto_compaction_signal
            .lock()
            .expect("poisoned")
            .as_ref()
        {
            signal.cancel();
        }
    }

    /// Cancels an in-progress branch summarization.
    pub fn abort_branch_summary(&self) {
        if let Some(signal) = self
            .branch_summary_signal
            .lock()
            .expect("poisoned")
            .as_ref()
        {
            signal.cancel();
        }
    }

    /// Decides whether compaction is needed and runs it. Called after
    /// `agent_end` and before a prompt is submitted.
    ///
    /// Two cases:
    ///  1. Recoverable failure — the provider reported a context overflow, or
    ///     stopped below its own output limit. The assistant message is removed
    ///     from state, the context is compacted, and the turn is retried once.
    ///  2. Threshold — the context is simply large. Compact, but do not retry:
    ///     the user continues by hand.
    async fn check_compaction(
        self: &Arc<Self>,
        assistant_message: &AssistantMessage,
        skip_aborted_check: bool,
    ) -> bool {
        let settings = self.compaction_settings();
        if !settings.enabled {
            return false;
        }

        // A message the user cancelled is not a reason to compact.
        if skip_aborted_check && assistant_message.stop_reason == StopReason::Aborted {
            return false;
        }

        let model = self.model();
        let context_window = model
            .as_ref()
            .map(|model| model.context_window)
            .unwrap_or(0);

        // An overflow reported by a different model says nothing about this one:
        // switching from a small-context model to a large one must not inherit
        // its complaint.
        let same_model = model.as_ref().is_some_and(|model| {
            assistant_message.provider == model.provider && assistant_message.model == model.id
        });

        // A message older than the latest compaction boundary carries stale
        // usage and would retrigger compaction on the first prompt after one.
        let compaction_entry_timestamp = {
            let manager = self.session_manager.lock().expect("poisoned");
            let branch: Vec<SessionEntry> = manager.get_branch(None).into_iter().cloned().collect();
            get_latest_compaction_entry(&branch).map(|entry| entry.timestamp.clone())
        };
        if let Some(timestamp) = compaction_entry_timestamp.as_ref()
            && assistant_message.timestamp <= parse_timestamp_millis(timestamp)
        {
            return false;
        }

        let recoverable_length = same_model
            && is_recoverable_length(
                assistant_message,
                model.as_ref().map(|model| model.max_tokens).unwrap_or(0),
            );
        if same_model
            && (is_context_overflow(assistant_message, Some(context_window)) || recoverable_length)
        {
            let will_retry = assistant_message.stop_reason != StopReason::Stop;

            if !will_retry {
                return self
                    .run_auto_compaction(CompactionReason::Overflow, false)
                    .await;
            }

            if self.overflow_recovery_attempted.load(Ordering::SeqCst) {
                self.emit(AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Overflow,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: Some(
                        "Context overflow recovery failed after one compact-and-retry attempt. Try reducing context or switching to a larger-context model."
                            .to_string(),
                    ),
                });
                return false;
            }

            self.overflow_recovery_attempted
                .store(true, Ordering::SeqCst);
            // The failed or truncated message stays in session history but must
            // not be part of the compact-and-retry context.
            self.drop_trailing_assistant_message(None);
            return self
                .run_auto_compaction(CompactionReason::Overflow, will_retry)
                .await;
        }

        // Threshold. For errored or all-zero-usage messages, estimate from the
        // last valid response, so a session that hit persistent API errors can
        // still compact.
        let direct = calculate_context_tokens(&assistant_message.usage);
        let context_tokens = if assistant_message.stop_reason == StopReason::Error || direct == 0 {
            let messages = self.agent.state().messages;
            let estimate = estimate_context_tokens(&messages);
            let Some(index) = estimate.last_usage_index else {
                return false;
            };
            // Verify the usage source is post-compaction: kept pre-compaction
            // messages carry usage from the larger context.
            if let Some(timestamp) = compaction_entry_timestamp.as_ref()
                && let Some(AgentMessage::Assistant(usage_message)) = messages.get(index)
                && usage_message.timestamp <= parse_timestamp_millis(timestamp)
            {
                return false;
            }
            estimate.tokens
        } else {
            direct
        };

        if should_compact(context_tokens, context_window, &settings) {
            return self
                .run_auto_compaction(CompactionReason::Threshold, false)
                .await;
        }
        false
    }

    async fn run_auto_compaction(
        self: &Arc<Self>,
        reason: CompactionReason,
        will_retry: bool,
    ) -> bool {
        if self.model().is_none() {
            return false;
        }
        let settings = self.compaction_settings();
        let path_entries: Vec<SessionEntry> = self
            .session_manager
            .lock()
            .expect("poisoned")
            .get_branch(None)
            .into_iter()
            .cloned()
            .collect();
        if prepare_compaction(&path_entries, &settings).is_none() {
            return false;
        }

        self.emit(AgentSessionEvent::CompactionStart { reason });
        let signal = CancellationToken::new();
        *self.auto_compaction_signal.lock().expect("poisoned") = Some(signal.clone());

        let outcome = self
            .run_compaction(reason, None, signal.clone(), will_retry)
            .await;
        *self.auto_compaction_signal.lock().expect("poisoned") = None;

        let result = match outcome {
            Ok(result) => result,
            Err(error_message) => {
                self.emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: error_message == "Compaction cancelled",
                    will_retry: false,
                    error_message: (error_message != "Compaction cancelled").then(
                        || match reason {
                            CompactionReason::Overflow => {
                                format!("Context overflow recovery failed: {error_message}")
                            }
                            _ => format!("Auto-compaction failed: {error_message}"),
                        },
                    ),
                });
                return false;
            }
        };

        if signal.is_cancelled() {
            self.emit(AgentSessionEvent::CompactionEnd {
                reason,
                result: None,
                aborted: true,
                will_retry: false,
                error_message: None,
            });
            return false;
        }

        self.emit(AgentSessionEvent::CompactionEnd {
            reason,
            result: Some(Box::new(result)),
            aborted: false,
            will_retry,
            error_message: None,
        });

        if will_retry {
            // The overflow response was persisted on message_end before
            // check_compaction removed it from agent state. Rebuilding state
            // from the new compaction can restore that kept entry, leaving an
            // assistant as the final message — which `continue` rejects.
            self.drop_trailing_assistant_message(Some(&[StopReason::Error, StopReason::Length]));
            return true;
        }

        // Auto-compaction can finish while queued messages are waiting.
        self.agent.has_queued_messages()
    }

    /// Removes a trailing assistant message from agent state, optionally only
    /// when its stop reason is one of `stop_reasons`.
    fn drop_trailing_assistant_message(&self, stop_reasons: Option<&[StopReason]>) {
        let mut messages = self.agent.state().messages;
        let matches = match messages.last() {
            Some(AgentMessage::Assistant(assistant)) => {
                stop_reasons.is_none_or(|reasons| reasons.contains(&assistant.stop_reason))
            }
            _ => false,
        };
        if matches {
            messages.pop();
            self.agent.set_messages(messages);
        }
    }

    pub fn set_auto_compaction_enabled(&self, enabled: bool) {
        self.settings_manager.set_compaction_enabled(enabled);
    }

    pub fn auto_compaction_enabled(&self) -> bool {
        self.settings_manager.get_compaction_settings().enabled
    }
}

fn parse_timestamp_millis(timestamp: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|value| value.timestamp_millis())
        .unwrap_or(0)
}

// ============================================================================
// Auto-retry
// ============================================================================

impl AgentSession {
    /// Whether an error is worth retrying. Context overflow is not: compaction
    /// handles that.
    fn is_retryable_error(&self, message: &AssistantMessage) -> bool {
        let context_window = self.model().map(|model| model.context_window).unwrap_or(0);
        if is_context_overflow(message, Some(context_window)) {
            return false;
        }
        is_retryable_assistant_error(message)
    }

    /// Retry policy callbacks shared by compaction and branch summarization, so
    /// a single dropped stream no longer fails the whole operation. `source`
    /// carries what the TUI needs to render the retry.
    fn summarization_retry_callbacks(&self, source: SummarizationSource) -> RetryCallbacks {
        let scheduled = {
            let weak = self.weak_self.lock().expect("poisoned").clone();
            Arc::new(
                move |attempt: u32, max_attempts: u32, delay_ms: u64, error_message: String| {
                    if let Some(session) = weak.upgrade() {
                        session.emit(AgentSessionEvent::SummarizationRetryScheduled {
                            attempt,
                            max_attempts,
                            delay_ms,
                            error_message,
                        });
                    }
                },
            )
        };
        let attempt_start = {
            let weak = self.weak_self.lock().expect("poisoned").clone();
            Arc::new(move || {
                if let Some(session) = weak.upgrade() {
                    session.emit(AgentSessionEvent::SummarizationRetryAttemptStart { source });
                }
            })
        };
        let finished = {
            let weak = self.weak_self.lock().expect("poisoned").clone();
            Arc::new(
                move |_success: bool, _attempt: u32, _final_error: Option<String>| {
                    if let Some(session) = weak.upgrade() {
                        session.emit(AgentSessionEvent::SummarizationRetryFinished);
                    }
                },
            )
        };
        RetryCallbacks {
            on_retry_scheduled: Some(scheduled),
            on_retry_attempt_start: Some(attempt_start),
            on_retry_finished: Some(finished),
        }
    }

    /// Prepares a retryable error for continuation with exponential backoff.
    /// Returns whether the caller should continue the agent.
    async fn prepare_retry(&self, message: &AssistantMessage) -> bool {
        let settings = self.settings_manager.get_retry_settings();
        if !settings.enabled {
            return false;
        }

        let attempt = self.retry_attempt.fetch_add(1, Ordering::SeqCst) + 1;

        if attempt > settings.max_retries {
            // Keep the completed attempt count so post-run handling can report
            // the final failure.
            self.retry_attempt.store(attempt - 1, Ordering::SeqCst);
            return false;
        }

        let delay_ms = settings.base_delay_ms * 2u64.pow((attempt - 1) as u32);

        self.emit(AgentSessionEvent::AutoRetryStart {
            attempt,
            max_attempts: settings.max_retries,
            delay_ms,
            error_message: message
                .error_message
                .clone()
                .unwrap_or_else(|| "Unknown error".to_string()),
        });

        // Remove the error message from agent state; it stays in session history.
        self.drop_trailing_assistant_message(None);

        let signal = CancellationToken::new();
        *self.retry_signal.lock().expect("poisoned") = Some(signal.clone());
        let aborted = tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => false,
            _ = signal.cancelled() => true,
        };
        *self.retry_signal.lock().expect("poisoned") = None;

        if aborted {
            let attempt = self.retry_attempt.swap(0, Ordering::SeqCst);
            self.emit(AgentSessionEvent::AutoRetryEnd {
                success: false,
                attempt,
                final_error: Some("Retry cancelled".to_string()),
            });
            return false;
        }

        true
    }

    pub fn abort_retry(&self) {
        if let Some(signal) = self.retry_signal.lock().expect("poisoned").as_ref() {
            signal.cancel();
        }
    }

    pub fn is_retrying(&self) -> bool {
        self.retry_signal.lock().expect("poisoned").is_some()
    }

    pub fn auto_retry_enabled(&self) -> bool {
        self.settings_manager.get_retry_settings().enabled
    }

    pub fn set_auto_retry_enabled(&self, enabled: bool) {
        self.settings_manager.set_retry_enabled(enabled);
    }
}

// ============================================================================
// Bash execution
// ============================================================================

/// The streaming callback `execute_bash` forwards each chunk to.
pub type BashChunkCallback = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
pub struct ExecuteBashOptions {
    /// `!!` prefix: the output is shown but not sent to the model.
    pub exclude_from_context: bool,
    /// Identifier included in the bash execution update events.
    pub id: Option<String>,
    /// Custom operations, for remote execution.
    pub operations: Option<Arc<dyn BashOperations>>,
}

impl AgentSession {
    /// Runs a bash command and records the result in the session.
    pub async fn execute_bash(
        &self,
        command: &str,
        on_chunk: Option<BashChunkCallback>,
        options: ExecuteBashOptions,
    ) -> BashResult {
        let signal = CancellationToken::new();
        self.bash_signals
            .lock()
            .expect("poisoned")
            .push(signal.clone());

        let prefix = self.settings_manager.get_shell_command_prefix();
        let shell_path = self.settings_manager.get_shell_path();
        let resolved_command = match prefix.as_deref() {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}\n{command}"),
            _ => command.to_string(),
        };

        let operations: Arc<dyn BashOperations> = options
            .operations
            .clone()
            .unwrap_or_else(|| Arc::new(create_local_bash_operations(shell_path)));

        let cwd = self
            .session_manager
            .lock()
            .expect("poisoned")
            .get_cwd()
            .to_string();

        let id = options.id.clone();
        let weak = self.weak_self.lock().expect("poisoned").clone();
        let emit_chunk: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |delta: &str| {
            if let Some(callback) = on_chunk.as_ref() {
                callback(delta);
            }
            if let Some(session) = weak.upgrade() {
                session.emit(AgentSessionEvent::BashExecutionUpdate {
                    id: id.clone(),
                    delta: delta.to_string(),
                });
            }
        });

        let result = execute_bash_with_operations(
            &resolved_command,
            &cwd,
            operations.as_ref(),
            Some(BashExecutorOptions {
                on_chunk: Some(emit_chunk),
                signal: Some(signal.clone()),
            }),
        )
        .await
        .unwrap_or_else(|error| BashResult {
            output: error.to_string(),
            exit_code: Some(1),
            cancelled: false,
            truncated: false,
            full_output_path: None,
        });

        self.bash_signals
            .lock()
            .expect("poisoned")
            .retain(|candidate| candidate != &signal);

        self.record_bash_result(command, &result, options.exclude_from_context);
        result
    }

    /// Records a bash result in session history.
    pub fn record_bash_result(
        &self,
        command: &str,
        result: &BashResult,
        exclude_from_context: bool,
    ) {
        let message = BashExecutionMessage {
            command: command.to_string(),
            output: result.output.clone(),
            exit_code: result.exit_code,
            cancelled: result.cancelled,
            truncated: result.truncated,
            full_output_path: result.full_output_path.clone(),
            timestamp: now_millis(),
            exclude_from_context: exclude_from_context.then_some(true),
        };

        // While a run is active, deferring keeps tool_use/tool_result ordering
        // intact; the messages are flushed when the run ends.
        if self.is_streaming() {
            self.pending_bash_messages
                .lock()
                .expect("poisoned")
                .push(message);
            return;
        }

        self.append_bash_message(message);
    }

    fn append_bash_message(&self, message: BashExecutionMessage) {
        let mut messages = self.agent.state().messages;
        messages.push(AgentMessage::BashExecution(message.clone()));
        self.agent.set_messages(messages);
        let value =
            serde_json::to_value(AgentMessage::BashExecution(message)).unwrap_or(Value::Null);
        let mut manager = self.session_manager.lock().expect("poisoned");
        let _ = manager.append_message_value(value);
    }

    pub fn abort_bash(&self) {
        let signals: Vec<CancellationToken> = self.bash_signals.lock().expect("poisoned").clone();
        for signal in signals {
            signal.cancel();
        }
    }

    pub fn is_bash_running(&self) -> bool {
        !self.bash_signals.lock().expect("poisoned").is_empty()
    }

    pub fn has_pending_bash_messages(&self) -> bool {
        !self
            .pending_bash_messages
            .lock()
            .expect("poisoned")
            .is_empty()
    }

    /// Flushes deferred bash messages once the turn is over, which is what keeps
    /// the message ordering valid.
    fn flush_pending_bash_messages(&self) {
        let pending = std::mem::take(&mut *self.pending_bash_messages.lock().expect("poisoned"));
        for message in pending {
            self.append_bash_message(message);
        }
    }
}

// ============================================================================
// Session management and the tree
// ============================================================================

/// Options of [`AgentSession::navigate_tree`].
#[derive(Debug, Clone, Default)]
pub struct NavigateTreeOptions {
    pub summarize: bool,
    pub custom_instructions: Option<String>,
    pub replace_instructions: bool,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NavigateTreeResult {
    /// Text of the entry navigated to, when it is one the user wrote.
    pub editor_text: Option<String>,
    pub cancelled: bool,
    pub aborted: bool,
    pub summary_entry: Option<BranchSummaryEntry>,
}

impl AgentSession {
    /// Sets a display name for the session.
    pub fn set_session_name(&self, name: &str) {
        let session_name = {
            let mut manager = self.session_manager.lock().expect("poisoned");
            let _ = manager.append_session_info(name);
            manager.get_session_name()
        };
        self.emit(AgentSessionEvent::SessionInfoChanged { name: session_name });
    }

    /// Navigates to a different node of the session tree.
    ///
    /// Unlike a fork, which creates a new session file, this stays in the same
    /// one. The summary of the branch being left is attached at the destination,
    /// not at the abandoned branch: it is context for what happens next.
    pub async fn navigate_tree(
        self: &Arc<Self>,
        target_id: &str,
        options: NavigateTreeOptions,
    ) -> Result<NavigateTreeResult, String> {
        if self.is_streaming() {
            return Err(
                "Wait for the current response to finish before navigating the session tree."
                    .to_string(),
            );
        }

        let old_leaf_id = self
            .session_manager
            .lock()
            .expect("poisoned")
            .get_leaf_id()
            .map(str::to_string);

        if Some(target_id) == old_leaf_id.as_deref() {
            return Ok(NavigateTreeResult::default());
        }

        if options.summarize && self.model().is_none() {
            return Err("No model available for summarization".to_string());
        }

        let target_entry = self
            .session_manager
            .lock()
            .expect("poisoned")
            .get_entry(target_id)
            .cloned()
            .ok_or_else(|| format!("Entry {target_id} not found"))?;

        let entries_to_summarize = {
            let manager = self.session_manager.lock().expect("poisoned");
            let view = SessionManagerBranchView { manager: &manager };
            collect_entries_for_branch_summary(&view, old_leaf_id.as_deref(), target_id).entries
        };

        let signal = CancellationToken::new();
        *self.branch_summary_signal.lock().expect("poisoned") = Some(signal.clone());
        let outcome = self
            .navigate_tree_inner(
                target_id,
                &target_entry,
                old_leaf_id.as_deref(),
                entries_to_summarize,
                options,
                signal,
            )
            .await;
        *self.branch_summary_signal.lock().expect("poisoned") = None;
        outcome
    }

    async fn navigate_tree_inner(
        self: &Arc<Self>,
        target_id: &str,
        target_entry: &SessionEntry,
        old_leaf_id: Option<&str>,
        entries_to_summarize: Vec<SessionEntry>,
        options: NavigateTreeOptions,
        signal: CancellationToken,
    ) -> Result<NavigateTreeResult, String> {
        let mut summary_text: Option<String> = None;
        let mut summary_details: Option<Value> = None;
        let mut summary_usage: Option<Usage> = None;

        if options.summarize && !entries_to_summarize.is_empty() {
            let model = self.model().expect("model checked above");
            let (request_model, auth) = self.summarization_request_auth(&model).await?;
            let branch_settings = self.settings_manager.get_branch_summary_settings();
            let BranchSummaryResult {
                summary,
                usage,
                read_files,
                modified_files,
                aborted,
                error,
            } = generate_branch_summary(
                &entries_to_summarize,
                &request_model,
                &GenerateBranchSummaryOptions {
                    api_key: auth.api_key,
                    headers: auth.headers,
                    env: auth.env,
                    signal: Some(signal),
                    custom_instructions: options.custom_instructions.clone(),
                    replace_instructions: options.replace_instructions,
                    reserve_tokens: Some(branch_settings.reserve_tokens),
                    stream_fn: self.agent.options().stream_fn,
                    retry: Some(self.retry_policy()),
                    callbacks: Some(
                        self.summarization_retry_callbacks(SummarizationSource::BranchSummary),
                    ),
                    thinking_level: Some(self.thinking_level()),
                },
            )
            .await;

            if aborted {
                return Ok(NavigateTreeResult {
                    cancelled: true,
                    aborted: true,
                    ..NavigateTreeResult::default()
                });
            }
            if let Some(error) = error {
                return Err(error);
            }
            summary_text = summary;
            summary_usage = usage;
            summary_details = serde_json::to_value(crate::core::compaction::BranchSummaryDetails {
                read_files: read_files.unwrap_or_default(),
                modified_files: modified_files.unwrap_or_default(),
            })
            .ok();
        }

        // Where the leaf lands depends on what was navigated to: a message the
        // user wrote goes back into the editor and the leaf moves to its parent.
        let (new_leaf_id, editor_text) = match target_entry {
            SessionEntry::Message(entry)
                if entry.message.get("role").and_then(Value::as_str) == Some("user") =>
            {
                let text = entry
                    .message
                    .get("content")
                    .map(json_content_text)
                    .unwrap_or_default();
                (entry.parent_id.clone(), Some(text))
            }
            SessionEntry::CustomMessage(entry) => (
                entry.parent_id.clone(),
                Some(json_content_text(&entry.content)),
            ),
            _ => (Some(target_id.to_string()), None),
        };

        let mut summary_entry: Option<BranchSummaryEntry> = None;
        {
            let mut manager = self.session_manager.lock().expect("poisoned");
            if let Some(summary) = summary_text.as_ref() {
                let summary_id = manager
                    .branch_with_summary(
                        new_leaf_id.as_deref(),
                        summary,
                        summary_details.clone(),
                        None,
                        summary_usage,
                    )
                    .map_err(|error| error.to_string())?;
                if let Some(SessionEntry::BranchSummary(entry)) = manager.get_entry(&summary_id) {
                    summary_entry = Some(entry.clone());
                }
                if let Some(label) = options.label.as_ref() {
                    let _ = manager.append_label_change(&summary_id, Some(label.as_str()));
                }
            } else if let Some(leaf) = new_leaf_id.as_deref() {
                manager.branch(leaf).map_err(|error| error.to_string())?;
            } else {
                manager.reset_leaf();
            }

            // Without a summary there is no summary entry to label, so the label
            // goes on the target.
            if summary_text.is_none()
                && let Some(label) = options.label.as_ref()
            {
                let _ = manager.append_label_change(target_id, Some(label.as_str()));
            }

            let context = manager.build_session_context();
            drop(manager);
            self.agent.set_messages(context.messages);
        }

        let _ = old_leaf_id;
        Ok(NavigateTreeResult {
            editor_text,
            cancelled: false,
            aborted: false,
            summary_entry,
        })
    }

    /// The user messages a fork selector offers.
    pub fn get_user_messages_for_forking(&self) -> Vec<(String, String)> {
        self.session_manager
            .lock()
            .expect("poisoned")
            .get_entries()
            .iter()
            .filter_map(|entry| {
                let SessionEntry::Message(entry) = entry else {
                    return None;
                };
                if entry.message.get("role").and_then(Value::as_str) != Some("user") {
                    return None;
                }
                let text = entry
                    .message
                    .get("content")
                    .map(json_content_text)
                    .unwrap_or_default();
                (!text.is_empty()).then(|| (entry.id.clone(), text))
            })
            .collect()
    }

    /// Session statistics.
    ///
    /// Aggregated over ALL entries, compacted-away history included, so the
    /// totals reflect what was actually billed across the session.
    pub fn get_session_stats(&self) -> SessionStats {
        let mut user_messages = 0u64;
        let mut assistant_messages = 0u64;
        let mut tool_results = 0u64;
        let mut total_messages = 0u64;
        let mut tool_calls = 0u64;
        let mut totals = SessionTokenTotals::default();
        let mut cost = 0.0f64;

        let add_usage = |usage: &Usage, totals: &mut SessionTokenTotals, cost: &mut f64| {
            totals.input += usage.input;
            totals.output += usage.output;
            totals.cache_read += usage.cache_read;
            totals.cache_write += usage.cache_write;
            *cost += usage.cost.total;
        };

        for entry in self.session_manager.lock().expect("poisoned").get_entries() {
            match &entry {
                SessionEntry::BranchSummary(entry) => {
                    if let Some(usage) = entry.usage.as_ref() {
                        add_usage(usage, &mut totals, &mut cost);
                    }
                }
                SessionEntry::Compaction(entry) => {
                    if let Some(usage) = entry.usage.as_ref() {
                        add_usage(usage, &mut totals, &mut cost);
                    }
                }
                SessionEntry::Message(entry) => {
                    total_messages += 1;
                    match entry.message.get("role").and_then(Value::as_str) {
                        Some("user") => user_messages += 1,
                        Some("toolResult") => {
                            tool_results += 1;
                            if let Some(usage) = entry.message.get("usage").and_then(|usage| {
                                serde_json::from_value::<Usage>(usage.clone()).ok()
                            }) {
                                add_usage(&usage, &mut totals, &mut cost);
                            }
                        }
                        Some("assistant") => {
                            assistant_messages += 1;
                            if let Some(content) =
                                entry.message.get("content").and_then(Value::as_array)
                            {
                                tool_calls += content
                                    .iter()
                                    .filter(|block| {
                                        block.get("type").and_then(Value::as_str)
                                            == Some("toolCall")
                                    })
                                    .count() as u64;
                            }
                            if let Some(usage) = entry.message.get("usage").and_then(|usage| {
                                serde_json::from_value::<Usage>(usage.clone()).ok()
                            }) {
                                add_usage(&usage, &mut totals, &mut cost);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        totals.total = totals.input + totals.output + totals.cache_read + totals.cache_write;

        SessionStats {
            session_file: self.session_file(),
            session_id: self.session_id(),
            user_messages,
            assistant_messages,
            tool_calls,
            tool_results,
            total_messages,
            tokens: totals,
            cost,
            context_usage: self.get_context_usage(),
        }
    }

    pub fn get_context_usage(&self) -> Option<ContextUsage> {
        let model = self.model()?;
        let context_window = model.context_window;
        if context_window == 0 {
            return None;
        }

        // After a compaction the last assistant usage still reflects the
        // pre-compaction context. Only a response that arrived after the
        // boundary can be trusted; until then the count is unknown.
        let branch_entries: Vec<SessionEntry> = self
            .session_manager
            .lock()
            .expect("poisoned")
            .get_branch(None)
            .into_iter()
            .cloned()
            .collect();
        if let Some(latest) = get_latest_compaction_entry(&branch_entries) {
            let compaction_index = branch_entries
                .iter()
                .rposition(|entry| entry.id() == latest.id)
                .unwrap_or(0);
            let has_post_compaction_usage = branch_entries[compaction_index + 1..]
                .iter()
                .rev()
                .any(|entry| {
                    let SessionEntry::Message(entry) = entry else {
                        return false;
                    };
                    let Ok(AgentMessage::Assistant(assistant)) =
                        serde_json::from_value::<AgentMessage>(entry.message.clone())
                    else {
                        return false;
                    };
                    assistant.stop_reason != StopReason::Aborted
                        && assistant.stop_reason != StopReason::Error
                        && calculate_context_tokens(&assistant.usage) > 0
                });

            if !has_post_compaction_usage {
                return Some(ContextUsage {
                    tokens: None,
                    context_window,
                    percent: None,
                });
            }
        }

        let estimate = estimate_context_tokens(&self.messages());
        Some(ContextUsage {
            tokens: Some(estimate.tokens),
            context_window,
            percent: Some(estimate.tokens as f64 / context_window as f64 * 100.0),
        })
    }

    /// Exports the current branch to a JSONL file: the header, then every entry
    /// on the path with its parents re-chained into a straight line.
    pub fn export_to_jsonl(&self, output_path: Option<&str>) -> Result<String, String> {
        let manager = self.session_manager.lock().expect("poisoned");
        let default_name = format!(
            "session-{}.jsonl",
            chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S-%3fZ")
        );
        let file_path = resolve_path_default(output_path.unwrap_or(&default_name), &current_dir())
            .map_err(|error| error.to_string())?;
        if let Some(directory) = Path::new(&file_path).parent()
            && !directory.exists()
        {
            std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        }

        let header = serde_json::json!({
            "type": "session",
            "version": crate::core::session_manager::CURRENT_SESSION_VERSION,
            "id": manager.get_session_id(),
            "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "cwd": manager.get_cwd(),
        });
        let mut lines = vec![header.to_string()];

        let mut previous_id: Option<String> = None;
        for entry in manager.get_branch(None) {
            let mut value = serde_json::to_value(entry).unwrap_or(Value::Null);
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "parentId".to_string(),
                    previous_id.clone().map_or(Value::Null, Value::from),
                );
            }
            lines.push(value.to_string());
            previous_id = Some(entry.id().to_string());
        }

        std::fs::write(&file_path, format!("{}\n", lines.join("\n")))
            .map_err(|error| error.to_string())?;
        Ok(file_path)
    }

    /// The text of the last assistant message, for `/copy`.
    pub fn get_last_assistant_text(&self) -> Option<String> {
        let messages = self.messages();
        let last = messages.iter().rev().find(|message| match message {
            AgentMessage::Assistant(assistant) => {
                // An aborted message with nothing in it is not an answer.
                !(assistant.stop_reason == StopReason::Aborted && assistant.content.is_empty())
            }
            _ => false,
        })?;
        let AgentMessage::Assistant(assistant) = last else {
            return None;
        };
        let text: String = assistant
            .content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect();
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    /// Reloads settings, resources, modes and tools.
    pub async fn reload(self: &Arc<Self>) {
        if let Some(hooks) = self.hooks.as_ref() {
            hooks.session_shutdown("reload").await;
        }
        self.settings_manager.reload();
        self.sync_queue_modes_from_settings();
        self.resource_loader.reload(Default::default()).await;
        self.build_runtime(BuildRuntimeOptions {
            active_tool_names: Some(self.get_active_tool_names()),
        });
        if let Some(hooks) = self.hooks.as_ref() {
            hooks.session_start("reload").await;
        }
    }

    /// Announces the end of the session to the user's hooks, with the reason the
    /// runtime is replacing it. Called before `dispose`.
    pub async fn shutdown_hooks(&self, reason: &str) {
        if let Some(hooks) = self.hooks.as_ref() {
            hooks.session_shutdown(reason).await;
        }
    }

    /// Announces the session to the user's hooks. Called once, after the host
    /// has bound whatever UI it has.
    pub async fn start(&self) {
        if let Some(hooks) = self.hooks.as_ref() {
            hooks.session_start(&self.session_start_reason).await;
        }
    }

    /// Removes all listeners, stops everything running, and disconnects from
    /// the agent. Call this when completely done with the session.
    pub fn dispose(&self) {
        self.abort_retry();
        self.abort_compaction();
        self.abort_branch_summary();
        self.abort_bash();
        self.agent.abort();

        self.stop_background_tasks();
        if let Some(unsubscribe) = self.unsubscribe_agent.lock().expect("poisoned").take() {
            unsubscribe();
        }
        self.listeners.lock().expect("poisoned").clear();
        notagent_ai::session_resources::cleanup_session_resources(Some(&self.session_id()));
    }
}

/// The branch view the summarizer needs, borrowed from the locked manager.
struct SessionManagerBranchView<'a> {
    manager: &'a SessionManager,
}

impl crate::core::compaction::BranchSummarySession for SessionManagerBranchView<'_> {
    fn get_branch(&self, leaf_id: &str) -> Vec<SessionEntry> {
        self.manager
            .get_branch(Some(leaf_id))
            .into_iter()
            .cloned()
            .collect()
    }

    fn get_entry(&self, id: &str) -> Option<SessionEntry> {
        self.manager.get_entry(id).cloned()
    }
}

/// The text of a raw JSON message content field: a string, or the text blocks of
/// an array.
fn json_content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Keeps `content_text` reachable for callers that hold typed content.
pub fn assistant_text(message: &AssistantMessage) -> String {
    content_text(&message.content)
}

/// The agent's thinking level as the model layer names it.
fn model_thinking_level(level: ThinkingLevel) -> notagent_ai::types::ModelThinkingLevel {
    use notagent_ai::types::ModelThinkingLevel;
    match level {
        ThinkingLevel::Off => ModelThinkingLevel::Off,
        ThinkingLevel::Minimal => ModelThinkingLevel::Minimal,
        ThinkingLevel::Low => ModelThinkingLevel::Low,
        ThinkingLevel::Medium => ModelThinkingLevel::Medium,
        ThinkingLevel::High => ModelThinkingLevel::High,
        ThinkingLevel::Xhigh => ModelThinkingLevel::Xhigh,
        ThinkingLevel::Max => ModelThinkingLevel::Max,
    }
}

/// The queue mode as the agent names it. The two enums are the same two values
/// in two crates; TypeScript has one string union for both.
fn agent_queue_mode(mode: crate::core::settings_manager::QueueMode) -> QueueMode {
    match mode {
        crate::core::settings_manager::QueueMode::All => QueueMode::All,
        crate::core::settings_manager::QueueMode::OneAtATime => QueueMode::OneAtATime,
    }
}

fn settings_queue_mode(mode: QueueMode) -> crate::core::settings_manager::QueueMode {
    match mode {
        QueueMode::All => crate::core::settings_manager::QueueMode::All,
        QueueMode::OneAtATime => crate::core::settings_manager::QueueMode::OneAtATime,
    }
}

/// The real model runtime, read through the seam.
///
/// The trait is C's and the type is B's, so the implementation lives here and
/// workstream B needs no change (interface request C-13, resolved this way once
/// `model-runtime.rs` landed).
impl SessionModelRuntime for crate::core::model_runtime::ModelRuntime {
    fn get_auth<'a>(
        &'a self,
        model: &'a Model,
    ) -> BoxFuture<'a, Result<Option<SessionAuth>, String>> {
        Box::pin(async move {
            match self.get_auth_for_model(model, None).await {
                Ok(Some(result)) => Ok(Some(SessionAuth {
                    api_key: result.auth.api_key,
                    // `withoutDeletedHeaders`: a `null` value deletes a default
                    // header and is not something a request carries.
                    headers: result.auth.headers.map(|headers| {
                        headers
                            .into_iter()
                            .filter_map(|(name, value)| value.map(|value| (name, value)))
                            .collect()
                    }),
                    base_url: result.auth.base_url,
                    env: result
                        .env
                        .map(|env| env.into_iter().collect::<Vec<(String, String)>>()),
                })),
                Ok(None) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        })
    }

    fn has_configured_auth(&self, provider: &str) -> bool {
        crate::core::model_runtime::ModelRuntime::has_configured_auth(self, provider)
    }

    fn check_auth<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            matches!(
                crate::core::model_runtime::ModelRuntime::check_auth(self, provider, None).await,
                Ok(Some(_))
            )
        })
    }

    fn is_using_oauth(&self, provider: &str) -> bool {
        crate::core::model_runtime::ModelRuntime::is_using_oauth(self, provider)
    }

    fn is_using_subscription(&self, provider: &str) -> bool {
        crate::core::model_runtime::ModelRuntime::is_using_subscription(self, provider)
    }

    fn get_available_snapshot(&self) -> Vec<Model> {
        crate::core::model_runtime::ModelRuntime::get_available_snapshot(self)
    }

    fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
        crate::core::model_runtime::ModelRuntime::get_model(self, provider, id)
    }
}
