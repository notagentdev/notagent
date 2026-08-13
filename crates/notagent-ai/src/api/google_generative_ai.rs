//! Google Generative AI (Gemini) adapter.
//!
//! 1:1 port of `packages/ai/src/api/google-generative-ai.ts`. Master-plan substitution
//! class 3: the `@google/genai` SDK is replaced by a direct REST call to
//! `…/models/{id}:streamGenerateContent?alt=sse`, which is what the SDK issues.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value, json};

use crate::api::google_shared::{
    convert_messages, convert_tools, is_thinking_part, map_stop_reason,
    resolve_google_function_calling_mode, retain_thought_signature,
    supports_google_strict_tool_sampling,
};
use crate::api::simple_options::build_base_options;
use crate::api::sse::SseDecoder;
use crate::models::{calculate_cost, clamp_thinking_level};
use crate::types::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, Context, DoneReason, ErrorReason,
    Model, ModelThinkingLevel, ProviderHeaders, ProviderRequestOptions, SimpleStreamOptions,
    StopReason, TextContent, ThinkingBudgets, ThinkingContent, ThinkingLevel, ToolCall, Usage,
};
use crate::utils::error_body::{RawProviderError, format_provider_error, normalize_provider_error};
use crate::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use crate::utils::fetch::{FetchBody, FetchFunction, FetchRequest, ReqwestFetch};
use crate::utils::provider_retry::{
    ProviderErrorInfo, ProviderRetryError, ProviderRetryOptions, retry_provider_request,
};
use crate::utils::sanitize_unicode::sanitize_surrogates;

/// The SDK's defaults for the Gemini endpoint.
pub const GOOGLE_AI_DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
pub const GOOGLE_AI_API_DEFAULT_VERSION: &str = "v1beta";

/// `let toolCallCounter = 0` — module-scoped in TS, so it keeps counting across streams.
static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `GoogleOptions extends StreamOptions`
#[derive(Debug, Clone, Default)]
pub struct GoogleOptions {
    /// `"auto" | "none" | "any"`
    pub tool_choice: Option<String>,
    pub thinking: Option<GoogleThinkingOptions>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
}

/// `thinking?: { enabled, budgetTokens?, level? }`
#[derive(Debug, Clone, Default)]
pub struct GoogleThinkingOptions {
    pub enabled: bool,
    /// `-1` asks for a dynamic budget, `0` disables thinking.
    pub budget_tokens: Option<i64>,
    pub level: Option<String>,
}

/// Error of this adapter.
#[derive(Debug, Clone, Default)]
pub struct GoogleError {
    pub message: String,
    pub status: Option<u16>,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl GoogleError {
    /// Named `from_message` rather than `message`, which is the accessor's name.
    pub fn from_message(message: impl Into<String>) -> Self {
        GoogleError {
            message: message.into(),
            ..Default::default()
        }
    }
}

impl std::fmt::Display for GoogleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl ProviderErrorInfo for GoogleError {
    fn status(&self) -> Option<u16> {
        self.status
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    fn message(&self) -> String {
        self.message.clone()
    }
}

// ---------------------------------------------------------------------------
// Model-family predicates
// ---------------------------------------------------------------------------

fn matches(model_id: &str, pattern: &str) -> bool {
    regex::Regex::new(pattern)
        .expect("valid pattern")
        .is_match(&model_id.to_lowercase())
}

/// `isGemma4Model(model)`
pub fn is_gemma4_model(model_id: &str) -> bool {
    matches(model_id, r"gemma-?4")
}

/// `isGemini3ProModel(model)`
pub fn is_gemini3_pro_model(model_id: &str) -> bool {
    matches(model_id, r"gemini-3(?:\.\d+)?-pro")
}

/// `isGemini3FlashModel(model)`
pub fn is_gemini3_flash_model(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    matches(model_id, r"gemini-3(?:\.\d+)?-flash")
        || id == "gemini-flash-latest"
        || id == "gemini-flash-lite-latest"
}

/// `getDisabledThinkingConfig(model)`
///
/// Google documents that Gemini 3.1 Pro cannot turn thinking off and that Gemini 3 Flash
/// and Flash-Lite do not support a full off either, so those get the lowest level without
/// `includeThoughts`, which keeps the hidden thinking invisible.
fn disabled_thinking_config(model_id: &str) -> Value {
    if is_gemini3_pro_model(model_id) {
        return json!({ "thinkingLevel": "LOW" });
    }
    if is_gemini3_flash_model(model_id) || is_gemma4_model(model_id) {
        return json!({ "thinkingLevel": "MINIMAL" });
    }
    // Gemini 2.x disables via a zero budget.
    json!({ "thinkingBudget": 0 })
}

/// `getThinkingLevel(effort, model)`
pub fn thinking_level(effort: ThinkingLevel, model_id: &str) -> &'static str {
    if is_gemini3_pro_model(model_id) {
        return match effort {
            ThinkingLevel::Minimal | ThinkingLevel::Low => "LOW",
            _ => "HIGH",
        };
    }
    if is_gemma4_model(model_id) {
        return match effort {
            ThinkingLevel::Minimal | ThinkingLevel::Low => "MINIMAL",
            _ => "HIGH",
        };
    }
    match effort {
        ThinkingLevel::Minimal => "MINIMAL",
        ThinkingLevel::Low => "LOW",
        ThinkingLevel::Medium => "MEDIUM",
        // `clampThinkingLevel` never yields xhigh or max here.
        _ => "HIGH",
    }
}

/// `getGoogleBudget(model, effort, customBudgets)`
pub fn google_budget(
    model_id: &str,
    effort: ThinkingLevel,
    custom_budgets: Option<ThinkingBudgets>,
) -> i64 {
    let custom = custom_budgets.and_then(|budgets| match effort {
        ThinkingLevel::Minimal => budgets.minimal,
        ThinkingLevel::Low => budgets.low,
        ThinkingLevel::Medium => budgets.medium,
        _ => budgets.high,
    });
    if let Some(custom) = custom {
        return custom as i64;
    }

    let pick = |minimal: i64, low: i64, medium: i64, high: i64| match effort {
        ThinkingLevel::Minimal => minimal,
        ThinkingLevel::Low => low,
        ThinkingLevel::Medium => medium,
        _ => high,
    };
    if model_id.contains("2.5-pro") {
        return pick(128, 2048, 8192, 32768);
    }
    if model_id.contains("2.5-flash-lite") {
        return pick(512, 2048, 8192, 24576);
    }
    if model_id.contains("2.5-flash") {
        return pick(128, 2048, 8192, 24576);
    }
    -1
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

/// The request body the SDK puts on the wire for `generateContentStream(params)`.
///
/// TS hands a `GenerateContentParameters` to the SDK, which flattens `config` into the
/// body: `systemInstruction`, `tools` and `toolConfig` go to the top level, everything
/// else into `generationConfig`.
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: &GoogleOptions,
    timestamp: i64,
) -> Result<Value, GoogleError> {
    let contents = convert_messages(model, context, timestamp);

    let mut generation_config = Map::new();
    if let Some(temperature) = options.temperature {
        generation_config.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_tokens) = options.max_tokens {
        generation_config.insert("maxOutputTokens".to_string(), json!(max_tokens));
    }

    let supports_strict_mode = supports_google_strict_tool_sampling(&model.id);
    let tools = context.tools.as_deref().unwrap_or_default();
    let function_calling_mode = if tools.is_empty() {
        None
    } else {
        resolve_google_function_calling_mode(
            tools,
            options.tool_choice.as_deref(),
            supports_strict_mode,
        )
        .map_err(|error| GoogleError::from_message(error.to_string()))?
    };

    let mut body = Map::new();
    body.insert("contents".to_string(), Value::Array(contents));
    if let Some(system_prompt) = context
        .system_prompt
        .as_deref()
        .filter(|prompt| !prompt.is_empty())
    {
        // The SDK wraps a string instruction into a user-role content.
        body.insert(
            "systemInstruction".to_string(),
            json!({ "parts": [{ "text": sanitize_surrogates(system_prompt) }], "role": "user" }),
        );
    }
    if !tools.is_empty()
        && let Some(converted) = convert_tools(tools, false, supports_strict_mode)
            .map_err(|error| GoogleError::from_message(error.to_string()))?
    {
        body.insert("tools".to_string(), converted);
    }
    if let Some(mode) = function_calling_mode {
        body.insert(
            "toolConfig".to_string(),
            json!({ "functionCallingConfig": { "mode": mode.as_str() } }),
        );
    }

    if model.reasoning
        && let Some(thinking) = &options.thinking
    {
        if thinking.enabled {
            let mut thinking_config = Map::new();
            thinking_config.insert("includeThoughts".to_string(), json!(true));
            if let Some(level) = &thinking.level {
                thinking_config.insert("thinkingLevel".to_string(), json!(level));
            } else if let Some(budget_tokens) = thinking.budget_tokens {
                thinking_config.insert("thinkingBudget".to_string(), json!(budget_tokens));
            }
            generation_config.insert("thinkingConfig".to_string(), Value::Object(thinking_config));
        } else {
            generation_config.insert(
                "thinkingConfig".to_string(),
                disabled_thinking_config(&model.id),
            );
        }
    }

    // `generationConfig` is always present, even when empty.
    body.insert(
        "generationConfig".to_string(),
        Value::Object(generation_config),
    );
    Ok(Value::Object(body))
}

/// The URL the SDK builds: a configured `model.baseUrl` already carries the version path,
/// so no version segment is appended in that case.
pub fn build_request_url(model: &Model) -> String {
    let base_url = model.base_url.trim_end_matches('/').to_string();
    let prefix = if base_url.is_empty() {
        format!("{GOOGLE_AI_DEFAULT_BASE_URL}/{GOOGLE_AI_API_DEFAULT_VERSION}")
    } else {
        base_url
    };
    format!("{prefix}/models/{}:streamGenerateContent?alt=sse", model.id)
}

/// The `httpOptions.headers` of `createClient`, merged model-first.
pub fn build_client_headers(
    model: &Model,
    options_headers: Option<&ProviderHeaders>,
) -> BTreeMap<String, Option<String>> {
    let mut headers: BTreeMap<String, Option<String>> = model
        .headers
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, Some(value)))
        .collect();
    if let Some(options_headers) = options_headers {
        for (key, value) in options_headers {
            headers.insert(key.clone(), value.clone());
        }
    }
    headers
}

// ---------------------------------------------------------------------------
// Streaming state
// ---------------------------------------------------------------------------

/// The `for await (const chunk of googleStream)` body as a state machine.
pub struct GoogleStreamState {
    pub output: AssistantMessage,
    model: Model,
    /// Index of the open text or thinking block, if any.
    current_block: Option<usize>,
}

impl GoogleStreamState {
    pub fn new(model: &Model, timestamp: i64) -> Self {
        Self::new_with_api(model, "google-generative-ai", timestamp)
    }

    /// Same, with the api literal the caller pins on the message.
    pub fn new_with_api(model: &Model, api: &str, timestamp: i64) -> Self {
        GoogleStreamState {
            output: AssistantMessage {
                content: Vec::new(),
                // TS pins the api to the literal, independent of `model.api`.
                api: api.to_string(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage {
                    total_tokens: Some(0),
                    ..Usage::default()
                },
                stop_reason: StopReason::Pending,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp,
            },
            model: model.clone(),
            current_block: None,
        }
    }

    fn block_index(&self) -> usize {
        self.output.content.len() - 1
    }

    /// Emits the `*_end` event of the open block and clears it.
    fn close_current_block(&mut self, emitted: &mut Vec<AssistantMessageEvent>) {
        let Some(index) = self.current_block.take() else {
            return;
        };
        match &self.output.content[index] {
            AssistantContent::Text(block) => {
                let content = block.text.clone();
                emitted.push(AssistantMessageEvent::TextEnd {
                    content_index: index,
                    content,
                    partial: self.output.clone(),
                });
            }
            AssistantContent::Thinking(block) => {
                let content = block.thinking.clone();
                emitted.push(AssistantMessageEvent::ThinkingEnd {
                    content_index: index,
                    content,
                    partial: self.output.clone(),
                });
            }
            AssistantContent::ToolCall(_) => {}
        }
    }

    /// One decoded `GenerateContentResponse`.
    pub fn process_chunk(&mut self, chunk: &Value, timestamp: i64) -> Vec<AssistantMessageEvent> {
        let mut emitted = Vec::new();

        // The SDK documents `responseId` as output-only; the first non-empty one wins.
        if self.output.response_id.as_deref().is_none_or(str::is_empty)
            && let Some(response_id) = chunk.get("responseId").and_then(Value::as_str)
        {
            self.output.response_id = Some(response_id.to_string());
        }

        let candidate = chunk
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first());

        if let Some(parts) = candidate
            .and_then(|candidate| candidate.get("content"))
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
        {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    let is_thinking = is_thinking_part(part);
                    let signature = part.get("thoughtSignature").and_then(Value::as_str);
                    let matches_current = self.current_block.is_some_and(|index| {
                        matches!(
                            (&self.output.content[index], is_thinking),
                            (AssistantContent::Thinking(_), true)
                                | (AssistantContent::Text(_), false)
                        )
                    });
                    if !matches_current {
                        self.close_current_block(&mut emitted);
                        if is_thinking {
                            self.output
                                .content
                                .push(AssistantContent::Thinking(ThinkingContent::default()));
                            let index = self.block_index();
                            self.current_block = Some(index);
                            emitted.push(AssistantMessageEvent::ThinkingStart {
                                content_index: index,
                                partial: self.output.clone(),
                            });
                        } else {
                            self.output
                                .content
                                .push(AssistantContent::Text(TextContent::default()));
                            let index = self.block_index();
                            self.current_block = Some(index);
                            emitted.push(AssistantMessageEvent::TextStart {
                                content_index: index,
                                partial: self.output.clone(),
                            });
                        }
                    }
                    let index = self.current_block.expect("just opened");
                    match &mut self.output.content[index] {
                        AssistantContent::Thinking(block) => {
                            block.thinking.push_str(text);
                            block.thinking_signature = retain_thought_signature(
                                block.thinking_signature.clone(),
                                signature,
                            );
                            emitted.push(AssistantMessageEvent::ThinkingDelta {
                                content_index: index,
                                delta: text.to_string(),
                                partial: self.output.clone(),
                            });
                        }
                        AssistantContent::Text(block) => {
                            block.text.push_str(text);
                            block.text_signature =
                                retain_thought_signature(block.text_signature.clone(), signature);
                            emitted.push(AssistantMessageEvent::TextDelta {
                                content_index: index,
                                delta: text.to_string(),
                                partial: self.output.clone(),
                            });
                        }
                        AssistantContent::ToolCall(_) => unreachable!("index points at a block"),
                    }
                }

                if let Some(function_call) =
                    part.get("functionCall").filter(|call| call.is_object())
                {
                    self.close_current_block(&mut emitted);

                    let name = function_call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let provided_id = function_call
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty());
                    let is_duplicate = provided_id.is_some_and(|provided_id| {
                        self.output.content.iter().any(|block| {
                            matches!(block, AssistantContent::ToolCall(call) if call.id == provided_id)
                        })
                    });
                    let tool_call_id = match provided_id {
                        Some(provided_id) if !is_duplicate => provided_id.to_string(),
                        _ => format!(
                            "{name}_{timestamp}_{}",
                            TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed) + 1
                        ),
                    };

                    let mut tool_call = ToolCall {
                        id: tool_call_id,
                        name,
                        arguments: function_call
                            .get("args")
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default(),
                        ..ToolCall::default()
                    };
                    if let Some(signature) = part
                        .get("thoughtSignature")
                        .and_then(Value::as_str)
                        .filter(|signature| !signature.is_empty())
                    {
                        tool_call.thought_signature = Some(signature.to_string());
                    }

                    self.output
                        .content
                        .push(AssistantContent::ToolCall(tool_call.clone()));
                    let index = self.block_index();
                    emitted.push(AssistantMessageEvent::ToolcallStart {
                        content_index: index,
                        partial: self.output.clone(),
                    });
                    emitted.push(AssistantMessageEvent::ToolcallDelta {
                        content_index: index,
                        delta: serde_json::to_string(&tool_call.arguments)
                            .unwrap_or_else(|_| "{}".to_string()),
                        partial: self.output.clone(),
                    });
                    emitted.push(AssistantMessageEvent::ToolcallEnd {
                        content_index: index,
                        tool_call,
                        partial: self.output.clone(),
                    });
                }
            }
        }

        if let Some(finish_reason) = candidate
            .and_then(|candidate| candidate.get("finishReason"))
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
        {
            self.output.raw_stop_reason = Some(finish_reason.to_string());
            self.output.stop_reason = map_stop_reason(finish_reason);
            if self
                .output
                .content
                .iter()
                .any(|block| matches!(block, AssistantContent::ToolCall(_)))
            {
                self.output.stop_reason = StopReason::ToolUse;
            }
        }

        if let Some(usage) = chunk.get("usageMetadata").filter(|usage| usage.is_object()) {
            let number = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
            let cached = number("cachedContentTokenCount");
            let thoughts = number("thoughtsTokenCount");
            self.output.usage = Usage {
                input: number("promptTokenCount").saturating_sub(cached),
                output: number("candidatesTokenCount") + thoughts,
                cache_read: cached,
                cache_write: 0,
                reasoning: Some(thoughts),
                total_tokens: Some(number("totalTokenCount")),
                ..Usage::default()
            };
            calculate_cost(&self.model, &mut self.output.usage);
        }

        emitted
    }

    /// The block after the chunk loop.
    pub fn finish(&mut self) -> (Vec<AssistantMessageEvent>, Result<DoneReason, GoogleError>) {
        let mut emitted = Vec::new();
        self.close_current_block(&mut emitted);

        if self.output.stop_reason == StopReason::Pending {
            return (
                emitted,
                Err(GoogleError::from_message(
                    "Google stream ended without a finish reason",
                )),
            );
        }
        if self.output.stop_reason == StopReason::Aborted
            || self.output.stop_reason == StopReason::Error
        {
            let message = match &self.output.raw_stop_reason {
                Some(reason) if !reason.is_empty() => format!("Provider stopped with: {reason}"),
                _ => "An unknown error occurred".to_string(),
            };
            return (emitted, Err(GoogleError::from_message(message)));
        }
        let reason = match self.output.stop_reason {
            StopReason::Length => DoneReason::Length,
            StopReason::ToolUse => DoneReason::ToolUse,
            StopReason::Deferred => DoneReason::Deferred,
            _ => DoneReason::Stop,
        };
        (emitted, Ok(reason))
    }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// `stream(model, context, options)`
pub fn stream(
    model: Model,
    context: Context,
    request: ProviderRequestOptions,
    options: GoogleOptions,
) -> AssistantMessageEventStream {
    let outer = create_assistant_message_event_stream();
    let stream = outer.clone();

    tokio::spawn(async move {
        let timestamp = crate::auth::resolve::now_ms();
        let mut state = GoogleStreamState::new(&model, timestamp);
        let outcome = run_request(
            &model, &context, &request, &options, &mut state, &stream, timestamp,
        )
        .await;

        match outcome {
            Ok(reason) => {
                stream.push(AssistantMessageEvent::Done {
                    reason,
                    message: state.output.clone(),
                });
                stream.end(Some(state.output));
            }
            Err(error) => {
                let aborted = request
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.is_cancelled());
                state.output.stop_reason = if aborted {
                    StopReason::Aborted
                } else {
                    StopReason::Error
                };
                state.output.error_message = Some(format_provider_error(
                    &normalize_provider_error(&RawProviderError {
                        status: error.status,
                        body_text: error.body.clone(),
                        body_json: None,
                        message: error.message.clone(),
                    }),
                    None,
                ));
                stream.push(AssistantMessageEvent::Error {
                    reason: if aborted {
                        ErrorReason::Aborted
                    } else {
                        ErrorReason::Error
                    },
                    error: state.output.clone(),
                });
                stream.end(Some(state.output));
            }
        }
    });

    outer
}

async fn run_request(
    model: &Model,
    context: &Context,
    request: &ProviderRequestOptions,
    options: &GoogleOptions,
    state: &mut GoogleStreamState,
    stream: &AssistantMessageEventStream,
    timestamp: i64,
) -> Result<DoneReason, GoogleError> {
    let Some(api_key) = request.api_key.as_deref().filter(|key| !key.is_empty()) else {
        return Err(GoogleError::from_message(format!(
            "No API key for provider: {}",
            model.provider
        )));
    };

    let mut body = build_request_body(model, context, options, timestamp)?;
    if let Some(on_payload) = &request.on_payload
        && let Some(replacement) = on_payload(body.clone(), model).await
    {
        body = replacement;
    }

    let mut request_headers: Vec<(String, String)> =
        build_client_headers(model, request.headers.as_ref())
            .into_iter()
            .filter_map(|(key, value)| value.map(|value| (key, value)))
            .collect();
    request_headers.push(("content-type".to_string(), "application/json".to_string()));
    request_headers.push(("x-goog-api-key".to_string(), api_key.to_string()));

    let fetch: FetchFunction = request
        .fetch
        .clone()
        .unwrap_or_else(|| Arc::new(ReqwestFetch::default()));
    let url = build_request_url(model);
    let payload =
        serde_json::to_vec(&body).map_err(|error| GoogleError::from_message(error.to_string()))?;

    let response = retry_provider_request(
        || {
            let fetch = fetch.clone();
            let url = url.clone();
            let request_headers = request_headers.clone();
            let payload = payload.clone();
            async move {
                let response = fetch
                    .fetch(FetchRequest {
                        method: "POST".to_string(),
                        url,
                        headers: request_headers,
                        body: Some(payload),
                    })
                    .await
                    .map_err(|error| GoogleError::from_message(error.to_string()))?;
                if response.status < 200 || response.status >= 300 {
                    let status = response.status;
                    let headers = response.headers.clone();
                    let body = read_body(response.body).await;
                    return Err(GoogleError {
                        message: format!("{status}: {body}"),
                        status: Some(status),
                        headers,
                        body: Some(body),
                    });
                }
                Ok(response)
            }
        },
        ProviderRetryOptions {
            max_retries: request.max_retries,
            max_retry_delay_ms: request.max_retry_delay_ms,
            signal: request.signal.clone(),
        },
    )
    .await
    .map_err(|error| match error {
        ProviderRetryError::Request(error) => error,
        ProviderRetryError::RetryDelayTooLong(message) => GoogleError::from_message(message),
        ProviderRetryError::Aborted => GoogleError::from_message("Request aborted"),
    })?;

    if let Some(on_response) = &request.on_response {
        on_response(
            crate::types::ProviderResponse {
                status: response.status,
                headers: response.headers.iter().cloned().collect(),
            },
            model,
        )
        .await;
    }

    stream.push(AssistantMessageEvent::Start {
        partial: state.output.clone(),
    });

    let mut decoder = SseDecoder::new();
    let mut feed = |text: &str, state: &mut GoogleStreamState| {
        for event in decoder.feed(text) {
            if event.data.trim().is_empty() {
                continue;
            }
            let Ok(chunk) = serde_json::from_str::<Value>(&event.data) else {
                continue;
            };
            for emitted in state.process_chunk(&chunk, timestamp) {
                stream.push(emitted);
            }
        }
    };

    match response.body {
        FetchBody::Bytes(bytes) => {
            feed(&String::from_utf8_lossy(&bytes), state);
        }
        FetchBody::Stream(mut receiver) => {
            while let Some(chunk) = receiver.recv().await {
                if request
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.is_cancelled())
                {
                    return Err(GoogleError::from_message("Request was aborted"));
                }
                let chunk = chunk.map_err(|error| GoogleError::from_message(error.to_string()))?;
                feed(&String::from_utf8_lossy(&chunk), state);
            }
        }
    }
    for event in decoder.finish() {
        if event.data.trim().is_empty() {
            continue;
        }
        if let Ok(chunk) = serde_json::from_str::<Value>(&event.data) {
            for emitted in state.process_chunk(&chunk, timestamp) {
                stream.push(emitted);
            }
        }
    }

    let (emitted, outcome) = state.finish();
    for event in emitted {
        stream.push(event);
    }
    if request
        .signal
        .as_ref()
        .is_some_and(|signal| signal.is_cancelled())
    {
        return Err(GoogleError::from_message("Request was aborted"));
    }
    outcome
}

async fn read_body(body: FetchBody) -> String {
    match body {
        FetchBody::Bytes(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        FetchBody::Stream(mut receiver) => {
            let mut collected = Vec::new();
            while let Some(Ok(chunk)) = receiver.recv().await {
                collected.extend(chunk);
            }
            String::from_utf8_lossy(&collected).to_string()
        }
    }
}

/// `streamSimple(model, context, options)` — errors without an api key, like TS.
pub fn stream_simple(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
) -> Result<AssistantMessageEventStream, GoogleError> {
    let options = options.unwrap_or_default();
    let Some(api_key) = options
        .base
        .base
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
    else {
        return Err(GoogleError::from_message(format!(
            "No API key for provider: {}",
            model.provider
        )));
    };

    let base = build_base_options(&model, &context, Some(&options), Some(api_key.to_string()));
    let google_options = |thinking: Option<GoogleThinkingOptions>| GoogleOptions {
        tool_choice: None,
        thinking,
        max_tokens: base.max_tokens,
        temperature: base.temperature,
    };

    let Some(reasoning) = options.reasoning else {
        return Ok(stream(
            model,
            context,
            base.base.clone(),
            google_options(Some(GoogleThinkingOptions {
                enabled: false,
                ..GoogleThinkingOptions::default()
            })),
        ));
    };

    let clamped = clamp_thinking_level(&model, reasoning.into());
    // `clampedReasoning === "off" ? "high" : clampedReasoning`
    let effort = match clamped {
        ModelThinkingLevel::Off => ThinkingLevel::High,
        ModelThinkingLevel::Minimal => ThinkingLevel::Minimal,
        ModelThinkingLevel::Low => ThinkingLevel::Low,
        ModelThinkingLevel::Medium => ThinkingLevel::Medium,
        ModelThinkingLevel::High => ThinkingLevel::High,
        // The cast in TS says these cannot occur after clamping.
        ModelThinkingLevel::Xhigh | ModelThinkingLevel::Max => ThinkingLevel::High,
    };

    let thinking = if is_gemini3_pro_model(&model.id)
        || is_gemini3_flash_model(&model.id)
        || is_gemma4_model(&model.id)
    {
        GoogleThinkingOptions {
            enabled: true,
            budget_tokens: None,
            level: Some(thinking_level(effort, &model.id).to_string()),
        }
    } else {
        GoogleThinkingOptions {
            enabled: true,
            budget_tokens: Some(google_budget(&model.id, effort, options.thinking_budgets)),
            level: None,
        }
    };

    Ok(stream(
        model,
        context,
        base.base.clone(),
        google_options(Some(thinking)),
    ))
}
