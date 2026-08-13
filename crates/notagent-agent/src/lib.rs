//! Agent-Kern: Agent-Loop, Tools, Queues, CustomMessages.
//!
//! 1:1-Port von `packages/agent` (siehe `crates/notagent-agent/PARITY.md`).

pub mod agent;
pub mod agent_loop;
pub mod harness;
pub mod stream_fn;
pub mod types;

pub use harness::messages::{
    BRANCH_SUMMARY_PREFIX, BRANCH_SUMMARY_SUFFIX, BashExecutionMessage, BranchSummaryMessage,
    COMPACTION_SUMMARY_PREFIX, COMPACTION_SUMMARY_SUFFIX, CompactionSummaryMessage, CustomMessage,
    bash_execution_to_text, convert_to_llm, create_branch_summary_message,
    create_compaction_summary_message, create_custom_message,
};
pub use notagent_ai::uuidv7;
pub use stream_fn::{NoDefaultStreamFn, get_default_stream_fn, set_default_stream_fn};
pub use types::*;
