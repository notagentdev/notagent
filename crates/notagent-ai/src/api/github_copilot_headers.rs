//! GitHub Copilot request headers.
//!
//! 1:1 port of `packages/ai/src/api/github-copilot-headers.ts` (37 LOC).

use std::collections::BTreeMap;

use crate::types::{Message, TextOrImageContent, UserContent};

/// `inferCopilotInitiator(messages)` — whether the request follows a user message.
pub fn infer_copilot_initiator(messages: &[Message]) -> &'static str {
    match messages.last() {
        Some(Message::User(_)) | None => "user",
        Some(_) => "agent",
    }
}

/// `hasCopilotVisionInput(messages)`
pub fn has_copilot_vision_input(messages: &[Message]) -> bool {
    messages.iter().any(|message| match message {
        Message::User(message) => match &message.content {
            UserContent::Blocks(blocks) => blocks
                .iter()
                .any(|block| matches!(block, TextOrImageContent::Image(_))),
            UserContent::Text(_) => false,
        },
        Message::ToolResult(message) => message
            .content
            .iter()
            .any(|block| matches!(block, TextOrImageContent::Image(_))),
        Message::Assistant(_) => false,
    })
}

/// `buildCopilotDynamicHeaders({ messages, hasImages })`
pub fn build_copilot_dynamic_headers(
    messages: &[Message],
    has_images: bool,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert(
        "X-Initiator".to_string(),
        infer_copilot_initiator(messages).to_string(),
    );
    headers.insert(
        "Openai-Intent".to_string(),
        "conversation-edits".to_string(),
    );
    if has_images {
        headers.insert("Copilot-Vision-Request".to_string(), "true".to_string());
    }
    headers
}
