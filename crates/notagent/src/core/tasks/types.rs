//! Port of `packages/coding-agent/src/core/tasks/types.ts`.
//!
//! The record every piece of background work is described by.
//!
//! There is deliberately one shape here rather than one per kind. A shell
//! command and a delegated subagent differ in how they are started and in what
//! they carry as extras, and in nothing else that matters to the machinery
//! around them: both are registered, listed, persisted, waited on, stopped, and
//! announced when they settle.
//!
//! Deviation (class 1): TS models a task as an object literal with methods; the
//! port uses the [`BackgroundTask`] trait behind an `Arc`, because the manager
//! hands the same task to a spawned tokio task and keeps reading `to_info` from
//! it. A task that mutates while it runs (a shell task learning its pid) needs
//! interior mutability for the same reason.

use notagent_agent::types::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// Where a task stands.
///
/// `lost` is not reachable while the process that owns a task is alive: it is
/// assigned during reconciliation to a record that was still marked running
/// when the process holding it exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Killed,
    Lost,
}

impl TaskStatus {
    /// The wire form, which is also what a tool result prints.
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::TimedOut => "timed_out",
            TaskStatus::Killed => "killed",
            TaskStatus::Lost => "lost",
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub const TERMINAL_TASK_STATUSES: [TaskStatus; 5] = [
    TaskStatus::Completed,
    TaskStatus::Failed,
    TaskStatus::TimedOut,
    TaskStatus::Killed,
    TaskStatus::Lost,
];

pub fn is_terminal_task_status(status: TaskStatus) -> bool {
    TERMINAL_TASK_STATUSES.contains(&status)
}

/// What a task can settle as from the inside. `lost` is excluded on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSettlementStatus {
    Completed,
    Failed,
    TimedOut,
    Killed,
}

impl From<TaskSettlementStatus> for TaskStatus {
    fn from(status: TaskSettlementStatus) -> Self {
        match status {
            TaskSettlementStatus::Completed => TaskStatus::Completed,
            TaskSettlementStatus::Failed => TaskStatus::Failed,
            TaskSettlementStatus::TimedOut => TaskStatus::TimedOut,
            TaskSettlementStatus::Killed => TaskStatus::Killed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSettlement {
    pub status: TaskSettlementStatus,
    /// Why it ended, when there is something to say beyond the status.
    pub stop_reason: Option<String>,
}

impl TaskSettlement {
    pub fn new(status: TaskSettlementStatus) -> Self {
        TaskSettlement {
            status,
            stop_reason: None,
        }
    }

    pub fn with_reason(status: TaskSettlementStatus, reason: impl Into<String>) -> Self {
        TaskSettlement {
            status,
            stop_reason: Some(reason.into()),
        }
    }
}

/// The two kinds of work that become tasks. The id prefix follows the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Shell,
    Subagent,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::Shell => "shell",
            TaskKind::Subagent => "subagent",
        }
    }
}

impl std::fmt::Display for TaskKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Fields shared by every task, filled by the manager rather than the task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfoBase {
    pub task_id: String,
    /// Short human label. Required for background work, derived for foreground.
    pub description: String,
    pub status: TaskStatus,
    /// `Some(false)` means a tool call is still waiting on this task. A record
    /// restored from disk without the field predates it and is treated as
    /// detached, since nothing can be waiting on it after a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached: Option<bool>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// Set when the result was already handed back, so no completion is announced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_suppressed: Option<bool>,
    /// The deadline this task was registered with, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellTaskInfo {
    #[serde(flatten)]
    pub base: TaskInfoBase,
    pub command: String,
    /// Process id, or 0 when the process never started.
    pub pid: u32,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentTaskInfo {
    #[serde(flatten)]
    pub base: TaskInfoBase,
    /// Tokens the child has spent so far, counted as its own conversation grows.
    pub tokens: u64,
    /// The subagent's own identifier, which is what continues it. Deliberately
    /// distinct from `taskId`: the two look alike and only this one is accepted.
    pub session_id: String,
    /// The type the subagent runs as: `read-only` or `worker`.
    pub agent: String,
    /// The star name every surface shows it under — the approval dialog, the
    /// tasks panel, the `/tasks` browser and the footer. A uuid identifies a
    /// child; only this tells the user *which* child.
    pub alias: String,
}

/// Deviation (class 1): serde writes the `kind` discriminant as the first key of
/// the record, while `{...base, kind}` puts it after the shared fields. Task
/// records are written and read only by this app, and both forms parse to the
/// same value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskInfo {
    Shell(ShellTaskInfo),
    Subagent(SubagentTaskInfo),
}

impl TaskInfo {
    pub fn base(&self) -> &TaskInfoBase {
        match self {
            TaskInfo::Shell(info) => &info.base,
            TaskInfo::Subagent(info) => &info.base,
        }
    }

    pub fn base_mut(&mut self) -> &mut TaskInfoBase {
        match self {
            TaskInfo::Shell(info) => &mut info.base,
            TaskInfo::Subagent(info) => &mut info.base,
        }
    }

    pub fn kind(&self) -> TaskKind {
        match self {
            TaskInfo::Shell(_) => TaskKind::Shell,
            TaskInfo::Subagent(_) => TaskKind::Subagent,
        }
    }

    pub fn task_id(&self) -> &str {
        &self.base().task_id
    }

    pub fn description(&self) -> &str {
        &self.base().description
    }

    pub fn status(&self) -> TaskStatus {
        self.base().status
    }

    pub fn stop_reason(&self) -> Option<&str> {
        self.base().stop_reason.as_deref()
    }

    /// `info.detached !== false` — a record without the field counts as detached.
    pub fn is_detached(&self) -> bool {
        self.base().detached != Some(false)
    }

    pub fn is_terminal(&self) -> bool {
        is_terminal_task_status(self.status())
    }
}

/// What a running task is given by the manager.
///
/// A task never touches the record, the log or the notification path directly.
/// It reports output, watches the signal, and settles exactly once; everything
/// that follows from settling is the manager's.
#[derive(Clone)]
pub struct TaskSink {
    /// Cancelled when the task is stopped, times out, or its session ends.
    pub signal: CancellationToken,
    target: std::sync::Arc<dyn TaskSinkTarget>,
}

impl TaskSink {
    pub fn new(signal: CancellationToken, target: std::sync::Arc<dyn TaskSinkTarget>) -> Self {
        TaskSink { signal, target }
    }

    pub fn append_output(&self, chunk: &str) {
        self.target.append_output(chunk);
    }

    /// Resolves false when the task had already settled.
    pub async fn settle(&self, settlement: TaskSettlement) -> bool {
        self.target.settle(settlement).await
    }
}

/// What a sink forwards to. Implemented by the manager's task record.
pub trait TaskSinkTarget: Send + Sync {
    fn append_output(&self, chunk: &str);
    fn settle<'a>(&'a self, settlement: TaskSettlement) -> BoxFuture<'a, bool>;
}

/// One piece of background work.
///
/// `force_stop` is separate from cancelling the signal because a graceful stop
/// is a request the work may ignore. The manager signals first, waits, and only
/// then reaches for this — a shell task kills its process group here, and a
/// subagent has nothing to add beyond the abort it already received.
pub trait BackgroundTask: Send + Sync {
    fn kind(&self) -> TaskKind;
    /// Leading segment of the generated id, so an id says what it is.
    fn id_prefix(&self) -> &str;
    fn description(&self) -> String;
    /// Deviation (class 1): a TS `throw` out of `start` becomes `Err`; the
    /// manager settles such work exactly as the TS `catch` does.
    fn start<'a>(&'a self, sink: TaskSink) -> BoxFuture<'a, Result<(), String>>;
    /// Called when a foreground task is moved to the background.
    fn on_detach(&self) {}
    fn force_stop<'a>(&'a self) -> Option<BoxFuture<'a, ()>> {
        None
    }
    /// Adds the per-kind fields to what the manager knows.
    fn to_info(&self, base: TaskInfoBase) -> TaskInfo;
}

/// Why a foreground tool call stopped waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForegroundRelease {
    Terminal,
    Detached,
    TimeoutDetached,
}
