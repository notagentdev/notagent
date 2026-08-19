//! OpenAI Codex Responses API (the ChatGPT backend).
//!
//! 1:1 port of `packages/ai/src/api/openai-codex-responses.ts`. The message, tool and
//! stream handling come from [`crate::api::openai_responses_shared`]; specific to Codex
//! are the request body, the JWT account id, the zstd-compressed SSE request, the
//! WebSocket transport with its session-scoped connection cache, and the fallback from
//! WebSocket to SSE with its `provider_transport_failure` diagnostics.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use serde_json::{Map, Value, json};
use url::Url;

use crate::api::constrained_sampling::create_grammar_tool_input_properties;
use crate::api::openai_prompt_cache::clamp_openai_prompt_cache_key;
use crate::api::openai_responses_shared::{
    ConvertResponsesMessagesOptions, ConvertResponsesToolsOptions, ResponsesDeferredToolsMode,
    ResponsesStreamOptions, ResponsesStreamState, convert_responses_messages,
    convert_responses_tools,
};
use crate::api::simple_options::build_base_options;
use crate::models::clamp_thinking_level;
use crate::types::{
    AssistantMessage, AssistantMessageEvent, CacheRetention, Context, DoneReason, ErrorReason,
    Message, Model, ModelThinkingLevel, ProviderEnv, ProviderHeaders, ProviderRequestOptions,
    SimpleStreamOptions, StopReason, ThinkingLevel, Transport, Usage,
};
use crate::utils::deferred_tools::split_deferred_tools;
use crate::utils::diagnostics::{
    append_assistant_message_diagnostic, create_assistant_message_diagnostic,
    extract_diagnostic_error, format_thrown_value,
};
use crate::utils::error_body::{RawProviderError, format_provider_error, normalize_provider_error};
use crate::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use crate::utils::fetch::{FetchBody, FetchFunction, FetchRequest, ReqwestFetch};
use crate::utils::uuid::uuidv7;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

pub const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";
const DEFAULT_MAX_RETRIES: u32 = 0;
const BASE_DELAY_MS: u64 = 1000;
const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;
const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS: u64 = 15_000;
/// The Codex backend accepts zstd-compressed request bodies on the SSE responses
/// endpoint (the same endpoint the official Codex client compresses against).
const REQUEST_COMPRESSION_ZSTD_LEVEL: i32 = 3;
const WEBSOCKET_MESSAGE_TOO_BIG_CLOSE_CODE: u16 = 1009;
const WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE: &str = "websocket_connection_limit_reached";
const PREVIOUS_RESPONSE_NOT_FOUND_CODE: &str = "previous_response_not_found";
const OPENAI_BETA_RESPONSES_WEBSOCKETS: &str = "responses_websockets=2026-02-06";
const SESSION_WEBSOCKET_CACHE_TTL_MS: u64 = 5 * 60 * 1000;
const SESSION_WEBSOCKET_MAX_AGE_MS: u64 = 55 * 60 * 1000;

/// `CODEX_TOOL_CALL_PROVIDERS`
pub fn codex_tool_call_providers() -> BTreeSet<String> {
    ["openai", "openai-codex", "opencode"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// `CODEX_RESPONSE_STATUSES`
const CODEX_RESPONSE_STATUSES: [&str; 6] = [
    "completed",
    "incomplete",
    "failed",
    "cancelled",
    "queued",
    "in_progress",
];

/// `OpenAICodexResponsesOptions extends StreamOptions`
#[derive(Debug, Clone, Default)]
pub struct OpenAICodexResponsesOptions {
    /// `"none"` in addition to the usual levels.
    pub reasoning_effort: Option<CodexReasoningEffort>,
    /// `"auto" | "concise" | "detailed" | "off" | "on" | null`
    pub reasoning_summary: Option<Option<String>>,
    pub service_tier: Option<String>,
    /// `"low" | "medium" | "high"`, default `"low"`.
    pub text_verbosity: Option<String>,
    /// `"auto" | "none" | "required"`, default `"auto"`.
    pub tool_choice: Option<String>,
    pub temperature: Option<f64>,
    pub transport: Option<Transport>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<String>,
    pub websocket_connect_timeout_ms: Option<u64>,
    pub env: Option<ProviderEnv>,
}

/// `reasoningEffort?: "none" | ThinkingLevel`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexReasoningEffort {
    None,
    Level(ThinkingLevel),
}

impl CodexReasoningEffort {
    fn as_str(self) -> &'static str {
        match self {
            CodexReasoningEffort::None => "none",
            CodexReasoningEffort::Level(ThinkingLevel::Minimal) => "minimal",
            CodexReasoningEffort::Level(ThinkingLevel::Low) => "low",
            CodexReasoningEffort::Level(ThinkingLevel::Medium) => "medium",
            CodexReasoningEffort::Level(ThinkingLevel::High) => "high",
            CodexReasoningEffort::Level(ThinkingLevel::Xhigh) => "xhigh",
            CodexReasoningEffort::Level(ThinkingLevel::Max) => "max",
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors of this provider, split the way the TS classes are: only `CodexApiError` and
/// `CodexProtocolError` count as non-transport errors and stop the WebSocket fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexError {
    /// `CodexApiError` — the backend reported an error event.
    Api {
        message: String,
        code: Option<String>,
    },
    /// `CodexProtocolError` — a frame could not be decoded.
    Protocol { message: String },
    /// A transport or generic failure; the WebSocket path falls back to SSE for these.
    Transport { message: String },
    /// `RetryDelayExceededError`
    RetryDelayExceeded { message: String },
    /// `WebSocketCloseError`
    WebSocketClose { message: String, code: Option<u16> },
}

impl std::fmt::Display for CodexError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for CodexError {}

impl CodexError {
    pub fn message(&self) -> &str {
        match self {
            CodexError::Api { message, .. }
            | CodexError::Protocol { message }
            | CodexError::Transport { message }
            | CodexError::RetryDelayExceeded { message }
            | CodexError::WebSocketClose { message, .. } => message,
        }
    }

    fn transport(message: impl Into<String>) -> Self {
        CodexError::Transport {
            message: message.into(),
        }
    }

    /// `isCodexNonTransportError(error)`
    fn is_non_transport(&self) -> bool {
        matches!(self, CodexError::Api { .. } | CodexError::Protocol { .. })
    }

    fn code(&self) -> Option<&str> {
        match self {
            CodexError::Api { code, .. } => code.as_deref(),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Retry helpers
// ---------------------------------------------------------------------------

/// `isTerminalRateLimitError(errorText)`
pub fn is_terminal_rate_limit_error(error_text: &str) -> bool {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            regex::Regex::new(
                r"(?i)GoUsageLimitError|FreeUsageLimitError|Monthly usage limit reached|available balance|insufficient_quota|out of budget|quota exceeded|billing",
            )
            .expect("valid pattern")
        })
        .is_match(error_text)
}

/// `isRetryableError(status, errorText)`
pub fn is_retryable_error(status: u16, error_text: &str) -> bool {
    if status == 429 && is_terminal_rate_limit_error(error_text) {
        return false;
    }
    if matches!(status, 429 | 500 | 502 | 503 | 504) {
        return true;
    }
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            regex::Regex::new(
                r"(?i)rate.?limit|overloaded|service.?unavailable|upstream.?connect|connection.?refused",
            )
            .expect("valid pattern")
        })
        .is_match(error_text)
}

/// `getRetryAfterDelayMs(headers)`
pub fn retry_after_delay_ms(headers: &[(String, String)], now_ms: i64) -> Option<f64> {
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };

    if let Some(retry_after_ms) = header("retry-after-ms") {
        // `Number(value)`: an empty string is 0, anything unparsable is NaN.
        if let Some(millis) = js_number(&retry_after_ms).filter(|value| value.is_finite()) {
            return Some(millis.max(0.0));
        }
    }

    let retry_after = header("retry-after").filter(|value| !value.is_empty())?;
    if let Some(seconds) = js_number(&retry_after).filter(|value| value.is_finite()) {
        return Some((seconds * 1000.0).max(0.0));
    }
    // `Date.parse(retryAfter)` for the HTTP-date form.
    if let Ok(date) = chrono::DateTime::parse_from_rfc2822(retry_after.trim()) {
        return Some(((date.timestamp_millis() - now_ms) as f64).max(0.0));
    }
    None
}

/// `Number(value)` — whitespace-only and empty strings are 0, everything unparsable NaN.
fn js_number(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Some(0.0);
    }
    trimmed.parse::<f64>().ok()
}

/// `validateRetryDelayMs(delayMs, options)`
pub fn validate_retry_delay_ms(
    delay_ms: f64,
    max_retry_delay_ms: Option<u64>,
) -> Result<f64, CodexError> {
    let max_retry_delay_ms = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS);
    if max_retry_delay_ms > 0 && delay_ms > max_retry_delay_ms as f64 {
        return Err(CodexError::RetryDelayExceeded {
            message: format!(
                "Server requested {}s retry delay (max: {}s)",
                (delay_ms / 1000.0).ceil(),
                (max_retry_delay_ms as f64 / 1000.0).ceil()
            ),
        });
    }
    Ok(delay_ms)
}

/// `normalizeTimeoutMs(value)`
pub fn normalize_timeout_ms(value: Option<u64>) -> Option<u64> {
    value
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

fn model_supports_grammar_tools(model: &Model) -> bool {
    match &model.compat {
        Some(crate::types::ModelCompat::OpenAIResponses(compat)) => {
            compat.supports_openai_grammar_tools.unwrap_or(false)
        }
        _ => false,
    }
}

fn model_supports_strict_mode(model: &Model) -> bool {
    match &model.compat {
        Some(crate::types::ModelCompat::OpenAIResponses(compat)) => {
            compat.supports_strict_mode.unwrap_or(true)
        }
        _ => true,
    }
}

fn model_deferred_tools_mode(model: &Model) -> Option<ResponsesDeferredToolsMode> {
    let compat = match &model.compat {
        Some(crate::types::ModelCompat::OpenAIResponses(compat)) => compat,
        _ => return None,
    };
    if compat.supports_additional_tools == Some(true) {
        Some(ResponsesDeferredToolsMode::AdditionalTools)
    } else if compat.supports_tool_search == Some(true) {
        Some(ResponsesDeferredToolsMode::ToolSearch)
    } else {
        None
    }
}

/// `buildRequestBody(model, context, options, cacheSessionId, grammarToolInputProperties)`
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: &OpenAICodexResponsesOptions,
    cache_session_id: Option<&str>,
    grammar_tool_input_properties: &BTreeMap<String, String>,
    timestamp: i64,
) -> Result<Value, crate::api::constrained_sampling::ConstrainedSamplingError> {
    let supports_strict_mode = model_supports_strict_mode(model);
    let supports_openai_grammar_tools = model_supports_grammar_tools(model);
    let deferred_tools_mode = model_deferred_tools_mode(model);
    let tool_placement = split_deferred_tools(context, deferred_tools_mode.is_some(), |name| {
        name.to_string()
    });
    let tool_options = ConvertResponsesToolsOptions {
        // An explicit `null` here means "no strict flag unless a tool demands one".
        strict: Some(None),
        supports_strict_mode: Some(supports_strict_mode),
        supports_openai_grammar_tools: Some(supports_openai_grammar_tools),
        defer_loading: false,
    };
    let messages = convert_responses_messages(
        model,
        context,
        &codex_tool_call_providers(),
        &ConvertResponsesMessagesOptions {
            include_system_prompt: Some(false),
            grammar_tool_input_properties: Some(grammar_tool_input_properties),
            deferred_tools: Some(&tool_placement.deferred),
            deferred_tools_mode,
            tool_options: Some(tool_options.clone()),
        },
        timestamp,
    )?;

    let mut body = Map::new();
    body.insert("model".to_string(), json!(model.id));
    body.insert("store".to_string(), json!(false));
    body.insert("stream".to_string(), json!(true));
    body.insert(
        "instructions".to_string(),
        json!(
            context
                .system_prompt
                .as_deref()
                .filter(|prompt| !prompt.is_empty())
                .unwrap_or("You are a helpful assistant.")
        ),
    );
    body.insert("input".to_string(), Value::Array(messages));
    body.insert(
        "text".to_string(),
        json!({
            "verbosity": options
                .text_verbosity
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or("low")
        }),
    );
    body.insert(
        "include".to_string(),
        json!(["reasoning.encrypted_content"]),
    );
    if let Some(cache_session_id) = cache_session_id {
        body.insert("prompt_cache_key".to_string(), json!(cache_session_id));
    }
    body.insert(
        "tool_choice".to_string(),
        json!(options.tool_choice.as_deref().unwrap_or("auto")),
    );
    body.insert("parallel_tool_calls".to_string(), json!(true));

    if let Some(temperature) = options.temperature {
        body.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(service_tier) = &options.service_tier {
        body.insert("service_tier".to_string(), json!(service_tier));
    }
    if !tool_placement.immediate.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(convert_responses_tools(
                &tool_placement.immediate,
                &tool_options,
            )?),
        );
    }

    if let Some(reasoning_effort) = options.reasoning_effort {
        let level = match reasoning_effort {
            CodexReasoningEffort::None => ModelThinkingLevel::Off,
            CodexReasoningEffort::Level(level) => level.into(),
        };
        // `?? fallback` swallows both a missing entry and an explicit null, so the
        // `effort !== null` guard in TS can never fire.
        let effort = model
            .thinking_level_map
            .as_ref()
            .and_then(|map| map.get(&level))
            .cloned()
            .flatten()
            .unwrap_or_else(|| {
                match reasoning_effort {
                    CodexReasoningEffort::None => "none",
                    other => other.as_str(),
                }
                .to_string()
            });
        let summary = options
            .reasoning_summary
            .clone()
            .flatten()
            .unwrap_or_else(|| "auto".to_string());
        body.insert(
            "reasoning".to_string(),
            json!({ "effort": effort, "summary": summary }),
        );
    }

    Ok(Value::Object(body))
}

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

/// `resolveCodexServiceTier(responseServiceTier, requestServiceTier)`
pub fn resolve_codex_service_tier(
    response_service_tier: Option<&str>,
    request_service_tier: Option<&str>,
) -> Option<String> {
    if response_service_tier == Some("default")
        && matches!(request_service_tier, Some("flex") | Some("priority"))
    {
        return request_service_tier.map(str::to_string);
    }
    response_service_tier
        .map(str::to_string)
        .or_else(|| request_service_tier.map(str::to_string))
}

/// `resolveCodexUrl(baseUrl)`
pub fn resolve_codex_url(base_url: &str) -> String {
    let raw = if base_url.trim().is_empty() {
        DEFAULT_CODEX_BASE_URL
    } else {
        base_url
    };
    let normalized = raw.trim_end_matches('/');
    if normalized.ends_with("/codex/responses") {
        return normalized.to_string();
    }
    if normalized.ends_with("/codex") {
        return format!("{normalized}/responses");
    }
    format!("{normalized}/codex/responses")
}

/// `resolveCodexWebSocketUrl(baseUrl)`
pub fn resolve_codex_websocket_url(base_url: &str) -> Result<String, CodexError> {
    let mut url = Url::parse(&resolve_codex_url(base_url))
        .map_err(|error| CodexError::transport(error.to_string()))?;
    match url.scheme() {
        "https" => {
            let _ = url.set_scheme("wss");
        }
        "http" => {
            let _ = url.set_scheme("ws");
        }
        _ => {}
    }
    Ok(url.to_string())
}

// ---------------------------------------------------------------------------
// Auth and headers
// ---------------------------------------------------------------------------

/// `extractAccountId(token)`
pub fn extract_account_id(token: &str) -> Result<String, CodexError> {
    let failed = || CodexError::transport("Failed to extract accountId from token");
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(failed());
    }
    // `atob` decodes standard base64 without padding requirements.
    let decoded = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(parts[1].trim_end_matches('='))
        .map_err(|_| failed())?;
    let payload: Value = serde_json::from_slice(&decoded).map_err(|_| failed())?;
    payload
        .get(JWT_CLAIM_PATH)
        .and_then(|claim| claim.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .filter(|account_id| !account_id.is_empty())
        .map(str::to_string)
        .ok_or_else(failed)
}

/// Header list in insertion order, with the `Headers` set/delete semantics of TS.
#[derive(Debug, Clone, Default)]
pub struct CodexHeaders(Vec<(String, String)>);

impl CodexHeaders {
    fn set(&mut self, name: &str, value: &str) {
        // `Headers.set` replaces in place, keeping the original position.
        if let Some(entry) = self
            .0
            .iter_mut()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
        {
            entry.1 = value.to_string();
            return;
        }
        self.0.push((name.to_lowercase(), value.to_string()));
    }

    fn delete(&mut self, name: &str) {
        self.0.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn into_pairs(self) -> Vec<(String, String)> {
        self.0
    }

    pub fn pairs(&self) -> &[(String, String)] {
        &self.0
    }
}

/// `buildBaseCodexHeaders(initHeaders, additionalHeaders, accountId, token)`
fn build_base_codex_headers(
    init_headers: Option<&BTreeMap<String, String>>,
    additional_headers: Option<&ProviderHeaders>,
    account_id: &str,
    token: &str,
    user_agent: &str,
) -> CodexHeaders {
    let mut headers = CodexHeaders::default();
    for (key, value) in init_headers.into_iter().flatten() {
        headers.set(key, value);
    }
    for (key, value) in additional_headers.into_iter().flatten() {
        match value {
            None => headers.delete(key),
            Some(value) => headers.set(key, value),
        }
    }
    headers.set("Authorization", &format!("Bearer {token}"));
    headers.set("chatgpt-account-id", account_id);
    headers.set("originator", "notagent");
    headers.set("User-Agent", user_agent);
    headers
}

/// `notagent (<platform> <release>; <arch>)`, the string `node:os` produces.
pub fn codex_user_agent() -> String {
    // TS reads `os.platform()`, `os.release()` and `os.arch()`; the Rust equivalents are
    // compile-time constants plus the kernel release from uname.
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("notagent ({platform} {}; {arch})", kernel_release())
}

fn kernel_release() -> String {
    // `os.release()` is the kernel version; `uname -r` is the portable way to read it.
    std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|release| release.trim().to_string())
        .filter(|release| !release.is_empty())
        .unwrap_or_default()
}

/// `buildSSEHeaders(...)`
pub fn build_sse_headers(
    init_headers: Option<&BTreeMap<String, String>>,
    additional_headers: Option<&ProviderHeaders>,
    account_id: &str,
    token: &str,
    session_id: Option<&str>,
    user_agent: &str,
) -> CodexHeaders {
    let mut headers = build_base_codex_headers(
        init_headers,
        additional_headers,
        account_id,
        token,
        user_agent,
    );
    headers.set("OpenAI-Beta", "responses=experimental");
    headers.set("accept", "text/event-stream");
    headers.set("content-type", "application/json");
    if let Some(session_id) = session_id.filter(|value| !value.is_empty()) {
        headers.set("session-id", session_id);
        headers.set("x-client-request-id", session_id);
    }
    headers
}

/// `buildWebSocketHeaders(...)`
pub fn build_websocket_headers(
    init_headers: Option<&BTreeMap<String, String>>,
    additional_headers: Option<&ProviderHeaders>,
    account_id: &str,
    token: &str,
    request_id: &str,
    user_agent: &str,
) -> CodexHeaders {
    let mut headers = build_base_codex_headers(
        init_headers,
        additional_headers,
        account_id,
        token,
        user_agent,
    );
    headers.delete("accept");
    headers.delete("content-type");
    headers.delete("OpenAI-Beta");
    headers.delete("openai-beta");
    headers.set("OpenAI-Beta", OPENAI_BETA_RESPONSES_WEBSOCKETS);
    headers.set("x-client-request-id", request_id);
    headers.set("session-id", request_id);
    headers
}

// ---------------------------------------------------------------------------
// Error responses
// ---------------------------------------------------------------------------

/// `parseErrorResponse(response)`
pub fn parse_error_response(
    status: u16,
    status_text: &str,
    raw: &str,
    now_ms: i64,
) -> (String, Option<String>) {
    let mut message = if !raw.is_empty() {
        raw.to_string()
    } else if !status_text.is_empty() {
        status_text.to_string()
    } else {
        "Request failed".to_string()
    };
    let mut friendly_message: Option<String> = None;

    if let Ok(parsed) = serde_json::from_str::<Value>(raw)
        && let Some(error) = parsed.get("error").filter(|error| !error.is_null())
    {
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .filter(|code| !code.is_empty())
            .or_else(|| {
                error
                    .get("type")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or("");
        static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
        let pattern = PATTERN.get_or_init(|| {
            regex::Regex::new(r"(?i)usage_limit_reached|usage_not_included|rate_limit_exceeded")
                .expect("valid pattern")
        });
        if pattern.is_match(code) || status == 429 {
            let plan = error
                .get("plan_type")
                .and_then(Value::as_str)
                .filter(|plan| !plan.is_empty())
                .map(|plan| format!(" ({} plan)", plan.to_lowercase()))
                .unwrap_or_default();
            let when = error
                .get("resets_at")
                .and_then(Value::as_f64)
                .filter(|resets_at| *resets_at != 0.0)
                .map(|resets_at| {
                    let minutes = (((resets_at * 1000.0) - now_ms as f64) / 60000.0)
                        .round()
                        .max(0.0);
                    format!(" Try again in ~{minutes} min.")
                })
                .unwrap_or_default();
            friendly_message = Some(
                format!("You have hit your ChatGPT usage limit{plan}.{when}")
                    .trim()
                    .to_string(),
            );
        }
        if let Some(error_message) = error
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.is_empty())
        {
            message = error_message.to_string();
        } else if let Some(friendly) = &friendly_message {
            message = friendly.clone();
        }
    }

    (message, friendly_message)
}

// ---------------------------------------------------------------------------
// Codex event mapping
// ---------------------------------------------------------------------------

/// `normalizeCodexStatus(status)`
fn normalize_codex_status(status: Option<&Value>) -> Option<String> {
    let status = status?.as_str()?;
    CODEX_RESPONSE_STATUSES
        .contains(&status)
        .then(|| status.to_string())
}

/// `extractCodexEventError(event)`
fn extract_codex_event_error(event: &Value) -> (Option<String>, Option<String>) {
    let nested = event.get("error").filter(|error| error.is_object());
    let pick = |key: &str| {
        event
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                nested
                    .and_then(|nested| nested.get(key))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    };
    (pick("code"), pick("message"))
}

/// The outcome of mapping one Codex event onto a Responses event.
enum MappedEvent {
    /// Forward this event to the shared stream state.
    Forward(Value),
    /// The terminal event: forward it and stop reading.
    Terminal(Value),
    /// Nothing to forward (an event without a type).
    Skip,
}

/// `mapCodexEvents(events, output)` for a single event.
fn map_codex_event(
    event: &Value,
    output: &mut AssistantMessage,
) -> Result<MappedEvent, CodexError> {
    let Some(event_type) = event.get("type").and_then(Value::as_str) else {
        return Ok(MappedEvent::Skip);
    };

    if event_type == "error" {
        let (code, message) = extract_codex_event_error(event);
        let detail = message
            .clone()
            .or_else(|| code.clone())
            .unwrap_or_else(|| event.to_string());
        return Err(CodexError::Api {
            message: format!("Codex error: {detail}"),
            code,
        });
    }

    if event_type == "response.failed" {
        let response = event.get("response");
        let error = response.and_then(|response| response.get("error"));
        let code = error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let message = error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .filter(|message| !message.is_empty())
            .map(str::to_string);
        return Err(CodexError::Api {
            message: message.unwrap_or_else(|| "Codex response failed".to_string()),
            code,
        });
    }

    if matches!(
        event_type,
        "response.done" | "response.completed" | "response.incomplete"
    ) {
        let mut mapped = event.clone();
        if let Some(response) = event.get("response") {
            if let Some(end_turn) = response.get("end_turn").and_then(Value::as_bool) {
                output.end_turn = Some(end_turn);
            }
            let mut normalized = response.clone();
            if let Some(object) = normalized.as_object_mut() {
                match normalize_codex_status(response.get("status")) {
                    Some(status) => {
                        object.insert("status".to_string(), json!(status));
                    }
                    // `status: undefined` is dropped by the object spread.
                    None => {
                        object.remove("status");
                    }
                }
            }
            if let Some(object) = mapped.as_object_mut() {
                object.insert("response".to_string(), normalized);
            }
        }
        if let Some(object) = mapped.as_object_mut() {
            object.insert("type".to_string(), json!("response.completed"));
        }
        return Ok(MappedEvent::Terminal(mapped));
    }

    Ok(MappedEvent::Forward(event.clone()))
}

// ---------------------------------------------------------------------------
// SSE parsing
// ---------------------------------------------------------------------------

/// `parseSSE(response, signal)` — the Codex-specific parser, not the shared decoder.
///
/// It splits on `\n\n`, keeps only `data:` lines and joins them with newlines. Unlike a
/// spec SSE decoder it never looks at `event:` names.
#[derive(Debug, Default)]
pub struct CodexSseParser {
    buffer: String,
}

impl CodexSseParser {
    pub fn new() -> Self {
        CodexSseParser::default()
    }

    /// Feeds a chunk and returns the events it completed.
    pub fn feed(&mut self, chunk: &str) -> Result<Vec<Value>, CodexError> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();
        while let Some(index) = self.buffer.find("\n\n") {
            let chunk = self.buffer[..index].to_string();
            self.buffer = self.buffer[index + 2..].to_string();

            let data_lines: Vec<&str> = chunk
                .split('\n')
                .filter(|line| line.starts_with("data:"))
                .map(|line| line[5..].trim())
                .collect();
            if data_lines.is_empty() {
                continue;
            }
            let data = data_lines.join("\n");
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let parsed =
                serde_json::from_str::<Value>(data).map_err(|cause| CodexError::Protocol {
                    message: format!(
                        "Invalid Codex SSE JSON: {}",
                        format_thrown_value(&cause.to_string())
                    ),
                })?;
            events.push(parsed);
        }
        Ok(events)
    }
}

// ---------------------------------------------------------------------------
// Request compression
// ---------------------------------------------------------------------------

/// `compressRequestBodyZstd(bodyJson)`
pub fn compress_request_body_zstd(body_json: &str) -> Option<Vec<u8>> {
    zstd::stream::encode_all(body_json.as_bytes(), REQUEST_COMPRESSION_ZSTD_LEVEL).ok()
}

// ---------------------------------------------------------------------------
// WebSocket session cache and debug stats
// ---------------------------------------------------------------------------

/// `OpenAICodexWebSocketDebugStats`
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAICodexWebSocketDebugStats {
    pub requests: u64,
    pub connections_created: u64,
    pub connections_reused: u64,
    pub cached_context_requests: u64,
    pub store_true_requests: u64,
    pub full_context_requests: u64,
    pub delta_requests: u64,
    pub last_input_items: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delta_input_items: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_previous_response_id: Option<String>,
    pub websocket_failures: u64,
    pub sse_fallbacks: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket_fallback_active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_websocket_error: Option<String>,
}

/// `CachedWebSocketContinuationState`
#[derive(Debug, Clone)]
struct ContinuationState {
    last_request_body: Value,
    last_response_id: String,
    last_response_items: Vec<Value>,
}

type WebSocketStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// `CachedWebSocketConnection` — the socket itself is cached alongside the continuation.
/// Codex forces `store: false`, so `previous_response_id` state only exists on the
/// server for the lifetime of the WebSocket connection; a continuation replayed over a
/// fresh connection is rejected with "Invalid `previous_response_id`".
struct CachedWebSocketConnection {
    /// `None` while a request has the socket checked out (TS `busy: true`).
    socket: Option<WebSocketStream>,
    continuation: Option<ContinuationState>,
    /// `entry.createdAt` — the age limit closes a connection after 55 minutes.
    created_at: i64,
    /// When the entry was last released; TS arms an idle timer here, the Rust port
    /// checks the timestamps lazily on the next acquire (substitution class 3: an
    /// idle-expired socket lingers as an open TCP connection until then).
    last_used_at: i64,
    /// Guards release against an entry that was cleaned up and recreated meanwhile.
    generation: u64,
}

#[derive(Default)]
struct CodexWebSocketState {
    /// sessionId → accountId → cached connection (TS `websocketSessionCache`).
    connections: BTreeMap<String, BTreeMap<String, CachedWebSocketConnection>>,
    next_generation: u64,
    debug_stats: BTreeMap<String, OpenAICodexWebSocketDebugStats>,
    sse_fallback_sessions: BTreeSet<String>,
}

fn websocket_state() -> &'static Mutex<CodexWebSocketState> {
    static STATE: OnceLock<Mutex<CodexWebSocketState>> = OnceLock::new();
    STATE.get_or_init(|| {
        // TS registers the cache cleanup at module load; the Rust port does it on first
        // use, which is the earliest point the state exists.
        crate::session_resources::register_session_resource_cleanup(Arc::new(
            |session_id: Option<&str>| {
                close_openai_codex_websocket_sessions(session_id);
            },
        ));
        Mutex::new(CodexWebSocketState::default())
    })
}

/// `getOpenAICodexWebSocketDebugStats(sessionId)`
pub fn openai_codex_websocket_debug_stats(
    session_id: &str,
) -> Option<OpenAICodexWebSocketDebugStats> {
    websocket_state()
        .lock()
        .expect("poisoned")
        .debug_stats
        .get(session_id)
        .cloned()
}

/// `resetOpenAICodexWebSocketDebugStats(sessionId?)`
pub fn reset_openai_codex_websocket_debug_stats(session_id: Option<&str>) {
    let mut state = websocket_state().lock().expect("poisoned");
    match session_id {
        Some(session_id) => {
            state.debug_stats.remove(session_id);
            state.sse_fallback_sessions.remove(session_id);
        }
        None => {
            state.debug_stats.clear();
            state.sse_fallback_sessions.clear();
        }
    }
}

/// `closeOpenAICodexWebSocketSessions(sessionId?)` — dropping an entry closes its TCP
/// connection; the close frame TS sends ("debug_close") is skipped in the sync path.
pub fn close_openai_codex_websocket_sessions(session_id: Option<&str>) {
    let mut state = websocket_state().lock().expect("poisoned");
    match session_id {
        Some(session_id) => {
            state.connections.remove(session_id);
        }
        None => state.connections.clear(),
    }
}

/// `isWebSocketSseFallbackActive(sessionId)`
fn is_websocket_sse_fallback_active(session_id: Option<&str>) -> bool {
    let Some(session_id) = session_id else {
        return false;
    };
    websocket_state()
        .lock()
        .expect("poisoned")
        .sse_fallback_sessions
        .contains(session_id)
}

/// `recordWebSocketSseFallback(sessionId)`
fn record_websocket_sse_fallback(session_id: Option<&str>) {
    let Some(session_id) = session_id else {
        return;
    };
    let mut state = websocket_state().lock().expect("poisoned");
    let active = state.sse_fallback_sessions.contains(session_id);
    let stats = state.debug_stats.entry(session_id.to_string()).or_default();
    stats.sse_fallbacks += 1;
    stats.websocket_fallback_active = Some(active);
}

/// `recordWebSocketFailure(sessionId, error)`
fn record_websocket_failure(session_id: Option<&str>, error: &CodexError) {
    let Some(session_id) = session_id else {
        return;
    };
    let mut state = websocket_state().lock().expect("poisoned");
    state.sse_fallback_sessions.insert(session_id.to_string());
    let stats = state.debug_stats.entry(session_id.to_string()).or_default();
    stats.websocket_failures += 1;
    stats.last_websocket_error = Some(format_thrown_value(&error.message().to_string()));
    stats.websocket_fallback_active = Some(true);
}

fn record_websocket_request(
    session_id: Option<&str>,
    reused: bool,
    use_cached_context: bool,
    request_body: &Value,
) {
    let Some(session_id) = session_id else {
        return;
    };
    let mut state = websocket_state().lock().expect("poisoned");
    let stats = state.debug_stats.entry(session_id.to_string()).or_default();
    stats.requests += 1;
    if reused {
        stats.connections_reused += 1;
    } else {
        stats.connections_created += 1;
    }
    if use_cached_context {
        stats.cached_context_requests += 1;
    }
    if request_body.get("store") == Some(&Value::Bool(true)) {
        stats.store_true_requests += 1;
    }
    let input_items = request_body
        .get("input")
        .and_then(Value::as_array)
        .map(|input| input.len() as u64)
        .unwrap_or(0);
    stats.last_input_items = input_items;
    match request_body
        .get("previous_response_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        Some(previous_response_id) => {
            stats.delta_requests += 1;
            stats.last_delta_input_items = Some(input_items);
            stats.last_previous_response_id = Some(previous_response_id.to_string());
        }
        None => {
            stats.full_context_requests += 1;
            stats.last_delta_input_items = None;
            stats.last_previous_response_id = None;
        }
    }
}

/// `requestBodyWithoutInput(body)`
fn request_body_without_input(body: &Value) -> Value {
    let mut copy = body.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("input");
        object.remove("previous_response_id");
    }
    copy
}

/// `requestBodiesMatchExceptInput(a, b)`
fn request_bodies_match_except_input(a: &Value, b: &Value) -> bool {
    request_body_without_input(a) == request_body_without_input(b)
}

/// `getCachedWebSocketInputDelta(body, continuation)`
fn cached_websocket_input_delta(
    body: &Value,
    continuation: &ContinuationState,
) -> Option<Vec<Value>> {
    if !request_bodies_match_except_input(body, &continuation.last_request_body) {
        return None;
    }
    let current_input: Vec<Value> = body
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut baseline: Vec<Value> = continuation
        .last_request_body
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    baseline.extend(continuation.last_response_items.clone());
    if current_input.len() < baseline.len() {
        return None;
    }
    if current_input[..baseline.len()] != baseline[..] {
        return None;
    }
    Some(current_input[baseline.len()..].to_vec())
}

/// `buildCachedWebSocketRequestBody(entry, body)`
fn build_cached_websocket_request_body(
    continuation: Option<&ContinuationState>,
    body: &Value,
) -> (Value, bool) {
    let Some(continuation) = continuation else {
        return (body.clone(), true);
    };
    let Some(delta) = cached_websocket_input_delta(body, continuation) else {
        return (body.clone(), false);
    };
    if continuation.last_response_id.is_empty() {
        return (body.clone(), false);
    }
    let mut cached = body.clone();
    if let Some(object) = cached.as_object_mut() {
        object.insert(
            "previous_response_id".to_string(),
            json!(continuation.last_response_id),
        );
        object.insert("input".to_string(), Value::Array(delta));
    }
    (cached, true)
}

/// The result of `acquireWebSocket`: the socket, whether it came from the cache, the
/// continuation taken from the reused entry, and the cache slot the release settles.
struct AcquiredWebSocket {
    socket: WebSocketStream,
    reused: bool,
    continuation: Option<ContinuationState>,
    /// `Some((session, account, generation))` when the socket belongs to a cache entry.
    cache_key: Option<(String, String, u64)>,
}

/// `isWebSocketReusable(socket)` — TS reads `readyState`; the Rust port polls the
/// stream once without blocking: a pending close frame, error, or EOF means the server
/// already gave up on the connection.
fn websocket_is_reusable(socket: &mut WebSocketStream) -> bool {
    use futures::{FutureExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    loop {
        match socket.next().now_or_never() {
            None => return true,
            Some(Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_)))) => continue,
            _ => return false,
        }
    }
}

fn remove_cached_connection(session_id: &str, account_id: &str, generation: Option<u64>) {
    let mut state = websocket_state().lock().expect("poisoned");
    if let Some(accounts) = state.connections.get_mut(session_id) {
        let matches = accounts
            .get(account_id)
            .is_some_and(|entry| generation.is_none_or(|generation| entry.generation == generation));
        if matches {
            accounts.remove(account_id);
        }
        if accounts.is_empty() {
            state.connections.remove(session_id);
        }
    }
}

/// `acquireWebSocket(url, headers, sessionId, accountId, ...)`
async fn acquire_websocket(
    url: &str,
    headers: &CodexHeaders,
    request: &ProviderRequestOptions,
    connect_timeout_ms: Option<u64>,
    session_id: Option<&str>,
    account_id: &str,
    now_ms: i64,
) -> Result<AcquiredWebSocket, CodexError> {
    let Some(session_id) = session_id else {
        let socket = connect_websocket(url, headers, request, connect_timeout_ms).await?;
        return Ok(AcquiredWebSocket {
            socket,
            reused: false,
            continuation: None,
            cache_key: None,
        });
    };

    enum CacheLookup {
        /// The entry's socket, checked out; the busy placeholder stays in the map.
        Checked {
            socket: Box<WebSocketStream>,
            continuation: Option<ContinuationState>,
            generation: u64,
        },
        /// Another request has the socket checked out — connect uncached (TS `busy`).
        Busy,
        Vacant,
    }

    let lookup = {
        let mut state = websocket_state().lock().expect("poisoned");
        let mut expired_entry = false;
        let lookup = match state
            .connections
            .get_mut(session_id)
            .and_then(|accounts| accounts.get_mut(account_id))
        {
            None => CacheLookup::Vacant,
            Some(entry) if entry.socket.is_none() => CacheLookup::Busy,
            Some(entry) => {
                // `isWebSocketSessionExpired(entry)` plus the idle timer TS schedules
                // on release, both checked lazily here.
                let expired = now_ms - entry.created_at >= SESSION_WEBSOCKET_MAX_AGE_MS as i64
                    || now_ms - entry.last_used_at >= SESSION_WEBSOCKET_CACHE_TTL_MS as i64;
                if expired {
                    expired_entry = true;
                    CacheLookup::Vacant
                } else {
                    match entry.socket.take() {
                        Some(socket) => CacheLookup::Checked {
                            socket: Box::new(socket),
                            continuation: entry.continuation.take(),
                            generation: entry.generation,
                        },
                        None => CacheLookup::Busy,
                    }
                }
            }
        };
        if expired_entry
            && let Some(accounts) = state.connections.get_mut(session_id)
        {
            accounts.remove(account_id);
            if accounts.is_empty() {
                state.connections.remove(session_id);
            }
        }
        lookup
    };

    match lookup {
        CacheLookup::Checked {
            mut socket,
            continuation,
            generation,
        } => {
            if websocket_is_reusable(&mut socket) {
                return Ok(AcquiredWebSocket {
                    socket: *socket,
                    reused: true,
                    continuation,
                    cache_key: Some((session_id.to_string(), account_id.to_string(), generation)),
                });
            }
            let _ = socket.close(None).await;
            remove_cached_connection(session_id, account_id, Some(generation));
            // Fall through to a fresh cached connection, like the TS fallthrough.
        }
        CacheLookup::Busy => {
            let socket = connect_websocket(url, headers, request, connect_timeout_ms).await?;
            return Ok(AcquiredWebSocket {
                socket,
                reused: false,
                continuation: None,
                cache_key: None,
            });
        }
        CacheLookup::Vacant => {}
    }

    // TS registers the entry after the connect resolves; the Rust port inserts a busy
    // placeholder first so a concurrent request goes uncached instead of racing the slot.
    let generation = {
        let mut state = websocket_state().lock().expect("poisoned");
        state.next_generation += 1;
        let generation = state.next_generation;
        state.connections.entry(session_id.to_string()).or_default().insert(
            account_id.to_string(),
            CachedWebSocketConnection {
                socket: None,
                continuation: None,
                created_at: now_ms,
                last_used_at: now_ms,
                generation,
            },
        );
        generation
    };
    match connect_websocket(url, headers, request, connect_timeout_ms).await {
        Ok(socket) => Ok(AcquiredWebSocket {
            socket,
            reused: false,
            continuation: None,
            cache_key: Some((session_id.to_string(), account_id.to_string(), generation)),
        }),
        Err(error) => {
            remove_cached_connection(session_id, account_id, Some(generation));
            Err(error)
        }
    }
}

/// The `release({ keep })` closure from TS `acquireWebSocket`, plus settling the
/// entry's continuation while the slot is written back.
async fn release_websocket(
    cache_key: Option<(String, String, u64)>,
    mut socket: WebSocketStream,
    keep: bool,
    continuation: Option<ContinuationState>,
    now_ms: i64,
) {
    let Some((session_id, account_id, generation)) = cache_key else {
        let _ = socket.close(None).await;
        return;
    };
    if !keep {
        let _ = socket.close(None).await;
        remove_cached_connection(&session_id, &account_id, Some(generation));
        return;
    }
    let stored = {
        let mut state = websocket_state().lock().expect("poisoned");
        match state
            .connections
            .get_mut(&session_id)
            .and_then(|accounts| accounts.get_mut(&account_id))
            .filter(|entry| entry.generation == generation)
        {
            Some(entry) => {
                entry.socket = Some(socket);
                entry.continuation = continuation;
                entry.last_used_at = now_ms;
                None
            }
            // The entry was cleaned up while checked out — close instead of caching.
            None => Some(socket),
        }
    };
    if let Some(mut socket) = stored {
        let _ = socket.close(None).await;
    }
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

fn empty_output(model: &Model, timestamp: i64) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        // TS pins the api to the literal, independent of `model.api`.
        api: "openai-codex-responses".to_string(),
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

/// `assertSuccessfulOutput(output)`
fn assert_successful_output(output: &AssistantMessage) -> Result<DoneReason, CodexError> {
    if output.stop_reason == StopReason::Pending {
        return Err(CodexError::transport(
            "Codex stream ended without a stop reason",
        ));
    }
    if output.stop_reason == StopReason::Error || output.stop_reason == StopReason::Aborted {
        return Err(CodexError::transport(
            output
                .error_message
                .clone()
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| "An unknown error occurred".to_string()),
        ));
    }
    Ok(match output.stop_reason {
        StopReason::Length => DoneReason::Length,
        StopReason::ToolUse => DoneReason::ToolUse,
        StopReason::Deferred => DoneReason::Deferred,
        _ => DoneReason::Stop,
    })
}

/// Feeds decoded Codex events into the shared Responses state machine.
struct CodexEventPump<'a> {
    state: ResponsesStreamState,
    options: ResponsesStreamOptions<'a>,
    saw_terminal: bool,
}

impl<'a> CodexEventPump<'a> {
    fn new(state: ResponsesStreamState, options: ResponsesStreamOptions<'a>) -> Self {
        CodexEventPump {
            state,
            options,
            saw_terminal: false,
        }
    }

    /// Returns the events to push; sets `saw_terminal` once the stream is complete.
    fn push(&mut self, event: &Value) -> Result<Vec<AssistantMessageEvent>, CodexError> {
        let mut output = std::mem::replace(&mut self.state.output, empty_placeholder());
        let mapped = map_codex_event(event, &mut output);
        self.state.output = output;
        match mapped? {
            MappedEvent::Skip => Ok(Vec::new()),
            MappedEvent::Forward(event) => self
                .state
                .process_event(&event, &self.options)
                .map_err(|error| CodexError::transport(error.to_string())),
            MappedEvent::Terminal(event) => {
                self.saw_terminal = true;
                self.state
                    .process_event(&event, &self.options)
                    .map_err(|error| CodexError::transport(error.to_string()))
            }
        }
    }
}

/// A throwaway message used while `output` is moved out for the event mapping.
fn empty_placeholder() -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Pending,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

/// `streamSimple(model, context, options)` — errors without an api key, like TS.
pub fn stream_simple(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
) -> Result<AssistantMessageEventStream, CodexError> {
    let options = options.unwrap_or_default();
    let Some(api_key) = options
        .base
        .base
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
    else {
        return Err(CodexError::transport(format!(
            "No API key for provider: {}",
            model.provider
        )));
    };

    let base = build_base_options(&model, &context, Some(&options), Some(api_key.to_string()));
    let clamped_reasoning = options
        .reasoning
        .map(|reasoning| clamp_thinking_level(&model, reasoning.into()));
    let reasoning_effort = match clamped_reasoning {
        Some(ModelThinkingLevel::Minimal) => {
            Some(CodexReasoningEffort::Level(ThinkingLevel::Minimal))
        }
        Some(ModelThinkingLevel::Low) => Some(CodexReasoningEffort::Level(ThinkingLevel::Low)),
        Some(ModelThinkingLevel::Medium) => {
            Some(CodexReasoningEffort::Level(ThinkingLevel::Medium))
        }
        Some(ModelThinkingLevel::High) => Some(CodexReasoningEffort::Level(ThinkingLevel::High)),
        Some(ModelThinkingLevel::Xhigh) => Some(CodexReasoningEffort::Level(ThinkingLevel::Xhigh)),
        Some(ModelThinkingLevel::Max) => Some(CodexReasoningEffort::Level(ThinkingLevel::Max)),
        Some(ModelThinkingLevel::Off) | None => None,
    };

    Ok(stream(
        model,
        context,
        base.base.clone(),
        OpenAICodexResponsesOptions {
            reasoning_effort,
            reasoning_summary: None,
            service_tier: None,
            text_verbosity: None,
            tool_choice: None,
            temperature: base.temperature,
            transport: base.transport,
            cache_retention: base.cache_retention,
            session_id: base.session_id.clone(),
            websocket_connect_timeout_ms: base.websocket_connect_timeout_ms,
            env: base.base.env.clone(),
        },
    ))
}

/// `stream(model, context, options)`
pub fn stream(
    model: Model,
    context: Context,
    request: ProviderRequestOptions,
    options: OpenAICodexResponsesOptions,
) -> AssistantMessageEventStream {
    let outer = create_assistant_message_event_stream();
    let stream = outer.clone();

    tokio::spawn(async move {
        let timestamp = crate::auth::resolve::now_ms();
        let mut output = empty_output(&model, timestamp);
        let outcome = run(
            &model,
            &context,
            &request,
            &options,
            &mut output,
            &stream,
            timestamp,
        )
        .await;

        match outcome {
            Ok(reason) => {
                stream.push(AssistantMessageEvent::Done {
                    reason,
                    message: output.clone(),
                });
                stream.end(Some(output));
            }
            Err(error) => {
                let aborted = request
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.is_cancelled());
                output.stop_reason = if aborted {
                    StopReason::Aborted
                } else {
                    StopReason::Error
                };
                output.error_message = Some(format_provider_error(
                    &normalize_provider_error(&RawProviderError {
                        status: None,
                        body_text: None,
                        body_json: None,
                        message: error.message().to_string(),
                    }),
                    None,
                ));
                stream.push(AssistantMessageEvent::Error {
                    reason: if aborted {
                        ErrorReason::Aborted
                    } else {
                        ErrorReason::Error
                    },
                    error: output.clone(),
                });
                stream.end(Some(output));
            }
        }
    });

    outer
}

#[allow(clippy::too_many_arguments)]
async fn run(
    model: &Model,
    context: &Context,
    request: &ProviderRequestOptions,
    options: &OpenAICodexResponsesOptions,
    output: &mut AssistantMessage,
    stream: &AssistantMessageEventStream,
    timestamp: i64,
) -> Result<DoneReason, CodexError> {
    let Some(api_key) = request.api_key.as_deref().filter(|key| !key.is_empty()) else {
        return Err(CodexError::transport(format!(
            "No API key for provider: {}",
            model.provider
        )));
    };
    let account_id = extract_account_id(api_key)?;
    let grammar_tool_input_properties = create_grammar_tool_input_properties(
        context.tools.as_deref(),
        model_supports_grammar_tools(model),
    )
    .map_err(|error| CodexError::transport(error.to_string()))?;

    let cache_session_id = if options.cache_retention == Some(CacheRetention::None) {
        None
    } else {
        options.session_id.as_deref()
    };
    let codex_session_id = clamp_openai_prompt_cache_key(cache_session_id);

    let mut body = build_request_body(
        model,
        context,
        options,
        codex_session_id.as_deref(),
        &grammar_tool_input_properties,
        timestamp,
    )
    .map_err(|error| CodexError::transport(error.to_string()))?;
    if let Some(on_payload) = &request.on_payload
        && let Some(replacement) = on_payload(body.clone(), model).await
    {
        body = replacement;
    }

    let websocket_request_id = codex_session_id.clone().unwrap_or_else(uuidv7);
    let user_agent = codex_user_agent();
    let sse_headers = build_sse_headers(
        model.headers.as_ref(),
        request.headers.as_ref(),
        &account_id,
        api_key,
        codex_session_id.as_deref(),
        &user_agent,
    );
    let websocket_headers = build_websocket_headers(
        model.headers.as_ref(),
        request.headers.as_ref(),
        &account_id,
        api_key,
        &websocket_request_id,
        &user_agent,
    );
    let body_json =
        serde_json::to_string(&body).map_err(|error| CodexError::transport(error.to_string()))?;
    let http_timeout_ms = normalize_timeout_ms(request.timeout_ms);
    let websocket_connect_timeout_ms = normalize_timeout_ms(options.websocket_connect_timeout_ms);
    let transport = options.transport.unwrap_or(Transport::Auto);
    let mut start_emitted = false;

    let websocket_disabled_for_session =
        transport != Transport::Sse && is_websocket_sse_fallback_active(cache_session_id);
    if websocket_disabled_for_session {
        record_websocket_sse_fallback(cache_session_id);
    }

    if transport != Transport::Sse && !websocket_disabled_for_session {
        let mut retried_connection_limit = false;
        let mut retried_missing_continuation = false;
        loop {
            let attempt = run_websocket_stream(
                &resolve_codex_websocket_url(&model.base_url)?,
                &body,
                &websocket_headers,
                model,
                options,
                request,
                output,
                stream,
                &grammar_tool_input_properties,
                cache_session_id,
                &account_id,
                http_timeout_ms,
                websocket_connect_timeout_ms,
                timestamp,
                &mut start_emitted,
            )
            .await;

            match attempt {
                Ok(()) => {
                    if request
                        .signal
                        .as_ref()
                        .is_some_and(|signal| signal.is_cancelled())
                    {
                        return Err(CodexError::transport("Request was aborted"));
                    }
                    return assert_successful_output(output);
                }
                Err((error, websocket_started)) => {
                    let aborted = request
                        .signal
                        .as_ref()
                        .is_some_and(|signal| signal.is_cancelled());
                    let connection_limit_before_start = !websocket_started
                        && error.code() == Some(WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE);
                    let previous_response_not_found =
                        error.code() == Some(PREVIOUS_RESPONSE_NOT_FOUND_CODE);
                    if !aborted && previous_response_not_found && !retried_missing_continuation {
                        retried_missing_continuation = true;
                        continue;
                    }
                    if !aborted && connection_limit_before_start && !retried_connection_limit {
                        retried_connection_limit = true;
                        continue;
                    }
                    if aborted || (error.is_non_transport() && !connection_limit_before_start) {
                        return Err(error);
                    }
                    let mut details = Map::new();
                    details.insert(
                        "configuredTransport".to_string(),
                        json!(transport_str(transport)),
                    );
                    if !websocket_started {
                        details.insert("fallbackTransport".to_string(), json!("sse"));
                    }
                    details.insert("eventsEmitted".to_string(), json!(websocket_started));
                    details.insert(
                        "phase".to_string(),
                        json!(if websocket_started {
                            "after_message_stream_start"
                        } else {
                            "before_message_stream_start"
                        }),
                    );
                    details.insert("requestBytes".to_string(), json!(body_json.len()));
                    append_assistant_message_diagnostic(
                        &mut output.diagnostics,
                        create_assistant_message_diagnostic(
                            "provider_transport_failure",
                            extract_diagnostic_error(&error),
                            Some(details),
                            crate::auth::resolve::now_ms(),
                        ),
                    );
                    record_websocket_failure(cache_session_id, &error);
                    if websocket_started {
                        return Err(error);
                    }
                    record_websocket_sse_fallback(cache_session_id);
                    break;
                }
            }
        }
    }

    run_sse_stream(
        model,
        options,
        request,
        output,
        stream,
        &grammar_tool_input_properties,
        sse_headers,
        &body_json,
        http_timeout_ms,
        &mut start_emitted,
    )
    .await?;

    if request
        .signal
        .as_ref()
        .is_some_and(|signal| signal.is_cancelled())
    {
        return Err(CodexError::transport("Request was aborted"));
    }
    assert_successful_output(output)
}

fn transport_str(transport: Transport) -> &'static str {
    match transport {
        Transport::Sse => "sse",
        Transport::Websocket => "websocket",
        Transport::WebsocketCached => "websocket-cached",
        Transport::Auto => "auto",
    }
}

/// Shared stream options for both transports.
fn stream_options<'a>(
    options: &OpenAICodexResponsesOptions,
    grammar_tool_input_properties: &'a BTreeMap<String, String>,
    model_id: String,
) -> ResponsesStreamOptions<'a> {
    ResponsesStreamOptions {
        service_tier: options.service_tier.clone(),
        grammar_tool_input_properties: Some(grammar_tool_input_properties),
        resolve_service_tier: Some(Box::new(
            |response_tier: Option<&str>, request_tier: Option<&str>| {
                resolve_codex_service_tier(response_tier, request_tier)
            },
        )),
        apply_service_tier_pricing: Some(Box::new(move |usage: &mut Usage, tier: Option<&str>| {
            apply_service_tier_pricing(usage, tier, &model_id);
        })),
    }
}

/// The messages the Codex adapter replays as `previous_response_id` continuation state.
pub(crate) fn continuation_response_items(
    model: &Model,
    output: &AssistantMessage,
    grammar_tool_input_properties: &BTreeMap<String, String>,
    timestamp: i64,
) -> Vec<Value> {
    convert_responses_messages(
        model,
        &Context {
            messages: vec![Message::Assistant(output.clone())],
            ..Context::default()
        },
        &codex_tool_call_providers(),
        &ConvertResponsesMessagesOptions {
            include_system_prompt: Some(false),
            grammar_tool_input_properties: Some(grammar_tool_input_properties),
            ..ConvertResponsesMessagesOptions::default()
        },
        timestamp,
    )
    .unwrap_or_default()
    .into_iter()
    .filter(|item| {
        !matches!(
            item.get("type").and_then(Value::as_str),
            Some("function_call_output") | Some("custom_tool_call_output")
        )
    })
    .collect()
}

// ---------------------------------------------------------------------------
// SSE transport
// ---------------------------------------------------------------------------

/// The SSE half of `stream`: the retry loop, the zstd body and the event pump.
#[allow(clippy::too_many_arguments)]
async fn run_sse_stream(
    model: &Model,
    options: &OpenAICodexResponsesOptions,
    request: &ProviderRequestOptions,
    output: &mut AssistantMessage,
    stream: &AssistantMessageEventStream,
    grammar_tool_input_properties: &BTreeMap<String, String>,
    mut sse_headers: CodexHeaders,
    body_json: &str,
    http_timeout_ms: Option<u64>,
    start_emitted: &mut bool,
) -> Result<(), CodexError> {
    // The Codex backend decodes Content-Encoding: zstd; the WebSocket transport sends the
    // uncompressed JSON frame, matching the official Codex client.
    let compressed_body = compress_request_body_zstd(body_json);
    if compressed_body.is_some() {
        sse_headers.set("content-encoding", "zstd");
    }
    let sse_body = compressed_body.unwrap_or_else(|| body_json.as_bytes().to_vec());
    let request_headers = sse_headers.into_pairs();

    let fetch: FetchFunction = request
        .fetch
        .clone()
        .unwrap_or_else(|| Arc::new(ReqwestFetch::default()));
    let url = resolve_codex_url(&model.base_url);
    let max_retries = request.max_retries.unwrap_or(DEFAULT_MAX_RETRIES);

    let mut response = None;
    let mut last_error: Option<CodexError> = None;
    for attempt in 0..=max_retries {
        if request
            .signal
            .as_ref()
            .is_some_and(|signal| signal.is_cancelled())
        {
            return Err(CodexError::transport("Request was aborted"));
        }

        let attempted = fetch
            .fetch(FetchRequest {
                method: "POST".to_string(),
                url: url.clone(),
                headers: request_headers.clone(),
                body: Some(sse_body.clone()),
            })
            .await;

        let attempted = match attempted {
            Ok(attempted) => attempted,
            Err(error) => {
                let error = CodexError::transport(error.to_string());
                // Network errors are retryable.
                if attempt < max_retries && !error.message().contains("usage limit") {
                    last_error = Some(error);
                    if abortable_sleep(backoff_delay(attempt), request).await {
                        return Err(CodexError::transport("Request was aborted"));
                    }
                    continue;
                }
                return Err(error);
            }
        };

        if let Some(on_response) = &request.on_response {
            on_response(
                crate::types::ProviderResponse {
                    status: attempted.status,
                    headers: attempted.headers.iter().cloned().collect(),
                },
                model,
            )
            .await;
        }

        if (200..300).contains(&attempted.status) {
            response = Some(attempted);
            break;
        }

        let status = attempted.status;
        let status_text = attempted.status_text.clone();
        let headers = attempted.headers.clone();
        let error_text = read_body(attempted.body).await;
        if attempt < max_retries && is_retryable_error(status, &error_text) {
            let now_ms = crate::auth::resolve::now_ms();
            let delay = match retry_after_delay_ms(&headers, now_ms) {
                None => backoff_delay(attempt),
                Some(delay_ms) => Duration::from_millis(validate_retry_delay_ms(
                    delay_ms,
                    request.max_retry_delay_ms,
                )? as u64),
            };
            if abortable_sleep(delay, request).await {
                return Err(CodexError::transport("Request was aborted"));
            }
            continue;
        }

        let (message, friendly_message) = parse_error_response(
            status,
            &status_text,
            &error_text,
            crate::auth::resolve::now_ms(),
        );
        return Err(CodexError::transport(
            friendly_message
                .filter(|message| !message.is_empty())
                .unwrap_or(message),
        ));
    }

    let Some(response) = response else {
        return Err(last_error.unwrap_or_else(|| CodexError::transport("Failed after retries")));
    };

    if !*start_emitted {
        *start_emitted = true;
        stream.push(AssistantMessageEvent::Start {
            partial: output.clone(),
        });
    }

    let mut pump = CodexEventPump::new(
        ResponsesStreamState::new(std::mem::replace(output, empty_placeholder()), model),
        stream_options(options, grammar_tool_input_properties, model.id.clone()),
    );
    let mut result = drive_sse(response.body, &mut pump, request, stream, http_timeout_ms).await;
    if result.is_ok() && !pump.state.saw_terminal_response_event() {
        result = Err(CodexError::transport(
            "OpenAI Responses stream ended before a terminal response event",
        ));
    }
    *output = std::mem::replace(&mut pump.state.output, empty_placeholder());
    result
}

async fn drive_sse(
    body: FetchBody,
    pump: &mut CodexEventPump<'_>,
    request: &ProviderRequestOptions,
    stream: &AssistantMessageEventStream,
    http_timeout_ms: Option<u64>,
) -> Result<(), CodexError> {
    let mut parser = CodexSseParser::new();
    let mut feed = |text: &str, pump: &mut CodexEventPump<'_>| -> Result<(), CodexError> {
        for event in parser.feed(text)? {
            for emitted in pump.push(&event)? {
                stream.push(emitted);
            }
        }
        Ok(())
    };

    match body {
        FetchBody::Bytes(bytes) => {
            feed(&String::from_utf8_lossy(&bytes), pump)?;
        }
        FetchBody::Stream(mut receiver) => loop {
            if request
                .signal
                .as_ref()
                .is_some_and(|signal| signal.is_cancelled())
            {
                return Err(CodexError::transport("Request was aborted"));
            }
            let chunk = match http_timeout_ms.filter(|timeout| *timeout > 0) {
                Some(timeout) => {
                    match tokio::time::timeout(Duration::from_millis(timeout), receiver.recv())
                        .await
                    {
                        Ok(chunk) => chunk,
                        Err(_) => {
                            return Err(CodexError::transport(format!(
                                "Codex SSE response headers timed out after {timeout}ms"
                            )));
                        }
                    }
                }
                None => receiver.recv().await,
            };
            let Some(chunk) = chunk else {
                break;
            };
            let chunk = chunk.map_err(|error| CodexError::transport(error.to_string()))?;
            feed(&String::from_utf8_lossy(&chunk), pump)?;
        },
    }
    Ok(())
}

/// Sleeps unless the request was aborted; returns true when it was.
async fn abortable_sleep(duration: Duration, request: &ProviderRequestOptions) -> bool {
    let Some(signal) = request.signal.as_ref() else {
        tokio::time::sleep(duration).await;
        return false;
    };
    if signal.is_cancelled() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        _ = signal.cancelled() => true,
    }
}

/// The delay before a retry attempt: `BASE_DELAY_MS * 2 ** attempt`.
fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_millis(BASE_DELAY_MS * 2u64.pow(attempt))
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

// ---------------------------------------------------------------------------
// WebSocket transport
// ---------------------------------------------------------------------------

/// `processWebSocketStream(...)`
///
/// Returns whether the stream had started when it failed, which decides between a
/// fallback to SSE and a hard error.
#[allow(clippy::too_many_arguments)]
async fn run_websocket_stream(
    url: &str,
    body: &Value,
    headers: &CodexHeaders,
    model: &Model,
    options: &OpenAICodexResponsesOptions,
    request: &ProviderRequestOptions,
    output: &mut AssistantMessage,
    stream: &AssistantMessageEventStream,
    grammar_tool_input_properties: &BTreeMap<String, String>,
    cache_session_id: Option<&str>,
    account_id: &str,
    idle_timeout_ms: Option<u64>,
    connect_timeout_ms: Option<u64>,
    timestamp: i64,
    start_emitted: &mut bool,
) -> Result<(), (CodexError, bool)> {
    let now_ms = crate::auth::resolve::now_ms();
    let AcquiredWebSocket {
        mut socket,
        reused,
        continuation,
        cache_key,
    } = acquire_websocket(
        url,
        headers,
        request,
        connect_timeout_ms,
        cache_session_id,
        account_id,
        now_ms,
    )
    .await
    .map_err(|error| (error, false))?;
    let use_cached_context = matches!(
        options.transport.unwrap_or(Transport::Auto),
        Transport::WebsocketCached | Transport::Auto
    );
    let (request_body, keep_continuation) = if use_cached_context {
        build_cached_websocket_request_body(continuation.as_ref(), body)
    } else {
        (body.clone(), true)
    };
    record_websocket_request(cache_session_id, reused, use_cached_context, &request_body);

    let mut frame = request_body.clone();
    if let Some(object) = frame.as_object_mut() {
        // `{ type: "response.create", ...requestBody }` — the type comes first.
        let mut with_type = Map::new();
        with_type.insert("type".to_string(), json!("response.create"));
        for (key, value) in object.iter() {
            with_type.insert(key.clone(), value.clone());
        }
        frame = Value::Object(with_type);
    }

    let mut pump = CodexEventPump::new(
        ResponsesStreamState::new(std::mem::replace(output, empty_placeholder()), model),
        stream_options(options, grammar_tool_input_properties, model.id.clone()),
    );
    let mut websocket_started = false;
    let mut result = drive_websocket(
        &mut socket,
        &frame,
        &mut pump,
        request,
        stream,
        idle_timeout_ms,
        &mut websocket_started,
        start_emitted,
    )
    .await;
    if result.is_ok() && !pump.state.saw_terminal_response_event() {
        result = Err(CodexError::transport(
            "OpenAI Responses stream ended before a terminal response event",
        ));
    }
    *output = std::mem::replace(&mut pump.state.output, empty_placeholder());

    match result {
        Err(error) => {
            release_websocket(cache_key, socket, false, None, now_ms).await;
            Err((error, websocket_started))
        }
        Ok(()) => {
            let aborted = request
                .signal
                .as_ref()
                .is_some_and(|signal| signal.is_cancelled());
            let new_continuation = if !aborted && use_cached_context {
                output
                    .response_id
                    .clone()
                    .filter(|response_id| !response_id.is_empty())
                    .map(|response_id| ContinuationState {
                        last_request_body: body.clone(),
                        last_response_id: response_id,
                        last_response_items: continuation_response_items(
                            model,
                            output,
                            grammar_tool_input_properties,
                            timestamp,
                        ),
                    })
            } else {
                None
            };
            // Settle what the cache entry keeps: a fresh continuation from this
            // response; otherwise the one taken at acquire, unless the delta check
            // discarded it (TS clears `entry.continuation` in place there).
            let settled = if new_continuation.is_some() {
                new_continuation
            } else if !use_cached_context || keep_continuation {
                continuation
            } else {
                None
            };
            release_websocket(
                cache_key,
                socket,
                !aborted,
                settled,
                crate::auth::resolve::now_ms(),
            )
            .await;
            Ok(())
        }
    }
}

/// `connectWebSocket(url, headers, signal, connectTimeoutMs, env)`
async fn connect_websocket(
    url: &str,
    headers: &CodexHeaders,
    request: &ProviderRequestOptions,
    connect_timeout_ms: Option<u64>,
) -> Result<WebSocketStream, CodexError> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut client_request = url
        .into_client_request()
        .map_err(|error| CodexError::transport(error.to_string()))?;
    {
        let request_headers = client_request.headers_mut();
        for (key, value) in headers.pairs() {
            // TS deletes `OpenAI-Beta` from the connect headers of the raw socket.
            if key.eq_ignore_ascii_case("OpenAI-Beta") {
                continue;
            }
            let (Ok(name), Ok(value)) = (
                tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(key.as_bytes()),
                tokio_tungstenite::tungstenite::http::HeaderValue::from_str(value),
            ) else {
                continue;
            };
            request_headers.insert(name, value);
        }
    }

    let connect = tokio_tungstenite::connect_async(client_request);
    let timeout = connect_timeout_ms.unwrap_or(DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS);
    let connected = if timeout > 0 {
        match tokio::time::timeout(Duration::from_millis(timeout), connect).await {
            Ok(connected) => connected,
            Err(_) => {
                return Err(CodexError::transport(format!(
                    "WebSocket connect timeout after {timeout}ms"
                )));
            }
        }
    } else {
        connect.await
    };
    let (socket, _) = connected.map_err(|error| CodexError::transport(error.to_string()))?;
    if request
        .signal
        .as_ref()
        .is_some_and(|signal| signal.is_cancelled())
    {
        return Err(CodexError::transport("Request was aborted"));
    }
    Ok(socket)
}

/// `parseWebSocket(socket, signal, idleTimeoutMs)` plus the event pump.
#[allow(clippy::too_many_arguments)]
async fn drive_websocket(
    socket: &mut WebSocketStream,
    frame: &Value,
    pump: &mut CodexEventPump<'_>,
    request: &ProviderRequestOptions,
    stream: &AssistantMessageEventStream,
    idle_timeout_ms: Option<u64>,
    websocket_started: &mut bool,
    start_emitted: &mut bool,
) -> Result<(), CodexError> {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    socket
        .send(WsMessage::Text(frame.to_string().into()))
        .await
        .map_err(|error| CodexError::transport(error.to_string()))?;

    let mut saw_completion = false;
    loop {
        if request
            .signal
            .as_ref()
            .is_some_and(|signal| signal.is_cancelled())
        {
            return Err(CodexError::transport("Request was aborted"));
        }

        let next = match idle_timeout_ms.filter(|timeout| *timeout > 0) {
            Some(timeout) => {
                match tokio::time::timeout(Duration::from_millis(timeout), socket.next()).await {
                    Ok(next) => next,
                    Err(_) => {
                        let _ = socket.close(None).await;
                        return Err(CodexError::transport(format!(
                            "WebSocket idle timeout after {timeout}ms"
                        )));
                    }
                }
            }
            None => socket.next().await,
        };

        let Some(message) = next else {
            break;
        };
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                if saw_completion {
                    break;
                }
                return Err(CodexError::transport(error.to_string()));
            }
        };

        let text = match message {
            WsMessage::Text(text) => text.to_string(),
            WsMessage::Binary(bytes) => String::from_utf8_lossy(&bytes).to_string(),
            WsMessage::Close(close) => {
                if saw_completion {
                    break;
                }
                return Err(websocket_close_error(close.as_ref()));
            }
            // Ping/Pong/Frame carry no payload the parser looks at.
            _ => continue,
        };

        let parsed =
            serde_json::from_str::<Value>(&text).map_err(|cause| CodexError::Protocol {
                message: format!(
                    "Invalid Codex WebSocket JSON: {}",
                    format_thrown_value(&cause.to_string())
                ),
            })?;
        if matches!(
            parsed.get("type").and_then(Value::as_str),
            Some("response.completed") | Some("response.done") | Some("response.incomplete")
        ) {
            saw_completion = true;
        }

        if !*websocket_started {
            *websocket_started = true;
            if !*start_emitted {
                *start_emitted = true;
                stream.push(AssistantMessageEvent::Start {
                    partial: pump.state.output.clone(),
                });
            }
        }
        for emitted in pump.push(&parsed)? {
            stream.push(emitted);
        }
        if saw_completion {
            break;
        }
    }

    if !saw_completion {
        return Err(CodexError::transport(
            "WebSocket stream closed before response.completed",
        ));
    }
    // The socket stays open: the caller returns it to the session cache so the next
    // request can continue via connection-scoped `previous_response_id` state.
    Ok(())
}

/// `extractWebSocketCloseError(event)`
fn websocket_close_error(
    close: Option<&tokio_tungstenite::tungstenite::protocol::CloseFrame>,
) -> CodexError {
    let Some(close) = close else {
        return CodexError::transport("WebSocket closed");
    };
    let code: u16 = close.code.into();
    let reason = close.reason.to_string();
    let code_text = format!(" {code}");
    let reason_text = if !reason.is_empty() {
        format!(" {reason}")
    } else if code == WEBSOCKET_MESSAGE_TOO_BIG_CLOSE_CODE {
        " message too big".to_string()
    } else {
        String::new()
    };
    CodexError::WebSocketClose {
        message: format!("WebSocket closed{code_text}{reason_text}")
            .trim()
            .to_string(),
        code: Some(code),
    }
}
