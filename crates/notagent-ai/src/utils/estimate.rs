use std::collections::BTreeSet;

use serde_json::Value;

use crate::types::{
    AssistantContent, Context, Message, StopReason, TextOrImageContent, Tool, Usage, UserContent,
};

const CHARS_PER_TOKEN: usize = 4;
const ESTIMATED_IMAGE_CHARS: usize = 4800;

/// `ContextUsageEstimate`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContextUsageEstimate {
    /// Estimated total context tokens.
    pub tokens: u64,
    /// Tokens reported by the most recent applicable assistant usage block.
    pub usage_tokens: u64,
    /// Estimated tokens after that usage block.
    pub trailing_tokens: u64,
    /// Index of the message that provided usage, or `None`.
    pub last_usage_index: Option<usize>,
}

/// `calculateContextTokens(usage)` — `usage.totalTokens || input + output + cacheRead + cacheWrite`.
/// `0` is falsy in JS, so a reported zero falls back to the sum just like a missing field.
pub fn calculate_context_tokens(usage: &Usage) -> u64 {
    match usage.total_tokens {
        Some(total) if total != 0 => total,
        _ => usage.input + usage.output + usage.cache_read + usage.cache_write,
    }
}

/// `safeJsonStringify(value)`
fn safe_json_stringify(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[unserializable]".to_string())
}

/// Character count of user/tool-result content.
fn estimate_text_and_image_content_chars(content: &[TextOrImageContent]) -> usize {
    content
        .iter()
        .map(|block| match block {
            TextOrImageContent::Text(text) => text.text.chars().count(),
            TextOrImageContent::Image(_) => ESTIMATED_IMAGE_CHARS,
        })
        .sum()
}

/// `estimateTextTokens(text)` — `Math.ceil(text.length / 4)` on UTF-16 code units.
pub fn estimate_text_tokens(text: &str) -> u64 {
    let length = text.encode_utf16().count();
    length.div_ceil(CHARS_PER_TOKEN) as u64
}

/// `estimateTextAndImageContentTokens(content)`
pub fn estimate_text_and_image_content_tokens(content: &UserContent) -> u64 {
    let chars = match content {
        UserContent::Text(text) => text.encode_utf16().count(),
        UserContent::Blocks(blocks) => estimate_text_and_image_content_chars(blocks),
    };
    chars.div_ceil(CHARS_PER_TOKEN) as u64
}

/// `estimateMessageTokens(message)`
pub fn estimate_message_tokens(message: &Message) -> u64 {
    match message {
        Message::User(message) => estimate_text_and_image_content_tokens(&message.content),
        Message::ToolResult(message) => {
            estimate_text_and_image_content_chars(&message.content).div_ceil(CHARS_PER_TOKEN) as u64
        }
        Message::Assistant(message) => {
            let mut chars = 0usize;
            for block in &message.content {
                match block {
                    AssistantContent::Text(text) => chars += text.text.encode_utf16().count(),
                    AssistantContent::Thinking(thinking) => {
                        chars += thinking.thinking.encode_utf16().count()
                    }
                    AssistantContent::ToolCall(tool_call) => {
                        chars += tool_call.name.encode_utf16().count()
                            + safe_json_stringify(&tool_call.arguments)
                                .encode_utf16()
                                .count();
                    }
                }
            }
            chars.div_ceil(CHARS_PER_TOKEN) as u64
        }
    }
}

/// `getLastAssistantUsageInfo(messages)` — skips usage invalidated by a newer prefix message.
fn last_assistant_usage_info(messages: &[Message]) -> Option<(Usage, usize)> {
    let mut latest_prefix_timestamp = i64::MIN;
    let mut usage_info = None;

    for (index, message) in messages.iter().enumerate() {
        if let Message::Assistant(assistant) = message {
            let usage_applies_to_prefix = assistant.timestamp >= latest_prefix_timestamp;
            if usage_applies_to_prefix
                && assistant.stop_reason != StopReason::Aborted
                && assistant.stop_reason != StopReason::Error
                && calculate_context_tokens(&assistant.usage) > 0
            {
                usage_info = Some((assistant.usage, index));
            }
        }
        latest_prefix_timestamp = latest_prefix_timestamp.max(message.timestamp());
    }

    usage_info
}

fn estimate_messages(messages: &[Message]) -> ContextUsageEstimate {
    if let Some((usage, index)) = last_assistant_usage_info(messages) {
        let usage_tokens = calculate_context_tokens(&usage);
        let trailing_tokens: u64 = messages[index + 1..]
            .iter()
            .map(estimate_message_tokens)
            .sum();
        return ContextUsageEstimate {
            tokens: usage_tokens + trailing_tokens,
            usage_tokens,
            trailing_tokens,
            last_usage_index: Some(index),
        };
    }

    let tokens: u64 = messages.iter().map(estimate_message_tokens).sum();
    ContextUsageEstimate {
        tokens,
        usage_tokens: 0,
        trailing_tokens: tokens,
        last_usage_index: None,
    }
}

/// `estimateToolsTokens(tools)`
fn estimate_tools_tokens(tools: Option<&[Tool]>) -> u64 {
    match tools {
        None => 0,
        Some([]) => 0,
        Some(tools) => estimate_text_tokens(&safe_json_stringify(&tools)),
    }
}

/// `estimateContextTokens(messages)` — message list without system prompt or tools.
pub fn estimate_messages_tokens(messages: &[Message]) -> ContextUsageEstimate {
    estimate_messages(messages)
}

/// `estimateContextTokens(context)`
pub fn estimate_context_tokens(context: &Context) -> ContextUsageEstimate {
    let estimate = estimate_messages(&context.messages);

    if let Some(last_usage_index) = estimate.last_usage_index {
        let added_names: BTreeSet<&str> = context.messages[last_usage_index + 1..]
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult(message) => message.added_tool_names.as_ref(),
                _ => None,
            })
            .flatten()
            .map(String::as_str)
            .collect();
        let added_tools: Vec<Tool> = context
            .tools
            .as_ref()
            .map(|tools| {
                tools
                    .iter()
                    .filter(|tool| added_names.contains(tool.name.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let added_tool_tokens = if context.tools.is_some() {
            estimate_tools_tokens(Some(&added_tools))
        } else {
            0
        };
        return ContextUsageEstimate {
            tokens: estimate.tokens + added_tool_tokens,
            usage_tokens: estimate.usage_tokens,
            trailing_tokens: estimate.trailing_tokens + added_tool_tokens,
            last_usage_index: estimate.last_usage_index,
        };
    }

    let prefix_tokens = context
        .system_prompt
        .as_ref()
        .map_or(0, |prompt| estimate_text_tokens(prompt))
        + estimate_tools_tokens(context.tools.as_deref());

    ContextUsageEstimate {
        tokens: estimate.tokens + prefix_tokens,
        usage_tokens: estimate.usage_tokens,
        trailing_tokens: estimate.trailing_tokens + prefix_tokens,
        last_usage_index: estimate.last_usage_index,
    }
}

/// Helper mirroring `JSON.stringify` for arbitrary values used in estimates.
pub fn safe_json_length(value: &Value) -> usize {
    safe_json_stringify(value).encode_utf16().count()
}
