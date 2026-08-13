//! Port of `packages/coding-agent/src/core/tools/todo-write.ts` (tool half).
//!
//! The `todo_write` tool: the session's task list.
//!
//! Every call carries the complete list. That is the whole protocol, and the
//! reason for it is that the list belongs to the model rather than to the user:
//! a model allowed to append will append, and a list nobody prunes stops being
//! read. Restating everything on each call forces a decision about each item,
//! and omission is how something leaves.
//!
//! The description is long because it is doing two jobs — teaching the protocol
//! and teaching when the list is worth keeping at all. Both come from the
//! reference implementation, which had them tuned against real sessions.
//!
//! `renderCall`/`renderResult` need the theme and are wired in task 13; the
//! data they render is already here (`todos::render::build_todo_diff_lines`).

use std::sync::{Arc, Mutex};

use notagent_agent::types::{
    AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::todos::render::render_todos_updated;
use crate::core::todos::{TODO_STATUSES, Todo, TodoStatus, TodoStore};
use crate::core::tools::tool_definition::{SystemPromptContribution, ToolContext, ToolDefinition};

pub const TODO_WRITE_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Track multi-step work in a task list",
        guidelines: &[
            "Keep a task list with todo_write for work of three or more steps: mark one task in_progress before starting it and completed as soon as it is genuinely done.",
        ],
    };

fn todo_write_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "todos": {
                "type": "array",
                "description": "Complete desired todo list. This replaces the currently stored list; omitted items are removed and an empty list clears all todos. Completed lists are cleaned up automatically.",
                "items": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "description": "The task in imperative form, e.g. \"Run tests\"." },
                        "activeForm": {
                            "type": "string",
                            "description": "The same task in present continuous form, shown while it runs, e.g. \"Running tests\".",
                        },
                        "status": {
                            "description": "Current status of the task.",
                            "enum": TODO_STATUSES.iter().map(|status| status.as_str()).collect::<Vec<_>>(),
                        },
                    },
                    "required": ["content", "activeForm", "status"],
                },
            },
        },
        "required": ["todos"],
    })
}

const DESCRIPTION: &str = concat!(
    "Create and manage a structured task list for the current session. It gives the user visibility into your progress; it is not a planning scratchpad.\n",
    "\n",
    "## Protocol\n",
    "\n",
    "Each call supplies the complete task list to keep and replaces the stored list.\n",
    "\n",
    "Each item has three required fields:\n",
    "- `content`: the task description in imperative form, e.g. \"Run tests\".\n",
    "- `activeForm`: the same task in present continuous form, shown while it runs, e.g. \"Running tests\".\n",
    "- `status`: one of `pending`, `in_progress`, or `completed`.\n",
    "\n",
    "Rules:\n",
    "- Include every task that should remain on each call; omitted tasks are removed.\n",
    "- Preserve the intended task order in the array; repeated descriptions remain separate entries.\n",
    "- Always supply both wordings; neither may be empty.\n",
    "- Send `todos: []` to clear the list immediately.\n",
    "- Once every item in a non-empty replacement list is `completed`, the server returns that completed state and then clears the list automatically.\n",
    "\n",
    "## Usage\n",
    "\n",
    "Use it for multi-step work (roughly three or more distinct steps), when the user lists several tasks, or when they ask for it. Skip it for single straightforward tasks, trivial changes, and purely conversational or informational requests — just do the work.\n",
    "\n",
    "Mark a task `in_progress` before starting it (one at a time), and `completed` as soon as it is genuinely done — implemented and, where relevant, verified. Don't batch completions, and don't narrate every status change in chat. If you hit a blocker, keep the task `in_progress` and add a new task describing what must be resolved. Never mark a task completed while tests fail, the implementation is partial, or errors remain unresolved."
);

/// The session's store. Absent where no list is kept, such as a subagent.
pub type TodoStoreSource = Arc<dyn Fn() -> Option<Arc<Mutex<TodoStore>>> + Send + Sync>;

#[derive(Clone)]
pub struct TodoWriteToolSources {
    pub store: TodoStoreSource,
}

impl Default for TodoWriteToolSources {
    /// Empty sources, so the tool is constructible before a session supplies a
    /// real store.
    fn default() -> Self {
        Self {
            store: Arc::new(|| None),
        }
    }
}

pub struct TodoWriteToolDefinition {
    sources: TodoWriteToolSources,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_todo_write_tool_definition(
    sources: Option<TodoWriteToolSources>,
) -> TodoWriteToolDefinition {
    TodoWriteToolDefinition {
        sources: sources.unwrap_or_default(),
        parameters: todo_write_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

/// One item as the tool receives it; an invalid item is a bad call, so the
/// error reaches the model rather than being repaired.
fn parse_todo(value: &Value) -> Result<Todo, String> {
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let active_form = value
        .get("activeForm")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .and_then(TodoStatus::parse)
        .ok_or_else(|| {
            format!(
                "Todo status must be one of {}",
                TODO_STATUSES
                    .iter()
                    .map(|status| status.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    Ok(Todo {
        content,
        active_form,
        status,
    })
}

impl ToolDefinition for TodoWriteToolDefinition {
    fn name(&self) -> &str {
        "todo_write"
    }

    fn label(&self) -> &str {
        "todo_write"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(TODO_WRITE_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        TODO_WRITE_TOOL_SYSTEM_PROMPT_CONTRIBUTION
            .guidelines
            .iter()
            .map(|guideline| (*guideline).to_owned())
            .collect()
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        self.constrained_sampling.as_ref()
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
            let Some(store) = (self.sources.store)() else {
                return Err(ToolExecutionError::new(
                    "A task list is not available in this session.",
                ));
            };
            let todos = params
                .get("todos")
                .and_then(Value::as_array)
                .map(|todos| todos.iter().map(parse_todo).collect::<Result<Vec<_>, _>>())
                .transpose()
                .map_err(ToolExecutionError::new)?
                .unwrap_or_default();

            let mut store = store.lock().expect("todo store mutex");
            let before: Vec<Todo> = store.all().to_vec();
            // `replace` rejects an invalid item, which reaches the model as a
            // tool error — the list is left untouched rather than half-applied.
            let after = store.replace(todos).map_err(ToolExecutionError::new)?;
            drop(store);

            let mut details = Map::new();
            details.insert(
                "before".to_owned(),
                serde_json::to_value(&before).expect("todos are serializable"),
            );
            details.insert(
                "after".to_owned(),
                serde_json::to_value(&after).expect("todos are serializable"),
            );
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(
                    render_todos_updated(&before, &after),
                ))],
                details: Some(Value::Object(details)),
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}
