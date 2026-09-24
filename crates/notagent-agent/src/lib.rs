pub mod agent;
pub mod agent_loop;
pub mod harness;
mod repeat_breaker;
pub mod stream_fn;
pub mod types;

pub use harness::messages::{
    BRANCH_SUMMARY_PREFIX, BRANCH_SUMMARY_SUFFIX, BashExecutionMessage, BranchSummaryMessage,
    COMPACTION_SUMMARY_PREFIX, COMPACTION_SUMMARY_SUFFIX, CompactionRecovery,
    CompactionSummaryMessage, CustomMessage, bash_execution_to_text, compaction_recovery_text,
    convert_to_llm, create_branch_summary_message, create_compaction_summary_message,
    create_custom_message,
};
// the telemetry types; the latter are re-exported from `notagent_telemetry` here rather
pub use agent::*;
pub use agent_loop::*;
pub use notagent_ai::uuidv7;
pub use notagent_telemetry::{
    InMemoryTelemetryContext, SpanAttributes, SpanOptions, SpanStatus, TelemetryContext,
    TelemetrySpan, noop_telemetry_context,
};
pub use stream_fn::{NoDefaultStreamFn, get_default_stream_fn, set_default_stream_fn};
pub use types::*;
