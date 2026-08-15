//! Port of `packages/coding-agent/src/cli/initial-message.ts`.
//!
//! Piped stdin, `@file` text and the first CLI message become one prompt. The
//! first message is *consumed* — it leaves the message list, so the caller's
//! loop over the remaining messages does not send it twice.

use notagent_ai::types::ImageContent;

use crate::cli::args::Args;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitialMessageResult {
    pub initial_message: Option<String>,
    pub initial_images: Option<Vec<ImageContent>>,
}

#[derive(Debug, Clone, Default)]
pub struct InitialMessageInput {
    pub file_text: Option<String>,
    pub file_images: Option<Vec<ImageContent>>,
    pub stdin_content: Option<String>,
}

pub fn build_initial_message(
    parsed: &mut Args,
    input: InitialMessageInput,
) -> InitialMessageResult {
    let mut parts: Vec<String> = Vec::new();
    if let Some(stdin_content) = input.stdin_content {
        parts.push(stdin_content);
    }
    if let Some(file_text) = input.file_text.filter(|text| !text.is_empty()) {
        parts.push(file_text);
    }

    if !parsed.messages.is_empty() {
        parts.push(parsed.messages.remove(0));
    }

    InitialMessageResult {
        initial_message: (!parts.is_empty()).then(|| parts.join("")),
        initial_images: input.file_images.filter(|images| !images.is_empty()),
    }
}
