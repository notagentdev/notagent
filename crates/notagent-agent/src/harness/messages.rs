use notagent_ai::types::{Message, TextContent, TextOrImageContent, UserContent, UserMessage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::AgentMessage;

pub const COMPACTION_SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";

pub const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";

pub const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";

pub const BRANCH_SUMMARY_SUFFIX: &str = "</summary>";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashExecutionMessage {
    pub command: String,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_from_context: Option<bool>,
}

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
    /// What the context measured once the compaction had been applied. Absent
    /// for a compaction written before the figure was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_after: Option<u64>,
    pub timestamp: i64,
    /// Where the history this summary replaced can still be read. Set when the
    /// context is built from a session that lives in a file; never stored,
    /// because the path belongs to wherever the file is now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<CompactionRecovery>,
}

/// The session log a summary's replaced history can be looked up in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionRecovery {
    pub session_file: String,
    /// Id of the compaction entry; the replaced history is the chain of
    /// `parentId` links leading back from it.
    pub compaction_id: String,
}

/// Tells the model where the history behind a summary is. Without it a
/// summary has to carry every output the next step might need verbatim, and
/// what it drops can only be re-run or guessed.
///
/// Reading instructions point at bash for single lines because one entry can
/// hold a whole tool result — hundreds of kilobytes on one line, beyond what a
/// line-oriented file reader shows.
pub fn compaction_recovery_text(recovery: &CompactionRecovery) -> String {
    format!(
        "\n\n## Context recovery\nEverything before this summary is still on disk, in this session's log (JSON Lines, append-only):\n  {file}\nThis summary is the entry with id \"{id}\". If you need exact command output, file contents, error text, or the wording of an earlier message, look it up there instead of guessing or re-running.\n- One JSON object per line; `type` says what it is. The conversation is in lines with `type: \"message\"`, whose `message.role` is `user`, `assistant` or `toolResult`. A tool result's `message.toolCallId` matches the `id` of a `toolCall` block in an earlier assistant message. Other types are bookkeeping.\n- Entries form a tree through `id` and `parentId`, and the file can hold branches this conversation later left. The history this summary replaced is the chain of `parentId` links leading back from entry \"{id}\"; an earlier `type: \"compaction\"` entry on that chain starts an older window.\n- Lines can be very long. Grep the file for a keyword to find line numbers, then extract single lines with bash, for example `sed -n '<N>p' <file> | jq -r '.message.content[] | select(.type == \"text\") | .text'`, rather than reading large ranges.",
        file = recovery.session_file,
        id = recovery.compaction_id,
    )
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
    tokens_after: Option<u64>,
    timestamp: i64,
) -> CompactionSummaryMessage {
    CompactionSummaryMessage {
        summary: summary.into(),
        tokens_before,
        tokens_after,
        timestamp,
        recovery: None,
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
                        "{COMPACTION_SUMMARY_PREFIX}{}{COMPACTION_SUMMARY_SUFFIX}{}",
                        message.summary,
                        message
                            .recovery
                            .as_ref()
                            .map(compaction_recovery_text)
                            .unwrap_or_default()
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
