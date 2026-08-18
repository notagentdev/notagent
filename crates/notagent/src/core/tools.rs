//! Port of `packages/coding-agent/src/core/tools/`.

pub mod bash;
pub mod edit;
pub mod edit_diff;
pub mod file_lease;
pub mod file_mutation_queue;
pub mod find;
pub mod find_codebase;
pub mod goal;
pub mod grep;
pub mod ls;
pub mod mcp;
pub mod mcp_auth;
pub mod mcp_render;
pub mod output_accumulator;
pub mod patch_minified;
pub mod path_utils;
pub mod read;
pub mod read_minified;
pub mod render_utils;
pub mod skill;
pub mod task;
pub mod task_tools;
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
    CreateGoal,
    UpdateGoal,
    Write,
    Grep,
    // Renamed from `find` and joined by `find_codebase` (user decision
    // 2026-08-16, v0.1.7): the shared `find_` prefix keeps the two search
    // tools adjacent and unambiguous for the model.
    FindFilesystem,
    FindCodebase,
    Ls,
}

/// Every tool name, in the order `allToolNames` inserts them.
pub const ALL_TOOL_NAMES: [ToolName; 19] = [
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
    ToolName::CreateGoal,
    ToolName::UpdateGoal,
    ToolName::Write,
    ToolName::Grep,
    ToolName::FindFilesystem,
    ToolName::FindCodebase,
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
            ToolName::CreateGoal => "create_goal",
            ToolName::UpdateGoal => "update_goal",
            ToolName::Write => "write",
            ToolName::Grep => "grep",
            ToolName::FindFilesystem => "find_filesystem",
            ToolName::FindCodebase => "find_codebase",
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

// ── the registry ─────────────────────────────────────────────────────
//
// The second half of `tools/index.ts`: one factory per tool name, the two
// presets, and the map of everything. It closes here because the last two tools
// it names — `task` and the three observation tools — arrive with plan task 10.

use std::collections::BTreeMap;
use std::sync::Arc;

use notagent_agent::types::AgentTool;

use crate::core::tools::bash::{BashToolOptions, create_bash_tool, create_bash_tool_definition};
use crate::core::tools::edit::{EditToolOptions, create_edit_tool, create_edit_tool_definition};
use crate::core::tools::find::{FindToolOptions, create_find_tool, create_find_tool_definition};
use crate::core::tools::find_codebase::{
    FindCodebaseToolOptions, create_find_codebase_tool, create_find_codebase_tool_definition,
};
use crate::core::tools::goal::{
    GoalToolSources, create_goal_tool, create_goal_tool_definition, create_update_goal_tool,
    create_update_goal_tool_definition,
};
use crate::core::tools::grep::{GrepToolOptions, create_grep_tool, create_grep_tool_definition};
use crate::core::tools::ls::{LsToolOptions, create_ls_tool, create_ls_tool_definition};
use crate::core::tools::patch_minified::{
    PatchMinifiedToolOptions, create_multi_patch_minified_tool,
    create_multi_patch_minified_tool_definition, create_patch_minified_tool,
    create_patch_minified_tool_definition,
};
use crate::core::tools::read::{ReadToolOptions, create_read_tool, create_read_tool_definition};
use crate::core::tools::read_minified::{
    ReadMinifiedToolOptions, create_read_minified_tool, create_read_minified_tool_definition,
};
use crate::core::tools::skill::{
    SkillToolSources, create_skill_tool, create_skill_tool_definition,
};
use crate::core::tools::task::{TaskToolSources, create_task_tool, create_task_tool_definition};
use crate::core::tools::task_tools::{
    TaskToolsSources, create_task_list_tool, create_task_list_tool_definition,
    create_task_output_tool, create_task_output_tool_definition, create_task_stop_tool,
    create_task_stop_tool_definition,
};
use crate::core::tools::todo_write::{
    TodoWriteToolSources, create_todo_write_tool, create_todo_write_tool_definition,
};
use crate::core::tools::tool_definition::ToolDefinition;
use crate::core::tools::write::{
    WriteToolOptions, create_write_tool, create_write_tool_definition,
};

/// `ToolDef` — a tool as the coding agent holds it.
pub type ToolDef = Arc<dyn ToolDefinition>;
/// `Tool` — the same tool as the agent loop takes it.
pub type Tool = Arc<dyn AgentTool>;

/// Deviation (class 1): TS keeps the per-tool option objects in one interface of
/// optional fields; the port does the same with `Option` fields, and the two
/// sources-only tools (`skill`, `todo_write`) carry their sources directly
/// rather than through a one-field wrapper object.
#[derive(Clone, Default)]
pub struct ToolsOptions {
    pub read: Option<ReadToolOptions>,
    pub read_minified: Option<ReadMinifiedToolOptions>,
    pub patch_minified: Option<PatchMinifiedToolOptions>,
    pub multi_patch_minified: Option<PatchMinifiedToolOptions>,
    pub skill: Option<SkillToolSources>,
    pub task: Option<TaskToolSources>,
    /// Shared by the three observation tools, which read one manager.
    pub tasks: Option<TaskToolsSources>,
    pub todo_write: Option<TodoWriteToolSources>,
    /// Port addition (v0.1.21): absent inside a subagent, which is what makes
    /// the two goal tools refuse there.
    pub goal: Option<GoalToolSources>,
    pub bash: Option<BashToolOptions>,
    pub write: Option<WriteToolOptions>,
    pub edit: Option<EditToolOptions>,
    pub grep: Option<GrepToolOptions>,
    pub find: Option<FindToolOptions>,
    pub find_codebase: Option<FindCodebaseToolOptions>,
    pub ls: Option<LsToolOptions>,
}

pub fn create_tool_definition(
    tool_name: ToolName,
    cwd: &str,
    options: Option<&ToolsOptions>,
) -> ToolDef {
    match tool_name {
        ToolName::Read => Arc::new(create_read_tool_definition(
            cwd,
            options.and_then(|options| options.read.clone()),
        )),
        ToolName::ReadMinified => Arc::new(create_read_minified_tool_definition(
            cwd,
            options.and_then(|options| options.read_minified.clone()),
        )),
        ToolName::Bash => Arc::new(create_bash_tool_definition(
            cwd,
            options.and_then(|options| options.bash.clone()),
        )),
        ToolName::Edit => Arc::new(create_edit_tool_definition(
            cwd,
            options.and_then(|options| options.edit.clone()),
        )),
        ToolName::PatchMinified => Arc::new(create_patch_minified_tool_definition(
            cwd,
            options.and_then(|options| options.patch_minified.clone()),
        )),
        ToolName::MultiPatchMinified => Arc::new(create_multi_patch_minified_tool_definition(
            cwd,
            options.and_then(|options| options.multi_patch_minified.clone()),
        )),
        ToolName::Skill => Arc::new(create_skill_tool_definition(
            options.and_then(|options| options.skill.clone()),
        )),
        ToolName::Task => Arc::new(create_task_tool_definition(
            options.and_then(|options| options.task.clone()),
        )),
        ToolName::TaskList => Arc::new(create_task_list_tool_definition(
            options.and_then(|options| options.tasks.clone()),
        )),
        ToolName::TaskOutput => Arc::new(create_task_output_tool_definition(
            options.and_then(|options| options.tasks.clone()),
        )),
        ToolName::TaskStop => Arc::new(create_task_stop_tool_definition(
            options.and_then(|options| options.tasks.clone()),
        )),
        ToolName::TodoWrite => Arc::new(create_todo_write_tool_definition(
            options.and_then(|options| options.todo_write.clone()),
        )),
        ToolName::CreateGoal => Arc::new(create_goal_tool_definition(
            options.and_then(|options| options.goal.clone()),
        )),
        ToolName::UpdateGoal => Arc::new(create_update_goal_tool_definition(
            options.and_then(|options| options.goal.clone()),
        )),
        ToolName::Write => Arc::new(create_write_tool_definition(
            cwd,
            options.and_then(|options| options.write.clone()),
        )),
        ToolName::Grep => Arc::new(create_grep_tool_definition(
            cwd,
            options.and_then(|options| options.grep.clone()),
        )),
        ToolName::FindFilesystem => Arc::new(create_find_tool_definition(
            cwd,
            options.and_then(|options| options.find.clone()),
        )),
        ToolName::FindCodebase => Arc::new(create_find_codebase_tool_definition(
            cwd,
            options.and_then(|options| options.find_codebase.clone()),
        )),
        ToolName::Ls => Arc::new(create_ls_tool_definition(
            cwd,
            options.and_then(|options| options.ls.clone()),
        )),
    }
}

pub fn create_tool(tool_name: ToolName, cwd: &str, options: Option<&ToolsOptions>) -> Tool {
    match tool_name {
        ToolName::Read => create_read_tool(cwd, options.and_then(|options| options.read.clone())),
        ToolName::ReadMinified => create_read_minified_tool(
            cwd,
            options.and_then(|options| options.read_minified.clone()),
        ),
        ToolName::Bash => create_bash_tool(cwd, options.and_then(|options| options.bash.clone())),
        ToolName::Edit => create_edit_tool(cwd, options.and_then(|options| options.edit.clone())),
        ToolName::PatchMinified => create_patch_minified_tool(
            cwd,
            options.and_then(|options| options.patch_minified.clone()),
        ),
        ToolName::MultiPatchMinified => create_multi_patch_minified_tool(
            cwd,
            options.and_then(|options| options.multi_patch_minified.clone()),
        ),
        ToolName::Skill => create_skill_tool(options.and_then(|options| options.skill.clone())),
        ToolName::Task => create_task_tool(options.and_then(|options| options.task.clone())),
        ToolName::TaskList => {
            create_task_list_tool(options.and_then(|options| options.tasks.clone()))
        }
        ToolName::TaskOutput => {
            create_task_output_tool(options.and_then(|options| options.tasks.clone()))
        }
        ToolName::TaskStop => {
            create_task_stop_tool(options.and_then(|options| options.tasks.clone()))
        }
        ToolName::TodoWrite => {
            create_todo_write_tool(options.and_then(|options| options.todo_write.clone()))
        }
        ToolName::CreateGoal => create_goal_tool(options.and_then(|options| options.goal.clone())),
        ToolName::UpdateGoal => {
            create_update_goal_tool(options.and_then(|options| options.goal.clone()))
        }
        ToolName::Write => {
            create_write_tool(cwd, options.and_then(|options| options.write.clone()))
        }
        ToolName::Grep => create_grep_tool(cwd, options.and_then(|options| options.grep.clone())),
        ToolName::FindFilesystem => {
            create_find_tool(cwd, options.and_then(|options| options.find.clone()))
        }
        ToolName::FindCodebase => create_find_codebase_tool(
            cwd,
            options.and_then(|options| options.find_codebase.clone()),
        ),
        ToolName::Ls => create_ls_tool(cwd, options.and_then(|options| options.ls.clone())),
    }
}

const CODING_TOOL_NAMES: [ToolName; 7] = [
    ToolName::Read,
    ToolName::ReadMinified,
    ToolName::Bash,
    ToolName::Edit,
    ToolName::PatchMinified,
    ToolName::MultiPatchMinified,
    ToolName::Write,
];

const READ_ONLY_TOOL_NAMES: [ToolName; 6] = [
    ToolName::Read,
    ToolName::ReadMinified,
    ToolName::Grep,
    ToolName::FindFilesystem,
    ToolName::FindCodebase,
    ToolName::Ls,
];

/// The tools that only look at the project. The transcript groups their calls
/// into one explore block, so the list is read there as well.
#[must_use]
pub fn read_only_tool_names() -> &'static [ToolName] {
    &READ_ONLY_TOOL_NAMES
}

pub fn create_coding_tool_definitions(cwd: &str, options: Option<&ToolsOptions>) -> Vec<ToolDef> {
    CODING_TOOL_NAMES
        .into_iter()
        .map(|name| create_tool_definition(name, cwd, options))
        .collect()
}

pub fn create_read_only_tool_definitions(
    cwd: &str,
    options: Option<&ToolsOptions>,
) -> Vec<ToolDef> {
    READ_ONLY_TOOL_NAMES
        .into_iter()
        .map(|name| create_tool_definition(name, cwd, options))
        .collect()
}

/// `createAllToolDefinitions` — every tool, keyed by name.
///
/// Deviation (class 1): a `BTreeMap` rather than a record. `ToolName` orders by
/// declaration, so iterating it yields the same sequence as the TS object
/// literal, which is the order the system prompt lists tools in.
pub fn create_all_tool_definitions(
    cwd: &str,
    options: Option<&ToolsOptions>,
) -> BTreeMap<ToolName, ToolDef> {
    ALL_TOOL_NAMES
        .into_iter()
        .map(|name| (name, create_tool_definition(name, cwd, options)))
        .collect()
}

pub fn create_coding_tools(cwd: &str, options: Option<&ToolsOptions>) -> Vec<Tool> {
    CODING_TOOL_NAMES
        .into_iter()
        .map(|name| create_tool(name, cwd, options))
        .collect()
}

pub fn create_read_only_tools(cwd: &str, options: Option<&ToolsOptions>) -> Vec<Tool> {
    READ_ONLY_TOOL_NAMES
        .into_iter()
        .map(|name| create_tool(name, cwd, options))
        .collect()
}

pub fn create_all_tools(cwd: &str, options: Option<&ToolsOptions>) -> BTreeMap<ToolName, Tool> {
    ALL_TOOL_NAMES
        .into_iter()
        .map(|name| (name, create_tool(name, cwd, options)))
        .collect()
}
