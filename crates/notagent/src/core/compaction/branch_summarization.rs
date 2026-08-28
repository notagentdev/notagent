use notagent_agent::types::{AgentMessage, StreamFn, ThinkingLevel};
use notagent_ai::types::{Model, StopReason, Usage};
use notagent_ai::utils::retry::{RetryCallbacks, RetryPolicy};
use notagent_ai::utils::text::content_text;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::compaction::compaction::{
    SummarizationRequest, complete_summarization, estimate_tokens,
};
use crate::core::compaction::utils::{
    FileOperations, compute_file_lists, create_file_ops, extract_file_ops_from_message,
    format_file_operations, serialize_conversation,
};
use crate::core::messages::convert_to_llm;
use crate::core::session_manager::{SessionEntry, session_entry_to_context_messages};

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BranchSummaryResult {
    pub summary: Option<String>,
    pub usage: Option<Usage>,
    pub read_files: Option<Vec<String>>,
    pub modified_files: Option<Vec<String>>,
    pub aborted: bool,
    pub error: Option<String>,
}

/// What a branch-summary entry stores about the files its branch touched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BranchPreparation {
    /// Messages selected for summarization, oldest first.
    pub messages: Vec<AgentMessage>,
    pub file_ops: FileOperations,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollectEntriesResult {
    /// Entries to summarize, oldest first.
    pub entries: Vec<SessionEntry>,
    /// Deepest node shared by the old and the new position.
    pub common_ancestor_id: Option<String>,
}

/// The read-only slice of the session manager this module needs.
pub trait BranchSummarySession {
    fn get_branch(&self, leaf_id: &str) -> Vec<SessionEntry>;
    fn get_entry(&self, id: &str) -> Option<SessionEntry>;
}

#[derive(Clone, Default)]
pub struct GenerateBranchSummaryOptions {
    pub api_key: Option<String>,
    pub headers: Option<Vec<(String, String)>>,
    pub env: Option<Vec<(String, String)>>,
    pub signal: Option<CancellationToken>,
    pub custom_instructions: Option<String>,
    /// When set, `custom_instructions` replaces the default prompt instead of
    /// being appended to it.
    pub replace_instructions: bool,
    /// Tokens reserved for the prompt and the response. Default 16384.
    pub reserve_tokens: Option<u64>,
    /// The session's stream function, so SDK request behaviour is preserved
    /// without running through agent state or events.
    pub stream_fn: Option<StreamFn>,
    pub retry: Option<RetryPolicy>,
    pub callbacks: Option<RetryCallbacks>,
    pub thinking_level: Option<ThinkingLevel>,
}

// ============================================================================
// Entry collection
// ============================================================================

/// Collects the entries that would be abandoned by navigating from `old_leaf_id`
/// to `target_id`.
/// Compaction boundaries are not stopped at: their summaries are content in
/// their own right and belong in what the branch summary sees.
pub fn collect_entries_for_branch_summary(
    session: &dyn BranchSummarySession,
    old_leaf_id: Option<&str>,
    target_id: &str,
) -> CollectEntriesResult {
    let Some(old_leaf_id) = old_leaf_id else {
        return CollectEntriesResult::default();
    };

    let old_path: Vec<String> = session
        .get_branch(old_leaf_id)
        .iter()
        .map(|entry| entry.id().to_string())
        .collect();
    let target_path = session.get_branch(target_id);

    // The target path is root-first, so the deepest shared node is the last one
    // that also appears on the old path.
    let common_ancestor_id = target_path
        .iter()
        .rev()
        .find(|entry| old_path.iter().any(|id| id == entry.id()))
        .map(|entry| entry.id().to_string());

    let mut entries: Vec<SessionEntry> = Vec::new();
    let mut current = Some(old_leaf_id.to_string());

    while let Some(id) = current {
        if Some(&id) == common_ancestor_id.as_ref() {
            break;
        }
        let Some(entry) = session.get_entry(&id) else {
            break;
        };
        current = entry.parent_id().map(str::to_string);
        entries.push(entry);
    }

    entries.reverse();

    CollectEntriesResult {
        entries,
        common_ancestor_id,
    }
}

// ============================================================================
// Entry to message conversion
// ============================================================================

/// Like the compaction module's version, but compaction entries do contribute
/// here: their summary is the only record of what came before them.
/// Deviation (class 1): a branch-summary entry whose summary is empty
/// estimate to zero tokens and serialize to nothing.
fn message_from_entry(entry: &SessionEntry) -> Option<AgentMessage> {
    match session_entry_to_context_messages(entry).into_iter().next() {
        // Tool results are skipped; the context sits in the tool call.
        Some(AgentMessage::ToolResult(_)) | None => None,
        other => other,
    }
}

/// Selects the messages that fit the budget, newest first, and collects file
/// operations from every entry regardless of what fits.
/// The two passes are deliberate: the file lists are cumulative across nested
/// branch summaries, and truncating them with the message budget would make a
/// long branch forget which files it had touched.
pub fn prepare_branch_entries(entries: &[SessionEntry], token_budget: u64) -> BranchPreparation {
    let mut messages: Vec<AgentMessage> = Vec::new();
    let mut file_ops = create_file_ops();
    let mut total_tokens: u64 = 0;

    for entry in entries {
        let SessionEntry::BranchSummary(entry) = entry else {
            continue;
        };
        if entry.from_hook == Some(true) {
            continue;
        }
        let Some(details) = entry.details.as_ref() else {
            continue;
        };
        if let Some(read_files) = details.get("readFiles").and_then(|value| value.as_array()) {
            for file in read_files.iter().filter_map(|file| file.as_str()) {
                file_ops.read.insert(file.to_string());
            }
        }
        if let Some(modified) = details
            .get("modifiedFiles")
            .and_then(|value| value.as_array())
        {
            // Modified files go into `edited` so the two lists deduplicate.
            for file in modified.iter().filter_map(|file| file.as_str()) {
                file_ops.edited.insert(file.to_string());
            }
        }
    }

    for entry in entries.iter().rev() {
        let Some(message) = message_from_entry(entry) else {
            continue;
        };

        extract_file_ops_from_message(&message, &mut file_ops);

        let tokens = estimate_tokens(&message);

        if token_budget > 0 && total_tokens + tokens > token_budget {
            // A summary entry is important enough to squeeze in anyway, as long
            // as there is real room left.
            if matches!(
                entry,
                SessionEntry::Compaction(_) | SessionEntry::BranchSummary(_)
            ) && (total_tokens as f64) < token_budget as f64 * 0.9
            {
                messages.insert(0, message);
                total_tokens += tokens;
            }
            break;
        }

        messages.insert(0, message);
        total_tokens += tokens;
    }

    BranchPreparation {
        messages,
        file_ops,
        total_tokens,
    }
}

// ============================================================================
// Summary generation
// ============================================================================

const BRANCH_SUMMARY_PREAMBLE: &str = "The user explored a different conversation branch before returning here.\nSummary of that exploration:\n\n";

const BRANCH_SUMMARY_PROMPT: &str = r#"Create a structured summary of this conversation branch for context when returning later.

Use this EXACT format:

## Goal
[What was the user trying to accomplish in this branch?]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Work that was started but not finished]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [What should happen next to continue this work]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

/// Summarizes the entries of an abandoned branch.
pub async fn generate_branch_summary(
    entries: &[SessionEntry],
    model: &Model,
    options: &GenerateBranchSummaryOptions,
) -> BranchSummaryResult {
    let reserve_tokens = options.reserve_tokens.unwrap_or(16384);
    // Budget = context window minus the room the prompt and the answer need.
    let context_window = if model.context_window > 0 {
        model.context_window
    } else {
        128000
    };
    let token_budget = context_window.saturating_sub(reserve_tokens);

    let BranchPreparation {
        messages, file_ops, ..
    } = prepare_branch_entries(entries, token_budget);

    if messages.is_empty() {
        return BranchSummaryResult {
            summary: Some("No content to summarize".to_string()),
            ..BranchSummaryResult::default()
        };
    }

    // Serialized rather than replayed, so the model summarizes instead of
    // continuing the conversation.
    let llm_messages = convert_to_llm(&messages);
    let conversation_text = serialize_conversation(&llm_messages);

    let instructions = match (
        options.replace_instructions,
        options.custom_instructions.as_deref(),
    ) {
        (true, Some(custom)) => custom.to_string(),
        (false, Some(custom)) => {
            format!("{BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: {custom}")
        }
        _ => BRANCH_SUMMARY_PROMPT.to_string(),
    };
    let prompt_text =
        format!("<conversation>\n{conversation_text}\n</conversation>\n\n{instructions}");

    let request = SummarizationRequest {
        api_key: options.api_key.clone(),
        headers: options.headers.clone(),
        env: options.env.clone(),
        signal: options.signal.clone(),
        thinking_level: options.thinking_level,
        stream_fn: options.stream_fn.clone(),
        retry: options.retry,
        callbacks: options.callbacks.clone(),
    };
    let mut stream_options =
        crate::core::compaction::compaction::branch_summary_options(model, &request);
    stream_options.base.max_tokens = Some(2048);

    let response = complete_summarization(
        model,
        crate::core::compaction::compaction::summarization_context_for(prompt_text),
        stream_options,
        options.stream_fn.clone(),
        options.retry,
        options.callbacks.as_ref(),
    )
    .await;

    if response.stop_reason == StopReason::Aborted {
        return BranchSummaryResult {
            aborted: true,
            ..BranchSummaryResult::default()
        };
    }
    if response.stop_reason == StopReason::Error {
        return BranchSummaryResult {
            error: Some(
                response
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "Summarization failed".to_string()),
            ),
            ..BranchSummaryResult::default()
        };
    }

    let mut summary = format!(
        "{BRANCH_SUMMARY_PREAMBLE}{}",
        content_text(&response.content)
    );

    let (read_files, modified_files) = compute_file_lists(&file_ops);
    summary.push_str(&format_file_operations(&read_files, &modified_files));

    BranchSummaryResult {
        summary: Some(summary),
        usage: Some(response.usage),
        read_files: Some(read_files),
        modified_files: Some(modified_files),
        aborted: false,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The collection and budgeting rules are cheap to pin directly.
    struct Tree {
        entries: Vec<SessionEntry>,
    }

    impl BranchSummarySession for Tree {
        fn get_branch(&self, leaf_id: &str) -> Vec<SessionEntry> {
            let mut path: Vec<SessionEntry> = Vec::new();
            let mut current = Some(leaf_id.to_string());
            while let Some(id) = current {
                let Some(entry) = self.get_entry(&id) else {
                    break;
                };
                current = entry.parent_id().map(str::to_string);
                path.push(entry);
            }
            path.reverse();
            path
        }

        fn get_entry(&self, id: &str) -> Option<SessionEntry> {
            self.entries.iter().find(|entry| entry.id() == id).cloned()
        }
    }

    fn message_entry(id: &str, parent: Option<&str>, text: &str) -> SessionEntry {
        serde_json::from_value(json!({
            "type": "message",
            "id": id,
            "parentId": parent,
            "timestamp": "2026-08-14T00:00:00.000Z",
            "message": { "role": "user", "content": text, "timestamp": 0 },
        }))
        .expect("entry")
    }

    fn branch_summary_entry(
        id: &str,
        parent: Option<&str>,
        details: serde_json::Value,
    ) -> SessionEntry {
        serde_json::from_value(json!({
            "type": "branch_summary",
            "id": id,
            "parentId": parent,
            "timestamp": "2026-08-14T00:00:00.000Z",
            "summary": "a summary",
            "fromId": "old",
            "details": details,
        }))
        .expect("entry")
    }

    #[test]
    fn collects_the_entries_between_the_old_leaf_and_the_common_ancestor() {
        let tree = Tree {
            entries: vec![
                message_entry("root", None, "root"),
                message_entry("a", Some("root"), "a"),
                message_entry("b", Some("a"), "b"),
                message_entry("other", Some("root"), "other"),
            ],
        };

        let result = collect_entries_for_branch_summary(&tree, Some("b"), "other");

        assert_eq!(result.common_ancestor_id.as_deref(), Some("root"));
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.id().to_string())
                .collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn collects_nothing_without_an_old_position() {
        let tree = Tree {
            entries: vec![message_entry("root", None, "root")],
        };
        let result = collect_entries_for_branch_summary(&tree, None, "root");
        assert!(result.entries.is_empty());
        assert_eq!(result.common_ancestor_id, None);
    }

    #[test]
    fn keeps_the_newest_messages_that_fit_the_budget() {
        let entries = vec![
            message_entry("a", None, &"a".repeat(400)),
            message_entry("b", Some("a"), &"b".repeat(400)),
            message_entry("c", Some("b"), "c"),
        ];

        // 400 characters estimate to 100 tokens each; only the last two fit.
        let prepared = prepare_branch_entries(&entries, 120);

        assert_eq!(prepared.messages.len(), 2);
        assert!(prepared.total_tokens <= 120);
    }

    #[test]
    fn keeps_everything_when_there_is_no_budget() {
        let entries = vec![
            message_entry("a", None, &"a".repeat(4000)),
            message_entry("b", Some("a"), &"b".repeat(4000)),
        ];

        let prepared = prepare_branch_entries(&entries, 0);
        assert_eq!(prepared.messages.len(), 2);
    }

    /// File lists are cumulative: a nested branch summary's record survives even
    /// when its own message did not fit the budget.
    #[test]
    fn collects_file_operations_from_every_entry_regardless_of_the_budget() {
        let entries = vec![
            branch_summary_entry(
                "s",
                None,
                json!({ "readFiles": ["/read.rs"], "modifiedFiles": ["/written.rs"] }),
            ),
            message_entry("a", Some("s"), &"a".repeat(8000)),
        ];

        let prepared = prepare_branch_entries(&entries, 100);

        assert!(prepared.file_ops.read.contains("/read.rs"));
        assert!(prepared.file_ops.edited.contains("/written.rs"));
    }

    #[test]
    fn ignores_the_details_of_a_summary_it_did_not_write() {
        let mut entry = branch_summary_entry("s", None, json!({ "readFiles": ["/read.rs"] }));
        if let SessionEntry::BranchSummary(entry) = &mut entry {
            entry.from_hook = Some(true);
        }

        let prepared = prepare_branch_entries(&[entry], 0);
        assert!(prepared.file_ops.read.is_empty());
    }
}
