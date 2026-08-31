# core/tools — file and public-function sketch

Scope: the tool module tree under `crates/notagent/src/core/tools/` (plus the
module declaration at `crates/notagent/src/core/tools.rs`). Read only; no build
or test executed. The crate is `notagent`.

## Root module — `crates/notagent/src/core/tools.rs`

Declares every sub-module (`pub mod bash; pub mod edit; …`) and owns the
registry that maps a `ToolName` to a definition and to an agent-loop tool.

Key types / signatures:
- `enum ToolName` (21 variants, `#[serde(rename_all = "snake_case")]`) — the
  wire names, e.g. `ToolName::Read`, `ToolName::Edit` (renamed from `edit`).
- `ALL_TOOL_NAMES: [ToolName; 21]` — insertion order; also the order the
  system prompt lists them in.
- `type ToolDef = Arc<dyn ToolDefinition>` and `type Tool = Arc<dyn AgentTool>`.
- `struct ToolsOptions` — a bag of optional per-tool sources/options that feeds
  the builders below.
- `create_tool_definition(ToolName, &str cwd, Option<&ToolsOptions>) -> ToolDef`
  and `create_tool(ToolName, &str, Option<&ToolsOptions>) -> Tool` — the two
  factory functions, each a big `match` over `ToolName`.
- Presets: `create_all_tool_definitions`, `create_coding_tool_definitions`,
  `create_read_only_tool_definitions`, and their `*_tools` equivalents; plus
  `CODING_TOOL_NAMES` (7), `READ_ONLY_TOOL_NAMES` (6),
  `read_only_tool_names()`.

Every concrete tool follows the same pair-of-builders pattern: a `*_ToolDefinition`
(struct describing name / description / JSON parameter schema / execution)
and a `create_*_tool` that wraps it into an `Arc<dyn AgentTool>` for the loop.

## Common infrastructure

- `tool_definition.rs` — the shared contract every tool implements.
  - `trait ToolDefinition: Send + Sync` — `name`, `label`, `description`,
    `parameters -> &Value`, `execute(...) -> BoxFuture<AgentToolResult>`, plus
    optional render hooks (`render_call`, `render_result`, `render_deadline`,
    `pump_render`) and `prompt_snippet` / `prompt_guidelines`.
  - `struct ToolContext` (tool_call_id, args, cwd, session metadata) and
    `ToolRenderResult` / `ToolRenderContext` used by the render hooks.
  - `wrap_tool_definition(Arc<dyn ToolDefinition>, Option<...>) -> Arc<dyn AgentTool>` —
    forwards `description`/`parameters` lazily so a tool can advertise based on
    live session state.
- `truncate.rs` — output limiting shared by most tools.
  - `const DEFAULT_MAX_LINES = 2000`, `DEFAULT_MAX_BYTES = 50 KB`,
    `GREP_MAX_LINE_LENGTH = 500`; `enum TruncatedBy`, `struct TruncationResult`.
  - `truncate_head` / `truncate_tail` / `truncate_line`, `truncation_from_details`,
    `format_size`.
- `render_utils.rs` — UI formatting helpers: `str_arg`, `get_text_output`,
  `invalid_arg_text`, `call_title`, `shorten_path`, `link_path`,
  `render_tool_path` / `render_mutation_tool_path`.
- `output_accumulator.rs` — buffers raw command output to a temp file and
  produces snapshots. `OutputAccumulator::new/append/finish/snapshot`,
  `OutputAccumulatorOptions`, `OutputSnapshot` (content, truncation,
  full_output_path).
- `path_utils.rs` — filesystem helpers: `path_exists`, `expand_path`,
  `resolve_to_cwd`, `resolve_read_path`.

## Individual tools (one line each)

- `read.rs` — returns file/text content. `ReadOperations` trait,
  `LocalReadOperations`, `ReadToolOptions`, `create_read_tool_definition`,
  `create_read_tool`.
- `read_minified.rs` — same as `read` but with comments/blank lines collapsed.
  `ReadMinifiedOperations`, `LocalReadMinifiedOperations`,
  `create_read_minified_tool_definition` / `create_read_minified_tool`.
- `bash.rs` — executes shell commands with a full lifecycle: timeouts,
  foreground/background, abort escalation, throttled output updates, bash
  filter. Large file. `BashOperations`/`LocalBashOperations` traits,
  `BashExecOptions`, `BashExecError`, `BashToolOptions`, `BashToolSources`,
  `BashTaskManager` (detach to the task manager), `BashSpawnHook`,
  `create_bash_tool_definition` / `create_bash_tool`. Defaults: foreground
  10 min / 1h max, background 10 min / 24h max / unlimited.
- `edit.rs` — the `patch` tool: `old_string`/`new_string` replacement.
  `EditOperations`, `LocalEditOperations`, `EditToolOptions`,
  `create_edit_tool_definition` / `create_edit_tool`, `prepare_edit_arguments`.
- `edit_diff.rs` — text-diff / fuzzy-match core used by edit & patch_minified:
  line-ending detection, BOM stripping, fuzzy matching, unified-patch generation.
  `detect_line_ending`, `fuzzy_find_text`, `apply_edits_to_normalized_content`,
  `generate_unified_patch` / `generate_diff_string`.
- `patch_minified.rs` — precise edits through the compact minified view; single
  and multi-patch. `PatchMinifiedOperations`, `LocalPatchMinifiedOperations`,
  `PatchMinifiedToolOptions`, `create_patch_minified_tool` /
  `create_multi_patch_minified_tool`.
- `ls.rs` — lists a directory. `LsOperations`, `LocalLsOperations`,
  `LsToolOptions`, `create_ls_tool_definition` / `create_ls_tool`.
- `grep.rs` — regex/substring search. `GrepOperations`, `LocalGrepOperations`,
  `GrepToolOptions`, `create_grep_tool_definition` / `create_grep_tool`.
- `find.rs` — glob `find_filesystem` search. `FindOperations`,
  `FindToolOptions`, `create_find_tool_definition` / `create_find_tool`,
  `relativize_find_result_path`.
- `find_codebase.rs` — BM25 tree-sitter symbol search, index rebuild.
  `IndexBuildPhase`, `IndexBuildProgress`, `rebuild_index_blocking`,
  `FindCodebaseToolOptions`, `create_find_codebase_tool` /
  `create_find_codebase_tool_definition`.
- `write.rs` — writes / overwrites files. `WriteOperations`,
  `LocalWriteOperations`, `WriteToolOptions`, `create_write_tool_definition` /
  `create_write_tool`.
- `plan_create.rs` — writes a file into the workspace `plans/` directory.
  `PlanCreateOperations`, `LocalPlanCreateOperations`, `PlanCreateToolOptions`,
  `PLANS_DIR_NAME`.
- `undo.rs` — reverts the last change to one file from a snapshot.
  `UndoToolOptions`, `create_undo_tool_definition` / `create_undo_tool`.
- `todo_write.rs` — manages a todo list store. `TodoStoreSource`,
  `TodoWriteToolSources`, `create_todo_write_tool_definition` / `create_todo_write_tool`.
- `goal.rs` — create / update goal states. `GoalStateSource`, `GoalTodoSource`,
  `GoalToolSources`, `create_goal_tool` / `create_update_goal_tool`.
- `task.rs` — the `task` (subagent) tool. `DelegatableSkill`,
  `TaskToolSources`, `TaskTranscriptStore`, `create_task_tool_definition` /
  `create_task_tool`.
- `task_tools.rs` — observation tools over a task manager: `task_list`,
  `task_output`, `task_stop`. `TaskManagerSource`, `TaskToolsSources`,
  `create_task_list_tool` / `create_task_output_tool` / `create_task_stop_tool`.
- `skill.rs` — loads a skill's instructions by name. `SkillToolSkill`,
  `SkillToolSources`, `create_skill_tool` / `create_skill_tool_definition`.
- `mcp.rs` — Model Control Protocol tools. `McpToolDefinition`,
  `create_mcp_tool_definition`, `mcp_tool_definitions`, `is_mcp_tool_name`.
- `mcp_auth.rs` — auth wiring for MCP servers. `McpToolsChanged`,
  `auth_tool_name`, `create_mcp_auth_tool_definition`.
- `mcp_render.rs` — formatting of MCP call / result UI. `format_mcp_call`,
  `format_mcp_result`.
- `file_lease.rs` — workspace file leases to prevent two agents editing the
  same file at once. `LeaseStatus`, `FileLease` (builder API), `FileLeaseStore`,
  `LeaseCoordinator::new(for_workspace)`, `LeaseGate`.
- `file_mutation_queue.rs` — groups in-flight file mutations so a write waits
  for concurrent edits to the same path. `is_queue_tracked`, `shares_queue`.

## Observation

The module is uniform: one `*_ToolDefinition` struct plus a matching
`create_*_tool` factory per tool, all funneling through the `ToolName` match in
`tools.rs`. Mutating tools share `file_lease` / `file_mutation_queue` guards and
write output through `output_accumulator`; read/search tools share `truncate`
and `path_utils`. `bash.rs` is by far the largest due to process lifecycle and
output throttling.
