//! Port of `packages/coding-agent/src/core/messages.ts`.
//!
//! The file is a byte-for-byte twin of `packages/agent/src/harness/messages.ts`
//! (only the `AgentMessage` import path and the `string | number` timestamp
//! overloads differ), and TS merges both declarations into the same
//! `CustomAgentMessages` interface. Rust has no declaration merging, so the four
//! custom message types live once in `notagent-agent` and are re-exported here
//! under the names the coding agent uses.

pub use notagent_agent::harness::messages::{
    BRANCH_SUMMARY_PREFIX, BRANCH_SUMMARY_SUFFIX, BashExecutionMessage, BranchSummaryMessage,
    COMPACTION_SUMMARY_PREFIX, COMPACTION_SUMMARY_SUFFIX, CompactionSummaryMessage, CustomMessage,
    bash_execution_to_text, convert_to_llm, create_branch_summary_message,
    create_compaction_summary_message, create_custom_message,
};

#[cfg(test)]
mod tests {
    use super::*;
    use notagent_agent::types::AgentMessage;
    use serde_json::json;

    #[test]
    fn the_summary_prefixes_match_the_coding_agent_source() {
        assert_eq!(
            COMPACTION_SUMMARY_PREFIX,
            "The conversation history before this point was compacted into the following summary:\n\n<summary>\n"
        );
        assert_eq!(COMPACTION_SUMMARY_SUFFIX, "\n</summary>");
        assert_eq!(
            BRANCH_SUMMARY_PREFIX,
            "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n"
        );
        assert_eq!(BRANCH_SUMMARY_SUFFIX, "</summary>");
    }

    #[test]
    fn converts_the_coding_agent_message_roles_to_llm_messages() {
        let messages = vec![
            AgentMessage::BranchSummary(create_branch_summary_message("branch", "abc", 1)),
            AgentMessage::CompactionSummary(create_compaction_summary_message(
                "compact", 10, None, 2,
            )),
        ];
        let converted: Vec<serde_json::Value> = convert_to_llm(&messages)
            .iter()
            .map(|message| serde_json::to_value(message).expect("message"))
            .collect();
        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0]["role"], json!("user"));
        assert_eq!(
            converted[0]["content"][0]["text"],
            json!(format!(
                "{BRANCH_SUMMARY_PREFIX}branch{BRANCH_SUMMARY_SUFFIX}"
            ))
        );
        assert_eq!(
            converted[1]["content"][0]["text"],
            json!(format!(
                "{COMPACTION_SUMMARY_PREFIX}compact{COMPACTION_SUMMARY_SUFFIX}"
            ))
        );
    }
}
