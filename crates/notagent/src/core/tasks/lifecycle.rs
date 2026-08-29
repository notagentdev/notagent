use serde::{Deserialize, Serialize};

use crate::core::session_manager::SessionEntry;
use crate::core::tasks::types::TaskInfo;

/// Session-only record type for the immutable chat lines around background work.
pub const TASK_LIFECYCLE_ENTRY_TYPE: &str = "task_lifecycle";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskLifecyclePhase {
    Started,
    Ended,
}

/// Everything needed to redraw one lifecycle line after a session resume.
///
/// This is stored as a plain custom session entry, not a custom message: it is
/// part of the user's transcript but not another instruction to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLifecycleRecord {
    pub phase: TaskLifecyclePhase,
    pub task: TaskInfo,
}

impl TaskLifecycleRecord {
    pub fn started(task: TaskInfo) -> Self {
        Self {
            phase: TaskLifecyclePhase::Started,
            task,
        }
    }

    pub fn ended(task: TaskInfo) -> Self {
        Self {
            phase: TaskLifecyclePhase::Ended,
            task,
        }
    }

    pub fn from_session_entry(entry: &SessionEntry) -> Option<Self> {
        let SessionEntry::Custom(entry) = entry else {
            return None;
        };
        if entry.custom_type != TASK_LIFECYCLE_ENTRY_TYPE {
            return None;
        }
        serde_json::from_value(entry.data.clone()?).ok()
    }
}
