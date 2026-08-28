use std::sync::Arc;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{TextContent, TextOrImageContent};
use notagent_tui::tui::ComponentRef;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::tools::path_utils::resolve_to_cwd;
use crate::core::tools::render_utils::{call_title, get_text_output, render_tool_path, str_arg};
use crate::core::tools::tool_definition::{
    SystemPromptContribution, ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult,
    ToolRenderResultOptions, display_arg, render_text_call, render_text_result,
    wrap_tool_definition,
};
use crate::core::tools::truncate::{
    DEFAULT_MAX_BYTES, TruncationOptions, format_size, truncate_head, truncation_from_details,
};
use crate::modes::interactive::components::keybinding_hints::key_hint;
use crate::modes::interactive::theme::theme::{Theme, ThemeColor};

pub const LS_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution = SystemPromptContribution {
    snippet: "List directory contents",
    guidelines: &[],
};

const DEFAULT_LIMIT: usize = 500;

fn ls_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Directory to list (default: current directory)" },
            "limit": { "type": "number", "description": "Maximum number of entries to return (default: 500)" },
        },
    })
}

/// Pluggable operations, so listing can be delegated to a remote system.
pub trait LsOperations: Send + Sync {
    fn exists<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, bool>;
    fn is_directory<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<bool, String>>;
    fn read_dir<'a>(&'a self, absolute_path: &'a str)
    -> BoxFuture<'a, Result<Vec<String>, String>>;
}

pub struct LocalLsOperations;

impl LsOperations for LocalLsOperations {
    fn exists<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move { tokio::fs::metadata(absolute_path).await.is_ok() })
    }

    fn is_directory<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            tokio::fs::metadata(absolute_path)
                .await
                .map(|metadata| metadata.is_dir())
                .map_err(|error| error.to_string())
        })
    }

    fn read_dir<'a>(
        &'a self,
        absolute_path: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, String>> {
        Box::pin(async move {
            let mut entries = tokio::fs::read_dir(absolute_path)
                .await
                .map_err(|error| error.to_string())?;
            let mut names: Vec<String> = Vec::new();
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| error.to_string())?
            {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
            Ok(names)
        })
    }
}

#[derive(Clone, Default)]
pub struct LsToolOptions {
    pub operations: Option<Arc<dyn LsOperations>>,
}

// ============================================================================
// Rendering
// ============================================================================

fn format_ls_call(args: &Value, theme: &Theme, cwd: &str) -> String {
    let path_display =
        render_tool_path(str_arg(args.get("path")).as_deref(), theme, cwd, Some("."));
    let mut text = format!("{}{path_display}", call_title(theme, "ls"));
    if let Some(limit) = args.get("limit") {
        text += &theme.fg(
            ThemeColor::ToolOutput,
            &format!(" (limit {})", display_arg(limit)),
        );
    }
    text
}

/// Preview rows of the result before it is expanded.
const LS_PREVIEW_LINES: usize = 20;

fn format_ls_result(
    result: ToolRenderResult<'_>,
    options: ToolRenderResultOptions,
    theme: &Theme,
    show_images: bool,
) -> String {
    let output = get_text_output(Some(result.content), show_images)
        .trim()
        .to_string();
    let mut text = String::new();
    if !output.is_empty() {
        let lines: Vec<&str> = output.split('\n').collect();
        let max_lines = if options.expanded {
            lines.len()
        } else {
            LS_PREVIEW_LINES
        };
        let display_lines = &lines[..max_lines.min(lines.len())];
        let remaining = lines.len() as isize - max_lines as isize;
        text += &format!(
            "\n{}",
            display_lines
                .iter()
                .map(|line| theme.fg(ThemeColor::ToolOutput, line))
                .collect::<Vec<_>>()
                .join("\n")
        );
        if remaining > 0 {
            text += &format!(
                "{} {}{}",
                theme.fg(
                    ThemeColor::Muted,
                    &format!("\n... ({remaining} more lines,")
                ),
                key_hint("app.tools.expand", "to expand"),
                theme.fg(ThemeColor::Muted, ")")
            );
        }
    }

    let entry_limit = result
        .details
        .and_then(|details| details.get("entryLimitReached"))
        .filter(|value| !value.is_null());
    let truncation = truncation_from_details(result.details);
    let truncated = truncation
        .as_ref()
        .is_some_and(|truncation| truncation.truncated);
    if entry_limit.is_some() || truncated {
        let mut warnings: Vec<String> = Vec::new();
        if let Some(entry_limit) = entry_limit {
            warnings.push(format!("{} entries limit", display_arg(entry_limit)));
        }
        if let Some(truncation) = truncation.filter(|truncation| truncation.truncated) {
            warnings.push(format!("{} limit", format_size(truncation.max_bytes)));
        }
        text += &format!(
            "\n{}",
            theme.fg(
                ThemeColor::Warning,
                &format!("[Truncated: {}]", warnings.join(", "))
            )
        );
    }
    text
}

pub struct LsToolDefinition {
    cwd: String,
    operations: Arc<dyn LsOperations>,
    description: String,
    parameters: Value,
}

pub fn create_ls_tool_definition(cwd: &str, options: Option<LsToolOptions>) -> LsToolDefinition {
    let options = options.unwrap_or_default();
    LsToolDefinition {
        cwd: cwd.to_owned(),
        operations: options
            .operations
            .unwrap_or_else(|| Arc::new(LocalLsOperations)),
        description: format!(
            "List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. Includes dotfiles. Output is truncated to {DEFAULT_LIMIT} entries or {}KB (whichever is hit first).",
            DEFAULT_MAX_BYTES / 1024
        ),
        parameters: ls_schema(),
    }
}

impl ToolDefinition for LsToolDefinition {
    fn name(&self) -> &str {
        "ls"
    }

    fn label(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(LS_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        Some(render_text_call(
            context,
            &format_ls_call(args, theme, &context.cwd),
        ))
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        Some(render_text_result(
            context,
            &format_ls_result(result, options, theme, context.show_images),
        ))
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let aborted = || matches!(&signal, Some(signal) if signal.is_cancelled());
            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }

            let path = params
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let limit = params
                .get("limit")
                .and_then(Value::as_f64)
                .filter(|limit| limit.is_finite() && *limit >= 0.0)
                .map(|limit| limit as usize);
            let directory_path =
                resolve_to_cwd(if path.is_empty() { "." } else { path }, &self.cwd);
            let effective_limit = limit.unwrap_or(DEFAULT_LIMIT);

            if !self.operations.exists(&directory_path).await {
                return Err(ToolExecutionError::new(format!(
                    "Path not found: {directory_path}"
                )));
            }
            let is_directory = self
                .operations
                .is_directory(&directory_path)
                .await
                .map_err(ToolExecutionError::new)?;
            if !is_directory {
                return Err(ToolExecutionError::new(format!(
                    "Not a directory: {directory_path}"
                )));
            }

            let mut entries = self
                .operations
                .read_dir(&directory_path)
                .await
                .map_err(|error| {
                    ToolExecutionError::new(format!("Cannot read directory: {error}"))
                })?;

            // name; Rust compares the lowercased name by code point, which differs
            // only for locale-specific accent ordering.
            entries.sort_by_key(|entry| entry.to_lowercase());

            let mut results: Vec<String> = Vec::new();
            let mut entry_limit_reached = false;
            for entry in entries {
                if results.len() >= effective_limit {
                    entry_limit_reached = true;
                    break;
                }
                let full_path = std::path::Path::new(&directory_path)
                    .join(&entry)
                    .to_string_lossy()
                    .into_owned();
                // Entries that cannot be stat'ed are skipped.
                let Ok(is_directory) = self.operations.is_directory(&full_path).await else {
                    continue;
                };
                results.push(if is_directory {
                    format!("{entry}/")
                } else {
                    entry
                });
            }

            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }
            if results.is_empty() {
                return Ok(AgentToolResult {
                    content: vec![TextOrImageContent::Text(TextContent::new(
                        "(empty directory)",
                    ))],
                    details: None,
                    usage: None,
                    added_tool_names: None,
                    terminate: None,
                });
            }

            let raw_output = results.join("\n");
            // Only the byte limit applies; the entry count is already capped.
            let truncation = truncate_head(
                &raw_output,
                TruncationOptions {
                    max_lines: Some(usize::MAX),
                    max_bytes: None,
                },
            );
            let mut output = truncation.content.clone();
            let mut details = Map::new();
            let mut notices: Vec<String> = Vec::new();
            if entry_limit_reached {
                notices.push(format!(
                    "{effective_limit} entries limit reached. Use limit={} for more",
                    effective_limit * 2
                ));
                details.insert("entryLimitReached".to_owned(), json!(effective_limit));
            }
            if truncation.truncated {
                notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
                details.insert(
                    "truncation".to_owned(),
                    serde_json::to_value(&truncation).expect("truncation result"),
                );
            }
            if !notices.is_empty() {
                output.push_str(&format!("\n\n[{}]", notices.join(". ")));
            }

            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(output))],
                details: (!details.is_empty()).then(|| Value::Object(details)),
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

/// `createCreateLsTool` — the tool as the agent loop takes it.
pub fn create_ls_tool(cwd: &str, options: Option<LsToolOptions>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_ls_tool_definition(cwd, options)), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let directory = tempfile::Builder::new()
                .prefix("notagent-ls-tool-")
                .tempdir()
                .expect("temp dir");
            let path = directory.path().to_path_buf();
            let _ = directory.keep();
            Self { path }
        }

        fn cwd(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn text_of(result: &AgentToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|block| match block {
                TextOrImageContent::Text(text) => Some(text.text.clone()),
                TextOrImageContent::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn lists_entries_sorted_with_a_slash_for_directories() {
        let directory = TempDir::new();
        std::fs::create_dir(directory.path.join("Beta")).expect("dir");
        std::fs::write(directory.path.join("alpha.txt"), "a").expect("file");
        std::fs::write(directory.path.join(".hidden"), "h").expect("file");

        let tool = create_ls_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute("call-1", json!({}), None, None, None)
            .await
            .expect("ls");
        assert_eq!(text_of(&result), ".hidden\nalpha.txt\nBeta/");
        assert_eq!(result.details, None);
    }

    #[tokio::test]
    async fn reports_an_empty_directory() {
        let directory = TempDir::new();
        let tool = create_ls_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute("call-1", json!({}), None, None, None)
            .await
            .expect("ls");
        assert_eq!(text_of(&result), "(empty directory)");
    }

    #[tokio::test]
    async fn fails_for_a_missing_path_and_for_a_file() {
        let directory = TempDir::new();
        let file = directory.path.join("file.txt");
        std::fs::write(&file, "x").expect("file");
        let tool = create_ls_tool_definition(&directory.cwd(), None);

        let error = tool
            .execute("call-1", json!({ "path": "missing" }), None, None, None)
            .await
            .expect_err("missing");
        assert_eq!(
            error.message,
            format!("Path not found: {}/missing", directory.cwd())
        );

        let error = tool
            .execute("call-1", json!({ "path": "file.txt" }), None, None, None)
            .await
            .expect_err("not a directory");
        assert_eq!(
            error.message,
            format!("Not a directory: {}", file.to_string_lossy())
        );
    }

    #[tokio::test]
    async fn caps_the_entry_count_and_reports_it() {
        let directory = TempDir::new();
        for index in 0..5 {
            std::fs::write(directory.path.join(format!("file-{index}.txt")), "x").expect("file");
        }
        let tool = create_ls_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute("call-1", json!({ "limit": 2 }), None, None, None)
            .await
            .expect("ls");
        let text = text_of(&result);
        assert_eq!(
            text,
            "file-0.txt\nfile-1.txt\n\n[2 entries limit reached. Use limit=4 for more]"
        );
        assert_eq!(
            result.details.expect("details")["entryLimitReached"],
            json!(2)
        );
    }

    #[tokio::test]
    async fn truncates_at_the_byte_limit() {
        let directory = TempDir::new();
        // Every name is 200 bytes, so 50 KiB is exceeded well before 500 entries.
        for index in 0..300 {
            let name = format!("{index:0>200}");
            std::fs::write(directory.path.join(&name), "x").expect("file");
        }
        let tool = create_ls_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute("call-1", json!({}), None, None, None)
            .await
            .expect("ls");
        let text = text_of(&result);
        assert!(
            text.ends_with("[50.0KB limit reached]"),
            "{}",
            &text[text.len() - 40..]
        );
        let details = result.details.expect("details");
        assert_eq!(details["truncation"]["truncated"], json!(true));
        assert_eq!(details["truncation"]["truncatedBy"], json!("bytes"));
    }

    #[tokio::test]
    async fn stops_on_an_aborted_signal() {
        let directory = TempDir::new();
        let tool = create_ls_tool_definition(&directory.cwd(), None);
        let signal = CancellationToken::new();
        signal.cancel();
        let error = tool
            .execute("call-1", json!({}), Some(signal), None, None)
            .await
            .expect_err("aborted");
        assert_eq!(error.message, "Operation aborted");
    }

    #[test]
    fn advertises_its_description_and_schema() {
        let tool = create_ls_tool_definition("/tmp", None);
        assert_eq!(tool.name(), "ls");
        assert_eq!(
            tool.description(),
            "List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. Includes dotfiles. Output is truncated to 500 entries or 50KB (whichever is hit first)."
        );
        assert_eq!(tool.prompt_snippet(), Some("List directory contents"));
        assert!(tool.prompt_guidelines().is_empty());
        assert!(tool.parameters()["properties"].get("limit").is_some());
    }
}
