//! Normalization of provider HTTP error objects.
//!
//! 1:1 port of `packages/ai/src/utils/error-body.ts` (149 LOC). The TS version probes
//! SDK-specific field names (Mistral, openai, @google/genai, AWS Bedrock); the Rust
//! port receives the same information from the HTTP layer as an explicit struct
//! (deviation class 3: provider SDKs are replaced by reqwest).

use serde_json::Value;

pub const MAX_PROVIDER_ERROR_BODY_CHARS: usize = 4000;

/// `NormalizedProviderError`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NormalizedProviderError {
    /// HTTP status code, when one could be extracted.
    pub status: Option<u16>,
    /// Raw HTTP body reason, already trimmed and truncated to the cap.
    pub body: Option<String>,
    /// The error message.
    pub message: String,
    /// True when `message` already contains the body.
    pub message_carries_body: bool,
}

/// Raw error information as handed over by the HTTP layer.
#[derive(Debug, Clone, Default)]
pub struct RawProviderError {
    pub status: Option<u16>,
    /// Body as text, when the response carried one.
    pub body_text: Option<String>,
    /// Parsed body, when the provider returned JSON (`error.error` in the openai SDK).
    pub body_json: Option<Value>,
    pub message: String,
}

/// `normalizeProviderError(error)`
pub fn normalize_provider_error(error: &RawProviderError) -> NormalizedProviderError {
    let body = extract_body(error);
    let message_carries_body = match &body {
        None => true,
        Some(body) => error.message.contains(body.as_str()),
    };
    NormalizedProviderError {
        status: error.status,
        body,
        message: error.message.clone(),
        message_carries_body,
    }
}

/// `extractBody(error)` — empty bodies and empty objects count as no body.
fn extract_body(error: &RawProviderError) -> Option<String> {
    let body_text = pick_body_text(error)?;
    let trimmed = body_text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_error_text(trimmed, MAX_PROVIDER_ERROR_BODY_CHARS))
}

/// `pickBodyText(error)` — body string first, then a non-empty parsed JSON object.
fn pick_body_text(error: &RawProviderError) -> Option<String> {
    if let Some(body_text) = &error.body_text {
        return Some(body_text.clone());
    }
    match &error.body_json {
        Some(Value::Object(object)) if !object.is_empty() => {
            Some(safe_json_stringify(&Value::Object(object.clone())))
        }
        _ => None,
    }
}

/// `formatProviderError(norm, prefix?)`
pub fn format_provider_error(normalized: &NormalizedProviderError, prefix: Option<&str>) -> String {
    if normalized.message_carries_body || normalized.status.is_none() || normalized.body.is_none() {
        return match (prefix, normalized.status) {
            (Some(prefix), Some(status)) => format!("{prefix} ({status}): {}", normalized.message),
            _ => normalized.message.clone(),
        };
    }
    let status = normalized.status.expect("checked above");
    let body = normalized.body.as_ref().expect("checked above");
    match prefix {
        Some(prefix) => format!("{prefix} ({status}): {body}"),
        None => format!("{status}: {body}"),
    }
}

/// `truncateErrorText(text, maxChars)` — counts UTF-16 code units like `String.length`.
pub fn truncate_error_text(text: &str, max_chars: usize) -> String {
    let length = text.encode_utf16().count();
    if length <= max_chars {
        return text.to_string();
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let head = String::from_utf16_lossy(&units[..max_chars]);
    format!("{head}... [truncated {} chars]", length - max_chars)
}

/// `safeJsonStringify(value)`
pub fn safe_json_stringify(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}
