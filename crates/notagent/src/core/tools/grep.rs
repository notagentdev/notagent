//! Port of `packages/coding-agent/src/core/tools/grep.ts` (tool half).

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{TextContent, TextOrImageContent};
use notagent_tui::tui::ComponentRef;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::tools::path_utils::resolve_to_cwd;
use crate::core::tools::render_utils::{get_text_output, invalid_arg_text, shorten_path, str_arg};
use crate::core::tools::tool_definition::{
    SystemPromptContribution, ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult,
    ToolRenderResultOptions, display_arg, render_text_call, render_text_result,
    wrap_tool_definition,
};
use crate::core::tools::truncate::{
    DEFAULT_MAX_BYTES, GREP_MAX_LINE_LENGTH, TruncationOptions, format_size, truncate_head,
    truncate_line, truncation_from_details,
};
use crate::modes::interactive::components::keybinding_hints::key_hint;
use crate::modes::interactive::theme::theme::{Theme, ThemeColor};
use crate::utils::tools_manager::{ManagedTool, ensure_tool};

pub const GREP_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Search file contents for patterns (respects .gitignore)",
        guidelines: &[],
    };

const DEFAULT_LIMIT: usize = 100;

fn grep_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string", "description": "Search pattern (regex or literal string)" },
            "path": { "type": "string", "description": "Directory or file to search (default: current directory)" },
            "glob": { "type": "string", "description": "Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'" },
            "ignoreCase": { "type": "boolean", "description": "Case-insensitive search (default: false)" },
            "literal": { "type": "boolean", "description": "Treat pattern as literal string instead of regex (default: false)" },
            "context": { "type": "number", "description": "Number of lines to show before and after each match (default: 0)" },
            "limit": { "type": "number", "description": "Maximum number of matches to return (default: 100)" },
        },
        "required": ["pattern"],
    })
}

/// Pluggable operations, so the search can be delegated to a remote system.
pub trait GrepOperations: Send + Sync {
    /// Errors when the path does not exist.
    fn is_directory<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<bool, String>>;
    /// File contents, used for context lines.
    fn read_file<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<String, String>>;
}

pub struct LocalGrepOperations;

impl GrepOperations for LocalGrepOperations {
    fn is_directory<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            tokio::fs::metadata(absolute_path)
                .await
                .map(|metadata| metadata.is_dir())
                .map_err(|error| error.to_string())
        })
    }

    fn read_file<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            tokio::fs::read_to_string(absolute_path)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

#[derive(Clone, Default)]
pub struct GrepToolOptions {
    pub operations: Option<Arc<dyn GrepOperations>>,
}

// ============================================================================
// Rendering
// ============================================================================

fn format_grep_call(args: &Value, theme: &Theme) -> String {
    let pattern = str_arg(args.get("pattern"));
    let raw_path = str_arg(args.get("path"));
    // `rawPath !== null ? shortenPath(rawPath || ".") : null`
    let path = raw_path
        .map(|raw_path| shorten_path(Some(if raw_path.is_empty() { "." } else { &raw_path })));
    let limit = args.get("limit");
    let invalid_arg = invalid_arg_text(theme);
    let mut text = theme.fg(ThemeColor::ToolTitle, &theme.bold("grep"))
        + " "
        + &match &pattern {
            None => invalid_arg.clone(),
            Some(pattern) => theme.fg(ThemeColor::Accent, &format!("/{pattern}/")),
        }
        + &theme.fg(
            ThemeColor::ToolOutput,
            &format!(
                " in {}",
                match &path {
                    None => invalid_arg.as_str(),
                    Some(path) => path.as_str(),
                }
            ),
        );
    // `if (glob)` — an empty or non-string glob adds nothing.
    if let Some(glob) = str_arg(args.get("glob")).filter(|glob| !glob.is_empty()) {
        text += &theme.fg(ThemeColor::ToolOutput, &format!(" ({glob})"));
    }
    if let Some(limit) = limit {
        text += &theme.fg(
            ThemeColor::ToolOutput,
            &format!(" limit {}", display_arg(limit)),
        );
    }
    text
}

/// Preview rows of the result before it is expanded.
const GREP_PREVIEW_LINES: usize = 15;

fn format_grep_result(
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
            GREP_PREVIEW_LINES
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

    let match_limit = result
        .details
        .and_then(|details| details.get("matchLimitReached"))
        .filter(|value| !value.is_null());
    let truncation = truncation_from_details(result.details);
    let lines_truncated = result
        .details
        .and_then(|details| details.get("linesTruncated"))
        .is_some_and(|value| value == &json!(true));
    let truncated = truncation
        .as_ref()
        .is_some_and(|truncation| truncation.truncated);
    if match_limit.is_some() || truncated || lines_truncated {
        let mut warnings: Vec<String> = Vec::new();
        if let Some(match_limit) = match_limit {
            warnings.push(format!("{} matches limit", display_arg(match_limit)));
        }
        if let Some(truncation) = truncation.filter(|truncation| truncation.truncated) {
            warnings.push(format!("{} limit", format_size(truncation.max_bytes)));
        }
        if lines_truncated {
            warnings.push("some lines truncated".to_string());
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

pub struct GrepToolDefinition {
    cwd: String,
    operations: Arc<dyn GrepOperations>,
    description: String,
    parameters: Value,
}

pub fn create_grep_tool_definition(
    cwd: &str,
    options: Option<GrepToolOptions>,
) -> GrepToolDefinition {
    let options = options.unwrap_or_default();
    GrepToolDefinition {
        cwd: cwd.to_owned(),
        operations: options
            .operations
            .unwrap_or_else(|| Arc::new(LocalGrepOperations)),
        description: format!(
            "Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to {DEFAULT_LIMIT} matches or {}KB (whichever is hit first). Long lines are truncated to {GREP_MAX_LINE_LENGTH} chars.",
            DEFAULT_MAX_BYTES / 1024
        ),
        parameters: grep_schema(),
    }
}

struct GrepMatch {
    file_path: String,
    line_number: usize,
    line_text: Option<String>,
}

impl ToolDefinition for GrepToolDefinition {
    fn name(&self) -> &str {
        "grep"
    }

    fn label(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(GREP_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
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
        Some(render_text_call(context, &format_grep_call(args, theme)))
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
            &format_grep_result(result, options, theme, context.show_images),
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
            use tokio::io::AsyncBufReadExt;

            let aborted = || matches!(&signal, Some(signal) if signal.is_cancelled());
            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }

            let Some(rg_path) = ensure_tool(ManagedTool::Rg, true).await else {
                return Err(ToolExecutionError::new(
                    "ripgrep (rg) is not available and could not be downloaded",
                ));
            };

            let pattern = params
                .get("pattern")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let search_dir = params
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let glob = params
                .get("glob")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let ignore_case = params
                .get("ignoreCase")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let literal = params
                .get("literal")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let context_value = params
                .get("context")
                .and_then(Value::as_f64)
                .filter(|context| context.is_finite() && *context > 0.0)
                .map(|context| context as usize)
                .unwrap_or(0);
            let effective_limit = params
                .get("limit")
                .and_then(Value::as_f64)
                .filter(|limit| limit.is_finite())
                .map(|limit| limit as usize)
                .unwrap_or(DEFAULT_LIMIT)
                .max(1);

            let search_path = resolve_to_cwd(
                if search_dir.is_empty() {
                    "."
                } else {
                    search_dir
                },
                &self.cwd,
            );
            let Ok(is_directory) = self.operations.is_directory(&search_path).await else {
                return Err(ToolExecutionError::new(format!(
                    "Path not found: {search_path}"
                )));
            };

            let format_path = |file_path: &str| -> String {
                if is_directory {
                    let relative = Path::new(file_path)
                        .strip_prefix(&search_path)
                        .ok()
                        .map(|relative| relative.to_string_lossy().into_owned());
                    if let Some(relative) = relative.filter(|relative| !relative.is_empty()) {
                        return relative.replace('\\', "/");
                    }
                }
                Path::new(file_path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| file_path.to_owned())
            };

            let mut args: Vec<String> = ["--json", "--line-number", "--color=never", "--hidden"]
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect();
            if ignore_case {
                args.push("--ignore-case".to_owned());
            }
            if literal {
                args.push("--fixed-strings".to_owned());
            }
            if let Some(glob) = &glob {
                args.push("--glob".to_owned());
                args.push(glob.clone());
            }
            args.push("--".to_owned());
            args.push(pattern);
            args.push(search_path.clone());

            let mut child = tokio::process::Command::new(&rg_path)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| {
                    ToolExecutionError::new(format!("Failed to run ripgrep: {error}"))
                })?;

            let stdout = child.stdout.take().expect("piped stdout");
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            let mut matches: Vec<GrepMatch> = Vec::new();
            let mut match_count = 0usize;
            let mut match_limit_reached = false;
            let mut killed_due_to_limit = false;
            let mut lines_truncated = false;

            loop {
                let next = match &signal {
                    Some(signal) => tokio::select! {
                        line = lines.next_line() => line,
                        () = signal.cancelled() => {
                            let _ = child.kill().await;
                            return Err(ToolExecutionError::new("Operation aborted"));
                        }
                    },
                    None => lines.next_line().await,
                };
                let Ok(Some(line)) = next else { break };
                if line.trim().is_empty() || match_count >= effective_limit {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if event.get("type").and_then(Value::as_str) != Some("match") {
                    continue;
                }
                match_count += 1;
                let file_path = event["data"]["path"]["text"].as_str().map(str::to_owned);
                let line_number = event["data"]["line_number"]
                    .as_u64()
                    .map(|number| number as usize);
                let line_text = event["data"]["lines"]["text"].as_str().map(str::to_owned);
                if let (Some(file_path), Some(line_number)) = (file_path, line_number) {
                    matches.push(GrepMatch {
                        file_path,
                        line_number,
                        line_text,
                    });
                }
                if match_count >= effective_limit {
                    match_limit_reached = true;
                    killed_due_to_limit = true;
                    let _ = child.kill().await;
                    break;
                }
            }

            let output = child.wait_with_output().await.map_err(|error| {
                ToolExecutionError::new(format!("Failed to run ripgrep: {error}"))
            })?;
            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }
            let exit_code = output.status.code();
            if !killed_due_to_limit && exit_code != Some(0) && exit_code != Some(1) {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                let message = if stderr.is_empty() {
                    match exit_code {
                        Some(code) => format!("ripgrep exited with code {code}"),
                        None => "ripgrep exited with code null".to_owned(),
                    }
                } else {
                    stderr
                };
                return Err(ToolExecutionError::new(message));
            }
            if match_count == 0 {
                return Ok(AgentToolResult {
                    content: vec![TextOrImageContent::Text(TextContent::new(
                        "No matches found",
                    ))],
                    details: None,
                    usage: None,
                    added_tool_names: None,
                    terminate: None,
                });
            }

            // Formatting runs after streaming so custom read_file backends can be async.
            let mut file_cache: HashMap<String, Vec<String>> = HashMap::new();
            let mut output_lines: Vec<String> = Vec::new();
            for grep_match in &matches {
                if context_value == 0
                    && let Some(line_text) = &grep_match.line_text
                {
                    let relative_path = format_path(&grep_match.file_path);
                    let sanitized = line_text.replace("\r\n", "\n").replace('\r', "");
                    let sanitized = sanitized.strip_suffix('\n').unwrap_or(&sanitized);
                    let truncated = truncate_line(sanitized, GREP_MAX_LINE_LENGTH);
                    if truncated.was_truncated {
                        lines_truncated = true;
                    }
                    output_lines.push(format!(
                        "{relative_path}:{}: {}",
                        grep_match.line_number, truncated.text
                    ));
                    continue;
                }

                let relative_path = format_path(&grep_match.file_path);
                if !file_cache.contains_key(&grep_match.file_path) {
                    let lines = match self.operations.read_file(&grep_match.file_path).await {
                        Ok(content) => content
                            .replace("\r\n", "\n")
                            .replace('\r', "\n")
                            .split('\n')
                            .map(str::to_owned)
                            .collect(),
                        Err(_) => Vec::new(),
                    };
                    file_cache.insert(grep_match.file_path.clone(), lines);
                }
                let lines = &file_cache[&grep_match.file_path];
                if lines.is_empty() {
                    output_lines.push(format!(
                        "{relative_path}:{}: (unable to read file)",
                        grep_match.line_number
                    ));
                    continue;
                }
                let start = if context_value > 0 {
                    grep_match.line_number.saturating_sub(context_value).max(1)
                } else {
                    grep_match.line_number
                };
                let end = if context_value > 0 {
                    (grep_match.line_number + context_value).min(lines.len())
                } else {
                    grep_match.line_number
                };
                for current in start..=end {
                    let line_text = lines.get(current - 1).cloned().unwrap_or_default();
                    let sanitized = line_text.replace('\r', "");
                    let truncated = truncate_line(&sanitized, GREP_MAX_LINE_LENGTH);
                    if truncated.was_truncated {
                        lines_truncated = true;
                    }
                    if current == grep_match.line_number {
                        output_lines.push(format!("{relative_path}:{current}: {}", truncated.text));
                    } else {
                        output_lines.push(format!("{relative_path}-{current}- {}", truncated.text));
                    }
                }
            }

            let raw_output = output_lines.join("\n");
            // Only the byte limit applies; the match limit already capped the rows.
            let truncation = truncate_head(
                &raw_output,
                TruncationOptions {
                    max_lines: Some(usize::MAX),
                    max_bytes: None,
                },
            );
            let mut output_text = truncation.content.clone();
            let mut details = Map::new();
            let mut notices: Vec<String> = Vec::new();
            if match_limit_reached {
                notices.push(format!(
                    "{effective_limit} matches limit reached. Use limit={} for more, or refine pattern",
                    effective_limit * 2
                ));
                details.insert("matchLimitReached".to_owned(), json!(effective_limit));
            }
            if truncation.truncated {
                notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
                details.insert(
                    "truncation".to_owned(),
                    serde_json::to_value(&truncation).expect("truncation result"),
                );
            }
            if lines_truncated {
                notices.push(format!(
                    "Some lines truncated to {GREP_MAX_LINE_LENGTH} chars. Use read tool to see full lines"
                ));
                details.insert("linesTruncated".to_owned(), json!(true));
            }
            if !notices.is_empty() {
                output_text.push_str(&format!("\n\n[{}]", notices.join(". ")));
            }

            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(output_text))],
                details: (!details.is_empty()).then(|| Value::Object(details)),
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

/// `createCreateGrepTool` — the tool as the agent loop takes it.
pub fn create_grep_tool(cwd: &str, options: Option<GrepToolOptions>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_grep_tool_definition(cwd, options)), None)
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
                .prefix("notagent-grep-tool-")
                .tempdir()
                .expect("temp dir");
            let path = directory.path().to_path_buf();
            let _ = directory.keep();
            Self { path }
        }

        fn cwd(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }

        fn write(&self, name: &str, contents: &str) {
            std::fs::write(self.path.join(name), contents).expect("write");
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

    fn ripgrep_available() -> bool {
        crate::utils::tools_manager::get_tool_path(ManagedTool::Rg).is_some()
    }

    #[tokio::test]
    async fn finds_matches_with_paths_and_line_numbers() {
        if !ripgrep_available() {
            return;
        }
        let directory = TempDir::new();
        directory.write("a.txt", "alpha\nbeta\ngamma\n");
        directory.write("b.txt", "delta\nbeta\n");
        let tool = create_grep_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute("call-1", json!({ "pattern": "beta" }), None, None, None)
            .await
            .expect("grep");
        let text = text_of(&result);
        let mut lines: Vec<&str> = text.lines().collect();
        lines.sort_unstable();
        assert_eq!(lines, vec!["a.txt:2: beta", "b.txt:2: beta"]);
        assert_eq!(result.details, None);
    }

    #[tokio::test]
    async fn reports_no_matches() {
        if !ripgrep_available() {
            return;
        }
        let directory = TempDir::new();
        directory.write("a.txt", "alpha\n");
        let tool = create_grep_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute("call-1", json!({ "pattern": "zzz" }), None, None, None)
            .await
            .expect("grep");
        assert_eq!(text_of(&result), "No matches found");
    }

    #[tokio::test]
    async fn shows_context_lines_around_a_match() {
        if !ripgrep_available() {
            return;
        }
        let directory = TempDir::new();
        directory.write("a.txt", "one\ntwo\nthree\nfour\n");
        let tool = create_grep_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute(
                "call-1",
                json!({ "pattern": "three", "context": 1 }),
                None,
                None,
                None,
            )
            .await
            .expect("grep");
        assert_eq!(
            text_of(&result),
            "a.txt-2- two\na.txt:3: three\na.txt-4- four"
        );
    }

    #[tokio::test]
    async fn honours_the_match_limit() {
        if !ripgrep_available() {
            return;
        }
        let directory = TempDir::new();
        directory.write("a.txt", "hit\nhit\nhit\nhit\n");
        let tool = create_grep_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute(
                "call-1",
                json!({ "pattern": "hit", "limit": 2 }),
                None,
                None,
                None,
            )
            .await
            .expect("grep");
        let text = text_of(&result);
        assert!(
            text.contains("[2 matches limit reached. Use limit=4 for more, or refine pattern]"),
            "{text}"
        );
        assert_eq!(
            result.details.expect("details")["matchLimitReached"],
            json!(2)
        );
    }

    #[tokio::test]
    async fn truncates_long_lines_and_says_so() {
        if !ripgrep_available() {
            return;
        }
        let directory = TempDir::new();
        directory.write("a.txt", &format!("needle{}\n", "x".repeat(600)));
        let tool = create_grep_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute("call-1", json!({ "pattern": "needle" }), None, None, None)
            .await
            .expect("grep");
        let text = text_of(&result);
        assert!(text.contains("... [truncated]"), "{text}");
        assert!(
            text.contains("Some lines truncated to 500 chars. Use read tool to see full lines")
        );
        assert_eq!(
            result.details.expect("details")["linesTruncated"],
            json!(true)
        );
    }

    #[tokio::test]
    async fn filters_by_glob_and_matches_case_insensitively() {
        if !ripgrep_available() {
            return;
        }
        let directory = TempDir::new();
        directory.write("a.txt", "Needle\n");
        directory.write("b.md", "needle\n");
        let tool = create_grep_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute(
                "call-1",
                json!({ "pattern": "needle", "glob": "*.md", "ignoreCase": true }),
                None,
                None,
                None,
            )
            .await
            .expect("grep");
        assert_eq!(text_of(&result), "b.md:1: needle");
    }

    #[tokio::test]
    async fn fails_for_a_missing_path() {
        if !ripgrep_available() {
            return;
        }
        let directory = TempDir::new();
        let tool = create_grep_tool_definition(&directory.cwd(), None);
        let error = tool
            .execute(
                "call-1",
                json!({ "pattern": "x", "path": "missing" }),
                None,
                None,
                None,
            )
            .await
            .expect_err("missing");
        assert_eq!(
            error.message,
            format!("Path not found: {}/missing", directory.cwd())
        );
    }

    #[test]
    fn advertises_its_description_and_schema() {
        let tool = create_grep_tool_definition("/tmp", None);
        assert_eq!(tool.name(), "grep");
        assert!(
            tool.description()
                .contains("truncated to 100 matches or 50KB")
        );
        assert!(
            tool.description()
                .contains("Long lines are truncated to 500 chars.")
        );
        assert_eq!(tool.parameters()["required"], json!(["pattern"]));
    }
}
