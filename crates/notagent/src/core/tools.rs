//! Port of `packages/coding-agent/src/core/tools/`.

pub mod bash;
pub mod edit;
pub mod edit_diff;
pub mod file_mutation_queue;
pub mod find;
pub mod grep;
pub mod ls;
pub mod output_accumulator;
pub mod patch_minified;
pub mod path_utils;
pub mod read;
pub mod read_minified;
pub mod skill;
pub mod todo_write;
pub mod tool_definition;
pub mod truncate;
pub mod write;

use serde::{Deserialize, Serialize};

/// The built-in tools, as named on the wire.
///
/// The `ToolName` union of `packages/coding-agent/src/core/tools/index.ts`. The
/// registry functions that build the tools themselves follow with the task
/// tools (task 10); the names are needed earlier, because a shell is a tool
/// allowlist and modes are validated against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    Read,
    ReadMinified,
    Bash,
    Edit,
    PatchMinified,
    MultiPatchMinified,
    Skill,
    Task,
    TaskList,
    TaskOutput,
    TaskStop,
    TodoWrite,
    Write,
    Grep,
    Find,
    Ls,
}

/// Every tool name, in the order `allToolNames` inserts them.
pub const ALL_TOOL_NAMES: [ToolName; 16] = [
    ToolName::Read,
    ToolName::ReadMinified,
    ToolName::Bash,
    ToolName::Edit,
    ToolName::PatchMinified,
    ToolName::MultiPatchMinified,
    ToolName::Skill,
    ToolName::Task,
    ToolName::TaskList,
    ToolName::TaskOutput,
    ToolName::TaskStop,
    ToolName::TodoWrite,
    ToolName::Write,
    ToolName::Grep,
    ToolName::Find,
    ToolName::Ls,
];

impl ToolName {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolName::Read => "read",
            ToolName::ReadMinified => "read_minified",
            ToolName::Bash => "bash",
            ToolName::Edit => "edit",
            ToolName::PatchMinified => "patch_minified",
            ToolName::MultiPatchMinified => "multi_patch_minified",
            ToolName::Skill => "skill",
            ToolName::Task => "task",
            ToolName::TaskList => "task_list",
            ToolName::TaskOutput => "task_output",
            ToolName::TaskStop => "task_stop",
            ToolName::TodoWrite => "todo_write",
            ToolName::Write => "write",
            ToolName::Grep => "grep",
            ToolName::Find => "find",
            ToolName::Ls => "ls",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        ALL_TOOL_NAMES
            .into_iter()
            .find(|name| name.as_str() == value)
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
