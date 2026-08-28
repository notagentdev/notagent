use std::collections::BTreeSet;

use notagent_agent::types::AgentMessage;
use notagent_ai::types::{AssistantContent, Message};
use notagent_ai::utils::text::content_text_with;
use serde::{Deserialize, Serialize};

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
            "patch" | "patch_minified" | "multi_patch_minified" => {
                file_ops.edited.insert(path.to_string());
            }
            _ => {}
        }
    }
}

/// Orders two strings the way `Array.prototype.sort` does: by UTF-16 code unit.
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

// ============================================================================
// Operation outline
// ============================================================================

/// What happened to one target, in the words the outline uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Operation {
    Read,
    Written,
    Patched,
    Ran,
}

impl Operation {
    fn label(self) -> &'static str {
        match self {
            Operation::Read => "read",
            Operation::Written => "wrote",
            Operation::Patched => "patched",
            Operation::Ran => "ran",
        }
    }
}

/// One line of the outline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub target: String,
    pub operation: Operation,
}

/// Longest a recorded command may be before the outline cuts it.
const COMMAND_MAX_CHARS: usize = 120;

/// Most lines the outline will carry.
/// Measured on a 377k-token session, keeping only the last operation per target
/// took 457 operations down to 259; the file operations among them cost about
/// 5.9k tokens while the shell commands cost 10.4k, which is why commands are
/// reduced to their first line and the whole thing is capped.
const OUTLINE_MAX_LINES: usize = 200;

/// What a stretch of conversation did, in order, with each target's last
/// operation winning.
/// This is derived from the recorded tool calls rather than written by a model,
/// so unlike the summary it cannot invent a path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperationOutline {
    records: Vec<OperationRecord>,
}

impl OperationOutline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn record(&mut self, target: impl Into<String>, operation: Operation) {
        let target = target.into();
        self.records.retain(|record| record.target != target);
        self.records.push(OperationRecord { target, operation });
    }

    /// The records, oldest surviving operation first.
    pub fn records(&self) -> &[OperationRecord] {
        &self.records
    }

    pub fn extend_from_records(&mut self, records: impl IntoIterator<Item = OperationRecord>) {
        for record in records {
            self.record(record.target, record.operation);
        }
    }
}

/// Records what each tool call in an assistant message did.
pub fn extract_operations_from_message(message: &AgentMessage, outline: &mut OperationOutline) {
    let AgentMessage::Assistant(message) = message else {
        return;
    };

    for block in &message.content {
        let AssistantContent::ToolCall(call) = block else {
            continue;
        };
        let path = call.arguments.get("path").and_then(|path| path.as_str());
        match (call.name.as_str(), path) {
            ("read" | "read_minified", Some(path)) => outline.record(path, Operation::Read),
            ("write", Some(path)) => outline.record(path, Operation::Written),
            ("patch" | "patch_minified" | "multi_patch_minified" | "edit", Some(path)) => {
                outline.record(path, Operation::Patched)
            }
            ("bash", _) => {
                if let Some(command) = call.arguments.get("command").and_then(|arg| arg.as_str()) {
                    outline.record(shorten_command(command), Operation::Ran);
                }
            }
            _ => {}
        }
    }
}

/// A command reduced to something an outline can carry: its first line, capped.
fn shorten_command(command: &str) -> String {
    let first_line = command.lines().next().unwrap_or("").trim();
    let units: Vec<u16> = first_line.encode_utf16().collect();
    let multiline = command.lines().nth(1).is_some();
    if units.len() <= COMMAND_MAX_CHARS && !multiline {
        return first_line.to_string();
    }
    let head = if units.len() > COMMAND_MAX_CHARS {
        String::from_utf16_lossy(&units[..COMMAND_MAX_CHARS])
    } else {
        first_line.to_string()
    };
    format!("{head} …")
}

/// Renders the outline as the section appended to a summary.
/// When the cap bites it says so, because an outline that silently stops is
/// read as a complete record of a session that did less than it did.
pub fn format_operation_outline(outline: &OperationOutline) -> String {
    if outline.is_empty() {
        return String::new();
    }
    let records = outline.records();
    let omitted = records.len().saturating_sub(OUTLINE_MAX_LINES);
    let shown = &records[omitted..];

    let mut lines: Vec<String> = Vec::with_capacity(shown.len() + 1);
    if omitted > 0 {
        lines.push(format!("[{omitted} earlier operations omitted]"));
    }
    for record in shown {
        lines.push(format!("{} {}", record.operation.label(), record.target));
    }
    format!("\n\n<operations>\n{}\n</operations>", lines.join("\n"))
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
/// Call `convert_to_llm` first so custom message types are already folded in.
pub fn serialize_conversation(messages: &[Message]) -> String {
    serialize_parts(messages).join("\n\n")
}

/// A conversation serialized to fit a request budget.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SerializedConversation {
    pub text: String,
    /// How many of the oldest messages had to go, and what they were worth.
    pub dropped_messages: usize,
    pub dropped_tokens: u64,
}

/// Estimated tokens of a serialized fragment, on the same chars/4 basis the
/// rest of compaction measures in.
fn fragment_tokens(text: &str) -> u64 {
    text.encode_utf16().count().div_ceil(4) as u64
}

/// Serializes a conversation, dropping the oldest messages until it fits.
/// The alternative is to send the whole thing and let the provider reject it.
/// Both ways of reacting to that rejection are bad: shrinking by a fixed ratio
/// throws away far more than necessary, and dropping one message per rejection
/// needs hundreds of round trips — measured on a 377k-token session, reaching a
/// 190k target one message at a time took 440 attempts to land within 1,716
/// tokens of where a single ratio step landed in two. Since the size is
/// knowable before sending, it is computed here and cut once.
/// A budget of zero means no limit.
pub fn serialize_conversation_within(messages: &[Message], budget: u64) -> SerializedConversation {
    let parts = serialize_parts(messages);
    let sizes: Vec<u64> = parts.iter().map(|part| fragment_tokens(part)).collect();
    let total: u64 = sizes.iter().sum();

    if budget == 0 || total <= budget {
        return SerializedConversation {
            text: parts.join("\n\n"),
            dropped_messages: 0,
            dropped_tokens: 0,
        };
    }

    // Drop from the front: the newest exchanges are the ones the summary most
    // needs, and the oldest are the ones an earlier summary most likely already
    // covers.
    let mut dropped_tokens = 0;
    let mut first_kept = 0;
    let mut remaining = total;
    while first_kept < parts.len() && remaining > budget {
        remaining -= sizes[first_kept];
        dropped_tokens += sizes[first_kept];
        first_kept += 1;
    }

    SerializedConversation {
        text: parts[first_kept..].join("\n\n"),
        dropped_messages: first_kept,
        dropped_tokens,
    }
}

fn serialize_parts(messages: &[Message]) -> Vec<String> {
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

    parts
}

/// The system prompt every summarization request carries. Its whole job is to
/// stop the model from answering the conversation it was handed.
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";
