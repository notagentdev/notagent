//! Faux provider for tests.
//!
//! 1:1 port of `packages/ai/src/providers/faux.ts` (708 LOC). It scripts assistant
//! responses, streams them as deltas, estimates usage with a prompt cache and is the
//! only provider that implements deferred responses. Workstream C's headless test
//! harness runs against it.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};
use crate::types::*;
use crate::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};

const DEFAULT_API: &str = "faux";
const DEFAULT_PROVIDER: &str = "faux";
const DEFAULT_MODEL_ID: &str = "faux-1";
const DEFAULT_MODEL_NAME: &str = "Faux Model";
const DEFAULT_BASE_URL: &str = "http://localhost:0";
const DEFAULT_MIN_TOKEN_SIZE: usize = 3;
const DEFAULT_MAX_TOKEN_SIZE: usize = 5;

/// `FauxModelDefinition`
#[derive(Debug, Clone)]
pub struct FauxModelDefinition {
    pub id: String,
    pub name: Option<String>,
    pub reasoning: Option<bool>,
    pub input: Option<Vec<Modality>>,
    pub cost: Option<ModelCost>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
}

impl FauxModelDefinition {
    pub fn new(id: impl Into<String>) -> Self {
        FauxModelDefinition {
            id: id.into(),
            name: None,
            reasoning: None,
            input: None,
            cost: None,
            context_window: None,
            max_tokens: None,
        }
    }
}

/// `fauxText(text)`
pub fn faux_text(text: impl Into<String>) -> AssistantContent {
    AssistantContent::Text(TextContent::new(text))
}

/// `fauxThinking(thinking)`
pub fn faux_thinking(thinking: impl Into<String>) -> AssistantContent {
    AssistantContent::Thinking(ThinkingContent {
        thinking: thinking.into(),
        thinking_signature: None,
        redacted: None,
        extra: Map::new(),
    })
}

/// `fauxToolCall(name, arguments, options?)`
pub fn faux_tool_call(
    name: impl Into<String>,
    arguments: Value,
    id: Option<String>,
) -> AssistantContent {
    AssistantContent::ToolCall(ToolCall {
        id: id.unwrap_or_else(|| random_id("tool")),
        name: name.into(),
        arguments: arguments.as_object().cloned().unwrap_or_default(),
        thought_signature: None,
        namespace: None,
        extra: Map::new(),
    })
}

/// `fauxAssistantMessage(content, options?)`
pub fn faux_assistant_message(
    content: Vec<AssistantContent>,
    stop_reason: StopReason,
) -> AssistantMessage {
    AssistantMessage {
        content,
        api: DEFAULT_API.to_string(),
        provider: DEFAULT_PROVIDER.to_string(),
        model: DEFAULT_MODEL_ID.to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    }
}

/// `FauxProviderState`
#[derive(Debug, Default)]
pub struct FauxProviderState {
    pub call_count: AtomicU64,
    pub deferred_fetch_count: AtomicU64,
    pub cancelled_deferred: Mutex<Vec<DeferredHandle>>,
}

/// `FauxResponseFactory` — evaluated per call with the request context.
pub type FauxResponseFactory = Arc<dyn Fn(&Context, &Model) -> AssistantMessage + Send + Sync>;

/// `FauxResponseStep` — a fixed message or a factory evaluated per call.
// Boxing the message variant would obscure the 1:1 shape of the TS union.
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum FauxResponseStep {
    Message(AssistantMessage),
    Factory(FauxResponseFactory),
}

impl From<AssistantMessage> for FauxResponseStep {
    fn from(message: AssistantMessage) -> Self {
        FauxResponseStep::Message(message)
    }
}

/// `RegisterFauxProviderOptions`
#[derive(Clone, Default)]
pub struct FauxProviderOptions {
    pub api: Option<String>,
    pub provider: Option<String>,
    pub models: Option<Vec<FauxModelDefinition>>,
    /// Fetches that return the handle again before the response becomes ready.
    pub deferred_pending_fetches: Option<u32>,
    pub deferred_poll_after_ms: Option<u64>,
    pub tokens_per_second: Option<f64>,
    pub token_size_min: Option<usize>,
    pub token_size_max: Option<usize>,
}

struct DeferredEntry {
    handle: DeferredHandle,
    step: FauxResponseStep,
    context: Context,
    model: Model,
    pending_fetches: u32,
    cancelled: bool,
    final_message: Option<AssistantMessage>,
}

/// The scriptable core; `FauxProviderHandle` wraps it as a `Provider`.
pub struct FauxCore {
    pub api: String,
    pub provider: String,
    pub models: Vec<Model>,
    pub state: Arc<FauxProviderState>,
    pending_responses: Mutex<Vec<FauxResponseStep>>,
    prompt_cache: Mutex<BTreeMap<String, String>>,
    deferred_responses: Mutex<BTreeMap<String, DeferredEntry>>,
    min_token_size: usize,
    max_token_size: usize,
    tokens_per_second: Option<f64>,
    deferred_pending_fetches: u32,
    deferred_poll_after_ms: Option<u64>,
}

/// `createFauxCore(options)`
pub fn create_faux_core(options: FauxProviderOptions) -> Arc<FauxCore> {
    let api = options
        .api
        .clone()
        .unwrap_or_else(|| random_id(DEFAULT_API));
    let provider = options
        .provider
        .clone()
        .unwrap_or_else(|| DEFAULT_PROVIDER.to_string());
    let min_token_size = options
        .token_size_min
        .unwrap_or(DEFAULT_MIN_TOKEN_SIZE)
        .min(options.token_size_max.unwrap_or(DEFAULT_MAX_TOKEN_SIZE))
        .max(1);
    let max_token_size = options
        .token_size_max
        .unwrap_or(DEFAULT_MAX_TOKEN_SIZE)
        .max(min_token_size);

    let definitions = match options.models.clone() {
        Some(models) if !models.is_empty() => models,
        _ => vec![FauxModelDefinition {
            name: Some(DEFAULT_MODEL_NAME.to_string()),
            input: Some(vec![Modality::Text, Modality::Image]),
            context_window: Some(128_000),
            max_tokens: Some(16_384),
            ..FauxModelDefinition::new(DEFAULT_MODEL_ID)
        }],
    };
    let models = definitions
        .into_iter()
        .map(|definition| Model {
            name: definition
                .name
                .clone()
                .unwrap_or_else(|| definition.id.clone()),
            id: definition.id,
            api: api.clone(),
            provider: provider.clone(),
            base_url: DEFAULT_BASE_URL.to_string(),
            reasoning: definition.reasoning.unwrap_or(false),
            thinking_level_map: None,
            input: definition
                .input
                .unwrap_or_else(|| vec![Modality::Text, Modality::Image]),
            cost: definition.cost.unwrap_or_default(),
            context_window: definition.context_window.unwrap_or(128_000),
            max_tokens: definition.max_tokens.unwrap_or(16_384),
            sampling_params: None,
            headers: None,
            compat: None,
        })
        .collect();

    Arc::new(FauxCore {
        api,
        provider,
        models,
        state: Arc::new(FauxProviderState::default()),
        pending_responses: Mutex::new(Vec::new()),
        prompt_cache: Mutex::new(BTreeMap::new()),
        deferred_responses: Mutex::new(BTreeMap::new()),
        min_token_size,
        max_token_size,
        tokens_per_second: options.tokens_per_second,
        deferred_pending_fetches: options.deferred_pending_fetches.unwrap_or(0),
        deferred_poll_after_ms: options.deferred_poll_after_ms,
    })
}

impl FauxCore {
    /// `setResponses(responses)`
    pub fn set_responses(&self, responses: Vec<FauxResponseStep>) {
        *self.pending_responses.lock().expect("poisoned") = responses;
    }

    /// `appendResponses(responses)`
    pub fn append_responses(&self, responses: Vec<FauxResponseStep>) {
        self.pending_responses
            .lock()
            .expect("poisoned")
            .extend(responses);
    }

    /// `getPendingResponseCount()`
    pub fn pending_response_count(&self) -> usize {
        self.pending_responses.lock().expect("poisoned").len()
    }

    /// `getModel()` / `getModel(id)`
    pub fn get_model(&self, model_id: Option<&str>) -> Option<Model> {
        match model_id {
            None => self.models.first().cloned(),
            Some(model_id) => self
                .models
                .iter()
                .find(|model| model.id == model_id)
                .cloned(),
        }
    }

    fn resolve_response(
        &self,
        step: &FauxResponseStep,
        context: &Context,
        model: &Model,
    ) -> AssistantMessage {
        let resolved = match step {
            FauxResponseStep::Message(message) => message.clone(),
            FauxResponseStep::Factory(factory) => factory(context, model),
        };
        let cloned = AssistantMessage {
            api: self.api.clone(),
            provider: self.provider.clone(),
            model: model.id.clone(),
            timestamp: if resolved.timestamp == 0 {
                now_ms()
            } else {
                resolved.timestamp
            },
            ..resolved
        };
        self.with_usage_estimate(cloned, context, None, None)
    }

    /// `withUsageEstimate(message, context, options, promptCache)`
    fn with_usage_estimate(
        &self,
        message: AssistantMessage,
        context: &Context,
        session_id: Option<&str>,
        cache_retention: Option<CacheRetention>,
    ) -> AssistantMessage {
        let prompt_text = serialize_context(context);
        let prompt_tokens = estimate_tokens(&prompt_text);
        let output_tokens = estimate_tokens(&assistant_content_to_text(&message.content));
        let mut input = prompt_tokens;
        let mut cache_read = 0;
        let mut cache_write = 0;

        if let Some(session_id) =
            session_id.filter(|_| cache_retention != Some(CacheRetention::None))
        {
            let mut cache = self.prompt_cache.lock().expect("poisoned");
            if let Some(previous_prompt) = cache.get(session_id) {
                let cached_chars = common_prefix_length(previous_prompt, &prompt_text);
                cache_read = estimate_tokens(&previous_prompt[..cached_chars]);
                cache_write = estimate_tokens(&prompt_text[cached_chars..]);
                input = prompt_tokens.saturating_sub(cache_read);
            } else {
                cache_write = prompt_tokens;
            }
            cache.insert(session_id.to_string(), prompt_text);
        }

        AssistantMessage {
            usage: Usage {
                input,
                output: output_tokens,
                cache_read,
                cache_write,
                cache_write1h: None,
                reasoning: None,
                total_tokens: Some(input + output_tokens + cache_read + cache_write),
                cost: UsageCost::default(),
            },
            ..message
        }
    }

    /// `stream(model, context, options)`
    pub fn stream(
        self: &Arc<Self>,
        request_model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        let outer = create_assistant_message_event_stream();
        let step = {
            let mut pending = self.pending_responses.lock().expect("poisoned");
            if pending.is_empty() {
                None
            } else {
                Some(pending.remove(0))
            }
        };
        self.state.call_count.fetch_add(1, Ordering::SeqCst);

        let core = Arc::clone(self);
        let stream = outer.clone();
        let request_model = request_model.clone();
        let context = context.clone();
        tokio::spawn(async move {
            let signal = options
                .as_ref()
                .and_then(|options| options.base.base.signal.clone());
            let session_id = options
                .as_ref()
                .and_then(|options| options.base.session_id.clone());
            let cache_retention = options
                .as_ref()
                .and_then(|options| options.base.cache_retention);

            let Some(step) = step else {
                let message = core.with_usage_estimate(
                    create_error_message(
                        "No more faux responses queued",
                        &core.api,
                        &core.provider,
                        &request_model.id,
                    ),
                    &context,
                    session_id.as_deref(),
                    cache_retention,
                );
                stream.push(AssistantMessageEvent::Error {
                    reason: ErrorReason::Error,
                    error: message.clone(),
                });
                stream.end(Some(message));
                return;
            };

            if options
                .as_ref()
                .and_then(|options| options.deferred)
                .is_some()
            {
                let handle = DeferredHandle {
                    provider: request_model.provider.clone(),
                    model_id: request_model.id.clone(),
                    api: request_model.api.clone(),
                    id: random_id("deferred"),
                    expires_at: None,
                    poll_after_ms: core.deferred_poll_after_ms,
                    data: None,
                };
                core.deferred_responses.lock().expect("poisoned").insert(
                    handle.id.clone(),
                    DeferredEntry {
                        handle: handle.clone(),
                        step,
                        context: context.clone(),
                        model: request_model.clone(),
                        pending_fetches: core.deferred_pending_fetches,
                        cancelled: false,
                        final_message: None,
                    },
                );
                core.stream_with_deltas(
                    &stream,
                    create_deferred_message(&request_model, handle),
                    signal,
                )
                .await;
                return;
            }

            let mut message = core.resolve_response(&step, &context, &request_model);
            message =
                core.with_usage_estimate(message, &context, session_id.as_deref(), cache_retention);
            core.stream_with_deltas(&stream, message, signal).await;
        });

        outer
    }

    /// `fetchDeferred(model, handle, options)`
    pub fn fetch_deferred(
        self: &Arc<Self>,
        request_model: &Model,
        handle: &DeferredHandle,
        options: Option<DeferredFetchOptions>,
    ) -> AssistantMessageEventStream {
        let outer = create_assistant_message_event_stream();
        self.state
            .deferred_fetch_count
            .fetch_add(1, Ordering::SeqCst);

        let core = Arc::clone(self);
        let stream = outer.clone();
        let request_model = request_model.clone();
        let handle = handle.clone();
        tokio::spawn(async move {
            let signal = options
                .as_ref()
                .and_then(|options| options.base.signal.clone());
            let outcome = {
                let mut entries = core.deferred_responses.lock().expect("poisoned");
                match entries.get_mut(&handle.id) {
                    None => Err(format!("Unknown faux deferred response: {}", handle.id)),
                    Some(entry)
                        if entry.handle.provider != handle.provider
                            || entry.handle.model_id != handle.model_id
                            || entry.handle.api != handle.api =>
                    {
                        Err(format!("Unknown faux deferred response: {}", handle.id))
                    }
                    Some(entry) if entry.cancelled => Err(format!(
                        "Faux deferred response was cancelled: {}",
                        handle.id
                    )),
                    Some(entry) => {
                        if entry.pending_fetches > 0 {
                            entry.pending_fetches -= 1;
                            Ok(None)
                        } else {
                            Ok(Some((
                                entry.step.clone(),
                                entry.context.clone(),
                                entry.model.clone(),
                            )))
                        }
                    }
                }
            };

            match outcome {
                Err(message) => {
                    let error =
                        create_error_message(message, &core.api, &core.provider, &request_model.id);
                    stream.push(AssistantMessageEvent::Error {
                        reason: ErrorReason::Error,
                        error: error.clone(),
                    });
                    stream.end(Some(error));
                }
                Ok(None) => {
                    core.stream_with_deltas(
                        &stream,
                        create_deferred_message(&request_model, handle),
                        signal,
                    )
                    .await;
                }
                Ok(Some((step, context, model))) => {
                    let cached = core
                        .deferred_responses
                        .lock()
                        .expect("poisoned")
                        .get(&handle.id)
                        .and_then(|entry| entry.final_message.clone());
                    let final_message = match cached {
                        Some(message) => message,
                        None => {
                            // The submission options are dropped except for the context,
                            // as in TS (`deferred`, `signal` and `onResponse` are removed).
                            let resolved = core.resolve_response(&step, &context, &model);
                            if let Some(entry) = core
                                .deferred_responses
                                .lock()
                                .expect("poisoned")
                                .get_mut(&handle.id)
                            {
                                entry.final_message = Some(resolved.clone());
                            }
                            resolved
                        }
                    };
                    core.stream_with_deltas(&stream, final_message, signal)
                        .await;
                }
            }
        });

        outer
    }

    /// `cancelDeferred(model, handle, options)`
    pub fn cancel_deferred(&self, handle: &DeferredHandle) {
        self.state
            .cancelled_deferred
            .lock()
            .expect("poisoned")
            .push(handle.clone());
        if let Some(entry) = self
            .deferred_responses
            .lock()
            .expect("poisoned")
            .get_mut(&handle.id)
        {
            entry.cancelled = true;
        }
    }

    /// `streamWithDeltas(...)` — emits the message as start/delta/end events.
    async fn stream_with_deltas(
        &self,
        stream: &AssistantMessageEventStream,
        message: AssistantMessage,
        signal: Option<CancellationToken>,
    ) {
        let aborted = |partial: &AssistantMessage| AssistantMessage {
            stop_reason: StopReason::Aborted,
            error_message: Some("Request was aborted".to_string()),
            timestamp: now_ms(),
            ..partial.clone()
        };
        let is_aborted = || signal.as_ref().is_some_and(CancellationToken::is_cancelled);

        let mut partial = AssistantMessage {
            content: vec![],
            stop_reason: StopReason::Pending,
            ..message.clone()
        };
        if is_aborted() {
            let message = aborted(&partial);
            stream.push(AssistantMessageEvent::Error {
                reason: ErrorReason::Aborted,
                error: message.clone(),
            });
            stream.end(Some(message));
            return;
        }

        stream.push(AssistantMessageEvent::Start {
            partial: partial.clone(),
        });

        for (index, block) in message.content.iter().enumerate() {
            if is_aborted() {
                let message = aborted(&partial);
                stream.push(AssistantMessageEvent::Error {
                    reason: ErrorReason::Aborted,
                    error: message.clone(),
                });
                stream.end(Some(message));
                return;
            }

            match block {
                AssistantContent::Thinking(thinking) => {
                    partial
                        .content
                        .push(AssistantContent::Thinking(ThinkingContent::default()));
                    stream.push(AssistantMessageEvent::ThinkingStart {
                        content_index: index,
                        partial: partial.clone(),
                    });
                    for chunk in split_by_token_size(
                        &thinking.thinking,
                        self.min_token_size,
                        self.max_token_size,
                    ) {
                        self.schedule_chunk(&chunk).await;
                        if is_aborted() {
                            let message = aborted(&partial);
                            stream.push(AssistantMessageEvent::Error {
                                reason: ErrorReason::Aborted,
                                error: message.clone(),
                            });
                            stream.end(Some(message));
                            return;
                        }
                        if let Some(AssistantContent::Thinking(block)) =
                            partial.content.get_mut(index)
                        {
                            block.thinking.push_str(&chunk);
                        }
                        stream.push(AssistantMessageEvent::ThinkingDelta {
                            content_index: index,
                            delta: chunk,
                            partial: partial.clone(),
                        });
                    }
                    stream.push(AssistantMessageEvent::ThinkingEnd {
                        content_index: index,
                        content: thinking.thinking.clone(),
                        partial: partial.clone(),
                    });
                }
                AssistantContent::Text(text) => {
                    partial
                        .content
                        .push(AssistantContent::Text(TextContent::default()));
                    stream.push(AssistantMessageEvent::TextStart {
                        content_index: index,
                        partial: partial.clone(),
                    });
                    for chunk in
                        split_by_token_size(&text.text, self.min_token_size, self.max_token_size)
                    {
                        self.schedule_chunk(&chunk).await;
                        if is_aborted() {
                            let message = aborted(&partial);
                            stream.push(AssistantMessageEvent::Error {
                                reason: ErrorReason::Aborted,
                                error: message.clone(),
                            });
                            stream.end(Some(message));
                            return;
                        }
                        if let Some(AssistantContent::Text(block)) = partial.content.get_mut(index)
                        {
                            block.text.push_str(&chunk);
                        }
                        stream.push(AssistantMessageEvent::TextDelta {
                            content_index: index,
                            delta: chunk,
                            partial: partial.clone(),
                        });
                    }
                    stream.push(AssistantMessageEvent::TextEnd {
                        content_index: index,
                        content: text.text.clone(),
                        partial: partial.clone(),
                    });
                }
                AssistantContent::ToolCall(tool_call) => {
                    partial.content.push(AssistantContent::ToolCall(ToolCall {
                        id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        ..Default::default()
                    }));
                    stream.push(AssistantMessageEvent::ToolcallStart {
                        content_index: index,
                        partial: partial.clone(),
                    });
                    let arguments = serde_json::to_string(&tool_call.arguments)
                        .unwrap_or_else(|_| "{}".to_string());
                    for chunk in
                        split_by_token_size(&arguments, self.min_token_size, self.max_token_size)
                    {
                        self.schedule_chunk(&chunk).await;
                        if is_aborted() {
                            let message = aborted(&partial);
                            stream.push(AssistantMessageEvent::Error {
                                reason: ErrorReason::Aborted,
                                error: message.clone(),
                            });
                            stream.end(Some(message));
                            return;
                        }
                        stream.push(AssistantMessageEvent::ToolcallDelta {
                            content_index: index,
                            delta: chunk,
                            partial: partial.clone(),
                        });
                    }
                    if let Some(AssistantContent::ToolCall(block)) = partial.content.get_mut(index)
                    {
                        block.arguments = tool_call.arguments.clone();
                    }
                    stream.push(AssistantMessageEvent::ToolcallEnd {
                        content_index: index,
                        tool_call: tool_call.clone(),
                        partial: partial.clone(),
                    });
                }
            }
        }

        match message.stop_reason {
            StopReason::Pending => {
                let error = AssistantMessage {
                    stop_reason: StopReason::Error,
                    error_message: Some("Faux response ended without a stop reason".to_string()),
                    ..message
                };
                stream.push(AssistantMessageEvent::Error {
                    reason: ErrorReason::Error,
                    error: error.clone(),
                });
                stream.end(Some(error));
            }
            StopReason::Error | StopReason::Aborted => {
                let reason = if message.stop_reason == StopReason::Aborted {
                    ErrorReason::Aborted
                } else {
                    ErrorReason::Error
                };
                stream.push(AssistantMessageEvent::Error {
                    reason,
                    error: message.clone(),
                });
                stream.end(Some(message));
            }
            StopReason::Stop => self.finish(stream, DoneReason::Stop, message),
            StopReason::Length => self.finish(stream, DoneReason::Length, message),
            StopReason::ToolUse => self.finish(stream, DoneReason::ToolUse, message),
            StopReason::Deferred => self.finish(stream, DoneReason::Deferred, message),
        }
    }

    fn finish(
        &self,
        stream: &AssistantMessageEventStream,
        reason: DoneReason,
        message: AssistantMessage,
    ) {
        stream.push(AssistantMessageEvent::Done {
            reason,
            message: message.clone(),
        });
        stream.end(Some(message));
    }

    /// `scheduleChunk(chunk, tokensPerSecond)`
    async fn schedule_chunk(&self, chunk: &str) {
        match self.tokens_per_second.filter(|rate| *rate > 0.0) {
            None => tokio::task::yield_now().await,
            Some(tokens_per_second) => {
                let delay_ms = (estimate_tokens(chunk) as f64 / tokens_per_second) * 1000.0;
                tokio::time::sleep(Duration::from_secs_f64(delay_ms / 1000.0)).await;
            }
        }
    }
}

/// `FauxProviderHandle`
pub struct FauxProviderHandle {
    pub provider: Arc<BuiltProvider>,
    pub core: Arc<FauxCore>,
}

impl FauxProviderHandle {
    pub fn api(&self) -> &str {
        &self.core.api
    }

    pub fn models(&self) -> &[Model] {
        &self.core.models
    }

    pub fn get_model(&self, model_id: Option<&str>) -> Option<Model> {
        self.core.get_model(model_id)
    }

    pub fn state(&self) -> &Arc<FauxProviderState> {
        &self.core.state
    }

    pub fn set_responses(&self, responses: Vec<FauxResponseStep>) {
        self.core.set_responses(responses);
    }

    pub fn append_responses(&self, responses: Vec<FauxResponseStep>) {
        self.core.append_responses(responses);
    }

    pub fn pending_response_count(&self) -> usize {
        self.core.pending_response_count()
    }
}

/// The `ProviderStreams` implementation backed by a [`FauxCore`].
struct FauxStreams {
    core: Arc<FauxCore>,
}

impl ProviderStreams for FauxStreams {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        self.core.stream(
            model,
            context,
            options.map(|options| SimpleStreamOptions {
                base: options,
                ..Default::default()
            }),
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        self.core.stream(model, context, options)
    }

    fn fetch_deferred(
        &self,
        model: &Model,
        handle: &DeferredHandle,
        options: Option<DeferredFetchOptions>,
    ) -> Option<AssistantMessageEventStream> {
        Some(self.core.fetch_deferred(model, handle, options))
    }

    fn cancel_deferred(
        &self,
        _model: &Model,
        handle: &DeferredHandle,
        _options: Option<DeferredCancelOptions>,
    ) -> Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>> {
        self.core.cancel_deferred(handle);
        Some(Box::pin(async {}))
    }

    fn supports_fetch_deferred(&self) -> bool {
        true
    }

    fn supports_cancel_deferred(&self) -> bool {
        true
    }
}

/// Keyless auth so the faux provider always counts as configured.
struct FauxAuth;

impl crate::auth::types::ApiKeyAuth for FauxAuth {
    fn name(&self) -> &str {
        "Faux"
    }

    fn resolve<'a>(
        &'a self,
        _input: crate::auth::types::ApiKeyAuthInput<'a>,
    ) -> crate::auth::types::BoxFuture<
        'a,
        Result<Option<crate::auth::types::AuthResult>, crate::auth::types::AuthError>,
    > {
        Box::pin(async { Ok(Some(crate::auth::types::AuthResult::default())) })
    }
}

/// `fauxProvider(options?)`
pub fn faux_provider(options: FauxProviderOptions) -> FauxProviderHandle {
    let core = create_faux_core(options);
    let provider = create_provider(CreateProviderOptions {
        id: core.provider.clone(),
        name: None,
        base_url: None,
        headers: None,
        auth: crate::auth::types::ProviderAuth {
            api_key: Some(Arc::new(FauxAuth)),
            oauth: None,
        },
        models: core.models.clone(),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(FauxStreams {
            core: Arc::clone(&core),
        })),
    });
    FauxProviderHandle { provider, core }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn estimate_tokens(text: &str) -> u64 {
    text.chars().count().div_ceil(4) as u64
}

fn random_id(prefix: &str) -> String {
    use rand::RngExt;
    let suffix: u64 = rand::rng().random();
    format!("{prefix}:{}:{}", now_ms(), radix36(suffix))
}

fn radix36(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut buffer = Vec::new();
    while value > 0 {
        buffer.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    buffer.reverse();
    String::from_utf8(buffer).expect("ASCII digits")
}

fn content_to_text(content: &[TextOrImageContent]) -> String {
    content
        .iter()
        .map(|block| match block {
            TextOrImageContent::Text(text) => text.text.clone(),
            TextOrImageContent::Image(image) => {
                format!("[image:{}:{}]", image.mime_type, image.data.chars().count())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assistant_content_to_text(content: &[AssistantContent]) -> String {
    content
        .iter()
        .map(|block| match block {
            AssistantContent::Text(text) => text.text.clone(),
            AssistantContent::Thinking(thinking) => thinking.thinking.clone(),
            AssistantContent::ToolCall(tool_call) => format!(
                "{}:{}",
                tool_call.name,
                serde_json::to_string(&tool_call.arguments).unwrap_or_else(|_| "{}".to_string())
            ),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn message_to_text(message: &Message) -> String {
    match message {
        Message::User(message) => match &message.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => content_to_text(blocks),
        },
        Message::Assistant(message) => assistant_content_to_text(&message.content),
        Message::ToolResult(message) => {
            let mut parts = vec![message.tool_name.clone()];
            parts.extend(
                message
                    .content
                    .iter()
                    .map(|block| content_to_text(std::slice::from_ref(block))),
            );
            parts.join("\n")
        }
    }
}

fn message_role(message: &Message) -> &'static str {
    match message {
        Message::User(_) => "user",
        Message::Assistant(_) => "assistant",
        Message::ToolResult(_) => "toolResult",
    }
}

fn serialize_context(context: &Context) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(system_prompt) = context
        .system_prompt
        .as_ref()
        .filter(|prompt| !prompt.is_empty())
    {
        parts.push(format!("system:{system_prompt}"));
    }
    for message in &context.messages {
        parts.push(format!(
            "{}:{}",
            message_role(message),
            message_to_text(message)
        ));
    }
    if let Some(tools) = context.tools.as_ref().filter(|tools| !tools.is_empty()) {
        parts.push(format!(
            "tools:{}",
            serde_json::to_string(tools).unwrap_or_else(|_| "[]".to_string())
        ));
    }
    parts.join("\n\n")
}

fn common_prefix_length(a: &str, b: &str) -> usize {
    let mut index = 0;
    for (left, right) in a.bytes().zip(b.bytes()) {
        if left != right {
            break;
        }
        index += 1;
    }
    // Never split a multi-byte character.
    while index > 0 && !a.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn split_by_token_size(text: &str, min_token_size: usize, max_token_size: usize) -> Vec<String> {
    use rand::RngExt;
    let characters: Vec<char> = text.chars().collect();
    let mut chunks = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let token_size =
            min_token_size + rand::rng().random_range(0..=(max_token_size - min_token_size));
        let char_size = (token_size * 4).max(1);
        let end = (index + char_size).min(characters.len());
        chunks.push(characters[index..end].iter().collect::<String>());
        index = end;
    }
    if chunks.is_empty() {
        vec![String::new()]
    } else {
        chunks
    }
}

fn create_deferred_message(model: &Model, handle: DeferredHandle) -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Deferred,
        deferred: Some(handle),
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    }
}

fn create_error_message(
    error: impl Into<String>,
    api: &str,
    provider: &str,
    model_id: &str,
) -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        api: api.to_string(),
        provider: provider.to_string(),
        model: model_id.to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        deferred: None,
        error_message: Some(error.into()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    }
}

fn now_ms() -> i64 {
    crate::auth::resolve::now_ms()
}
