use serde_json::{Map, Value, json};

use super::events::HookEvent;

/// Where the event happened. Present on every payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookSessionContext {
    pub session_id: String,
    /// Session transcript on disk, when the session has been written yet.
    pub transcript_path: Option<String>,
    pub cwd: String,
}

/// Why a notification was raised. NotMux branches on exactly these values.
pub const NOTIFICATION_PERMISSION_PROMPT: &str = "permission_prompt";
pub const NOTIFICATION_IDLE_PROMPT: &str = "idle_prompt";

pub fn build_payload(
    event: HookEvent,
    context: &HookSessionContext,
    fields: Map<String, Value>,
) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("session_id".to_string(), json!(context.session_id));
    // `undefined` is dropped by `JSON.stringify`; `null` is not the same thing,
    // so a session without a file on disk simply has no `transcript_path`.
    if let Some(transcript_path) = &context.transcript_path {
        payload.insert("transcript_path".to_string(), json!(transcript_path));
    }
    payload.insert("cwd".to_string(), json!(context.cwd));
    payload.insert("hook_event_name".to_string(), json!(event.as_str()));
    for (key, value) in fields {
        payload.insert(key, value);
    }
    payload
}

/// Trims a tool result to what a hook can act on.
/// A hook decides from the outcome, not from the bytes: a formatter reruns on
/// the path it was given, and a supervisor updates a badge. Passing whole file
/// contents on a pipe would cost more than every hook run put together, so the
/// text is capped and the truncation is stated rather than hidden.
pub const MAX_RESPONSE_CHARS: usize = 2_000;

pub fn summarize_tool_response(content: &str, is_error: bool) -> Map<String, Value> {
    let units: Vec<u16> = content.encode_utf16().collect();
    let truncated = units.len() > MAX_RESPONSE_CHARS;
    let output = if truncated {
        format!(
            "{}…",
            String::from_utf16_lossy(&units[..MAX_RESPONSE_CHARS])
        )
    } else {
        content.to_string()
    };
    let mut response = Map::new();
    response.insert("success".to_string(), json!(!is_error));
    response.insert("output".to_string(), json!(output));
    if truncated {
        response.insert("truncated".to_string(), json!(true));
    }
    let mut fields = Map::new();
    fields.insert("tool_response".to_string(), Value::Object(response));
    fields
}
