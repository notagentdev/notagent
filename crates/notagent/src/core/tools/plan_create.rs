use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Local;
use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_tui::tui::ComponentRef;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::tools::path_utils::resolve_to_cwd;
use crate::core::tools::render_utils::{call_title, str_arg};
use crate::core::tools::tool_definition::{
    SystemPromptContribution, ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult,
    ToolRenderResultOptions, render_text_call, render_text_result, wrap_tool_definition,
};
use crate::modes::interactive::theme::theme::{Theme, ThemeColor};

pub const PLAN_CREATE_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Write a plan to the plans directory",
        guidelines: &[
            "Use plan_create when the user requests a plan file or the active planning mode authorizes one. A separate file makes the plan easy to track and revise; plans in saved conversations remain part of the session history. Discussion alone does not authorize creating a file.",
        ],
    };

/// The directory plans are written to, relative to the working directory.
pub const PLANS_DIR_NAME: &str = "plans";

const DESCRIPTION: &str = concat!(
    "Creates a new plan file with the specified name, version, and content. Use this tool to record structured project plans, task breakdowns, or implementation strategies that can be tracked and referenced throughout development sessions.\n",
    "\n",
    "The file is written to `plans/<date>-<plan_name>-<version>.md`; the date is added for you. An existing file is never overwritten — pass a different version to supersede an earlier plan.\n",
    "\n",
    "`plan_name` and `version` name a file, so they may contain only letters, digits, dots, dashes and underscores."
);

fn plan_create_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "plan_name": {
                "type": "string",
                "description": "Name of the plan, used in the filename. Letters, digits, dots, dashes and underscores only.",
            },
            "version": {
                "type": "string",
                "description": "Version of the plan, e.g. \"v1\", \"v2\", \"1.0\". Letters, digits, dots, dashes and underscores only.",
            },
            "content": {
                "type": "string",
                "description": "The complete plan content, in markdown.",
            },
        },
        "required": ["plan_name", "version", "content"],
    })
}

/// Pluggable operations, so writing can be delegated to a remote system — and
/// so the tests do not touch the filesystem.
pub trait PlanCreateOperations: Send + Sync {
    fn file_exists<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, bool>;
    fn write_file<'a>(
        &'a self,
        absolute_path: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn mkdir<'a>(&'a self, directory: &'a str) -> BoxFuture<'a, Result<(), String>>;
}

pub struct LocalPlanCreateOperations;

impl PlanCreateOperations for LocalPlanCreateOperations {
    fn file_exists<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move { tokio::fs::try_exists(absolute_path).await.unwrap_or(false) })
    }

    fn write_file<'a>(
        &'a self,
        absolute_path: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            tokio::fs::write(absolute_path, content)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn mkdir<'a>(&'a self, directory: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            tokio::fs::create_dir_all(directory)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

#[derive(Clone, Default)]
pub struct PlanCreateToolOptions {
    pub operations: Option<Arc<dyn PlanCreateOperations>>,
    /// The plans directory, when it is not `<cwd>/plans`.
    pub plans_dir: Option<String>,
}

pub struct PlanCreateToolDefinition {
    cwd: String,
    plans_dir: Option<String>,
    operations: Arc<dyn PlanCreateOperations>,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_plan_create_tool_definition(
    cwd: &str,
    options: Option<PlanCreateToolOptions>,
) -> PlanCreateToolDefinition {
    let options = options.unwrap_or_default();
    PlanCreateToolDefinition {
        cwd: cwd.to_owned(),
        plans_dir: options.plans_dir,
        operations: options
            .operations
            .unwrap_or_else(|| Arc::new(LocalPlanCreateOperations)),
        parameters: plan_create_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

/// One filename component as the model supplied it.
/// Rejected rather than repaired: the name lands in a path, and a caller that
/// meant `../secrets` should be told no instead of silently getting
/// `..secrets`.
fn sanitize_component(label: &str, value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty."));
    }
    let allowed = trimmed
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_'));
    if !allowed {
        return Err(format!(
            "{label} may contain only letters, digits, dots, dashes and underscores, so that it names a file inside {PLANS_DIR_NAME}/. Got \"{trimmed}\"."
        ));
    }
    // `.` and `..` pass the character check but are not names.
    if trimmed.chars().all(|character| character == '.') {
        return Err(format!("{label} must not consist only of dots."));
    }
    Ok(trimmed.to_owned())
}

/// `<date>-<plan_name>-<version>.md`, the date supplied rather than read so the
/// tests do not depend on today.
fn plan_file_name(date: &str, plan_name: &str, version: &str) -> String {
    format!("{date}-{plan_name}-{version}.md")
}

impl PlanCreateToolDefinition {
    fn plans_dir(&self) -> String {
        match &self.plans_dir {
            Some(dir) => resolve_to_cwd(dir, &self.cwd),
            None => resolve_to_cwd(PLANS_DIR_NAME, &self.cwd),
        }
    }

    /// The path shown in the transcript: relative to the working directory
    /// where it sits below it, absolute otherwise.
    fn display_path(&self, absolute_path: &str) -> String {
        Path::new(absolute_path)
            .strip_prefix(&self.cwd)
            .map_or_else(
                |_| absolute_path.to_owned(),
                |relative| relative.to_string_lossy().into_owned(),
            )
    }
}

impl ToolDefinition for PlanCreateToolDefinition {
    fn name(&self) -> &str {
        "plan_create"
    }

    fn label(&self) -> &str {
        "plan_create"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(PLAN_CREATE_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        PLAN_CREATE_TOOL_SYSTEM_PROMPT_CONTRIBUTION
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

    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let plan_name = str_arg(args.get("plan_name")).unwrap_or_default();
        let version = str_arg(args.get("version")).unwrap_or_default();
        let label = if version.is_empty() {
            plan_name
        } else {
            format!("{plan_name} {version}")
        };
        Some(render_text_call(
            context,
            &format!(
                "{}{}",
                call_title(theme, "plan_create"),
                theme.fg(ThemeColor::Muted, &label)
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
        let path = result
            .details
            .and_then(|details| details.get("path"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if path.is_empty() {
            return Some(render_text_result(context, ""));
        }
        Some(render_text_result(
            context,
            &theme.fg(ThemeColor::Muted, &format!("\nwrote {path}")),
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
            let plan_name = sanitize_component(
                "plan_name",
                params
                    .get("plan_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .map_err(ToolExecutionError::new)?;
            let version = sanitize_component(
                "version",
                params
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .map_err(ToolExecutionError::new)?;
            let content = params
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if content.trim().is_empty() {
                return Err(ToolExecutionError::new(
                    "content must not be empty — pass the complete plan.",
                ));
            }

            let plans_dir = self.plans_dir();
            let date = Local::now().format("%Y-%m-%d").to_string();
            let file_name = plan_file_name(&date, &plan_name, &version);
            let absolute_path = PathBuf::from(&plans_dir)
                .join(&file_name)
                .to_string_lossy()
                .into_owned();

            self.operations.mkdir(&plans_dir).await.map_err(|error| {
                ToolExecutionError::new(format!(
                    "Failed to create plans directory {plans_dir}: {error}"
                ))
            })?;

            // An existing plan is never overwritten: a plan is a record of a
            // decision, and replacing one silently loses the decision it held.
            if self.operations.file_exists(&absolute_path).await {
                return Err(ToolExecutionError::new(format!(
                    "Plan file already exists at {absolute_path}. Use a different plan name or version to avoid conflicts."
                )));
            }

            self.operations
                .write_file(&absolute_path, content)
                .await
                .map_err(|error| {
                    ToolExecutionError::new(format!(
                        "Failed to write plan file {absolute_path}: {error}"
                    ))
                })?;

            let display_path = self.display_path(&absolute_path);
            let mut details = Map::new();
            details.insert("path".to_owned(), Value::String(display_path.clone()));
            details.insert(
                "absolute_path".to_owned(),
                Value::String(absolute_path.clone()),
            );
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(format!(
                    "Created plan at {display_path}"
                )))],
                details: Some(Value::Object(details)),
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

/// The tool as the agent loop takes it.
pub fn create_plan_create_tool(
    cwd: &str,
    options: Option<PlanCreateToolOptions>,
) -> Arc<dyn AgentTool> {
    wrap_tool_definition(
        Arc::new(create_plan_create_tool_definition(cwd, options)),
        None,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct RecordingOperations {
        existing: Vec<String>,
        written: Mutex<Vec<(String, String)>>,
        created_dirs: Mutex<Vec<String>>,
    }

    impl PlanCreateOperations for RecordingOperations {
        fn file_exists<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, bool> {
            Box::pin(async move { self.existing.iter().any(|path| path == absolute_path) })
        }

        fn write_file<'a>(
            &'a self,
            absolute_path: &'a str,
            content: &'a str,
        ) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                match self.written.lock() {
                    Ok(mut written) => {
                        written.push((absolute_path.to_owned(), content.to_owned()));
                        Ok(())
                    }
                    Err(_) => Err("recording mutex poisoned".to_owned()),
                }
            })
        }

        fn mkdir<'a>(&'a self, directory: &'a str) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                match self.created_dirs.lock() {
                    Ok(mut dirs) => {
                        dirs.push(directory.to_owned());
                        Ok(())
                    }
                    Err(_) => Err("recording mutex poisoned".to_owned()),
                }
            })
        }
    }

    fn tool_with(operations: Arc<RecordingOperations>) -> PlanCreateToolDefinition {
        create_plan_create_tool_definition(
            "/workspace",
            Some(PlanCreateToolOptions {
                operations: Some(operations),
                plans_dir: None,
            }),
        )
    }

    fn call(plan_name: &str, version: &str, content: &str) -> Value {
        json!({ "plan_name": plan_name, "version": version, "content": content })
    }

    #[tokio::test]
    async fn writes_a_dated_plan_below_the_plans_directory() {
        let operations = Arc::new(RecordingOperations::default());
        let tool = tool_with(Arc::clone(&operations));

        let result = tool
            .execute("call", call("rollout", "v1", "# Plan"), None, None, None)
            .await
            .expect("plan is written");

        let written = operations.written.lock().expect("recording mutex");
        assert_eq!(written.len(), 1);
        let (path, content) = &written[0];
        let date = Local::now().format("%Y-%m-%d").to_string();
        assert_eq!(path, &format!("/workspace/plans/{date}-rollout-v1.md"));
        assert_eq!(content, "# Plan");
        assert_eq!(
            result
                .details
                .as_ref()
                .and_then(|details| details.get("path"))
                .and_then(Value::as_str),
            Some(format!("plans/{date}-rollout-v1.md").as_str())
        );
    }

    #[tokio::test]
    async fn creates_the_plans_directory() {
        let operations = Arc::new(RecordingOperations::default());
        let tool = tool_with(Arc::clone(&operations));

        tool.execute("call", call("rollout", "v1", "# Plan"), None, None, None)
            .await
            .expect("plan is written");

        let dirs = operations.created_dirs.lock().expect("recording mutex");
        assert_eq!(dirs.as_slice(), ["/workspace/plans".to_owned()]);
    }

    #[tokio::test]
    async fn refuses_to_overwrite_an_existing_plan() {
        let date = Local::now().format("%Y-%m-%d").to_string();
        let operations = Arc::new(RecordingOperations {
            existing: vec![format!("/workspace/plans/{date}-rollout-v1.md")],
            ..RecordingOperations::default()
        });
        let tool = tool_with(Arc::clone(&operations));

        let error = tool
            .execute("call", call("rollout", "v1", "# Plan"), None, None, None)
            .await
            .expect_err("an existing plan is an error");

        assert!(error.to_string().contains("already exists"));
        assert!(
            operations
                .written
                .lock()
                .expect("recording mutex")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn rejects_names_that_would_leave_the_plans_directory() {
        let operations = Arc::new(RecordingOperations::default());
        let tool = tool_with(Arc::clone(&operations));

        for name in ["../escape", "nested/plan", "..", "with space"] {
            let error = tool
                .execute("call", call(name, "v1", "# Plan"), None, None, None)
                .await
                .expect_err("the name is refused");
            assert!(
                error.to_string().contains("plan_name"),
                "expected a plan_name error for {name:?}, got {error}"
            );
        }
        assert!(
            operations
                .written
                .lock()
                .expect("recording mutex")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn rejects_an_empty_version_and_empty_content() {
        let operations = Arc::new(RecordingOperations::default());
        let tool = tool_with(Arc::clone(&operations));

        let error = tool
            .execute("call", call("rollout", "  ", "# Plan"), None, None, None)
            .await
            .expect_err("the version is refused");
        assert!(error.to_string().contains("version"));

        let error = tool
            .execute("call", call("rollout", "v1", "   "), None, None, None)
            .await
            .expect_err("the content is refused");
        assert!(error.to_string().contains("content"));
    }

    #[test]
    fn builds_the_file_name_from_date_name_and_version() {
        assert_eq!(
            plan_file_name("2026-08-20", "rollout", "v2"),
            "2026-08-20-rollout-v2.md"
        );
    }
}
