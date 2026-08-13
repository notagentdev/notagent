//! Port of `packages/agent/src/harness/session/types.ts`.
//!
//! The harness submodules of the agent package are excluded from the port
//! (master plan, exclusion table) because the coding agent does not consume
//! them — but the SQLite backend implements `SessionRepo`/`SessionStorage`, so
//! WS-C task 4 ports the type surface here (WS-C plan, task 4).

use std::collections::BTreeMap;

use notagent_agent::AgentMessage;
use notagent_ai::{StopReason, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type JsonValue = Value;

/// `Exclude<StopReason, "pending"> | "deferred"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionStopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
    Deferred,
}

impl TryFrom<StopReason> for SessionStopReason {
    type Error = ();

    fn try_from(value: StopReason) -> Result<Self, ()> {
        Ok(match value {
            StopReason::Pending => return Err(()),
            StopReason::Stop => Self::Stop,
            StopReason::Length => Self::Length,
            StopReason::ToolUse => Self::ToolUse,
            StopReason::Error => Self::Error,
            StopReason::Aborted => Self::Aborted,
            StopReason::Deferred => Self::Deferred,
        })
    }
}

pub trait IdGenerator: Send + Sync {
    fn next(&self) -> String;
}

// --- Entries -----------------------------------------------------------------

/// Fields every entry carries. `seq`, `parent_id` and `timestamp` are assigned
/// by the storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryBase {
    pub id: String,
    /// Shared sequence; read-side, storage-assigned.
    pub seq: i64,
    /// Storage-assigned: the appending lane's leaf.
    pub parent_id: Option<String>,
    /// Unix ms, storage-assigned.
    pub timestamp: i64,
}

macro_rules! entry_struct {
    ($(#[$meta:meta])* $name:ident { $($(#[$field_meta:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct $name {
            pub id: String,
            pub seq: i64,
            pub parent_id: Option<String>,
            pub timestamp: i64,
            $($(#[$field_meta])* pub $field: $ty),*
        }
    };
}

entry_struct!(MessageEntry {
    message: AgentMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminate: Option<bool>,
});

entry_struct!(ModelChangeEntry {
    provider: String,
    model_id: String,
});

entry_struct!(ThinkingLevelEntry {
    thinking_level: String
});

entry_struct!(ActiveToolsEntry { active_tool_names: Vec<String> });

entry_struct!(CompactionEntry {
    summary: String,
    retained_tail: Vec<AgentMessage>,
    tokens_before: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<Usage>,
});

entry_struct!(BranchSummaryEntry {
    from_id: String,
    summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<Usage>,
});

entry_struct!(CustomEntry {
    custom_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
});

// Boxing the variants would hide the 1:1 shape of the TS union
// (CONVENTIONS.md §9); entries are constructed and matched directly.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    Message(MessageEntry),
    ModelChange(ModelChangeEntry),
    ThinkingLevelChange(ThinkingLevelEntry),
    ActiveToolsChange(ActiveToolsEntry),
    Compaction(CompactionEntry),
    BranchSummary(BranchSummaryEntry),
    Custom(CustomEntry),
}

/// `Entry["type"]`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    Message,
    ModelChange,
    ThinkingLevelChange,
    ActiveToolsChange,
    Compaction,
    BranchSummary,
    Custom,
}

impl EntryType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::ModelChange => "model_change",
            Self::ThinkingLevelChange => "thinking_level_change",
            Self::ActiveToolsChange => "active_tools_change",
            Self::Compaction => "compaction",
            Self::BranchSummary => "branch_summary",
            Self::Custom => "custom",
        }
    }
}

impl Entry {
    pub fn id(&self) -> &str {
        match self {
            Self::Message(entry) => &entry.id,
            Self::ModelChange(entry) => &entry.id,
            Self::ThinkingLevelChange(entry) => &entry.id,
            Self::ActiveToolsChange(entry) => &entry.id,
            Self::Compaction(entry) => &entry.id,
            Self::BranchSummary(entry) => &entry.id,
            Self::Custom(entry) => &entry.id,
        }
    }

    pub fn seq(&self) -> i64 {
        match self {
            Self::Message(entry) => entry.seq,
            Self::ModelChange(entry) => entry.seq,
            Self::ThinkingLevelChange(entry) => entry.seq,
            Self::ActiveToolsChange(entry) => entry.seq,
            Self::Compaction(entry) => entry.seq,
            Self::BranchSummary(entry) => entry.seq,
            Self::Custom(entry) => entry.seq,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        let parent = match self {
            Self::Message(entry) => &entry.parent_id,
            Self::ModelChange(entry) => &entry.parent_id,
            Self::ThinkingLevelChange(entry) => &entry.parent_id,
            Self::ActiveToolsChange(entry) => &entry.parent_id,
            Self::Compaction(entry) => &entry.parent_id,
            Self::BranchSummary(entry) => &entry.parent_id,
            Self::Custom(entry) => &entry.parent_id,
        };
        parent.as_deref()
    }

    pub fn timestamp(&self) -> i64 {
        match self {
            Self::Message(entry) => entry.timestamp,
            Self::ModelChange(entry) => entry.timestamp,
            Self::ThinkingLevelChange(entry) => entry.timestamp,
            Self::ActiveToolsChange(entry) => entry.timestamp,
            Self::Compaction(entry) => entry.timestamp,
            Self::BranchSummary(entry) => entry.timestamp,
            Self::Custom(entry) => entry.timestamp,
        }
    }

    pub fn entry_type(&self) -> EntryType {
        match self {
            Self::Message(_) => EntryType::Message,
            Self::ModelChange(_) => EntryType::ModelChange,
            Self::ThinkingLevelChange(_) => EntryType::ThinkingLevelChange,
            Self::ActiveToolsChange(_) => EntryType::ActiveToolsChange,
            Self::Compaction(_) => EntryType::Compaction,
            Self::BranchSummary(_) => EntryType::BranchSummary,
            Self::Custom(_) => EntryType::Custom,
        }
    }

    pub fn custom_type(&self) -> Option<&str> {
        match self {
            Self::Custom(entry) => Some(&entry.custom_type),
            _ => None,
        }
    }
}

/// `ProvisionedEntry<TEntry> = Omit<TEntry, "parentId" | "seq" | "timestamp">`
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProvisionedEntry {
    Message {
        id: String,
        message: AgentMessage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminate: Option<bool>,
    },
    ModelChange {
        id: String,
        provider: String,
        #[serde(rename = "modelId")]
        model_id: String,
    },
    ThinkingLevelChange {
        id: String,
        #[serde(rename = "thinkingLevel")]
        thinking_level: String,
    },
    ActiveToolsChange {
        id: String,
        #[serde(rename = "activeToolNames")]
        active_tool_names: Vec<String>,
    },
    Compaction {
        id: String,
        summary: String,
        #[serde(rename = "retainedTail")]
        retained_tail: Vec<AgentMessage>,
        #[serde(rename = "tokensBefore")]
        tokens_before: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    BranchSummary {
        id: String,
        #[serde(rename = "fromId")]
        from_id: String,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    Custom {
        id: String,
        #[serde(rename = "customType")]
        custom_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
}

impl ProvisionedEntry {
    pub fn id(&self) -> &str {
        match self {
            Self::Message { id, .. }
            | Self::ModelChange { id, .. }
            | Self::ThinkingLevelChange { id, .. }
            | Self::ActiveToolsChange { id, .. }
            | Self::Compaction { id, .. }
            | Self::BranchSummary { id, .. }
            | Self::Custom { id, .. } => id,
        }
    }

    pub fn entry_type(&self) -> EntryType {
        match self {
            Self::Message { .. } => EntryType::Message,
            Self::ModelChange { .. } => EntryType::ModelChange,
            Self::ThinkingLevelChange { .. } => EntryType::ThinkingLevelChange,
            Self::ActiveToolsChange { .. } => EntryType::ActiveToolsChange,
            Self::Compaction { .. } => EntryType::Compaction,
            Self::BranchSummary { .. } => EntryType::BranchSummary,
            Self::Custom { .. } => EntryType::Custom,
        }
    }
}

// --- Queries -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryOrder {
    #[default]
    NewestFirst,
    OldestFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryCursor {
    pub after_seq: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntryQuery {
    pub entry_type: Option<EntryType>,
    /// For type `custom`.
    pub custom_type: Option<String>,
    /// Default `newestFirst`.
    pub order: Option<EntryOrder>,
    pub limit: Option<i64>,
    pub cursor: Option<EntryCursor>,
}

/// Bounds of a branch scan. Default: the whole path, leaf to root.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BranchBounds {
    /// Default: the view's lane leaf.
    pub start: Option<String>,
    /// Scan ends after the first match, inclusive.
    pub stop_at_type: Option<EntryType>,
    pub stop_at_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordQuery {
    /// Exact lane match. `None` queries every lane.
    pub lane: Option<String>,
    /// Exact record discriminant match. `None` queries every record type.
    pub record_type: Option<RecordType>,
    /// Operation identity: matches `OperationStartedRecord.id` and the `runId`
    /// of operation-owned records.
    pub run_id: Option<String>,
    /// Exact operation intent kind. Valid only with type `operation_started`.
    pub operation_kind: Option<OperationKind>,
    /// Exclusive chronological lower bound: `seq > afterSeq`, regardless of order.
    pub after_seq: Option<i64>,
    /// Default `newestFirst`.
    pub order: Option<EntryOrder>,
    /// Positive maximum number of matching records.
    pub limit: Option<i64>,
}

// --- Records -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordType {
    OperationStarted,
    AbortRequested,
    OperationFinished,
    StepAttempt,
    ToolStarted,
    QueueEnqueued,
    QueueCancelled,
    WriteDeferred,
    Usage,
}

impl RecordType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OperationStarted => "operation_started",
            Self::AbortRequested => "abort_requested",
            Self::OperationFinished => "operation_finished",
            Self::StepAttempt => "step_attempt",
            Self::ToolStarted => "tool_started",
            Self::QueueEnqueued => "queue_enqueued",
            Self::QueueCancelled => "queue_cancelled",
            Self::WriteDeferred => "write_deferred",
            Self::Usage => "usage",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    Run,
    Compaction,
    Navigation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OperationIntent {
    Run {
        /// Normalized caller input before before_run.
        #[serde(rename = "originalPrompt")]
        original_prompt: Vec<AgentMessage>,
        /// Captured nextRun items, then the prompt, then before_run injections.
        #[serde(rename = "initialMessages")]
        initial_messages: Vec<ProvisionedEntry>,
        #[serde(
            default,
            rename = "systemPromptOverride",
            skip_serializing_if = "Option::is_none"
        )]
        system_prompt_override: Option<String>,
        #[serde(
            default,
            rename = "resumeData",
            skip_serializing_if = "Option::is_none"
        )]
        resume_data: Option<BTreeMap<String, Value>>,
    },
    Compaction {
        #[serde(
            default,
            rename = "customInstructions",
            skip_serializing_if = "Option::is_none"
        )]
        custom_instructions: Option<String>,
        #[serde(rename = "resultEntryId")]
        result_entry_id: String,
    },
    Navigation {
        #[serde(rename = "targetId")]
        target_id: Option<String>,
        summarize: bool,
        #[serde(
            default,
            rename = "customInstructions",
            skip_serializing_if = "Option::is_none"
        )]
        custom_instructions: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(
            default,
            rename = "summaryEntryId",
            skip_serializing_if = "Option::is_none"
        )]
        summary_entry_id: Option<String>,
    },
}

impl OperationIntent {
    pub fn kind(&self) -> OperationKind {
        match self {
            Self::Run { .. } => OperationKind::Run,
            Self::Compaction { .. } => OperationKind::Compaction,
            Self::Navigation { .. } => OperationKind::Navigation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationOutcome {
    Completed,
    Aborted,
    Failed,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordError {
    pub code: String,
    pub message: String,
}

macro_rules! record_struct {
    ($(#[$meta:meta])* $name:ident { $($(#[$field_meta:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct $name {
            pub id: String,
            pub seq: i64,
            pub lane: String,
            pub timestamp: i64,
            $($(#[$field_meta])* pub $field: $ty),*
        }
    };
}

record_struct!(OperationStartedRecord {
    source_leaf_id: Option<String>,
    intent: OperationIntent,
});

record_struct!(AbortRequestedRecord { run_id: String });

record_struct!(OperationFinishedRecord {
    run_id: String,
    outcome: OperationOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<RecordError>,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    Assistant,
    BranchSummary,
    Compaction,
}

record_struct!(StepAttemptRecord {
    run_id: String,
    step: StepKind,
    attempt: i64,
    result_entry_id: String,
    /// Persists why compaction summary generation started (only for `compaction`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compaction_reason: Option<CompactionReason>,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolReplay {
    Never,
    Safe,
}

record_struct!(ToolStartedRecord {
    run_id: String,
    assistant_entry_id: String,
    tool_index: i64,
    tool_call_id: String,
    tool_name: String,
    effective_args: BTreeMap<String, Value>,
    result_entry_id: String,
    replay: ToolReplay,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueueKind {
    Steer,
    FollowUp,
    NextRun,
}

record_struct!(QueueEnqueuedRecord {
    queue: QueueKind,
    /// Absent for the `nextRun` queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    target: ProvisionedEntry,
});

record_struct!(QueueCancelledRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    entry_id: String,
});

record_struct!(WriteDeferredRecord {
    run_id: String,
    target: ProvisionedEntry,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageCause {
    Assistant,
    Compaction,
    BranchSummary,
    DeferredFetch,
    Tool,
    Hook,
    Adjustment,
}

record_struct!(UsageRecord {
    usage: Usage,
    cause: UsageCause,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempt: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stop_reason: Option<SessionStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LaneRecord {
    OperationStarted(OperationStartedRecord),
    AbortRequested(AbortRequestedRecord),
    OperationFinished(OperationFinishedRecord),
    StepAttempt(StepAttemptRecord),
    ToolStarted(ToolStartedRecord),
    QueueEnqueued(QueueEnqueuedRecord),
    QueueCancelled(QueueCancelledRecord),
    WriteDeferred(WriteDeferredRecord),
    Usage(UsageRecord),
}

impl LaneRecord {
    pub fn id(&self) -> &str {
        match self {
            Self::OperationStarted(record) => &record.id,
            Self::AbortRequested(record) => &record.id,
            Self::OperationFinished(record) => &record.id,
            Self::StepAttempt(record) => &record.id,
            Self::ToolStarted(record) => &record.id,
            Self::QueueEnqueued(record) => &record.id,
            Self::QueueCancelled(record) => &record.id,
            Self::WriteDeferred(record) => &record.id,
            Self::Usage(record) => &record.id,
        }
    }

    pub fn seq(&self) -> i64 {
        match self {
            Self::OperationStarted(record) => record.seq,
            Self::AbortRequested(record) => record.seq,
            Self::OperationFinished(record) => record.seq,
            Self::StepAttempt(record) => record.seq,
            Self::ToolStarted(record) => record.seq,
            Self::QueueEnqueued(record) => record.seq,
            Self::QueueCancelled(record) => record.seq,
            Self::WriteDeferred(record) => record.seq,
            Self::Usage(record) => record.seq,
        }
    }

    pub fn lane(&self) -> &str {
        match self {
            Self::OperationStarted(record) => &record.lane,
            Self::AbortRequested(record) => &record.lane,
            Self::OperationFinished(record) => &record.lane,
            Self::StepAttempt(record) => &record.lane,
            Self::ToolStarted(record) => &record.lane,
            Self::QueueEnqueued(record) => &record.lane,
            Self::QueueCancelled(record) => &record.lane,
            Self::WriteDeferred(record) => &record.lane,
            Self::Usage(record) => &record.lane,
        }
    }

    pub fn record_type(&self) -> RecordType {
        match self {
            Self::OperationStarted(_) => RecordType::OperationStarted,
            Self::AbortRequested(_) => RecordType::AbortRequested,
            Self::OperationFinished(_) => RecordType::OperationFinished,
            Self::StepAttempt(_) => RecordType::StepAttempt,
            Self::ToolStarted(_) => RecordType::ToolStarted,
            Self::QueueEnqueued(_) => RecordType::QueueEnqueued,
            Self::QueueCancelled(_) => RecordType::QueueCancelled,
            Self::WriteDeferred(_) => RecordType::WriteDeferred,
            Self::Usage(_) => RecordType::Usage,
        }
    }

    /// Operation identity: `OperationStartedRecord.id`, otherwise the record's `runId`.
    pub fn run_id(&self) -> Option<&str> {
        match self {
            Self::OperationStarted(record) => Some(&record.id),
            Self::AbortRequested(record) => Some(&record.run_id),
            Self::OperationFinished(record) => Some(&record.run_id),
            Self::StepAttempt(record) => Some(&record.run_id),
            Self::ToolStarted(record) => Some(&record.run_id),
            Self::QueueEnqueued(record) => record.run_id.as_deref(),
            Self::QueueCancelled(record) => record.run_id.as_deref(),
            Self::WriteDeferred(record) => Some(&record.run_id),
            Self::Usage(record) => record.run_id.as_deref(),
        }
    }

    pub fn operation_kind(&self) -> Option<OperationKind> {
        match self {
            Self::OperationStarted(record) => Some(record.intent.kind()),
            _ => None,
        }
    }
}

// --- Session shape -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    pub id: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub message_count: i64,
    pub cached_tokens: f64,
    pub uncached_tokens: f64,
    pub total_tokens: f64,
    pub cost_total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanePointer {
    pub lane: String,
    pub leaf_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LogItem {
    Entry {
        seq: i64,
        entry: Entry,
    },
    Record {
        seq: i64,
        record: LaneRecord,
    },
    Lane {
        seq: i64,
        lane: String,
        #[serde(rename = "leafId")]
        leaf_id: Option<String>,
    },
    Fact(LogFact),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "fact", rename_all = "camelCase")]
pub enum LogFact {
    Name {
        seq: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Label {
        seq: i64,
        #[serde(rename = "targetId")]
        target_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogOptions {
    pub after_seq: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionCreateOptions {
    pub id: Option<String>,
    pub parent_session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkScope {
    Branch,
    Tree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkPosition {
    Before,
    At,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkOptions {
    pub scope: ForkScope,
    pub entry_id: Option<String>,
    pub position: Option<ForkPosition>,
}

impl Default for ForkOptions {
    fn default() -> Self {
        Self {
            scope: ForkScope::Branch,
            entry_id: None,
            position: None,
        }
    }
}

// --- Errors ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionErrorCode {
    NotFound,
    AlreadyExists,
    InvalidEntry,
    InvalidPayload,
    InvalidLane,
    InvalidQuery,
    InvalidForkTarget,
    Storage,
}

impl SessionErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::AlreadyExists => "already_exists",
            Self::InvalidEntry => "invalid_entry",
            Self::InvalidPayload => "invalid_payload",
            Self::InvalidLane => "invalid_lane",
            Self::InvalidQuery => "invalid_query",
            Self::InvalidForkTarget => "invalid_fork_target",
            Self::Storage => "storage",
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct SessionError {
    pub code: SessionErrorCode,
    pub message: String,
}

impl SessionError {
    pub fn new(code: SessionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(SessionErrorCode::NotFound, message)
    }

    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(SessionErrorCode::Storage, message)
    }

    pub fn invalid_query(message: impl Into<String>) -> Self {
        Self::new(SessionErrorCode::InvalidQuery, message)
    }
}

impl From<crate::sqlite::types::SqliteError> for SessionError {
    fn from(error: crate::sqlite::types::SqliteError) -> Self {
        Self::storage(error.0)
    }
}
