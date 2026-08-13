//! OpenAI Responses API.
//!
//! 1:1 port of `packages/ai/src/api/openai-responses.ts`. The message, tool and stream
//! handling live in [`crate::api::openai_responses_shared`]; this module builds the
//! request, resolves the compat matrix and runs the transport.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::create_grammar_tool_input_properties;
use crate::api::github_copilot_headers::{build_copilot_dynamic_headers, has_copilot_vision_input};
use crate::api::openai_completions_params::{get_client_api_key, resolve_cache_retention};
use crate::api::openai_prompt_cache::clamp_openai_prompt_cache_key;
use crate::api::openai_responses_shared::{
    ConvertResponsesMessagesOptions, ConvertResponsesToolsOptions, ResponsesDeferredToolsMode,
    ResponsesStreamOptions, ResponsesStreamState, convert_responses_messages,
    convert_responses_tools,
};
use crate::api::simple_options::build_base_options;
use crate::api::sse::SseDecoder;
use crate::models::clamp_thinking_level;
use crate::types::{
    AssistantMessage, AssistantMessageEvent, CacheRetention, Context, DoneReason, ErrorReason,
    Model, ModelThinkingLevel, ProviderHeaders, ProviderRequestOptions, SessionAffinityFormat,
    SimpleStreamOptions, StopReason, ThinkingLevel, Usage,
};
use crate::utils::deferred_tools::split_deferred_tools;
use crate::utils::error_body::{RawProviderError, format_provider_error, normalize_provider_error};
use crate::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use crate::utils::fetch::{FetchBody, FetchFunction, FetchRequest, ReqwestFetch};
use crate::utils::provider_retry::{
    ProviderErrorInfo, ProviderRetryError, ProviderRetryOptions, retry_provider_request,
};

/// `OPENAI_TOOL_CALL_PROVIDERS` — providers whose `callId|itemId` pairs survive replay.
pub fn openai_tool_call_providers() -> BTreeSet<String> {
    ["openai", "openai-codex", "opencode"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Responses rejects `max_output_tokens` below 16 (notagentdev/notagent#6265).
pub const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS: u64 = 16;

/// `Required<OpenAIResponsesCompat>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOpenAIResponsesCompat {
    pub supports_developer_role: bool,
    pub session_affinity_format: SessionAffinityFormat,
    pub supports_long_cache_retention: bool,
    pub supports_strict_mode: bool,
    pub supports_openai_grammar_tools: bool,
    pub supports_additional_tools: bool,
    pub supports_tool_search: bool,
    pub supports_explicit_prompt_cache_mode: bool,
}

/// `detectSessionAffinityFormat(model)`
fn detect_session_affinity_format(model: &Model) -> SessionAffinityFormat {
    if model.provider == "openrouter" || model.base_url.contains("openrouter.ai") {
        SessionAffinityFormat::Openrouter
    } else {
        SessionAffinityFormat::Openai
    }
}

/// `getCompat(model)`
pub fn get_compat(model: &Model) -> ResolvedOpenAIResponsesCompat {
    let compat = match &model.compat {
        Some(crate::types::ModelCompat::OpenAIResponses(compat)) => Some(compat),
        _ => None,
    };
    ResolvedOpenAIResponsesCompat {
        supports_developer_role: compat
            .and_then(|compat| compat.supports_developer_role)
            .unwrap_or(true),
        session_affinity_format: compat
            .and_then(|compat| compat.session_affinity_format)
            .unwrap_or_else(|| detect_session_affinity_format(model)),
        supports_long_cache_retention: compat
            .and_then(|compat| compat.supports_long_cache_retention)
            .unwrap_or(true),
        supports_strict_mode: compat
            .and_then(|compat| compat.supports_strict_mode)
            .unwrap_or(false),
        supports_openai_grammar_tools: compat
            .and_then(|compat| compat.supports_openai_grammar_tools)
            .unwrap_or(false),
        supports_additional_tools: compat
            .and_then(|compat| compat.supports_additional_tools)
            .unwrap_or(false),
        supports_tool_search: compat
            .and_then(|compat| compat.supports_tool_search)
            .unwrap_or(false),
        supports_explicit_prompt_cache_mode: compat
            .and_then(|compat| compat.supports_explicit_prompt_cache_mode)
            .unwrap_or(false),
    }
}

/// `getPromptCacheRetention(compat, cacheRetention)`
fn prompt_cache_retention(
    compat: &ResolvedOpenAIResponsesCompat,
    cache_retention: CacheRetention,
) -> Option<&'static str> {
    (cache_retention == CacheRetention::Long && compat.supports_long_cache_retention)
        .then_some("24h")
}

/// `OpenAIResponsesOptions extends StreamOptions`
#[derive(Debug, Clone, Default)]
pub struct OpenAIResponsesOptions {
    pub reasoning_effort: Option<ThinkingLevel>,
    /// `"auto" | "detailed" | "concise" | null`; `Some(None)` is an explicit `null`.
    pub reasoning_summary: Option<Option<String>>,
    pub service_tier: Option<String>,
    pub tool_choice: Option<Value>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub sampling_params: Option<Map<String, Value>>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<String>,
    pub env: Option<crate::types::ProviderEnv>,
}

fn effort_str(effort: ThinkingLevel) -> &'static str {
    match effort {
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

/// `buildParams(model, context, options, compat, grammarToolInputProperties)`
pub fn build_params(
    model: &Model,
    context: &Context,
    options: &OpenAIResponsesOptions,
    compat: &ResolvedOpenAIResponsesCompat,
    grammar_tool_input_properties: &BTreeMap<String, String>,
    timestamp: i64,
) -> Result<Value, crate::api::constrained_sampling::ConstrainedSamplingError> {
    let deferred_tools_mode = if compat.supports_additional_tools {
        Some(ResponsesDeferredToolsMode::AdditionalTools)
    } else if compat.supports_tool_search {
        Some(ResponsesDeferredToolsMode::ToolSearch)
    } else {
        None
    };
    let tool_placement = split_deferred_tools(context, deferred_tools_mode.is_some(), |name| {
        name.to_string()
    });
    let tool_options = ConvertResponsesToolsOptions {
        supports_strict_mode: Some(compat.supports_strict_mode),
        supports_openai_grammar_tools: Some(compat.supports_openai_grammar_tools),
        ..ConvertResponsesToolsOptions::default()
    };
    let messages = convert_responses_messages(
        model,
        context,
        &openai_tool_call_providers(),
        &ConvertResponsesMessagesOptions {
            grammar_tool_input_properties: Some(grammar_tool_input_properties),
            deferred_tools: Some(&tool_placement.deferred),
            deferred_tools_mode,
            tool_options: Some(tool_options.clone()),
            ..ConvertResponsesMessagesOptions::default()
        },
        timestamp,
    )?;

    let cache_retention = resolve_cache_retention(options.cache_retention, options.env.as_ref());
    let disable_implicit_prompt_cache =
        cache_retention == CacheRetention::None && compat.supports_explicit_prompt_cache_mode;

    let mut params = Map::new();
    params.insert("model".to_string(), json!(model.id));
    params.insert("input".to_string(), Value::Array(messages));
    params.insert("stream".to_string(), json!(true));
    if cache_retention != CacheRetention::None
        && let Some(key) = clamp_openai_prompt_cache_key(options.session_id.as_deref())
    {
        params.insert("prompt_cache_key".to_string(), json!(key));
    }
    if let Some(retention) = prompt_cache_retention(compat, cache_retention) {
        params.insert("prompt_cache_retention".to_string(), json!(retention));
    }
    if disable_implicit_prompt_cache {
        params.insert(
            "prompt_cache_options".to_string(),
            json!({ "mode": "explicit" }),
        );
    }
    params.insert("store".to_string(), json!(false));

    if let Some(max_tokens) = options.max_tokens.filter(|value| *value != 0) {
        params.insert(
            "max_output_tokens".to_string(),
            json!(max_tokens.max(OPENAI_RESPONSES_MIN_OUTPUT_TOKENS)),
        );
    }
    if let Some(temperature) = options.temperature {
        params.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(service_tier) = &options.service_tier {
        params.insert("service_tier".to_string(), json!(service_tier));
    }
    if !tool_placement.immediate.is_empty() {
        params.insert(
            "tools".to_string(),
            Value::Array(convert_responses_tools(
                &tool_placement.immediate,
                &tool_options,
            )?),
        );
    }
    if let Some(tool_choice) = &options.tool_choice {
        params.insert("tool_choice".to_string(), tool_choice.clone());
    }

    if model.reasoning {
        let has_summary = options
            .reasoning_summary
            .as_ref()
            .is_some_and(|summary| summary.as_deref().is_some_and(|value| !value.is_empty()));
        if options.reasoning_effort.is_some() || has_summary {
            let effort = match options.reasoning_effort {
                Some(effort) => model
                    .thinking_level_map
                    .as_ref()
                    .and_then(|map| map.get(&effort.into()))
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| effort_str(effort).to_string()),
                None => "medium".to_string(),
            };
            let summary = options
                .reasoning_summary
                .clone()
                .flatten()
                .filter(|summary| !summary.is_empty())
                .unwrap_or_else(|| "auto".to_string());
            params.insert(
                "reasoning".to_string(),
                json!({ "effort": effort, "summary": summary }),
            );
            params.insert(
                "include".to_string(),
                json!(["reasoning.encrypted_content"]),
            );
        } else if model.provider != "github-copilot" && off_is_not_null(model) {
            let effort = model
                .thinking_level_map
                .as_ref()
                .and_then(|map| map.get(&ModelThinkingLevel::Off))
                .cloned()
                .flatten()
                .unwrap_or_else(|| "none".to_string());
            params.insert("reasoning".to_string(), json!({ "effort": effort }));
        }
        if model.provider == "xai" {
            params.insert(
                "include".to_string(),
                json!(["reasoning.encrypted_content"]),
            );
        }
    }

    // Last, so custom keys override the named request fields.
    if let Some(sampling_params) = &options.sampling_params {
        for (key, value) in sampling_params {
            params.insert(key.clone(), value.clone());
        }
    }

    Ok(Value::Object(params))
}

/// `model.thinkingLevelMap?.off !== null`
fn off_is_not_null(model: &Model) -> bool {
    !matches!(
        model
            .thinking_level_map
            .as_ref()
            .and_then(|map| map.get(&ModelThinkingLevel::Off)),
        Some(None)
    )
}

/// The `defaultHeaders` half of `createClient(model, context, …)`.
pub fn build_client_headers(
    model: &Model,
    context: &Context,
    options_headers: Option<&ProviderHeaders>,
    session_id: Option<&str>,
) -> BTreeMap<String, Option<String>> {
    let compat = get_compat(model);
    let mut headers: BTreeMap<String, Option<String>> = model
        .headers
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, Some(value)))
        .collect();

    if model.provider == "github-copilot" {
        let has_images = has_copilot_vision_input(&context.messages);
        for (key, value) in build_copilot_dynamic_headers(&context.messages, has_images) {
            headers.insert(key, Some(value));
        }
    }

    if let Some(session_id) = session_id.filter(|value| !value.is_empty()) {
        if compat.session_affinity_format == SessionAffinityFormat::Openrouter {
            headers.insert("x-session-id".to_string(), Some(session_id.to_string()));
        } else {
            if compat.session_affinity_format == SessionAffinityFormat::Openai {
                headers.insert("session_id".to_string(), Some(session_id.to_string()));
            }
            headers.insert(
                "x-client-request-id".to_string(),
                Some(session_id.to_string()),
            );
        }
    }

    // Options headers merge last so they can override the defaults.
    if let Some(options_headers) = options_headers {
        for (key, value) in options_headers {
            headers.insert(key.clone(), value.clone());
        }
    }

    headers
}

// ---------------------------------------------------------------------------
// Service-tier pricing
// ---------------------------------------------------------------------------

/// `getServiceTierCostMultiplier(model, serviceTier)`
pub fn service_tier_cost_multiplier(model_id: &str, service_tier: Option<&str>) -> f64 {
    match service_tier {
        Some("flex") => 0.5,
        Some("priority") => {
            if model_id == "gpt-5.5" {
                2.5
            } else {
                2.0
            }
        }
        _ => 1.0,
    }
}

/// `applyServiceTierPricing(usage, serviceTier, model)`
pub fn apply_service_tier_pricing(usage: &mut Usage, service_tier: Option<&str>, model_id: &str) {
    let multiplier = service_tier_cost_multiplier(model_id, service_tier);
    if multiplier == 1.0 {
        return;
    }
    usage.cost.input *= multiplier;
    usage.cost.output *= multiplier;
    usage.cost.cache_read *= multiplier;
    usage.cost.cache_write *= multiplier;
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// The HTTP half of an `APIError`; boxed so the error itself stays small.
#[derive(Debug, Clone)]
pub struct ResponsesApiErrorResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// `error.error` — the parsed JSON body, as the openai SDK exposes it.
    pub body: Option<Value>,
}

/// Error of the stream; carries the HTTP details the retry policy probes.
#[derive(Debug, Clone, Default)]
pub struct ResponsesTransportError {
    pub message: String,
    /// Absent for plain thrown errors (aborts, transport failures).
    pub response: Option<Box<ResponsesApiErrorResponse>>,
}

impl ResponsesTransportError {
    fn message(message: impl Into<String>) -> Self {
        ResponsesTransportError {
            message: message.into(),
            response: None,
        }
    }
}

impl ProviderErrorInfo for ResponsesTransportError {
    fn status(&self) -> Option<u16> {
        self.response.as_ref().map(|response| response.status)
    }

    fn header(&self, name: &str) -> Option<String> {
        self.response.as_ref().and_then(|response| {
            response
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
        })
    }

    fn message(&self) -> String {
        self.message.clone()
    }
}

/// `formatOpenAIResponsesError(error)`
fn format_openai_responses_error(error: &ResponsesTransportError) -> String {
    format_provider_error(
        &normalize_provider_error(&RawProviderError {
            status: error.response.as_ref().map(|response| response.status),
            body_text: None,
            body_json: error
                .response
                .as_ref()
                .and_then(|response| response.body.clone()),
            message: error.message.clone(),
        }),
        Some("OpenAI API error"),
    )
}

fn empty_output(model: &Model, timestamp: i64) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
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
    }
}

/// `stream(model, context, options)`
pub fn stream(
    model: Model,
    context: Context,
    request: ProviderRequestOptions,
    options: OpenAIResponsesOptions,
) -> AssistantMessageEventStream {
    let outer = create_assistant_message_event_stream();
    let stream = outer.clone();

    tokio::spawn(async move {
        let timestamp = crate::auth::resolve::now_ms();
        let compat = get_compat(&model);
        let grammar_tool_input_properties = create_grammar_tool_input_properties(
            context.tools.as_deref(),
            compat.supports_openai_grammar_tools,
        )
        .unwrap_or_default();
        let mut state = ResponsesStreamState::new(empty_output(&model, timestamp), &model);

        let outcome = run_request(
            &model,
            &context,
            &request,
            &options,
            &compat,
            &grammar_tool_input_properties,
            &mut state,
            &stream,
            timestamp,
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
                state.output.error_message = Some(format_openai_responses_error(&error));
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

#[allow(clippy::too_many_arguments)]
async fn run_request(
    model: &Model,
    context: &Context,
    request: &ProviderRequestOptions,
    options: &OpenAIResponsesOptions,
    compat: &ResolvedOpenAIResponsesCompat,
    grammar_tool_input_properties: &BTreeMap<String, String>,
    state: &mut ResponsesStreamState,
    stream: &AssistantMessageEventStream,
    timestamp: i64,
) -> Result<DoneReason, ResponsesTransportError> {
    let api_key = get_client_api_key(
        &model.provider,
        request.api_key.as_deref(),
        request.headers.as_ref(),
    )
    .map_err(|error| ResponsesTransportError::message(error.to_string()))?;

    let cache_retention = resolve_cache_retention(options.cache_retention, options.env.as_ref());
    let cache_session_id = if cache_retention == CacheRetention::None {
        None
    } else {
        options.session_id.as_deref()
    };

    let mut params = build_params(
        model,
        context,
        options,
        compat,
        grammar_tool_input_properties,
        timestamp,
    )
    .map_err(|error| ResponsesTransportError::message(error.to_string()))?;
    if let Some(on_payload) = &request.on_payload
        && let Some(replacement) = on_payload(params.clone(), model).await
    {
        params = replacement;
    }

    let headers = build_client_headers(model, context, request.headers.as_ref(), cache_session_id);
    let mut request_headers: Vec<(String, String)> = headers
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .collect();
    request_headers.push(("content-type".to_string(), "application/json".to_string()));
    request_headers.push(("accept".to_string(), "text/event-stream".to_string()));
    if !request_headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("authorization"))
    {
        request_headers.push(("authorization".to_string(), format!("Bearer {api_key}")));
    }

    let fetch: FetchFunction = request
        .fetch
        .clone()
        .unwrap_or_else(|| Arc::new(ReqwestFetch::default()));
    let url = format!("{}/responses", model.base_url.trim_end_matches('/'));
    let body = serde_json::to_vec(&params)
        .map_err(|error| ResponsesTransportError::message(error.to_string()))?;

    let response = retry_provider_request(
        || {
            let fetch = fetch.clone();
            let url = url.clone();
            let request_headers = request_headers.clone();
            let body = body.clone();
            async move {
                let response = fetch
                    .fetch(FetchRequest {
                        method: "POST".to_string(),
                        url,
                        headers: request_headers,
                        body: Some(body),
                    })
                    .await
                    .map_err(|error| ResponsesTransportError::message(error.to_string()))?;
                if response.status < 200 || response.status >= 300 {
                    let status = response.status;
                    let headers = response.headers.clone();
                    let body = read_body(response.body).await;
                    return Err(api_error(status, headers, &body));
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
        ProviderRetryError::RetryDelayTooLong(message) => ResponsesTransportError::message(message),
        ProviderRetryError::Aborted => ResponsesTransportError::message("Request aborted"),
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

    let model_id = model.id.clone();
    let stream_options = ResponsesStreamOptions {
        service_tier: options.service_tier.clone(),
        grammar_tool_input_properties: Some(grammar_tool_input_properties),
        resolve_service_tier: None,
        apply_service_tier_pricing: Some(Box::new(move |usage: &mut Usage, tier: Option<&str>| {
            apply_service_tier_pricing(usage, tier, &model_id);
        })),
    };

    let mut decoder = SseDecoder::new();
    let pump = |events: Vec<crate::api::sse::ServerSentEvent>,
                state: &mut ResponsesStreamState|
     -> Result<(), ResponsesTransportError> {
        for event in events {
            for emitted in process_sse_event(&event, state, &stream_options)? {
                stream.push(emitted);
            }
        }
        Ok(())
    };

    match response.body {
        FetchBody::Bytes(bytes) => {
            let text = String::from_utf8_lossy(&bytes).to_string();
            let events: Vec<_> = decoder
                .feed(&text)
                .into_iter()
                .chain(decoder.finish())
                .collect();
            pump(events, state)?;
        }
        FetchBody::Stream(mut body) => {
            while let Some(chunk) = body.recv().await {
                if request
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.is_cancelled())
                {
                    return Err(ResponsesTransportError::message("Request was aborted"));
                }
                let chunk =
                    chunk.map_err(|error| ResponsesTransportError::message(error.to_string()))?;
                let text = String::from_utf8_lossy(&chunk).to_string();
                pump(decoder.feed(&text), state)?;
            }
            pump(decoder.finish(), state)?;
        }
    }

    if !state.saw_terminal_response_event() {
        return Err(ResponsesTransportError::message(
            "OpenAI Responses stream ended before a terminal response event",
        ));
    }
    if request
        .signal
        .as_ref()
        .is_some_and(|signal| signal.is_cancelled())
    {
        return Err(ResponsesTransportError::message("Request was aborted"));
    }
    if state.output.stop_reason == StopReason::Pending {
        return Err(ResponsesTransportError::message(
            "OpenAI Responses stream ended without a stop reason",
        ));
    }
    if state.output.stop_reason == StopReason::Aborted
        || state.output.stop_reason == StopReason::Error
    {
        return Err(ResponsesTransportError::message(
            state
                .output
                .error_message
                .clone()
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| "An unknown error occurred".to_string()),
        ));
    }
    Ok(match state.output.stop_reason {
        StopReason::Length => DoneReason::Length,
        StopReason::ToolUse => DoneReason::ToolUse,
        StopReason::Deferred => DoneReason::Deferred,
        _ => DoneReason::Stop,
    })
}

fn process_sse_event(
    event: &crate::api::sse::ServerSentEvent,
    state: &mut ResponsesStreamState,
    options: &ResponsesStreamOptions<'_>,
) -> Result<Vec<AssistantMessageEvent>, ResponsesTransportError> {
    if event.data.starts_with("[DONE]") || event.data.trim().is_empty() {
        return Ok(Vec::new());
    }
    let parsed = serde_json::from_str::<Value>(&event.data)
        .map_err(|error| ResponsesTransportError::message(error.to_string()))?;
    state
        .process_event(&parsed, options)
        .map_err(|error| ResponsesTransportError::message(error.to_string()))
}

/// The openai SDK's `APIError.generate(status, errJSON, errText, headers)`.
fn api_error(status: u16, headers: Vec<(String, String)>, body: &str) -> ResponsesTransportError {
    let err_json = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|body| body.get("error").cloned());
    let message = match err_json.as_ref().filter(|json| !json.is_null()) {
        Some(json) => match json.get("message").filter(|message| !message.is_null()) {
            Some(Value::String(message)) if !message.is_empty() => message.clone(),
            Some(Value::String(_)) | None => json.to_string(),
            Some(message) => message.to_string(),
        },
        None => body.to_string(),
    };
    let message = if message.is_empty() {
        format!("{status} status code (no body)")
    } else {
        format!("{status} {message}")
    };
    ResponsesTransportError {
        message,
        response: Some(Box::new(ResponsesApiErrorResponse {
            status,
            headers,
            body: err_json,
        })),
    }
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

/// `streamSimple(model, context, options)`
pub fn stream_simple(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    let options = options.unwrap_or_default();
    if let Err(error) = get_client_api_key(
        &model.provider,
        options.base.base.api_key.as_deref(),
        options.base.base.headers.as_ref(),
    ) {
        let stream = create_assistant_message_event_stream();
        let mut output = empty_output(&model, crate::auth::resolve::now_ms());
        output.stop_reason = StopReason::Error;
        output.error_message = Some(error.to_string());
        stream.push(AssistantMessageEvent::Error {
            reason: ErrorReason::Error,
            error: output.clone(),
        });
        stream.end(Some(output));
        return stream;
    }

    let base = build_base_options(
        &model,
        &context,
        Some(&options),
        options.base.base.api_key.clone(),
    );
    let clamped_reasoning = options
        .reasoning
        .map(|reasoning| clamp_thinking_level(&model, reasoning.into()));
    let reasoning_effort = match clamped_reasoning {
        Some(ModelThinkingLevel::Minimal) => Some(ThinkingLevel::Minimal),
        Some(ModelThinkingLevel::Low) => Some(ThinkingLevel::Low),
        Some(ModelThinkingLevel::Medium) => Some(ThinkingLevel::Medium),
        Some(ModelThinkingLevel::High) => Some(ThinkingLevel::High),
        Some(ModelThinkingLevel::Xhigh) => Some(ThinkingLevel::Xhigh),
        Some(ModelThinkingLevel::Max) => Some(ThinkingLevel::Max),
        Some(ModelThinkingLevel::Off) | None => None,
    };

    stream(
        model,
        context,
        base.base.clone(),
        OpenAIResponsesOptions {
            reasoning_effort,
            reasoning_summary: None,
            service_tier: None,
            tool_choice: None,
            max_tokens: base.max_tokens,
            temperature: base.temperature,
            sampling_params: base.sampling_params.clone(),
            cache_retention: base.cache_retention,
            session_id: base.session_id.clone(),
            env: base.base.env.clone(),
        },
    )
}
