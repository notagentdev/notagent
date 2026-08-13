//! Anthropic Messages protocol.
//!
//! 1:1 port of `packages/ai/src/api/anthropic-messages.ts` (1 352 LOC). This module
//! holds the response side: the stream state machine that turns Anthropic's SSE events
//! into `AssistantMessageEvent`s and the final `AssistantMessage`. Request building and
//! the HTTP transport follow in the same task.
//!
//! Architecture note (master plan): TypeScript mutates one `partial` object in place and
//! keeps streaming scratch fields (`index`, `partialJson`) on the content blocks until
//! `content_block_stop`. Here the scratch lives in [`AnthropicStreamState`] and never
//! reaches the content types; snapshots are cloned per event. Reading old session files
//! that still contain `partialJson` stays lossless through `ToolCall::extra`.

use serde_json::Value;

use crate::models::calculate_cost;
use crate::types::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, DoneReason, Model, StopReason,
    TextContent, ThinkingContent, ToolCall, Usage,
};
use crate::utils::json_parse::parse_streaming_json;

/// The six event names the TS implementation processes; everything else is ignored.
pub const ANTHROPIC_MESSAGE_EVENTS: [&str; 6] = [
    "message_start",
    "message_delta",
    "message_stop",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
];

/// Per-block streaming scratch: Anthropic's block index and the raw tool-argument JSON.
#[derive(Debug, Default, Clone)]
struct BlockScratch {
    index: i64,
    partial_json: String,
}

/// Streaming state of one Anthropic response.
pub struct AnthropicStreamState {
    /// The message being assembled; `partial` in TS.
    pub output: AssistantMessage,
    scratch: Vec<BlockScratch>,
    model: Model,
    /// Maps Claude Code tool names back to the caller's names for OAuth requests.
    is_oauth: bool,
    tool_names: Vec<String>,
    saw_message_start: bool,
    saw_message_stop: bool,
}

/// Error of the stream state machine; the caller turns it into an `error` event.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AnthropicStreamError(pub String);

impl AnthropicStreamState {
    /// Creates the pending output message, matching the TS initializer.
    pub fn new(model: &Model, is_oauth: bool, tool_names: Vec<String>, timestamp: i64) -> Self {
        AnthropicStreamState {
            output: AssistantMessage {
                content: vec![],
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
            scratch: Vec::new(),
            model: model.clone(),
            is_oauth,
            tool_names,
            saw_message_start: false,
            saw_message_stop: false,
        }
    }

    /// A snapshot of the message as the TS code would have handed it out as `partial`.
    fn snapshot(&self) -> AssistantMessage {
        self.output.clone()
    }

    fn block_position(&self, index: i64) -> Option<usize> {
        self.scratch
            .iter()
            .position(|scratch| scratch.index == index)
    }

    /// `fromClaudeCodeName(name, tools)` — case-insensitive match against the caller's tools.
    fn from_claude_code_name(&self, name: &str) -> String {
        if !self.is_oauth {
            return name.to_string();
        }
        let lower_name = name.to_lowercase();
        self.tool_names
            .iter()
            .find(|tool_name| tool_name.to_lowercase() == lower_name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }

    /// Whether the stream ended without `message_stop` after a `message_start`.
    pub fn ended_without_message_stop(&self) -> bool {
        self.saw_message_start && !self.saw_message_stop
    }

    /// Processes one decoded Anthropic event and returns the resulting stream events.
    pub fn process_event(
        &mut self,
        event: &Value,
    ) -> Result<Vec<AssistantMessageEvent>, AnthropicStreamError> {
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut emitted = Vec::new();

        match event_type {
            "message_start" => {
                self.saw_message_start = true;
                let message = event.get("message");
                self.output.response_id = message
                    .and_then(|message| message.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let usage = message.and_then(|message| message.get("usage"));
                // Captured here so input counts survive an early abort.
                self.output.usage.input = number(usage, "input_tokens");
                self.output.usage.output = number(usage, "output_tokens");
                self.output.usage.cache_read = number(usage, "cache_read_input_tokens");
                self.output.usage.cache_write = number(usage, "cache_creation_input_tokens");
                self.output.usage.cache_write1h = Some(
                    usage
                        .and_then(|usage| usage.get("cache_creation"))
                        .map(|creation| number(Some(creation), "ephemeral_1h_input_tokens"))
                        .unwrap_or(0),
                );
                self.recompute_usage();
            }
            "content_block_start" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let block = event.get("content_block");
                let block_type = block
                    .and_then(|block| block.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match block_type {
                    "text" => {
                        self.push_block(
                            AssistantContent::Text(TextContent::new(
                                block
                                    .and_then(|block| block.get("text"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default(),
                            )),
                            index,
                        );
                        emitted.push(AssistantMessageEvent::TextStart {
                            content_index: self.output.content.len() - 1,
                            partial: self.snapshot(),
                        });
                    }
                    "thinking" => {
                        self.push_block(
                            AssistantContent::Thinking(ThinkingContent {
                                thinking: block
                                    .and_then(|block| block.get("thinking"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                // TS defaults the signature to "" here, not to undefined.
                                thinking_signature: Some(
                                    block
                                        .and_then(|block| block.get("signature"))
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                ),
                                redacted: None,
                                extra: Default::default(),
                            }),
                            index,
                        );
                        emitted.push(AssistantMessageEvent::ThinkingStart {
                            content_index: self.output.content.len() - 1,
                            partial: self.snapshot(),
                        });
                    }
                    "redacted_thinking" => {
                        self.push_block(
                            AssistantContent::Thinking(ThinkingContent {
                                thinking: "[Reasoning redacted]".to_string(),
                                thinking_signature: block
                                    .and_then(|block| block.get("data"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                redacted: Some(true),
                                extra: Default::default(),
                            }),
                            index,
                        );
                        emitted.push(AssistantMessageEvent::ThinkingStart {
                            content_index: self.output.content.len() - 1,
                            partial: self.snapshot(),
                        });
                    }
                    "tool_use" => {
                        let name = block
                            .and_then(|block| block.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let arguments = block
                            .and_then(|block| block.get("input"))
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        self.push_block(
                            AssistantContent::ToolCall(ToolCall {
                                id: block
                                    .and_then(|block| block.get("id"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                name: self.from_claude_code_name(name),
                                arguments,
                                thought_signature: None,
                                namespace: None,
                                extra: Default::default(),
                            }),
                            index,
                        );
                        emitted.push(AssistantMessageEvent::ToolcallStart {
                            content_index: self.output.content.len() - 1,
                            partial: self.snapshot(),
                        });
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let delta = event.get("delta");
                let delta_type = delta
                    .and_then(|delta| delta.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let Some(position) = self.block_position(index) else {
                    return Ok(emitted);
                };

                match delta_type {
                    "text_delta" => {
                        let text = delta
                            .and_then(|delta| delta.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if let Some(AssistantContent::Text(block)) =
                            self.output.content.get_mut(position)
                        {
                            block.text.push_str(text);
                            emitted.push(AssistantMessageEvent::TextDelta {
                                content_index: position,
                                delta: text.to_string(),
                                partial: self.snapshot(),
                            });
                        }
                    }
                    "thinking_delta" => {
                        let text = delta
                            .and_then(|delta| delta.get("thinking"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if let Some(AssistantContent::Thinking(block)) =
                            self.output.content.get_mut(position)
                        {
                            block.thinking.push_str(text);
                            emitted.push(AssistantMessageEvent::ThinkingDelta {
                                content_index: position,
                                delta: text.to_string(),
                                partial: self.snapshot(),
                            });
                        }
                    }
                    "input_json_delta" => {
                        let partial_json = delta
                            .and_then(|delta| delta.get("partial_json"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let is_tool_call = matches!(
                            self.output.content.get(position),
                            Some(AssistantContent::ToolCall(_))
                        );
                        if is_tool_call {
                            self.scratch[position].partial_json.push_str(partial_json);
                            let parsed =
                                parse_streaming_json(Some(&self.scratch[position].partial_json));
                            if let Some(AssistantContent::ToolCall(block)) =
                                self.output.content.get_mut(position)
                            {
                                block.arguments = parsed.as_object().cloned().unwrap_or_default();
                            }
                            emitted.push(AssistantMessageEvent::ToolcallDelta {
                                content_index: position,
                                delta: partial_json.to_string(),
                                partial: self.snapshot(),
                            });
                        }
                    }
                    "signature_delta" => {
                        let signature = delta
                            .and_then(|delta| delta.get("signature"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if let Some(AssistantContent::Thinking(block)) =
                            self.output.content.get_mut(position)
                        {
                            block
                                .thinking_signature
                                .get_or_insert_default()
                                .push_str(signature);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let Some(position) = self.block_position(index) else {
                    return Ok(emitted);
                };
                // TS deletes the scratch fields from the block here; ours live outside it.
                let partial_json = std::mem::take(&mut self.scratch[position].partial_json);
                self.scratch[position].index = i64::MIN;

                match self.output.content.get_mut(position) {
                    Some(AssistantContent::Text(block)) => {
                        let content = block.text.clone();
                        emitted.push(AssistantMessageEvent::TextEnd {
                            content_index: position,
                            content,
                            partial: self.snapshot(),
                        });
                    }
                    Some(AssistantContent::Thinking(block)) => {
                        let content = block.thinking.clone();
                        emitted.push(AssistantMessageEvent::ThinkingEnd {
                            content_index: position,
                            content,
                            partial: self.snapshot(),
                        });
                    }
                    Some(AssistantContent::ToolCall(block)) => {
                        block.arguments = parse_streaming_json(Some(&partial_json))
                            .as_object()
                            .cloned()
                            .unwrap_or_default();
                        let tool_call = block.clone();
                        emitted.push(AssistantMessageEvent::ToolcallEnd {
                            content_index: position,
                            tool_call,
                            partial: self.snapshot(),
                        });
                    }
                    None => {}
                }
            }
            "message_delta" => {
                let delta = event.get("delta");
                if let Some(stop_reason) = delta
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.output.raw_stop_reason = Some(stop_reason.to_string());
                    let stop_details = delta.and_then(|delta| delta.get("stop_details"));
                    let mapped = map_stop_reason(stop_reason, stop_details)?;
                    self.output.stop_reason = mapped.0;
                    if let Some(error_message) = mapped.1 {
                        self.output.error_message = Some(error_message);
                    }
                }
                // Only present fields are updated, so input counts from message_start
                // survive proxies that omit them here.
                if let Some(usage) = event.get("usage") {
                    if let Some(value) = usage.get("input_tokens").and_then(Value::as_u64) {
                        self.output.usage.input = value;
                    }
                    if let Some(value) = usage.get("output_tokens").and_then(Value::as_u64) {
                        self.output.usage.output = value;
                    }
                    if let Some(value) =
                        usage.get("cache_read_input_tokens").and_then(Value::as_u64)
                    {
                        self.output.usage.cache_read = value;
                    }
                    if let Some(value) = usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                    {
                        self.output.usage.cache_write = value;
                    }
                    // Anthropic reports thinking tokens as a subset of output_tokens.
                    if let Some(value) = usage
                        .get("output_tokens_details")
                        .and_then(|details| details.get("thinking_tokens"))
                        .and_then(Value::as_u64)
                    {
                        self.output.usage.reasoning = Some(value);
                    }
                }
                self.recompute_usage();
            }
            "message_stop" => {
                self.saw_message_stop = true;
            }
            _ => {}
        }

        Ok(emitted)
    }

    fn push_block(&mut self, block: AssistantContent, index: i64) {
        self.output.content.push(block);
        self.scratch.push(BlockScratch {
            index,
            partial_json: String::new(),
        });
    }

    /// Anthropic sends no total; TS computes it and recalculates the cost.
    fn recompute_usage(&mut self) {
        self.output.usage.total_tokens = Some(
            self.output.usage.input
                + self.output.usage.output
                + self.output.usage.cache_read
                + self.output.usage.cache_write,
        );
        let mut usage = self.output.usage;
        calculate_cost(&self.model, &mut usage);
        self.output.usage = usage;
    }

    /// The terminal checks of the TS stream body, after the event loop.
    pub fn finish(&self) -> Result<DoneReason, AnthropicStreamError> {
        match self.output.stop_reason {
            StopReason::Pending => Err(AnthropicStreamError(
                "Anthropic stream ended without a stop reason".to_string(),
            )),
            StopReason::Aborted | StopReason::Error => Err(AnthropicStreamError(
                self.output
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "An unknown error occurred".to_string()),
            )),
            StopReason::Stop => Ok(DoneReason::Stop),
            StopReason::Length => Ok(DoneReason::Length),
            StopReason::ToolUse => Ok(DoneReason::ToolUse),
            StopReason::Deferred => Ok(DoneReason::Deferred),
        }
    }
}

fn number(value: Option<&Value>, field: &str) -> u64 {
    value
        .and_then(|value| value.get(field))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// `mapStopReason(reason, stopDetails)` — unknown values fail loudly, as in TS.
pub fn map_stop_reason(
    reason: &str,
    stop_details: Option<&Value>,
) -> Result<(StopReason, Option<String>), AnthropicStreamError> {
    Ok(match reason {
        "end_turn" => (StopReason::Stop, None),
        "max_tokens" => (StopReason::Length, None),
        "tool_use" => (StopReason::ToolUse, None),
        "refusal" => (
            StopReason::Error,
            Some(
                stop_details
                    .and_then(|details| details.get("explanation"))
                    .and_then(Value::as_str)
                    .filter(|explanation| !explanation.is_empty())
                    .unwrap_or("The model refused to complete the request")
                    .to_string(),
            ),
        ),
        // Resubmitting is good enough.
        "pause_turn" => (StopReason::Stop, None),
        // We never send stop sequences, so this should not occur.
        "stop_sequence" => (StopReason::Stop, None),
        "sensitive" => (
            StopReason::Error,
            Some("Provider stopped with: sensitive".to_string()),
        ),
        other => {
            return Err(AnthropicStreamError(format!(
                "Unhandled stop reason: {other}"
            )));
        }
    })
}
