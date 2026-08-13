//! Mistral Chat Completions adapter.
//!
//! 1:1 port of `packages/ai/src/api/mistral-conversations.ts` (931 LOC). Class 3
//! substitution: the `fetch` call and the event reader become reqwest plus the
//! adapter's own boundary scanner — Mistral's stream is *not* read with the shared
//! SSE decoder, because the TS reader accepts eight different separator forms.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::{
    get_json_schema_tool_parameters, resolve_json_schema_strict_sampling,
};
use crate::api::simple_options::build_base_options;
use crate::api::transform_messages::transform_messages;
use crate::models::{calculate_cost, clamp_thinking_level};
use crate::types::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, CacheRetention, Context, DoneReason,
    ErrorReason, Message, Model, ModelThinkingLevel, ProviderHeaders, ProviderRequestOptions,
    SimpleStreamOptions, StopReason, TextContent, ThinkingContent, Tool, ToolCall, Usage,
    UserContent,
};
use crate::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use crate::utils::fetch::{FetchBody, FetchRequest, ReqwestFetch};
use crate::utils::hash::short_hash;
use crate::utils::json_parse::parse_streaming_json;
use crate::utils::sanitize_unicode::sanitize_surrogates;

const MISTRAL_TOOL_CALL_ID_LENGTH: usize = 9;
const MAX_MISTRAL_ERROR_BODY_CHARS: usize = 4000;

/// `MistralReasoningEffort = "none" | "high"`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MistralReasoningEffort {
    None,
    High,
}

impl MistralReasoningEffort {
    fn as_str(self) -> &'static str {
        match self {
            MistralReasoningEffort::None => "none",
            MistralReasoningEffort::High => "high",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(MistralReasoningEffort::None),
            "high" => Some(MistralReasoningEffort::High),
            _ => None,
        }
    }
}

/// `toolChoice?: "auto" | "none" | "any" | "required" | { type: "function", … }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MistralToolChoice {
    Auto,
    None,
    Any,
    Required,
    Function(String),
}

impl MistralToolChoice {
    fn to_value(&self) -> Value {
        match self {
            MistralToolChoice::Auto => json!("auto"),
            MistralToolChoice::None => json!("none"),
            MistralToolChoice::Any => json!("any"),
            MistralToolChoice::Required => json!("required"),
            MistralToolChoice::Function(name) => {
                json!({ "type": "function", "function": { "name": name } })
            }
        }
    }
}

/// `MistralOptions extends StreamOptions`
#[derive(Debug, Clone, Default)]
pub struct MistralOptions {
    pub tool_choice: Option<MistralToolChoice>,
    /// `promptMode?: "reasoning"`
    pub prompt_mode_reasoning: bool,
    pub reasoning_effort: Option<MistralReasoningEffort>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<String>,
    pub headers: Option<ProviderHeaders>,
}

/// The error the adapter reports through the `error` event.
#[derive(Debug, Clone)]
pub struct MistralError {
    pub message: String,
    pub status: Option<u16>,
    pub body: Option<String>,
}

impl MistralError {
    fn from_message(message: impl Into<String>) -> Self {
        MistralError {
            message: message.into(),
            status: None,
            body: None,
        }
    }

    /// `formatMistralError(error)`
    fn format(&self) -> String {
        match (self.status, self.body.as_deref()) {
            (Some(status), Some(body)) if !body.trim().is_empty() => format!(
                "Mistral API error ({status}): {}",
                truncate_error_text(body.trim(), MAX_MISTRAL_ERROR_BODY_CHARS)
            ),
            (Some(status), _) => format!("Mistral API error ({status}): {}", self.message),
            (None, _) => self.message.clone(),
        }
    }
}

fn truncate_error_text(text: &str, max_chars: usize) -> String {
    let characters: Vec<char> = text.chars().collect();
    if characters.len() <= max_chars {
        return text.to_string();
    }
    format!(
        "{}... [truncated {} chars]",
        characters[..max_chars].iter().collect::<String>(),
        characters.len() - max_chars
    )
}

// ---------------------------------------------------------------------------
// Tool call ids
// ---------------------------------------------------------------------------

/// `createMistralToolCallIdNormalizer()`
#[derive(Default)]
pub struct MistralToolCallIdNormalizer {
    id_map: BTreeMap<String, String>,
    reverse_map: BTreeMap<String, String>,
}

impl MistralToolCallIdNormalizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn normalize(&mut self, id: &str) -> String {
        if let Some(existing) = self.id_map.get(id) {
            return existing.clone();
        }
        let mut attempt = 0;
        loop {
            let candidate = derive_mistral_tool_call_id(id, attempt);
            match self.reverse_map.get(&candidate) {
                Some(owner) if owner != id => attempt += 1,
                _ => {
                    self.id_map.insert(id.to_string(), candidate.clone());
                    self.reverse_map.insert(candidate.clone(), id.to_string());
                    return candidate;
                }
            }
        }
    }
}

/// `deriveMistralToolCallId(id, attempt)`
pub fn derive_mistral_tool_call_id(id: &str, attempt: u32) -> String {
    let normalized: String = id.chars().filter(char::is_ascii_alphanumeric).collect();
    if attempt == 0 && normalized.chars().count() == MISTRAL_TOOL_CALL_ID_LENGTH {
        return normalized;
    }
    let seed_base = if normalized.is_empty() {
        id
    } else {
        &normalized
    };
    let seed = if attempt == 0 {
        seed_base.to_string()
    } else {
        format!("{seed_base}:{attempt}")
    };
    short_hash(&seed)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MISTRAL_TOOL_CALL_ID_LENGTH)
        .collect()
}

// ---------------------------------------------------------------------------
// Request body
// ---------------------------------------------------------------------------

/// `buildChatPayload(model, context, messages, options)` followed by
/// `toMistralWirePayload(payload)` — the port builds the wire body directly, in the
/// key order the TS remapping produces.
pub fn build_request_body(
    model: &Model,
    context: &Context,
    messages: &[Message],
    options: &MistralOptions,
) -> Result<Value, MistralError> {
    let supports_images = model.input.contains(&crate::types::Modality::Image);
    let mut body = Map::new();
    body.insert("model".into(), json!(model.id));
    body.insert("stream".into(), json!(true));

    let mut chat_messages = to_chat_messages(messages, supports_images);
    if let Some(system_prompt) = context
        .system_prompt
        .as_deref()
        .filter(|prompt| !prompt.is_empty())
    {
        chat_messages.insert(
            0,
            json!({ "role": "system", "content": sanitize_surrogates(system_prompt) }),
        );
    }
    body.insert("messages".into(), Value::Array(chat_messages));

    if let Some(tools) = context.tools.as_ref().filter(|tools| !tools.is_empty()) {
        body.insert("tools".into(), Value::Array(to_function_tools(tools)?));
    }
    if let Some(temperature) = options.temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = options.max_tokens {
        body.insert("max_tokens".into(), json!(max_tokens));
    }
    if let Some(tool_choice) = &options.tool_choice {
        body.insert("tool_choice".into(), tool_choice.to_value());
    }
    if options.prompt_mode_reasoning {
        body.insert("prompt_mode".into(), json!("reasoning"));
    }
    if let Some(reasoning_effort) = options.reasoning_effort {
        body.insert("reasoning_effort".into(), json!(reasoning_effort.as_str()));
    }
    if let Some(session_id) = prompt_cache_session_id(options) {
        body.insert("prompt_cache_key".into(), json!(session_id));
    }

    Ok(Value::Object(body))
}

/// `shouldUsePromptCaching(options)`
fn prompt_cache_session_id(options: &MistralOptions) -> Option<&str> {
    if options.cache_retention == Some(CacheRetention::None) {
        return None;
    }
    options
        .session_id
        .as_deref()
        .filter(|session_id| !session_id.is_empty())
}

/// `toFunctionTools(tools)`
fn to_function_tools(tools: &[Tool]) -> Result<Vec<Value>, MistralError> {
    tools
        .iter()
        .map(|tool| {
            let strict = resolve_json_schema_strict_sampling(tool, true)
                .map_err(|error| MistralError::from_message(error.to_string()))?;
            Ok(json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": get_json_schema_tool_parameters(tool, strict),
                    "strict": strict.unwrap_or(false),
                },
            }))
        })
        .collect()
}

/// `toChatMessages(messages, supportsImages)` plus the wire remapping of each message.
fn to_chat_messages(messages: &[Message], supports_images: bool) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();

    for message in messages {
        match message {
            Message::User(user) => match &user.content {
                UserContent::Text(text) => {
                    result.push(json!({ "role": "user", "content": sanitize_surrogates(text) }));
                }
                UserContent::Blocks(blocks) => {
                    let had_images = blocks
                        .iter()
                        .any(|block| matches!(block, crate::types::TextOrImageContent::Image(_)));
                    let content: Vec<Value> = blocks
                        .iter()
                        .filter_map(|block| match block {
                            crate::types::TextOrImageContent::Text(text) => Some(
                                json!({ "type": "text", "text": sanitize_surrogates(&text.text) }),
                            ),
                            crate::types::TextOrImageContent::Image(image) if supports_images => {
                                Some(json!({
                                    "type": "image_url",
                                    "image_url": format!(
                                        "data:{};base64,{}",
                                        image.mime_type, image.data
                                    ),
                                }))
                            }
                            crate::types::TextOrImageContent::Image(_) => None,
                        })
                        .collect();
                    if !content.is_empty() {
                        result.push(json!({ "role": "user", "content": content }));
                        continue;
                    }
                    if had_images && !supports_images {
                        result.push(json!({
                            "role": "user",
                            "content": "(image omitted: model does not support images)",
                        }));
                    }
                }
            },

            Message::Assistant(assistant) => {
                let mut content_parts: Vec<Value> = Vec::new();
                let mut tool_calls: Vec<Value> = Vec::new();
                for block in &assistant.content {
                    match block {
                        AssistantContent::Text(text) => {
                            if !text.text.trim().is_empty() {
                                content_parts.push(json!({
                                    "type": "text",
                                    "text": sanitize_surrogates(&text.text),
                                }));
                            }
                        }
                        AssistantContent::Thinking(thinking) => {
                            if !thinking.thinking.trim().is_empty() {
                                content_parts.push(json!({
                                    "type": "thinking",
                                    "thinking": [{
                                        "type": "text",
                                        "text": sanitize_surrogates(&thinking.thinking),
                                    }],
                                }));
                            }
                        }
                        AssistantContent::ToolCall(tool_call) => {
                            tool_calls.push(json!({
                                "id": tool_call.id,
                                "type": "function",
                                "function": {
                                    "name": tool_call.name,
                                    "arguments": serde_json::to_string(&tool_call.arguments)
                                        .unwrap_or_else(|_| "{}".to_string()),
                                },
                                "index": 0,
                            }));
                        }
                    }
                }

                // `{ role, prefix }` first, then content and tool calls — the remapping
                // moves `toolCalls` to the end of the object.
                let mut assistant_message = Map::new();
                assistant_message.insert("role".into(), json!("assistant"));
                assistant_message.insert("prefix".into(), json!(false));
                if !content_parts.is_empty() {
                    assistant_message.insert("content".into(), Value::Array(content_parts.clone()));
                }
                if !tool_calls.is_empty() {
                    assistant_message.insert("tool_calls".into(), Value::Array(tool_calls.clone()));
                }
                if !content_parts.is_empty() || !tool_calls.is_empty() {
                    result.push(Value::Object(assistant_message));
                }
            }

            Message::ToolResult(tool_result) => {
                let text_result = tool_result
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        crate::types::TextOrImageContent::Text(text) => {
                            Some(sanitize_surrogates(&text.text))
                        }
                        crate::types::TextOrImageContent::Image(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let has_images = tool_result
                    .content
                    .iter()
                    .any(|part| matches!(part, crate::types::TextOrImageContent::Image(_)));
                let tool_text = build_tool_result_text(
                    &text_result,
                    has_images,
                    supports_images,
                    tool_result.is_error,
                );

                let mut tool_content: Vec<Value> =
                    vec![json!({ "type": "text", "text": tool_text })];
                if supports_images {
                    for part in &tool_result.content {
                        if let crate::types::TextOrImageContent::Image(image) = part {
                            tool_content.push(json!({
                                "type": "image_url",
                                "image_url": format!(
                                    "data:{};base64,{}",
                                    image.mime_type, image.data
                                ),
                            }));
                        }
                    }
                }

                // `toolCallId` is remapped last, so it ends up behind `content`.
                let mut tool_message = Map::new();
                tool_message.insert("role".into(), json!("tool"));
                tool_message.insert("name".into(), json!(tool_result.tool_name));
                tool_message.insert("content".into(), Value::Array(tool_content));
                tool_message.insert("tool_call_id".into(), json!(tool_result.tool_call_id));
                result.push(Value::Object(tool_message));
            }
        }
    }

    result
}

/// `buildToolResultText(text, hasImages, supportsImages, isError)`
fn build_tool_result_text(
    text: &str,
    has_images: bool,
    supports_images: bool,
    is_error: bool,
) -> String {
    let trimmed = text.trim();
    let error_prefix = if is_error { "[tool error] " } else { "" };

    if !trimmed.is_empty() {
        let image_suffix = if has_images && !supports_images {
            "\n[tool image omitted: model does not support images]"
        } else {
            ""
        };
        return format!("{error_prefix}{trimmed}{image_suffix}");
    }

    if has_images {
        if supports_images {
            return if is_error {
                "[tool error] (see attached image)".to_string()
            } else {
                "(see attached image)".to_string()
            };
        }
        return if is_error {
            "[tool error] (image omitted: model does not support images)".to_string()
        } else {
            "(image omitted: model does not support images)".to_string()
        };
    }

    if is_error {
        "[tool error] (no tool output)".to_string()
    } else {
        "(no tool output)".to_string()
    }
}

// ---------------------------------------------------------------------------
// Event reader
// ---------------------------------------------------------------------------

/// `findMistralEventBoundary(buffer)` — eight separator forms, longest first.
fn find_event_boundary(buffer: &str) -> Option<(usize, usize)> {
    const SEPARATORS: [&str; 8] = [
        "\r\n\r\n", "\r\n\r", "\r\n\n", "\r\r\n", "\n\r\n", "\r\r", "\n\r", "\n\n",
    ];
    let mut best: Option<(usize, usize)> = None;
    for separator in SEPARATORS {
        if let Some(index) = buffer.find(separator) {
            let candidate = (index, separator.len());
            best = match best {
                // The regex alternation picks the earliest match; among matches at the
                // same index the first alternative wins, which is the longest one.
                Some((best_index, _)) if best_index <= index => best,
                _ => Some(candidate),
            };
        }
    }
    best
}

/// The outcome of `parseMistralEvent(raw)`.
enum ParsedEvent {
    Chunk(Value),
    Done,
    Empty,
}

/// `parseMistralEvent(raw)`
fn parse_event(raw: &str) -> Result<ParsedEvent, MistralError> {
    let data = raw
        .split(['\r', '\n'])
        .filter(|line| line.starts_with("data:"))
        .map(|line| line[5..].trim_start())
        .collect::<Vec<_>>()
        .join("\n");
    let data = data.trim();
    if data.is_empty() {
        return Ok(ParsedEvent::Empty);
    }
    if data == "[DONE]" {
        return Ok(ParsedEvent::Done);
    }
    let parsed: Value = serde_json::from_str(data)
        .map_err(|error| MistralError::from_message(error.to_string()))?;
    if !parsed.is_object() || !parsed.get("choices").is_some_and(Value::is_array) {
        return Err(MistralError::from_message(
            "Invalid Mistral streaming event",
        ));
    }
    Ok(ParsedEvent::Chunk(parsed))
}

// ---------------------------------------------------------------------------
// Streaming state
// ---------------------------------------------------------------------------

/// `consumeChatStream` as a state machine.
struct MistralStreamState {
    output: AssistantMessage,
    model: Model,
    /// Index of the open text or thinking block, if any.
    current_block: Option<usize>,
    /// `toolBlocksByKey`: `"{callId}:{index}"` to content index.
    tool_blocks_by_key: BTreeMap<String, usize>,
    /// The scratch buffers `partialArgs` of the open tool calls.
    partial_args: BTreeMap<usize, String>,
}

impl MistralStreamState {
    fn new(model: &Model, timestamp: i64) -> Self {
        MistralStreamState {
            output: AssistantMessage {
                content: Vec::new(),
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Pending,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp,
            },
            model: model.clone(),
            current_block: None,
            tool_blocks_by_key: BTreeMap::new(),
            partial_args: BTreeMap::new(),
        }
    }

    fn block_index(&self) -> usize {
        self.output.content.len().saturating_sub(1)
    }

    /// `finishCurrentBlock(block)`
    fn finish_current_block(&mut self, events: &mut Vec<AssistantMessageEvent>) {
        let Some(index) = self.current_block.take() else {
            return;
        };
        match self.output.content.get(index) {
            Some(AssistantContent::Text(text)) => {
                let content = text.text.clone();
                events.push(AssistantMessageEvent::TextEnd {
                    content_index: self.block_index(),
                    content,
                    partial: self.output.clone(),
                });
            }
            Some(AssistantContent::Thinking(thinking)) => {
                let content = thinking.thinking.clone();
                events.push(AssistantMessageEvent::ThinkingEnd {
                    content_index: self.block_index(),
                    content,
                    partial: self.output.clone(),
                });
            }
            _ => {}
        }
    }

    fn process_chunk(&mut self, chunk: &Value) -> Vec<AssistantMessageEvent> {
        let mut events: Vec<AssistantMessageEvent> = Vec::new();

        if self.output.response_id.is_none()
            && let Some(id) = chunk
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
        {
            self.output.response_id = Some(id.to_string());
        }

        if let Some(usage) = chunk.get("usage").filter(|usage| !usage.is_null()) {
            let prompt_tokens = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let cached_prompt_tokens = cached_prompt_tokens(usage, prompt_tokens);
            self.output.usage.input = prompt_tokens.saturating_sub(cached_prompt_tokens);
            self.output.usage.output = usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            self.output.usage.cache_read = cached_prompt_tokens;
            self.output.usage.cache_write = 0;
            self.output.usage.total_tokens = Some(
                usage
                    .get("total_tokens")
                    .and_then(Value::as_u64)
                    .filter(|total| *total != 0)
                    .unwrap_or(
                        self.output.usage.input
                            + self.output.usage.output
                            + self.output.usage.cache_read
                            + self.output.usage.cache_write,
                    ),
            );
            calculate_cost(&self.model, &mut self.output.usage);
        }

        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return events;
        };

        if let Some(finish_reason) = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
        {
            self.output.raw_stop_reason = Some(finish_reason.to_string());
            let (stop_reason, error_message) = map_chat_stop_reason(Some(finish_reason));
            self.output.stop_reason = stop_reason;
            if let Some(error_message) = error_message {
                self.output.error_message = Some(error_message);
            }
        } else if choice.get("finish_reason").is_some_and(Value::is_null) {
            // `choice.finish_reason` is null: TS skips the branch (falsy).
        }

        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
        if let Some(content) = delta.get("content").filter(|content| !content.is_null()) {
            let items: Vec<Value> = match content {
                Value::String(text) => vec![Value::String(text.clone())],
                Value::Array(items) => items.clone(),
                other => vec![other.clone()],
            };
            for item in items {
                match &item {
                    Value::String(text) => {
                        let text_delta = sanitize_surrogates(text);
                        self.push_text_delta(&text_delta, &mut events);
                    }
                    Value::Object(object) => {
                        let item_type = object.get("type").and_then(Value::as_str).unwrap_or("");
                        if item_type == "thinking" {
                            let delta_text: String = object
                                .get("thinking")
                                .and_then(Value::as_array)
                                .map(|parts| {
                                    parts
                                        .iter()
                                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                                        .filter(|text| !text.is_empty())
                                        .collect::<Vec<_>>()
                                        .join("")
                                })
                                .unwrap_or_default();
                            let thinking_delta = sanitize_surrogates(&delta_text);
                            if thinking_delta.is_empty() {
                                continue;
                            }
                            self.push_thinking_delta(&thinking_delta, &mut events);
                        } else if item_type == "text" {
                            let text = object.get("text").and_then(Value::as_str).unwrap_or("");
                            let text_delta = sanitize_surrogates(text);
                            self.push_text_delta(&text_delta, &mut events);
                        }
                    }
                    _ => {}
                }
            }
        }

        let tool_calls = delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for tool_call in tool_calls {
            if self.current_block.is_some() {
                self.finish_current_block(&mut events);
            }
            let raw_id = tool_call.get("id").and_then(Value::as_str).unwrap_or("");
            let index = tool_call.get("index").and_then(Value::as_i64).unwrap_or(0);
            let call_id = if !raw_id.is_empty() && raw_id != "null" {
                raw_id.to_string()
            } else {
                derive_mistral_tool_call_id(&format!("toolcall:{index}"), 0)
            };
            let key = format!("{call_id}:{}", if index == 0 { 0 } else { index });

            let content_index = match self.tool_blocks_by_key.get(&key).copied().filter(|index| {
                matches!(
                    self.output.content.get(*index),
                    Some(AssistantContent::ToolCall(_))
                )
            }) {
                Some(index) => index,
                None => {
                    let name = tool_call
                        .get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    self.output
                        .content
                        .push(AssistantContent::ToolCall(ToolCall {
                            id: call_id.clone(),
                            name,
                            arguments: Map::new(),
                            thought_signature: None,
                            namespace: None,
                            extra: Map::new(),
                        }));
                    let content_index = self.output.content.len() - 1;
                    self.tool_blocks_by_key.insert(key.clone(), content_index);
                    self.partial_args.insert(content_index, String::new());
                    events.push(AssistantMessageEvent::ToolcallStart {
                        content_index,
                        partial: self.output.clone(),
                    });
                    content_index
                }
            };

            let arguments = tool_call
                .get("function")
                .and_then(|function| function.get("arguments"));
            let args_delta = match arguments {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Null) | None => "{}".to_string(),
                Some(other) => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
            };
            let buffer = self.partial_args.entry(content_index).or_default();
            buffer.push_str(&args_delta);
            let parsed = parse_streaming_json(Some(buffer.as_str()));
            if let Some(AssistantContent::ToolCall(block)) =
                self.output.content.get_mut(content_index)
            {
                block.arguments = parsed.as_object().cloned().unwrap_or_default();
            }
            events.push(AssistantMessageEvent::ToolcallDelta {
                content_index,
                delta: args_delta,
                partial: self.output.clone(),
            });
        }

        events
    }

    fn push_text_delta(&mut self, delta: &str, events: &mut Vec<AssistantMessageEvent>) {
        let is_text = matches!(
            self.current_block
                .and_then(|index| self.output.content.get(index)),
            Some(AssistantContent::Text(_))
        );
        if !is_text {
            self.finish_current_block(events);
            self.output
                .content
                .push(AssistantContent::Text(TextContent::default()));
            self.current_block = Some(self.output.content.len() - 1);
            events.push(AssistantMessageEvent::TextStart {
                content_index: self.block_index(),
                partial: self.output.clone(),
            });
        }
        if let Some(AssistantContent::Text(block)) = self
            .current_block
            .and_then(|index| self.output.content.get_mut(index))
        {
            block.text.push_str(delta);
        }
        events.push(AssistantMessageEvent::TextDelta {
            content_index: self.block_index(),
            delta: delta.to_string(),
            partial: self.output.clone(),
        });
    }

    fn push_thinking_delta(&mut self, delta: &str, events: &mut Vec<AssistantMessageEvent>) {
        let is_thinking = matches!(
            self.current_block
                .and_then(|index| self.output.content.get(index)),
            Some(AssistantContent::Thinking(_))
        );
        if !is_thinking {
            self.finish_current_block(events);
            self.output
                .content
                .push(AssistantContent::Thinking(ThinkingContent::default()));
            self.current_block = Some(self.output.content.len() - 1);
            events.push(AssistantMessageEvent::ThinkingStart {
                content_index: self.block_index(),
                partial: self.output.clone(),
            });
        }
        if let Some(AssistantContent::Thinking(block)) = self
            .current_block
            .and_then(|index| self.output.content.get_mut(index))
        {
            block.thinking.push_str(delta);
        }
        events.push(AssistantMessageEvent::ThinkingDelta {
            content_index: self.block_index(),
            delta: delta.to_string(),
            partial: self.output.clone(),
        });
    }

    /// The tail of `consumeChatStream` after the loop.
    fn finish(&mut self, events: &mut Vec<AssistantMessageEvent>) {
        self.finish_current_block(events);
        let indices: Vec<usize> = self.tool_blocks_by_key.values().copied().collect();
        for index in indices {
            let buffer = self.partial_args.remove(&index);
            let parsed = parse_streaming_json(buffer.as_deref());
            let Some(AssistantContent::ToolCall(block)) = self.output.content.get_mut(index) else {
                continue;
            };
            block.arguments = parsed.as_object().cloned().unwrap_or_default();
            let tool_call = block.clone();
            events.push(AssistantMessageEvent::ToolcallEnd {
                content_index: index,
                tool_call,
                partial: self.output.clone(),
            });
        }
    }
}

/// `getMistralCachedPromptTokens(usage, promptTokens)`
fn cached_prompt_tokens(usage: &Value, prompt_tokens: u64) -> u64 {
    let raw = [
        ("promptTokensDetails", "cachedTokens"),
        ("prompt_tokens_details", "cached_tokens"),
        ("promptTokenDetails", "cachedTokens"),
        ("prompt_token_details", "cached_tokens"),
    ]
    .into_iter()
    .find_map(|(outer, inner)| {
        usage
            .get(outer)
            .filter(|details| !details.is_null())
            .and_then(|details| details.get(inner))
            .cloned()
    })
    .or_else(|| usage.get("numCachedTokens").cloned())
    .or_else(|| usage.get("num_cached_tokens").cloned());

    let cached = raw
        .as_ref()
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(0.0);
    prompt_tokens.min(cached.max(0.0) as u64)
}

/// `mapChatStopReason(reason)`
fn map_chat_stop_reason(reason: Option<&str>) -> (StopReason, Option<String>) {
    match reason {
        None => (StopReason::Stop, None),
        Some("stop") => (StopReason::Stop, None),
        Some("length" | "model_length") => (StopReason::Length, None),
        Some("tool_calls") => (StopReason::ToolUse, None),
        Some("error") => (
            StopReason::Error,
            Some("Provider stopped with: error".to_string()),
        ),
        Some(other) => (
            StopReason::Error,
            Some(format!("Provider stopped with: {other}")),
        ),
    }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// `new URL("v1/chat/completions", baseUrl)` with the trailing-slash rule of the TS
/// adapter: the base path always ends in `/`, so the relative segment is appended.
pub fn build_request_url(model: &Model) -> String {
    let base = model.base_url.trim_end_matches('/');
    format!("{base}/v1/chat/completions")
}

/// `buildMistralHeaders(model, apiKey, options)`
pub fn build_request_headers(
    model: &Model,
    api_key: &str,
    options: &MistralOptions,
) -> Vec<(String, String)> {
    // A `Headers` object lower-cases its names and keeps insertion order.
    let mut headers: Vec<(String, String)> = vec![
        ("accept".to_string(), "text/event-stream".to_string()),
        ("authorization".to_string(), format!("Bearer {api_key}")),
        ("content-type".to_string(), "application/json".to_string()),
    ];

    let mut apply = |overrides: Option<&BTreeMap<String, String>>| {
        let Some(overrides) = overrides else { return };
        for (name, value) in overrides {
            set_header(&mut headers, name, Some(value.as_str()));
        }
    };
    apply(model.headers.as_ref());
    if let Some(option_headers) = &options.headers {
        for (name, value) in option_headers {
            set_header(&mut headers, name, value.as_deref());
        }
    }

    let has_explicit_affinity = model.headers.as_ref().is_some_and(|headers| {
        headers
            .keys()
            .any(|name| name.to_lowercase() == "x-affinity")
    }) || options.headers.as_ref().is_some_and(|headers| {
        headers
            .keys()
            .any(|name| name.to_lowercase() == "x-affinity")
    });
    if let Some(session_id) = prompt_cache_session_id(options)
        && !has_explicit_affinity
    {
        set_header(&mut headers, "x-affinity", Some(session_id));
    }

    headers
}

/// `headers.set(name, value)` / `headers.delete(name)`.
fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: Option<&str>) {
    let lowered = name.to_lowercase();
    let existing = headers
        .iter()
        .position(|(existing, _)| existing.to_lowercase() == lowered);
    match value {
        Some(value) => match existing {
            Some(index) => headers[index] = (lowered, value.to_string()),
            None => headers.push((lowered, value.to_string())),
        },
        None => {
            if let Some(index) = existing {
                headers.remove(index);
            }
        }
    }
}

/// `stream(model, context, options)`
pub fn stream(
    model: Model,
    context: Context,
    request: ProviderRequestOptions,
    options: MistralOptions,
) -> AssistantMessageEventStream {
    let outer = create_assistant_message_event_stream();
    let stream = outer.clone();

    tokio::spawn(async move {
        let timestamp = crate::auth::resolve::now_ms();
        let mut state = MistralStreamState::new(&model, timestamp);
        let outcome = run_request(&model, &context, &request, &options, &mut state, &stream).await;

        match outcome {
            Ok(()) => {
                let done_reason = match state.output.stop_reason {
                    StopReason::Stop => Some(DoneReason::Stop),
                    StopReason::Length => Some(DoneReason::Length),
                    StopReason::ToolUse => Some(DoneReason::ToolUse),
                    _ => None,
                };
                match done_reason {
                    Some(reason) => {
                        stream.push(AssistantMessageEvent::Done {
                            reason,
                            message: state.output.clone(),
                        });
                        stream.end(Some(state.output));
                    }
                    None => {
                        // `stopReason` was pending, aborted or error: the TS adapter throws.
                        let message = match state.output.stop_reason {
                            StopReason::Pending => {
                                "Mistral stream ended without a finish reason".to_string()
                            }
                            _ => state
                                .output
                                .error_message
                                .clone()
                                .filter(|message| !message.is_empty())
                                .unwrap_or_else(|| "An unknown error occurred".to_string()),
                        };
                        fail(
                            &mut state,
                            &stream,
                            &request,
                            MistralError::from_message(message),
                        );
                    }
                }
            }
            Err(error) => fail(&mut state, &stream, &request, error),
        }
    });

    outer
}

/// The `catch` branch of the TS adapter.
fn fail(
    state: &mut MistralStreamState,
    stream: &AssistantMessageEventStream,
    request: &ProviderRequestOptions,
    error: MistralError,
) {
    let aborted = request
        .signal
        .as_ref()
        .is_some_and(tokio_util::sync::CancellationToken::is_cancelled);
    state.output.stop_reason = if aborted {
        StopReason::Aborted
    } else {
        StopReason::Error
    };
    state.output.error_message = Some(error.format());
    stream.push(AssistantMessageEvent::Error {
        reason: if aborted {
            ErrorReason::Aborted
        } else {
            ErrorReason::Error
        },
        error: state.output.clone(),
    });
    stream.end(Some(state.output.clone()));
}

async fn run_request(
    model: &Model,
    context: &Context,
    request: &ProviderRequestOptions,
    options: &MistralOptions,
    state: &mut MistralStreamState,
    stream: &AssistantMessageEventStream,
) -> Result<(), MistralError> {
    let Some(api_key) = request.api_key.as_deref().filter(|key| !key.is_empty()) else {
        return Err(MistralError::from_message(format!(
            "No API key for provider: {}",
            model.provider
        )));
    };

    let normalizer = std::sync::Mutex::new(MistralToolCallIdNormalizer::new());
    let normalize = |id: &str, _message: &AssistantMessage| -> String {
        normalizer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .normalize(id)
    };
    let transformed = transform_messages(
        &context.messages,
        model,
        Some(&normalize),
        state.output.timestamp,
    );

    let mut payload = build_request_body(model, context, &transformed, options)?;
    if let Some(on_payload) = &request.on_payload
        && let Some(next) = on_payload(payload.clone(), model).await
    {
        payload = next;
    }

    let fetch = request
        .fetch
        .clone()
        .unwrap_or_else(|| std::sync::Arc::new(ReqwestFetch::default()));
    let response = fetch
        .fetch(FetchRequest {
            method: "POST".to_string(),
            url: build_request_url(model),
            headers: build_request_headers(model, api_key, options),
            body: Some(
                serde_json::to_vec(&payload)
                    .map_err(|error| MistralError::from_message(error.to_string()))?,
            ),
        })
        .await
        .map_err(|error| MistralError::from_message(error.to_string()))?;

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

    if !(200..300).contains(&response.status) {
        let body = read_body(response.body).await;
        return Err(MistralError {
            message: if response.status_text.is_empty() {
                format!("Request failed with status {}", response.status)
            } else {
                response.status_text.clone()
            },
            status: Some(response.status),
            body: Some(body),
        });
    }

    stream.push(AssistantMessageEvent::Start {
        partial: state.output.clone(),
    });

    let mut buffer = String::new();
    let mut done = false;
    let mut body = response.body;
    loop {
        let chunk = match &mut body {
            FetchBody::Bytes(bytes) => {
                let text = String::from_utf8_lossy(bytes).into_owned();
                *bytes = Vec::new();
                if text.is_empty() { None } else { Some(text) }
            }
            FetchBody::Stream(receiver) => match receiver.recv().await {
                Some(Ok(bytes)) => Some(String::from_utf8_lossy(&bytes).into_owned()),
                Some(Err(error)) => {
                    return Err(MistralError::from_message(error.to_string()));
                }
                None => None,
            },
        };
        let Some(chunk) = chunk else { break };
        buffer.push_str(&chunk);

        while let Some((index, length)) = find_event_boundary(&buffer) {
            let raw = buffer[..index].to_string();
            buffer = buffer[index + length..].to_string();
            match parse_event(&raw)? {
                ParsedEvent::Done => {
                    done = true;
                    break;
                }
                ParsedEvent::Chunk(chunk) => {
                    for event in state.process_chunk(&chunk) {
                        stream.push(event);
                    }
                }
                ParsedEvent::Empty => {}
            }
        }
        if done {
            break;
        }
    }

    if !done
        && !buffer.trim().is_empty()
        && let ParsedEvent::Chunk(chunk) = parse_event(&buffer)?
    {
        for event in state.process_chunk(&chunk) {
            stream.push(event);
        }
    }

    let mut trailing: Vec<AssistantMessageEvent> = Vec::new();
    state.finish(&mut trailing);
    for event in trailing {
        stream.push(event);
    }

    if request
        .signal
        .as_ref()
        .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
    {
        return Err(MistralError::from_message("Request was aborted"));
    }

    Ok(())
}

async fn read_body(body: FetchBody) -> String {
    match body {
        FetchBody::Bytes(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        FetchBody::Stream(mut receiver) => {
            let mut text = String::new();
            while let Some(Ok(chunk)) = receiver.recv().await {
                text.push_str(&String::from_utf8_lossy(&chunk));
            }
            text
        }
    }
}

/// `streamSimple(model, context, options)`
pub fn stream_simple(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
) -> Result<AssistantMessageEventStream, MistralError> {
    let options = options.unwrap_or_default();
    let Some(api_key) = options
        .base
        .base
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
    else {
        return Err(MistralError::from_message(format!(
            "No API key for provider: {}",
            model.provider
        )));
    };

    let base = build_base_options(&model, &context, Some(&options), Some(api_key.to_string()));
    // `clampedReasoning === "off" ? undefined : clampedReasoning`
    let clamped = options
        .reasoning
        .map(|level| clamp_thinking_level(&model, level.into()));
    let reasoning = clamped.filter(|level| *level != ModelThinkingLevel::Off);
    let should_use_reasoning = model.reasoning && reasoning.is_some();

    let mistral_options = MistralOptions {
        tool_choice: None,
        prompt_mode_reasoning: should_use_reasoning && uses_prompt_mode_reasoning(&model),
        reasoning_effort: match (
            should_use_reasoning && uses_reasoning_effort(&model),
            reasoning,
        ) {
            (true, Some(level)) => Some(map_reasoning_effort(&model, level)),
            _ => None,
        },
        temperature: base.temperature,
        max_tokens: base.max_tokens,
        cache_retention: base.cache_retention,
        session_id: base.session_id.clone(),
        headers: base.base.headers.clone(),
    };

    Ok(stream(model, context, base.base, mistral_options))
}

/// `usesReasoningEffort(model)`
pub fn uses_reasoning_effort(model: &Model) -> bool {
    matches!(
        model.id.as_str(),
        "mistral-small-2603" | "mistral-small-latest" | "mistral-medium-3.5"
    )
}

/// `usesPromptModeReasoning(model)`
pub fn uses_prompt_mode_reasoning(model: &Model) -> bool {
    model.reasoning && !uses_reasoning_effort(model)
}

/// `mapReasoningEffort(model, level)`
fn map_reasoning_effort(model: &Model, level: ModelThinkingLevel) -> MistralReasoningEffort {
    model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(&level))
        .and_then(|mapped| mapped.as_deref())
        .and_then(MistralReasoningEffort::parse)
        .unwrap_or(MistralReasoningEffort::High)
}
