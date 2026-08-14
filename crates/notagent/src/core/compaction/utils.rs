//! Port of `packages/coding-agent/src/core/compaction/utils.ts`.
//!
//! What compaction and branch summarization share: which files a run touched,
//! and how a conversation is flattened into something a model will summarize
//! rather than continue.
//!
//! The flattening is the point. Handing the transcript over as messages invites
//! the model to answer the last one; handing it over as a single block of
//! labelled text does not.

use std::collections::BTreeSet;

use notagent_agent::types::AgentMessage;
use notagent_ai::types::{AssistantContent, Message};
use notagent_ai::utils::text::content_text_with;

/// The files a stretch of conversation read, wrote and edited.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileOperations {
    pub read: BTreeSet<String>,
    pub written: BTreeSet<String>,
    pub edited: BTreeSet<String>,
}

pub fn create_file_ops() -> FileOperations {
    FileOperations::default()
}

/// Records the file each tool call in an assistant message touched.
pub fn extract_file_ops_from_message(message: &AgentMessage, file_ops: &mut FileOperations) {
    let AgentMessage::Assistant(message) = message else {
        return;
    };

    for block in &message.content {
        let AssistantContent::ToolCall(call) = block else {
            continue;
        };
        let Some(path) = call.arguments.get("path").and_then(|path| path.as_str()) else {
            continue;
        };

        match call.name.as_str() {
            "read" | "read_minified" => {
                file_ops.read.insert(path.to_string());
            }
            "write" => {
                file_ops.written.insert(path.to_string());
            }
            "edit" | "patch_minified" | "multi_patch_minified" => {
                file_ops.edited.insert(path.to_string());
            }
            _ => {}
        }
    }
}

/// Orders two strings the way `Array.prototype.sort` does: by UTF-16 code unit.
///
/// It differs from Rust's byte order only above the BMP, but the file lists end
/// up in a summary the next session reads, and a stable order across the two
/// implementations is worth four lines.
pub fn js_string_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

/// Splits the touched files into those only read and those changed.
pub fn compute_file_lists(file_ops: &FileOperations) -> (Vec<String>, Vec<String>) {
    let mut modified: Vec<String> = file_ops.edited.union(&file_ops.written).cloned().collect();
    let mut read_only: Vec<String> = file_ops
        .read
        .iter()
        .filter(|path| !modified.contains(path))
        .cloned()
        .collect();
    read_only.sort_by(|left, right| js_string_cmp(left, right));
    modified.sort_by(|left, right| js_string_cmp(left, right));
    (read_only, modified)
}

/// Renders the file lists as the XML tags appended to a summary.
pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !read_files.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_files.join("\n")
        ));
    }
    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}

/// Longest a tool result may be in a serialized summary request.
const TOOL_RESULT_MAX_CHARS: usize = 2000;

/// Keeps the beginning and says how much was dropped. The tail of a tool result
/// is rarely what a summary needs, and the whole point is to stay inside a
/// sensible request budget.
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= max_chars {
        return text.to_string();
    }
    let truncated = units.len() - max_chars;
    format!(
        "{}\n\n[... {truncated} more characters truncated]",
        String::from_utf16_lossy(&units[..max_chars])
    )
}

/// Serializes LLM messages to one block of labelled text.
///
/// Call `convert_to_llm` first so custom message types are already folded in.
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for message in messages {
        match message {
            Message::User(message) => {
                let content = content_text_with(&message.content, "");
                if !content.is_empty() {
                    parts.push(format!("[User]: {content}"));
                }
            }
            Message::Assistant(message) => {
                let mut thinking_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<String> = Vec::new();

                for block in &message.content {
                    match block {
                        AssistantContent::Thinking(thinking) => {
                            thinking_parts.push(thinking.thinking.clone());
                        }
                        AssistantContent::ToolCall(call) => {
                            let arguments = call
                                .arguments
                                .iter()
                                .map(|(key, value)| format!("{key}={value}"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            tool_calls.push(format!("{}({arguments})", call.name));
                        }
                        _ => {}
                    }
                }

                if !thinking_parts.is_empty() {
                    parts.push(format!(
                        "[Assistant thinking]: {}",
                        thinking_parts.join("\n")
                    ));
                }
                if message
                    .content
                    .iter()
                    .any(|block| matches!(block, AssistantContent::Text(_)))
                {
                    parts.push(format!(
                        "[Assistant]: {}",
                        notagent_ai::utils::text::content_text(&message.content)
                    ));
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }
            }
            Message::ToolResult(message) => {
                let content = content_text_with(&message.content, "");
                if !content.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&content, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
        }
    }

    parts.join("\n\n")
}

/// The system prompt every summarization request carries. Its whole job is to
/// stop the model from answering the conversation it was handed.
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";
