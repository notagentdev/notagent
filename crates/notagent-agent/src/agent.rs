//! Stateful wrapper around the low-level agent loop.
//!
//! 1:1 port of `packages/agent/src/agent.ts` (592 LOC). The `Agent` owns the transcript,
//! reduces loop events into its state, awaits its listeners in subscription order and
//! exposes the steering and follow-up queues.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use notagent_ai::types::{
    AssistantContent, ImageContent, Message, Model, ModelCost, SimpleStreamOptions, StopReason,
    TextContent, TextOrImageContent, ThinkingBudgets, Transport, Usage, UserContent, UserMessage,
};
use tokio_util::sync::CancellationToken;

use crate::agent_loop::{AgentEventSink, run_agent_loop, run_agent_loop_continue};
use crate::stream_fn::get_default_stream_fn;
use crate::types::{
    AfterToolCallFn, AgentContext, AgentEvent, AgentLoopConfig, AgentMessage, AgentTool,
    AgentToolResult, BeforeToolCallFn, BoxFuture, ConvertToLlmFn, GetApiKeyFn, PrepareNextTurnFn,
    QueueMode, ShouldStopAfterTurnFn, StreamFn, ThinkingLevel, ToolExecutionMode,
    TransformContextFn,
};

/// `DEFAULT_MODEL` of `agent.ts:48-59`.
pub fn default_model() -> Model {
    Model {
        id: "unknown".to_string(),
        name: "unknown".to_string(),
        api: "unknown".to_string(),
        provider: "unknown".to_string(),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

/// `EMPTY_USAGE`
fn empty_usage() -> Usage {
    Usage::default()
}

/// `defaultConvertToLlm` — keeps the three LLM roles.
pub fn default_convert_to_llm() -> ConvertToLlmFn {
    Arc::new(|messages: Vec<AgentMessage>| {
        Box::pin(async move {
            messages
                .iter()
                .filter_map(AgentMessage::as_llm_message)
                .collect()
        })
    })
}

/// `PendingMessageQueue`
struct PendingMessageQueue {
    messages: Vec<AgentMessage>,
    mode: QueueMode,
}

impl PendingMessageQueue {
    fn new(mode: QueueMode) -> Self {
        PendingMessageQueue {
            messages: Vec::new(),
            mode,
        }
    }

    fn enqueue(&mut self, message: AgentMessage) {
        self.messages.push(message);
    }

    fn has_items(&self) -> bool {
        !self.messages.is_empty()
    }

    fn drain(&mut self) -> Vec<AgentMessage> {
        match self.mode {
            QueueMode::All => std::mem::take(&mut self.messages),
            QueueMode::OneAtATime => {
                if self.messages.is_empty() {
                    Vec::new()
                } else {
                    vec![self.messages.remove(0)]
                }
            }
        }
    }

    fn clear(&mut self) {
        self.messages.clear();
    }
}

/// Mutable runtime state; the public snapshot is [`crate::types::AgentState`].
struct MutableAgentState {
    system_prompt: String,
    model: Model,
    thinking_level: ThinkingLevel,
    tools: Vec<Arc<dyn AgentTool>>,
    messages: Vec<AgentMessage>,
    is_streaming: bool,
    streaming_message: Option<AgentMessage>,
    pending_tool_calls: BTreeSet<String>,
    error_message: Option<String>,
}

/// `AgentOptions`
#[derive(Clone, Default)]
pub struct AgentOptions {
    pub system_prompt: Option<String>,
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub tools: Option<Vec<Arc<dyn AgentTool>>>,
    pub messages: Option<Vec<AgentMessage>>,
    pub convert_to_llm: Option<ConvertToLlmFn>,
    pub transform_context: Option<TransformContextFn>,
    pub stream_fn: Option<StreamFn>,
    pub get_api_key: Option<GetApiKeyFn>,
    /// `onPayload`/`onResponse` — TS forwards them into `streamSimple` (`agent.ts:452`).
    pub on_payload: Option<notagent_ai::types::OnPayload<Model>>,
    pub on_response: Option<notagent_ai::types::OnResponse<Model>>,
    pub before_tool_call: Option<BeforeToolCallFn>,
    pub after_tool_call: Option<AfterToolCallFn>,
    pub should_stop_after_turn: Option<ShouldStopAfterTurnFn>,
    pub prepare_next_turn: Option<PrepareNextTurnFn>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
    pub session_id: Option<String>,
    pub thinking_budgets: Option<ThinkingBudgets>,
    pub transport: Option<Transport>,
    pub max_retry_delay_ms: Option<u64>,
    pub tool_execution: Option<ToolExecutionMode>,
}

/// A subscribed listener; awaited in subscription order with the run's signal.
pub type AgentListener =
    Arc<dyn Fn(AgentEvent, CancellationToken) -> BoxFuture<'static, ()> + Send + Sync>;

/// Error of the run-lifecycle methods (TS throws).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AgentError(pub String);

struct ActiveRun {
    signal: CancellationToken,
    /// Resolves once the run and its `agent_end` listeners have settled.
    idle: Arc<tokio::sync::Notify>,
}

/// `Agent`
pub struct Agent {
    state: Mutex<MutableAgentState>,
    listeners: Mutex<Vec<(u64, AgentListener)>>,
    next_listener_id: Mutex<u64>,
    steering_queue: Mutex<PendingMessageQueue>,
    follow_up_queue: Mutex<PendingMessageQueue>,
    active_run: Mutex<Option<ActiveRun>>,
    options: Mutex<AgentOptions>,
}

impl Agent {
    /// `new Agent(options)`
    pub fn new(options: AgentOptions) -> Arc<Self> {
        let state = MutableAgentState {
            system_prompt: options.system_prompt.clone().unwrap_or_default(),
            model: options.model.clone().unwrap_or_else(default_model),
            thinking_level: options.thinking_level.unwrap_or(ThinkingLevel::Off),
            tools: options.tools.clone().unwrap_or_default(),
            messages: options.messages.clone().unwrap_or_default(),
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: BTreeSet::new(),
            error_message: None,
        };
        Arc::new(Agent {
            state: Mutex::new(state),
            listeners: Mutex::new(Vec::new()),
            next_listener_id: Mutex::new(0),
            steering_queue: Mutex::new(PendingMessageQueue::new(
                options.steering_mode.unwrap_or(QueueMode::OneAtATime),
            )),
            follow_up_queue: Mutex::new(PendingMessageQueue::new(
                options.follow_up_mode.unwrap_or(QueueMode::OneAtATime),
            )),
            active_run: Mutex::new(None),
            options: Mutex::new(options),
        })
    }

    /// `subscribe(listener)` — returns the unsubscribe handle.
    pub fn subscribe(self: &Arc<Self>, listener: AgentListener) -> impl FnOnce() + use<> {
        let mut next_id = self.next_listener_id.lock().expect("poisoned");
        let id = *next_id;
        *next_id += 1;
        drop(next_id);
        self.listeners
            .lock()
            .expect("poisoned")
            .push((id, listener));

        let agent = Arc::clone(self);
        move || {
            agent
                .listeners
                .lock()
                .expect("poisoned")
                .retain(|(listener_id, _)| *listener_id != id);
        }
    }

    /// `state` — a snapshot; the live state stays owned by the agent.
    pub fn state(&self) -> crate::types::AgentState {
        let state = self.state.lock().expect("poisoned");
        crate::types::AgentState {
            system_prompt: state.system_prompt.clone(),
            model: state.model.clone(),
            thinking_level: state.thinking_level,
            tools: state.tools.clone(),
            messages: state.messages.clone(),
            is_streaming: state.is_streaming,
            streaming_message: state.streaming_message.clone(),
            pending_tool_calls: state.pending_tool_calls.clone(),
            error_message: state.error_message.clone(),
        }
    }

    /// Assigning tools copies the top-level array, as the TS setter does.
    pub fn set_tools(&self, tools: Vec<Arc<dyn AgentTool>>) {
        self.state.lock().expect("poisoned").tools = tools;
    }

    pub fn set_messages(&self, messages: Vec<AgentMessage>) {
        self.state.lock().expect("poisoned").messages = messages;
    }

    pub fn set_system_prompt(&self, system_prompt: impl Into<String>) {
        self.state.lock().expect("poisoned").system_prompt = system_prompt.into();
    }

    pub fn set_model(&self, model: Model) {
        self.state.lock().expect("poisoned").model = model;
    }

    pub fn set_thinking_level(&self, thinking_level: ThinkingLevel) {
        self.state.lock().expect("poisoned").thinking_level = thinking_level;
    }

    pub fn steering_mode(&self) -> QueueMode {
        self.steering_queue.lock().expect("poisoned").mode
    }

    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.steering_queue.lock().expect("poisoned").mode = mode;
    }

    pub fn follow_up_mode(&self) -> QueueMode {
        self.follow_up_queue.lock().expect("poisoned").mode
    }

    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.follow_up_queue.lock().expect("poisoned").mode = mode;
    }

    /// The current provider wiring. TS keeps `streamFunction`, `getApiKey`, `onPayload`,
    /// `onResponse`, `beforeToolCall`, `afterToolCall`, `thinkingBudgets`, `transport`,
    /// `maxRetryDelayMs` and `toolExecution` as public fields on `Agent`
    /// (`packages/agent/src/agent.ts:180-201`); the Rust port folds them into
    /// `AgentOptions`, so this hands out the same view as a clone.
    pub fn options(&self) -> AgentOptions {
        self.options.lock().expect("poisoned").clone()
    }

    /// The counterpart of TS's field assignments (`session.agent.transport = …`,
    /// `agent.beforeToolCall = …`). The next `createLoopConfig` picks the change up, so a
    /// running turn keeps the wiring it started with — exactly as in TS, where the loop
    /// captured the field values when the run began.
    pub fn update_options(&self, update: impl FnOnce(&mut AgentOptions)) {
        update(&mut self.options.lock().expect("poisoned"));
    }

    /// `steer(message)`
    pub fn steer(&self, message: AgentMessage) {
        self.steering_queue
            .lock()
            .expect("poisoned")
            .enqueue(message);
    }

    /// `followUp(message)`
    pub fn follow_up(&self, message: AgentMessage) {
        self.follow_up_queue
            .lock()
            .expect("poisoned")
            .enqueue(message);
    }

    pub fn clear_steering_queue(&self) {
        self.steering_queue.lock().expect("poisoned").clear();
    }

    pub fn clear_follow_up_queue(&self) {
        self.follow_up_queue.lock().expect("poisoned").clear();
    }

    pub fn clear_all_queues(&self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }

    /// `hasQueuedMessages()`
    pub fn has_queued_messages(&self) -> bool {
        self.steering_queue.lock().expect("poisoned").has_items()
            || self.follow_up_queue.lock().expect("poisoned").has_items()
    }

    /// `signal` — the active run's token, if any.
    pub fn signal(&self) -> Option<CancellationToken> {
        self.active_run
            .lock()
            .expect("poisoned")
            .as_ref()
            .map(|run| run.signal.clone())
    }

    /// `abort()`
    pub fn abort(&self) {
        if let Some(run) = self.active_run.lock().expect("poisoned").as_ref() {
            run.signal.cancel();
        }
    }

    /// `waitForIdle()` — resolves after the `agent_end` listeners have settled.
    pub async fn wait_for_idle(&self) {
        let idle = self
            .active_run
            .lock()
            .expect("poisoned")
            .as_ref()
            .map(|run| Arc::clone(&run.idle));
        let Some(idle) = idle else {
            return;
        };
        // The waiter is registered before the second look at `active_run`:
        // `notified()` registers on first poll, so a run that finishes in
        // between would otherwise notify nobody and this would wait forever.
        let notified = idle.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // The second look asks whether *this* run is still the active one, not
        // whether any run is. TS hands out `activeRun.promise`, which stays
        // resolved once that run finished; a successor run occupying the slot
        // does not make `waitForIdle()` wait again.
        let still_running = self
            .active_run
            .lock()
            .expect("poisoned")
            .as_ref()
            .is_some_and(|run| Arc::ptr_eq(&run.idle, &idle));
        if !still_running {
            return;
        }
        notified.await;
    }

    /// `reset()`
    pub fn reset(&self) -> Result<(), AgentError> {
        if self.active_run.lock().expect("poisoned").is_some() {
            return Err(AgentError(
                "Agent is already processing. Wait for completion before resetting.".to_string(),
            ));
        }
        {
            let mut state = self.state.lock().expect("poisoned");
            state.messages.clear();
            state.is_streaming = false;
            state.streaming_message = None;
            state.pending_tool_calls.clear();
            state.error_message = None;
        }
        self.clear_all_queues();
        Ok(())
    }

    /// `prompt(input, images?)` for plain text.
    pub async fn prompt_text(
        self: &Arc<Self>,
        input: impl Into<String>,
        images: Option<Vec<ImageContent>>,
        timestamp: i64,
    ) -> Result<(), AgentError> {
        let mut content: Vec<TextOrImageContent> =
            vec![TextOrImageContent::Text(TextContent::new(input))];
        content.extend(images.into_iter().flatten().map(TextOrImageContent::Image));
        self.prompt(vec![AgentMessage::User(UserMessage {
            content: UserContent::Blocks(content),
            timestamp,
        })])
        .await
    }

    /// `prompt(messages)`
    pub async fn prompt(self: &Arc<Self>, messages: Vec<AgentMessage>) -> Result<(), AgentError> {
        if self.active_run.lock().expect("poisoned").is_some() {
            return Err(AgentError(
                "Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion."
                    .to_string(),
            ));
        }
        self.run_prompt_messages(messages, false).await
    }

    /// `continue()`
    pub async fn continue_run(self: &Arc<Self>) -> Result<(), AgentError> {
        if self.active_run.lock().expect("poisoned").is_some() {
            return Err(AgentError(
                "Agent is already processing. Wait for completion before continuing.".to_string(),
            ));
        }

        let last_message = self
            .state
            .lock()
            .expect("poisoned")
            .messages
            .last()
            .cloned();
        let Some(last_message) = last_message else {
            return Err(AgentError("No messages to continue from".to_string()));
        };

        if matches!(last_message, AgentMessage::Assistant(_)) {
            // Steering first, then follow-ups.
            let queued_steering = self.steering_queue.lock().expect("poisoned").drain();
            if !queued_steering.is_empty() {
                return self.run_prompt_messages(queued_steering, true).await;
            }
            let queued_follow_ups = self.follow_up_queue.lock().expect("poisoned").drain();
            if !queued_follow_ups.is_empty() {
                return self.run_prompt_messages(queued_follow_ups, false).await;
            }
            return Err(AgentError(
                "Cannot continue from message role: assistant".to_string(),
            ));
        }

        self.run_continuation().await
    }

    fn create_context_snapshot(&self) -> AgentContext {
        let state = self.state.lock().expect("poisoned");
        AgentContext {
            system_prompt: state.system_prompt.clone(),
            messages: state.messages.clone(),
            tools: Some(state.tools.clone()),
        }
    }

    fn create_loop_config(self: &Arc<Self>, skip_initial_steering_poll: bool) -> AgentLoopConfig {
        let options = self.options.lock().expect("poisoned").clone();
        let (model, thinking_level) = {
            let state = self.state.lock().expect("poisoned");
            (state.model.clone(), state.thinking_level)
        };

        let mut base = SimpleStreamOptions {
            reasoning: thinking_level.to_reasoning(),
            thinking_budgets: options.thinking_budgets,
            ..Default::default()
        };
        base.base.session_id = options.session_id.clone();
        base.base.base.on_payload = options.on_payload.clone();
        base.base.base.on_response = options.on_response.clone();
        base.base.transport = Some(options.transport.unwrap_or(Transport::Auto));
        base.base.base.max_retry_delay_ms = options.max_retry_delay_ms;

        let skip_poll = Arc::new(std::sync::atomic::AtomicBool::new(
            skip_initial_steering_poll,
        ));
        let steering_agent = Arc::clone(self);
        let follow_up_agent = Arc::clone(self);

        AgentLoopConfig {
            base,
            model,
            convert_to_llm: options
                .convert_to_llm
                .clone()
                .unwrap_or_else(default_convert_to_llm),
            transform_context: options.transform_context.clone(),
            get_api_key: options.get_api_key.clone(),
            should_stop_after_turn: options.should_stop_after_turn.clone(),
            prepare_next_turn: options.prepare_next_turn.clone(),
            get_steering_messages: Some(Arc::new(move || {
                let agent = Arc::clone(&steering_agent);
                let skip_poll = Arc::clone(&skip_poll);
                Box::pin(async move {
                    // `continue()` already drained the queue for this run.
                    if skip_poll.swap(false, std::sync::atomic::Ordering::SeqCst) {
                        return Vec::new();
                    }
                    agent.steering_queue.lock().expect("poisoned").drain()
                })
            })),
            get_follow_up_messages: Some(Arc::new(move || {
                let agent = Arc::clone(&follow_up_agent);
                Box::pin(async move { agent.follow_up_queue.lock().expect("poisoned").drain() })
            })),
            tool_execution: Some(
                options
                    .tool_execution
                    .unwrap_or(ToolExecutionMode::Parallel),
            ),
            before_tool_call: options.before_tool_call.clone(),
            after_tool_call: options.after_tool_call.clone(),
        }
    }

    fn event_sink(self: &Arc<Self>) -> AgentEventSink {
        let agent = Arc::clone(self);
        Arc::new(move |event| {
            let agent = Arc::clone(&agent);
            Box::pin(async move { agent.process_events(event).await })
        })
    }

    fn stream_function(&self) -> Option<StreamFn> {
        self.options
            .lock()
            .expect("poisoned")
            .stream_fn
            .clone()
            .or_else(|| get_default_stream_fn().ok())
    }

    async fn run_prompt_messages(
        self: &Arc<Self>,
        messages: Vec<AgentMessage>,
        skip_initial_steering_poll: bool,
    ) -> Result<(), AgentError> {
        let context = self.create_context_snapshot();
        let config = self.create_loop_config(skip_initial_steering_poll);
        let sink = self.event_sink();
        let stream_fn = self.stream_function();

        self.run_with_lifecycle(move |signal| {
            Box::pin(async move {
                run_agent_loop(messages, context, config, sink, Some(signal), stream_fn).await;
                Ok(())
            })
        })
        .await
    }

    async fn run_continuation(self: &Arc<Self>) -> Result<(), AgentError> {
        let context = self.create_context_snapshot();
        let config = self.create_loop_config(false);
        let sink = self.event_sink();
        let stream_fn = self.stream_function();

        self.run_with_lifecycle(move |signal| {
            Box::pin(async move {
                run_agent_loop_continue(context, config, sink, Some(signal), stream_fn)
                    .await
                    .map(|_| ())
                    .map_err(|error| AgentError(error.to_string()))
            })
        })
        .await
    }

    /// `runWithLifecycle(executor)`
    async fn run_with_lifecycle<F>(self: &Arc<Self>, executor: F) -> Result<(), AgentError>
    where
        F: FnOnce(CancellationToken) -> BoxFuture<'static, Result<(), AgentError>>,
    {
        let signal = CancellationToken::new();
        let idle = Arc::new(tokio::sync::Notify::new());
        {
            let mut active_run = self.active_run.lock().expect("poisoned");
            if active_run.is_some() {
                return Err(AgentError("Agent is already processing.".to_string()));
            }
            *active_run = Some(ActiveRun {
                signal: signal.clone(),
                idle: Arc::clone(&idle),
            });
        }
        {
            let mut state = self.state.lock().expect("poisoned");
            state.is_streaming = true;
            state.streaming_message = None;
            state.error_message = None;
        }

        let outcome = executor(signal.clone()).await;
        if let Err(error) = &outcome {
            self.handle_run_failure(error, signal.is_cancelled()).await;
        }
        self.finish_run();
        idle.notify_waiters();
        Ok(())
    }

    /// `handleRunFailure(error, aborted)` — synthesizes the final assistant message.
    async fn handle_run_failure(self: &Arc<Self>, error: &AgentError, aborted: bool) {
        let model = self.state.lock().expect("poisoned").model.clone();
        let failure_message = AgentMessage::Assistant(notagent_ai::types::AssistantMessage {
            content: vec![AssistantContent::Text(TextContent::new(""))],
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: empty_usage(),
            stop_reason: if aborted {
                StopReason::Aborted
            } else {
                StopReason::Error
            },
            deferred: None,
            error_message: Some(error.to_string()),
            raw_stop_reason: None,
            end_turn: None,
            timestamp: now_ms(),
        });
        self.process_events(AgentEvent::MessageStart {
            message: failure_message.clone(),
        })
        .await;
        self.process_events(AgentEvent::MessageEnd {
            message: failure_message.clone(),
        })
        .await;
        self.process_events(AgentEvent::TurnEnd {
            message: failure_message.clone(),
            tool_results: Vec::new(),
        })
        .await;
        self.process_events(AgentEvent::AgentEnd {
            messages: vec![failure_message],
        })
        .await;
    }

    fn finish_run(&self) {
        {
            let mut state = self.state.lock().expect("poisoned");
            state.is_streaming = false;
            state.streaming_message = None;
            state.pending_tool_calls.clear();
        }
        *self.active_run.lock().expect("poisoned") = None;
    }

    /// `processEvents(event)` — reducer first, then the listeners in subscription order.
    async fn process_events(self: &Arc<Self>, event: AgentEvent) {
        {
            let mut state = self.state.lock().expect("poisoned");
            match &event {
                AgentEvent::MessageStart { message }
                | AgentEvent::MessageUpdate { message, .. } => {
                    state.streaming_message = Some(message.clone());
                }
                AgentEvent::MessageEnd { message } => {
                    state.streaming_message = None;
                    state.messages.push(message.clone());
                }
                AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
                    // Copy-on-write, as in TS.
                    let mut pending = state.pending_tool_calls.clone();
                    pending.insert(tool_call_id.clone());
                    state.pending_tool_calls = pending;
                }
                AgentEvent::ToolExecutionEnd { tool_call_id, .. } => {
                    let mut pending = state.pending_tool_calls.clone();
                    pending.remove(tool_call_id);
                    state.pending_tool_calls = pending;
                }
                AgentEvent::TurnEnd { message, .. } => {
                    if let AgentMessage::Assistant(message) = message
                        && let Some(error_message) = &message.error_message
                    {
                        state.error_message = Some(error_message.clone());
                    }
                }
                AgentEvent::AgentEnd { .. } => {
                    state.streaming_message = None;
                }
                _ => {}
            }
        }

        let signal = self.signal();
        let Some(signal) = signal else {
            // TS throws here; without an active run there is nothing to notify.
            return;
        };
        let listeners: Vec<AgentListener> = self
            .listeners
            .lock()
            .expect("poisoned")
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        // Sequential awaits are semantically relevant (backpressure, idle definition).
        for listener in listeners {
            listener(event.clone(), signal.clone()).await;
        }
    }
}

/// Convenience for callers that build tool results.
pub fn text_tool_result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult::text(text)
}

/// Convenience re-export for `Message` consumers.
pub fn as_llm_messages(messages: &[AgentMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(AgentMessage::as_llm_message)
        .collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before the Unix epoch")
        .as_millis() as i64
}
