use std::sync::{Arc, Mutex};

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_tui::tui::ComponentRef;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::goal::{
    CompletionVerdict, GoalState, ThreadGoal, validate_goal_block_reason, validate_goal_budget,
    validate_thread_goal_objective,
};
use crate::core::todos::{Todo, is_todo_active};
use crate::core::tools::render_utils::{call_title, get_text_output};
use crate::core::tools::tool_definition::{
    SystemPromptContribution, ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult,
    ToolRenderResultOptions, render_text_call, render_text_result, wrap_tool_definition,
};
use crate::modes::interactive::theme::theme::Theme;

pub const GOAL_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Pursue a long-running goal across turns",
        guidelines: &[
            "While a goal is active, ending a turn does not end the goal. Use update_goal to report genuine completion or a blocker; continuation also stops at a budget limit or when the user pauses the goal.",
        ],
    };

const CREATE_DESCRIPTION: &str = "Create a long-running goal for this session.

Create a goal only when explicitly requested by the user or by system/developer instructions; do not infer a goal from an ordinary task. Once set, the agent keeps working toward the objective across turns on its own until the goal is marked complete (via `update_goal`) or a budget is reached.

Starts a new active goal only when no goal is currently defined; if a goal already exists this tool fails — use `update_goal` to complete the existing one first. Set the budgets only when an explicit budget is requested.";

const UPDATE_DESCRIPTION: &str = "End the session's goal, either because it is achieved or because you cannot proceed.

While a goal is active you are continued automatically after every turn unless a budget limit is reached or the user pauses the goal. Use this tool to report genuine completion or a blocker; ending a turn with an explanation does not by itself stop continuation.

Set `status` to `complete` only when the objective has actually been achieved and no required work remains. Do not mark a goal complete merely because its budget is nearly exhausted, because you produced a plan or a first pass, or because you are stopping work. A completion claim made while the task list still has open items is refused; finish them, or drop what is no longer required with todo_write.

Set `status` to `blocked`, with a concrete `reason`, when progress needs something only the user can supply, when the next step would be irreversible or externally visible and has not been authorized, or when the objective cannot be met as stated. Do not use `blocked` because a step is merely hard — attempt the work first. A blocked goal is not discarded: the user reads your reason and resumes it with `/goal resume`.

Pausing, resuming and budget limits are controlled by the user, not this tool.";

/// Reads the session's goal state. Absent inside a subagent, which is what
/// makes the two tools refuse there.
pub type GoalStateSource = Arc<dyn Fn() -> Option<Arc<Mutex<GoalState>>> + Send + Sync>;

/// Reads the session's task list, for the completion guard.
pub type GoalTodoSource = Arc<dyn Fn() -> Vec<Todo> + Send + Sync>;

#[derive(Clone)]
pub struct GoalToolSources {
    pub state: GoalStateSource,
    pub todos: GoalTodoSource,
}

impl Default for GoalToolSources {
    /// Empty sources, so the tools are constructible before a session supplies
    /// real ones — and so a subagent's tools refuse rather than act.
    fn default() -> Self {
        Self {
            state: Arc::new(|| None),
            todos: Arc::new(Vec::new),
        }
    }
}

const NO_SESSION: &str = "Goals are not available here: this agent has no session-level goal state. Report what you found to whoever delegated the work instead.";

fn create_goal_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "objective": {
                "type": "string",
                "description": "The concrete objective to start pursuing. Starts a new active goal only when no goal exists; if one already exists, this tool fails.",
            },
            "token_budget": {
                "type": "integer",
                "description": "Optional positive token budget for the new goal.",
            },
            "turn_budget": {
                "type": "integer",
                "description": "Optional positive limit on continuation turns. Whichever budget runs out first stops the goal.",
            },
        },
        "required": ["objective"],
    })
}

fn update_goal_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "status": {
                "description": "How the goal ends: `complete` when the objective is achieved, `blocked` when you cannot proceed.",
                "enum": ["complete", "blocked"],
            },
            "reason": {
                "type": "string",
                "description": "Why you cannot proceed. Required for `blocked`, ignored for `complete`. Name the missing thing concretely — the user reads this to decide whether to unblock and resume the goal.",
            },
        },
        "required": ["status"],
    })
}

pub struct GoalToolDefinition {
    /// `false` for `create_goal`, `true` for `update_goal`.
    update: bool,
    sources: GoalToolSources,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_goal_tool_definition(sources: Option<GoalToolSources>) -> GoalToolDefinition {
    GoalToolDefinition {
        update: false,
        sources: sources.unwrap_or_default(),
        parameters: create_goal_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

pub fn create_update_goal_tool_definition(sources: Option<GoalToolSources>) -> GoalToolDefinition {
    GoalToolDefinition {
        update: true,
        sources: sources.unwrap_or_default(),
        parameters: update_goal_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

impl GoalToolDefinition {
    fn state(&self) -> Result<Arc<Mutex<GoalState>>, ToolExecutionError> {
        (self.sources.state)().ok_or_else(|| ToolExecutionError::new(NO_SESSION))
    }

    /// The still-open items of the task list, as the guard reads them.
    fn open_todos(&self) -> Vec<String> {
        (self.sources.todos)()
            .into_iter()
            .filter(is_todo_active)
            .map(|todo| todo.content)
            .collect()
    }

    fn run_create(&self, params: &Value) -> Result<String, ToolExecutionError> {
        let state = self.state()?;
        let objective = params
            .get("objective")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let token_budget = params.get("token_budget").and_then(Value::as_i64);
        let turn_budget = params.get("turn_budget").and_then(Value::as_i64);
        validate_thread_goal_objective(&objective).map_err(ToolExecutionError::new)?;
        validate_goal_budget(token_budget).map_err(ToolExecutionError::new)?;
        validate_goal_budget(turn_budget).map_err(ToolExecutionError::new)?;

        let mut state = state.lock().expect("goal state");
        if state.goal.is_some() {
            return Err(ToolExecutionError::new(
                "this session already has a goal; use update_goal to complete it before creating a new one",
            ));
        }
        let goal = ThreadGoal::new(objective, token_budget, turn_budget);
        let mut budgets = Vec::new();
        if let Some(budget) = goal.token_budget {
            budgets.push(format!("{budget} tokens"));
        }
        if let Some(budget) = goal.turn_budget {
            budgets.push(format!("{budget} turns"));
        }
        let summary = if budgets.is_empty() {
            format!("Goal set: {}", goal.objective)
        } else {
            format!(
                "Goal set (budget {}): {}",
                budgets.join(", "),
                goal.objective
            )
        };
        state.goal = Some(goal);
        state.completion.clear();
        Ok(summary)
    }

    fn run_update(&self, params: &Value) -> Result<String, ToolExecutionError> {
        let state = self.state()?;
        let status = params
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let reason = params
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        // Read the task list before taking the goal lock: the guard needs it,
        // and the two locks have no ordering between them.
        let open = self.open_todos();

        let mut state = state.lock().expect("goal state");
        let strict = state.goal.as_ref().is_some_and(|goal| goal.strict);
        let Some(goal) = state.goal.as_ref() else {
            return Err(ToolExecutionError::new("there is no goal to update"));
        };
        let usage = match goal.token_budget {
            Some(budget) => format!("{} of {budget} tokens", goal.tokens_used),
            None => format!("{} tokens", goal.tokens_used),
        };

        match status.as_str() {
            "complete" => match state.completion.judge(strict, &open) {
                CompletionVerdict::Refuse(message) | CompletionVerdict::SelfCheck(message) => {
                    Err(ToolExecutionError::new(message))
                }
                CompletionVerdict::Accept => {
                    state.goal.as_mut().expect("goal").mark_complete();
                    Ok(format!("Goal complete. Final usage: {usage}."))
                }
            },
            "blocked" => {
                validate_goal_block_reason(&reason).map_err(ToolExecutionError::new)?;
                state.completion.clear();
                state.goal.as_mut().expect("goal").mark_blocked(&reason);
                Ok(format!(
                    "Goal blocked: {reason}. Usage so far: {usage}. It stays stopped until the \
                     user runs `/goal resume`."
                ))
            }
            other => Err(ToolExecutionError::new(format!(
                "unknown goal status `{other}`; use `complete` or `blocked`"
            ))),
        }
    }
}

impl ToolDefinition for GoalToolDefinition {
    fn name(&self) -> &str {
        if self.update {
            "update_goal"
        } else {
            "create_goal"
        }
    }

    fn label(&self) -> &str {
        if self.update {
            "update goal"
        } else {
            "create goal"
        }
    }

    fn description(&self) -> &str {
        if self.update {
            UPDATE_DESCRIPTION
        } else {
            CREATE_DESCRIPTION
        }
    }

    fn prompt_snippet(&self) -> Option<&str> {
        // One contribution for the pair; the create half carries it.
        (!self.update).then_some(GOAL_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        if self.update {
            return Vec::new();
        }
        GOAL_TOOL_SYSTEM_PROMPT_CONTRIBUTION
            .guidelines
            .iter()
            .map(|line| (*line).to_owned())
            .collect()
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
        let detail = if self.update {
            args.get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        } else {
            args.get("objective")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let title = call_title(theme, self.label());
        Some(render_text_call(context, &format!("{title}{detail}")))
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        _options: ToolRenderResultOptions,
        _theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        Some(render_text_result(
            context,
            &get_text_output(Some(result.content), false),
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
            let output = if self.update {
                self.run_update(&params)?
            } else {
                self.run_create(&params)?
            };
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(output))],
                details: None,
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

pub fn create_goal_tool(sources: Option<GoalToolSources>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_goal_tool_definition(sources)), None)
}

pub fn create_update_goal_tool(sources: Option<GoalToolSources>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_update_goal_tool_definition(sources)), None)
}
