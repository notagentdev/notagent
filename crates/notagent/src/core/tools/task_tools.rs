//! Port of `packages/coding-agent/src/core/tools/task-tools.ts` (tool halves).
//!
//! The three tools that observe background work: `task_list`, `task_output`
//! and `task_stop`.
//!
//! They are one file because they are one capability. A model that can list
//! tasks but not read one has nothing useful, and a model that can stop one but
//! not see it exists is dangerous — which is why background execution is gated
//! on all three being present rather than on any one of them.
//!
//! None of them blocks. `task_output` in particular returns whatever exists
//! right now and says whether the task has settled, because the alternative —
//! a tool that waits — is how a model turns "I started this in the background"
//! into a blocked turn with extra steps.
//!
//! `renderCall`/`renderResult` need the theme and are wired in task 13.

use std::sync::Arc;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_tui::tui::ComponentRef;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::tasks::manager::TaskManager;
use crate::core::tasks::types::{TaskInfo, is_terminal_task_status};
use crate::core::tools::render_utils::str_arg;
use crate::core::tools::tool_definition::{
    ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
    display_arg, render_text_call, render_text_result, wrap_tool_definition,
};
use crate::modes::interactive::theme::theme::{Theme, ThemeColor};

/// Bytes of output returned inline. The complete log is read with `read`.
const OUTPUT_PREVIEW_BYTES: u64 = 32 * 1024;

/// What the three tools need from the session.
///
/// Undefined in a session that has no task manager, such as a subagent.
pub type TaskManagerSource = Arc<dyn Fn() -> Option<TaskManager> + Send + Sync>;

#[derive(Clone)]
pub struct TaskToolsSources {
    pub manager: TaskManagerSource,
}

impl Default for TaskToolsSources {
    fn default() -> Self {
        TaskToolsSources {
            manager: Arc::new(|| None),
        }
    }
}

fn require_manager(sources: &TaskToolsSources) -> Result<TaskManager, ToolExecutionError> {
    (sources.manager)().ok_or_else(|| {
        ToolExecutionError::new("Background tasks are not available in this session.")
    })
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// One task as a block of plain fields, which reads well in a tool result.
fn render_task(info: &TaskInfo) -> String {
    let mut lines = vec![
        format!("task_id: {}", info.task_id()),
        format!("kind: {}", info.kind()),
        format!("status: {}", info.status()),
        format!("description: {}", info.description()),
    ];
    match info {
        TaskInfo::Shell(shell) => {
            lines.push(format!("command: {}", shell.command));
            if shell.pid > 0 {
                lines.push(format!("pid: {}", shell.pid));
            }
            if let Some(exit_code) = shell.exit_code {
                lines.push(format!("exit_code: {exit_code}"));
            }
        }
        TaskInfo::Subagent(subagent) => {
            lines.push(format!("session_id: {}", subagent.session_id));
            lines.push(format!("mode: {}", subagent.mode_id));
        }
    }
    if let Some(stop_reason) = info.stop_reason() {
        lines.push(format!("stop_reason: {stop_reason}"));
    }
    let base = info.base();
    let elapsed = (base.ended_at.unwrap_or_else(now_ms) - base.started_at) as f64 / 1000.0;
    let seconds = js_round(elapsed);
    lines.push(format!(
        "{}: {seconds}s",
        if is_terminal_task_status(info.status()) {
            "ran_for"
        } else {
            "running_for"
        }
    ));
    lines.join("\n")
}

/// `Math.round` — half away from zero towards positive infinity, unlike the
/// half-away-from-zero of Rust's `f64::round`.
fn js_round(value: f64) -> i64 {
    (value + 0.5).floor() as i64
}

fn text_result(text: String, details: Option<Value>) -> AgentToolResult {
    AgentToolResult {
        content: vec![TextOrImageContent::Text(TextContent::new(text))],
        details,
        usage: None,
        added_tool_names: None,
        terminate: None,
    }
}

// ── task_list ────────────────────────────────────────────────────────

const LIST_DESCRIPTION: &str = concat!(
    "Lists background tasks and where each one stands.\n",
    "\n",
    "Use it when you are unsure what is still running or have lost a task id — after a compaction, for instance. It reports the id, kind, status and description of every task, plus the command and exit code for a shell task and the session id for a subagent.\n",
    "\n",
    "By default only running tasks are listed. Pass `all` to include ones that have finished; that listing can also contain tasks marked lost, which are left over from a previous session and can no longer be read or stopped.\n",
    "\n",
    "This changes nothing and is always safe to call.",
);

pub struct TaskListToolDefinition {
    sources: TaskToolsSources,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_task_list_tool_definition(
    sources: Option<TaskToolsSources>,
) -> TaskListToolDefinition {
    TaskListToolDefinition {
        sources: sources.unwrap_or_default(),
        parameters: json!({
            "type": "object",
            "properties": {
                "all": {
                    "type": "boolean",
                    "description": "Include tasks that have already finished. Defaults to false, which lists only running ones.",
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of tasks to return. Defaults to 20.",
                },
            },
        }),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

impl ToolDefinition for TaskListToolDefinition {
    fn name(&self) -> &str {
        "task_list"
    }

    fn label(&self) -> &str {
        "task_list"
    }

    fn description(&self) -> &str {
        LIST_DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("List running background tasks")
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        self.constrained_sampling.as_ref()
    }

    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        // `=== true` — anything else is the running scope.
        let scope = if args.get("all") == Some(&Value::Bool(true)) {
            "all"
        } else {
            "running"
        };
        Some(render_text_call(
            context,
            &format!(
                "{} {}",
                theme.fg(ThemeColor::ToolTitle, &theme.bold("task_list")),
                theme.fg(ThemeColor::Muted, scope)
            ),
        ))
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        _options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let count = result
            .details
            .and_then(|details| details.get("count"))
            .map_or_else(|| "0".to_string(), display_arg);
        Some(render_text_result(
            context,
            &theme.fg(ThemeColor::Muted, &format!("\n{count} task(s)")),
        ))
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        _signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let manager = require_manager(&self.sources)?;
            let all = params.get("all").and_then(Value::as_bool) == Some(true);
            let limit = match params.get("limit").and_then(Value::as_f64) {
                Some(limit) if limit > 0.0 => limit.trunc() as usize,
                _ => 20,
            };
            let tasks = manager.list(!all, Some(limit));
            let header = format!(
                "{}: {}",
                if all {
                    "background_tasks"
                } else {
                    "running_background_tasks"
                },
                tasks.len()
            );
            let body = if tasks.is_empty() {
                "No matching background tasks.".to_owned()
            } else {
                tasks
                    .iter()
                    .map(render_task)
                    .collect::<Vec<String>>()
                    .join("\n---\n")
            };
            Ok(text_result(
                format!("{header}\n{body}"),
                Some(json!({ "count": tasks.len(), "all": all })),
            ))
        })
    }
}

// ── task_output ──────────────────────────────────────────────────────

const OUTPUT_DESCRIPTION: &str = concat!(
    "Returns a task's current status and a tail of its output.\n",
    "\n",
    "This never waits. It answers with whatever exists at the moment you ask, so it is a progress check you act on and continue from — not a way to sit and wait for a task you just started. If you need a result before you can go on, run that work in the foreground instead.\n",
    "\n",
    "You do not need this to learn that a task finished: a completion arrives on its own. Reach for it when you want to see progress before then.\n",
    "\n",
    "The tail is capped. When the full log exists its path is reported, and `read` will page through the whole thing.",
);

pub struct TaskOutputToolDefinition {
    sources: TaskToolsSources,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_task_output_tool_definition(
    sources: Option<TaskToolsSources>,
) -> TaskOutputToolDefinition {
    TaskOutputToolDefinition {
        sources: sources.unwrap_or_default(),
        parameters: json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "The background task to inspect." },
            },
            "required": ["task_id"],
        }),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

impl ToolDefinition for TaskOutputToolDefinition {
    fn name(&self) -> &str {
        "task_output"
    }

    fn label(&self) -> &str {
        "task_output"
    }

    fn description(&self) -> &str {
        OUTPUT_DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("Read a background task's output")
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        self.constrained_sampling.as_ref()
    }

    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let task_id = str_arg(args.get("task_id")).unwrap_or_default();
        Some(render_text_call(
            context,
            &format!(
                "{} {}",
                theme.fg(ThemeColor::ToolTitle, &theme.bold("task_output")),
                theme.fg(ThemeColor::Accent, &task_id)
            ),
        ))
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        _options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let text = match result.details {
            Some(details) => theme.fg(
                ThemeColor::Muted,
                &format!(
                    "\n{}",
                    details.get("status").map_or_else(String::new, display_arg)
                ),
            ),
            None => String::new(),
        };
        Some(render_text_result(context, &text))
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        _signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let manager = require_manager(&self.sources)?;
            let task_id = params
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let Some(info) = manager.get(&task_id) else {
                return Err(ToolExecutionError::new(format!(
                    "No background task with id \"{task_id}\"."
                )));
            };

            let output = manager
                .output_snapshot(&task_id, OUTPUT_PREVIEW_BYTES)
                .await;
            let mut lines = vec![
                format!(
                    "retrieval: {}",
                    if is_terminal_task_status(info.status()) {
                        "final"
                    } else {
                        "in_progress"
                    }
                ),
                render_task(&info),
                format!("output_bytes: {}", output.total_bytes),
            ];
            if let Some(output_path) = &output.output_path {
                lines.push(format!("output_path: {output_path}"));
            }
            if output.truncated {
                lines.push(match &output.output_path {
                    Some(_) => format!(
                        "[showing the last {} bytes; read output_path for the whole log]",
                        output.preview_bytes
                    ),
                    None => format!(
                        "[showing the last {} bytes; no full log was kept for this task]",
                        output.preview_bytes
                    ),
                });
            }
            lines.push("[output]".to_owned());
            lines.push(if output.preview.is_empty() {
                "(nothing yet)".to_owned()
            } else {
                output.preview.clone()
            });
            let mut details = json!({
                "taskId": info.task_id(),
                "status": info.status().as_str(),
                "truncated": output.truncated,
            });
            if let Some(output_path) = &output.output_path {
                details["outputPath"] = Value::String(output_path.clone());
            }
            Ok(text_result(lines.join("\n"), Some(details)))
        })
    }
}

// ── task_stop ────────────────────────────────────────────────────────

const STOP_DESCRIPTION: &str = concat!(
    "Stops a running background task.\n",
    "\n",
    "Only for work that genuinely has to be cancelled. A task that is finishing normally will report itself; stopping it to find out where it got to loses whatever it had not written yet.\n",
    "\n",
    "Stopping is destructive — a command interrupted halfway may leave partial changes behind. A task that has already finished is reported as it stands and nothing happens.",
);

pub struct TaskStopToolDefinition {
    sources: TaskToolsSources,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_task_stop_tool_definition(
    sources: Option<TaskToolsSources>,
) -> TaskStopToolDefinition {
    TaskStopToolDefinition {
        sources: sources.unwrap_or_default(),
        parameters: json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "The background task to stop." },
                "reason": { "type": "string", "description": "Short reason, recorded with the task." },
            },
            "required": ["task_id"],
        }),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

impl ToolDefinition for TaskStopToolDefinition {
    fn name(&self) -> &str {
        "task_stop"
    }

    fn label(&self) -> &str {
        "task_stop"
    }

    fn description(&self) -> &str {
        STOP_DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("Stop a background task")
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        self.constrained_sampling.as_ref()
    }

    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let task_id = str_arg(args.get("task_id")).unwrap_or_default();
        Some(render_text_call(
            context,
            &format!(
                "{} {}",
                theme.fg(ThemeColor::ToolTitle, &theme.bold("task_stop")),
                theme.fg(ThemeColor::Accent, &task_id)
            ),
        ))
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        _options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let text = match result.details {
            Some(details) => theme.fg(
                ThemeColor::Muted,
                &format!(
                    "\n{}",
                    details.get("status").map_or_else(String::new, display_arg)
                ),
            ),
            None => String::new(),
        };
        Some(render_text_result(context, &text))
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        _signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let manager = require_manager(&self.sources)?;
            let task_id = params
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let Some(info) = manager.get(&task_id) else {
                return Err(ToolExecutionError::new(format!(
                    "No background task with id \"{task_id}\"."
                )));
            };

            if is_terminal_task_status(info.status()) {
                return Ok(text_result(
                    format!(
                        "{}\nnote: already finished; nothing was stopped.",
                        render_task(&info)
                    ),
                    Some(json!({ "taskId": info.task_id(), "status": info.status().as_str() })),
                ));
            }

            let reason = params
                .get("reason")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .unwrap_or("Stopped by task_stop")
                .to_owned();
            // Suppressed first: the result below is the answer, and a completion
            // arriving afterwards would tell the model the same thing twice.
            manager.suppress_notification(&task_id).await;
            let Some(stopped) = manager.stop(&task_id, Some(&reason)).await else {
                return Err(ToolExecutionError::new(format!(
                    "Could not stop task \"{task_id}\"."
                )));
            };
            Ok(text_result(
                render_task(&stopped),
                Some(json!({ "taskId": stopped.task_id(), "status": stopped.status().as_str() })),
            ))
        })
    }
}

pub fn create_task_list_tool(sources: Option<TaskToolsSources>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_task_list_tool_definition(sources)), None)
}

pub fn create_task_output_tool(sources: Option<TaskToolsSources>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_task_output_tool_definition(sources)), None)
}

pub fn create_task_stop_tool(sources: Option<TaskToolsSources>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_task_stop_tool_definition(sources)), None)
}
