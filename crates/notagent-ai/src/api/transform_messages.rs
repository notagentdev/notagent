//! Cross-provider message normalization applied before every request.
//!
//! 1:1 port of `packages/ai/src/api/transform-messages.ts` (223 LOC).

use crate::types::{
    AssistantContent, ImageContent, Message, Model, TextContent, TextOrImageContent, ToolCall,
    ToolResultMessage, UserContent,
};

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

/// Normalizer for tool-call ids, e.g. Anthropic's `^[a-zA-Z0-9_-]{1,64}$`.
pub type NormalizeToolCallId<'a> = &'a dyn Fn(&str) -> String;

/// `replaceImagesWithPlaceholder(content, placeholder)` — collapses runs of images.
fn replace_images_with_placeholder(
    content: &[TextOrImageContent],
    placeholder: &str,
) -> Vec<TextOrImageContent> {
    let mut result = Vec::new();
    let mut previous_was_placeholder = false;

    for block in content {
        match block {
            TextOrImageContent::Image(_) => {
                if !previous_was_placeholder {
                    result.push(TextOrImageContent::Text(TextContent::new(placeholder)));
                }
                previous_was_placeholder = true;
            }
            TextOrImageContent::Text(text) => {
                previous_was_placeholder = text.text == placeholder;
                result.push(block.clone());
            }
        }
    }
    result
}

/// `downgradeUnsupportedImages(messages, model)`
fn downgrade_unsupported_images(messages: Vec<Message>, model: &Model) -> Vec<Message> {
    if model.input.contains(&crate::types::Modality::Image) {
        return messages;
    }

    messages
        .into_iter()
        .map(|message| match message {
            Message::User(mut message) => {
                if let UserContent::Blocks(blocks) = &message.content {
                    message.content = UserContent::Blocks(replace_images_with_placeholder(
                        blocks,
                        NON_VISION_USER_IMAGE_PLACEHOLDER,
                    ));
                }
                Message::User(message)
            }
            Message::ToolResult(mut message) => {
                message.content = replace_images_with_placeholder(
                    &message.content,
                    NON_VISION_TOOL_IMAGE_PLACEHOLDER,
                );
                Message::ToolResult(message)
            }
            other => other,
        })
        .collect()
}

/// `transformMessages(messages, model, normalizeToolCallId?)`
///
/// `timestamp` supplies `Date.now()` for the synthetic tool results.
pub fn transform_messages(
    messages: &[Message],
    model: &Model,
    normalize_tool_call_id: Option<NormalizeToolCallId<'_>>,
    timestamp: i64,
) -> Vec<Message> {
    let mut tool_call_id_map: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    // TS normalizes null/undefined content from untyped callers; the Rust types already
    // guarantee a content array, so that step has no counterpart.
    let image_aware = downgrade_unsupported_images(messages.to_vec(), model);

    // First pass: image downgrade, thinking blocks, tool-call id normalization.
    let mut transformed = Vec::with_capacity(image_aware.len());
    for message in image_aware {
        match message {
            Message::User(message) => transformed.push(Message::User(message)),
            Message::ToolResult(mut message) => {
                if let Some(normalized_id) = tool_call_id_map.get(&message.tool_call_id)
                    && normalized_id != &message.tool_call_id
                {
                    message.tool_call_id = normalized_id.clone();
                }
                transformed.push(Message::ToolResult(message));
            }
            Message::Assistant(mut assistant) => {
                let is_same_model = assistant.provider == model.provider
                    && assistant.api == model.api
                    && assistant.model == model.id;

                let mut content = Vec::with_capacity(assistant.content.len());
                for block in &assistant.content {
                    match block {
                        AssistantContent::Thinking(thinking) => {
                            // Redacted thinking is opaque encrypted content, valid only for
                            // the same model.
                            if thinking.redacted.unwrap_or(false) {
                                if is_same_model {
                                    content.push(block.clone());
                                }
                                continue;
                            }
                            // Same model: keep signed thinking blocks even when the text is
                            // empty (OpenAI encrypted reasoning).
                            if is_same_model
                                && thinking
                                    .thinking_signature
                                    .as_ref()
                                    .is_some_and(|signature| !signature.is_empty())
                            {
                                content.push(block.clone());
                                continue;
                            }
                            if thinking.thinking.trim().is_empty() {
                                continue;
                            }
                            if is_same_model {
                                content.push(block.clone());
                            } else {
                                content.push(AssistantContent::Text(TextContent::new(
                                    thinking.thinking.clone(),
                                )));
                            }
                        }
                        AssistantContent::Text(text) => {
                            if is_same_model {
                                content.push(block.clone());
                            } else {
                                // Cross-model: keep only the text, drop the signature.
                                content.push(AssistantContent::Text(TextContent::new(
                                    text.text.clone(),
                                )));
                            }
                        }
                        AssistantContent::ToolCall(tool_call) => {
                            let mut normalized = tool_call.clone();
                            if !is_same_model && normalized.thought_signature.is_some() {
                                normalized.thought_signature = None;
                            }
                            if !is_same_model && let Some(normalize) = normalize_tool_call_id {
                                let normalized_id = normalize(&tool_call.id);
                                if normalized_id != tool_call.id {
                                    tool_call_id_map
                                        .insert(tool_call.id.clone(), normalized_id.clone());
                                    normalized.id = normalized_id;
                                }
                            }
                            content.push(AssistantContent::ToolCall(normalized));
                        }
                    }
                }
                assistant.content = content;
                transformed.push(Message::Assistant(assistant));
            }
        }
    }

    // Second pass: synthetic results for orphaned tool calls.
    let mut result: Vec<Message> = Vec::new();
    let mut pending_tool_calls: Vec<ToolCall> = Vec::new();
    let mut existing_tool_result_ids: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    fn insert_synthetic_tool_results(
        result: &mut Vec<Message>,
        pending_tool_calls: &mut Vec<ToolCall>,
        existing_tool_result_ids: &mut std::collections::BTreeSet<String>,
        timestamp: i64,
    ) {
        if pending_tool_calls.is_empty() {
            return;
        }
        for tool_call in pending_tool_calls.iter() {
            if existing_tool_result_ids.contains(&tool_call.id) {
                continue;
            }
            result.push(Message::ToolResult(ToolResultMessage {
                tool_call_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                content: vec![TextOrImageContent::Text(TextContent::new(
                    "No result provided",
                ))],
                details: None,
                usage: None,
                added_tool_names: None,
                is_error: true,
                timestamp,
            }));
        }
        pending_tool_calls.clear();
        existing_tool_result_ids.clear();
    }

    for message in transformed {
        match message {
            Message::Assistant(assistant) => {
                insert_synthetic_tool_results(
                    &mut result,
                    &mut pending_tool_calls,
                    &mut existing_tool_result_ids,
                    timestamp,
                );
                // Errored and aborted turns are incomplete and must not be replayed.
                if matches!(
                    assistant.stop_reason,
                    crate::types::StopReason::Error | crate::types::StopReason::Aborted
                ) {
                    continue;
                }
                let tool_calls: Vec<ToolCall> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantContent::ToolCall(tool_call) => Some(tool_call.clone()),
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    pending_tool_calls = tool_calls;
                    existing_tool_result_ids.clear();
                }
                result.push(Message::Assistant(assistant));
            }
            Message::ToolResult(message) => {
                existing_tool_result_ids.insert(message.tool_call_id.clone());
                result.push(Message::ToolResult(message));
            }
            Message::User(message) => {
                // A user message interrupts the tool flow.
                insert_synthetic_tool_results(
                    &mut result,
                    &mut pending_tool_calls,
                    &mut existing_tool_result_ids,
                    timestamp,
                );
                result.push(Message::User(message));
            }
        }
    }

    insert_synthetic_tool_results(
        &mut result,
        &mut pending_tool_calls,
        &mut existing_tool_result_ids,
        timestamp,
    );
    result
}

/// Helper used by the Anthropic port: `(see attached image)` placeholder handling lives
/// in the provider, this only exposes the image type for reuse.
pub fn image_placeholder(mime_type: &str, data: &str) -> ImageContent {
    ImageContent {
        data: data.to_string(),
        mime_type: mime_type.to_string(),
    }
}
