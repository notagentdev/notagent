//! Custom-Message-Typen und ihre Umwandlung an der LLM-Grenze.
//!
//! 1:1-Port von `packages/agent/src/harness/messages.ts` (168 LOC).

use notagent_ai::types::{Message, TextContent, TextOrImageContent, UserContent, UserMessage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::AgentMessage;

pub const COMPACTION_SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";

pub const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";

pub const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";

pub const BRANCH_SUMMARY_SUFFIX: &str = "</summary>";

/// `BashExecutionMessage` — Bash-Ausführungen über das `!`-Kommando.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashExecutionMessage {
    pub command: String,
    pub output: String,
    /// TS: `number | undefined` (Feld ist vorhanden, der Wert kann fehlen).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
    pub timestamp: i64,
    /// Bei `!!`-Präfix: Nachricht bleibt aus dem LLM-Kontext ausgeschlossen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_from_context: Option<bool>,
}

/// `CustomMessage<T>` — von Apps eingespeiste Nachrichten.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessage {
    pub custom_type: String,
    pub content: UserContent,
    pub display: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    pub timestamp: i64,
}

/// `BranchSummaryMessage`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryMessage {
    pub summary: String,
    pub from_id: String,
    pub timestamp: i64,
}

/// `CompactionSummaryMessage`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummaryMessage {
    pub summary: String,
    pub tokens_before: u64,
    pub timestamp: i64,
}

/// `bashExecutionToText(msg)`
pub fn bash_execution_to_text(message: &BashExecutionMessage) -> String {
    let mut text = format!("Ran `{}`\n", message.command);
    if !message.output.is_empty() {
        text.push_str(&format!("```\n{}\n```", message.output));
    } else {
        text.push_str("(no output)");
    }
    if message.cancelled {
        text.push_str("\n\n(command cancelled)");
    } else if let Some(exit_code) = message.exit_code
        && exit_code != 0
    {
        text.push_str(&format!("\n\nCommand exited with code {exit_code}"));
    }
    if message.truncated
        && let Some(path) = &message.full_output_path
    {
        text.push_str(&format!("\n\n[Output truncated. Full output: {path}]"));
    }
    text
}

/// `createBranchSummaryMessage(summary, fromId, timestamp)`
pub fn create_branch_summary_message(
    summary: impl Into<String>,
    from_id: impl Into<String>,
    timestamp: i64,
) -> BranchSummaryMessage {
    BranchSummaryMessage {
        summary: summary.into(),
        from_id: from_id.into(),
        timestamp,
    }
}

/// `createCompactionSummaryMessage(summary, tokensBefore, timestamp)`
pub fn create_compaction_summary_message(
    summary: impl Into<String>,
    tokens_before: u64,
    timestamp: i64,
) -> CompactionSummaryMessage {
    CompactionSummaryMessage {
        summary: summary.into(),
        tokens_before,
        timestamp,
    }
}

/// `createCustomMessage(customType, content, display, details, timestamp)`
pub fn create_custom_message(
    custom_type: impl Into<String>,
    content: UserContent,
    display: bool,
    details: Option<Value>,
    timestamp: i64,
) -> CustomMessage {
    CustomMessage {
        custom_type: custom_type.into(),
        content,
        display,
        details,
        timestamp,
    }
}

/// `convertToLlm(messages)` — Standardumwandlung an der LLM-Grenze.
pub fn convert_to_llm(messages: &[AgentMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::BashExecution(message) => {
                if message.exclude_from_context.unwrap_or(false) {
                    return None;
                }
                Some(Message::User(UserMessage {
                    content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(
                        bash_execution_to_text(message),
                    ))]),
                    timestamp: message.timestamp,
                }))
            }
            AgentMessage::Custom(message) => {
                let content = match &message.content {
                    UserContent::Text(text) => UserContent::Blocks(vec![TextOrImageContent::Text(
                        TextContent::new(text.clone()),
                    )]),
                    blocks @ UserContent::Blocks(_) => blocks.clone(),
                };
                Some(Message::User(UserMessage {
                    content,
                    timestamp: message.timestamp,
                }))
            }
            AgentMessage::BranchSummary(message) => Some(Message::User(UserMessage {
                content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(
                    format!(
                        "{BRANCH_SUMMARY_PREFIX}{}{BRANCH_SUMMARY_SUFFIX}",
                        message.summary
                    ),
                ))]),
                timestamp: message.timestamp,
            })),
            AgentMessage::CompactionSummary(message) => Some(Message::User(UserMessage {
                content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(
                    format!(
                        "{COMPACTION_SUMMARY_PREFIX}{}{COMPACTION_SUMMARY_SUFFIX}",
                        message.summary
                    ),
                ))]),
                timestamp: message.timestamp,
            })),
            AgentMessage::User(message) => Some(Message::User(message.clone())),
            AgentMessage::Assistant(message) => Some(Message::Assistant(message.clone())),
            AgentMessage::ToolResult(message) => Some(Message::ToolResult(message.clone())),
        })
        .collect()
}
