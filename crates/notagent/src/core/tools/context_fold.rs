//! `context_fold`: lets the model fold finished work into a summary of its
//! own, take a fold back, and search inside folds. The mechanics live in
//! `core/context_fold.rs`; this is the tool surface and its wiring to the
//! session.
//! One tool with an `action` rather than four: every tool's schema is sent on
//! every request, and the four share all their concepts.

use std::sync::Arc;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_tui::tui::ComponentRef;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::tools::render_utils::{call_title, get_text_output};
use crate::core::tools::tool_definition::{
    ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
    render_text_call, render_text_result, wrap_tool_definition,
};
use crate::modes::interactive::theme::theme::Theme;

const DESCRIPTION: &str = "Manage your own context: fold finished work into a summary, restore it, or search inside folds.

Long tasks fill the context with tool output you no longer need verbatim: a search whose result you have used, a test run you have read, a dead end. Folding replaces such a stretch with a summary you write, which keeps the context small and the session going much longer. The original stays in the session; nothing is deleted.

Actions:
- status: shows how full the context is, the steps you can fold (each with an id, its size and a preview), where the next fold starts, and the folds in force.
- fold: replaces the steps from `from` to `to` (step or fold ids from status) with `summary`. `from` must be the oldest unfolded step, or the fold right before it to extend that fold. The user's messages inside the range are kept as they are.
- unfold: restores the steps behind `fold_id` when you need their exact content again.
- search: finds `query` inside the folded steps, so you can decide whether to unfold.

Guidelines:
- Folds go in order and should be few and large: each fold re-sends everything after its start without the prompt cache, so one fold over a long finished stretch is far cheaper than many small ones. Fold when a reminder asks, or when a large stretch of finished work is behind you, not after every step.
- Write the summary for yourself later: decisions and their reasons, file paths, exact identifiers, commands and their outcome, errors and how they were resolved, what is still open. Leave out narration.
- Do not fold work you are still in the middle of, or output you will need to quote exactly; the newest step can never be folded.";

/// What the model asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextFoldAction {
    Status,
    Fold {
        from: String,
        to: String,
        summary: String,
    },
    Unfold {
        fold_id: String,
    },
    Search {
        query: String,
    },
}

/// Carries out an action against the session; the text is the tool result.
pub type ContextFoldHandler =
    Arc<dyn Fn(ContextFoldAction) -> Result<String, String> + Send + Sync>;

/// Present only for the main session. A subagent's history is not a session
/// the user can come back to, and it ends with its answer anyway.
#[derive(Clone)]
pub struct ContextFoldToolSources {
    pub handler: ContextFoldHandler,
}

pub struct ContextFoldToolDefinition {
    sources: Option<ContextFoldToolSources>,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_context_fold_tool_definition(
    sources: Option<ContextFoldToolSources>,
) -> ContextFoldToolDefinition {
    ContextFoldToolDefinition {
        sources,
        parameters: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["status", "fold", "unfold", "search"],
                    "description": "What to do.",
                },
                "from": { "type": "string", "description": "fold: id of the oldest step or fold to include." },
                "to": { "type": "string", "description": "fold: id of the newest step or fold to include." },
                "summary": { "type": "string", "description": "fold: what replaces the range; everything later steps need from it." },
                "fold_id": { "type": "string", "description": "unfold: id of the fold to restore." },
                "query": { "type": "string", "description": "search: text to look for inside folds, case-insensitive." },
            },
            "required": ["action"],
        }),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

fn string_param(params: &Value, key: &str) -> String {
    params
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// The action a call names, or why it cannot be carried out.
pub fn parse_action(params: &Value) -> Result<ContextFoldAction, String> {
    let required = |key: &str| {
        let value = string_param(params, key);
        if value.is_empty() {
            Err(format!("{key} is required for this action"))
        } else {
            Ok(value)
        }
    };
    match string_param(params, "action").as_str() {
        "status" => Ok(ContextFoldAction::Status),
        "fold" => Ok(ContextFoldAction::Fold {
            from: required("from")?,
            to: required("to")?,
            summary: required("summary")?,
        }),
        "unfold" => Ok(ContextFoldAction::Unfold {
            fold_id: required("fold_id")?,
        }),
        "search" => Ok(ContextFoldAction::Search {
            query: required("query")?,
        }),
        other => Err(format!(
            "unknown action `{other}`; use status, fold, unfold or search"
        )),
    }
}

impl ToolDefinition for ContextFoldToolDefinition {
    fn name(&self) -> &str {
        "context_fold"
    }

    fn label(&self) -> &str {
        "context fold"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("Fold finished work into your own summaries to keep the context small")
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        vec![
            "When a context reminder arrives, or a long stretch of finished tool work is behind you, fold it with context_fold in one piece, starting at the oldest unfolded step, with a summary that keeps every detail you still need. Do not fold after every step: each fold costs prompt cache.".to_owned(),
        ]
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
        let action = string_param(args, "action");
        let detail = match action.as_str() {
            "fold" => format!(
                "fold {} .. {}",
                string_param(args, "from"),
                string_param(args, "to")
            ),
            "unfold" => format!("unfold {}", string_param(args, "fold_id")),
            "search" => format!("search {}", string_param(args, "query")),
            other => other.to_owned(),
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
            let Some(sources) = self.sources.as_ref() else {
                return Err(ToolExecutionError::new(
                    "context_fold is only available in the main session",
                ));
            };
            let action = parse_action(&params).map_err(ToolExecutionError::new)?;
            let output = (sources.handler)(action).map_err(ToolExecutionError::new)?;
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

pub fn create_context_fold_tool(sources: Option<ContextFoldToolSources>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_context_fold_tool_definition(sources)), None)
}
