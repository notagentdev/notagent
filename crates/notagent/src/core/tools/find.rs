//! Port of `packages/coding-agent/src/core/tools/find.ts` (tool half).

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use notagent_agent::types::{
    AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{TextContent, TextOrImageContent};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::tools::path_utils::{path_exists, resolve_to_cwd};
use crate::core::tools::tool_definition::{SystemPromptContribution, ToolContext, ToolDefinition};
use crate::core::tools::truncate::{
    DEFAULT_MAX_BYTES, TruncationOptions, format_size, truncate_head,
};
use crate::utils::tools_manager::{ManagedTool, ensure_tool};

pub const FIND_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Find files by glob pattern (respects .gitignore)",
        guidelines: &[],
    };

const DEFAULT_LIMIT: usize = 1000;

/// Relativize a find result against the search root and normalize to posix separators.
pub fn relativize_find_result_path(result_path: &str, search_path: &str) -> String {
    let separator = if cfg!(windows) { '\\' } else { '/' };
    let had_trailing_separator =
        result_path.ends_with(separator) || (cfg!(windows) && result_path.ends_with('/'));
    let relative_path = if Path::new(result_path).is_absolute() {
        pathdiff(result_path, search_path)
    } else {
        result_path.to_owned()
    };
    let posix_path = relative_path.split(separator).collect::<Vec<_>>().join("/");
    if had_trailing_separator && !posix_path.ends_with('/') {
        format!("{posix_path}/")
    } else {
        posix_path
    }
}

/// `path.relative(from, to)` for the two absolute paths find deals with.
fn pathdiff(result_path: &str, search_path: &str) -> String {
    let separator = if cfg!(windows) { '\\' } else { '/' };
    let from: Vec<&str> = search_path
        .split(separator)
        .filter(|part| !part.is_empty())
        .collect();
    let to: Vec<&str> = result_path
        .split(separator)
        .filter(|part| !part.is_empty())
        .collect();
    let mut common = 0;
    while common < from.len() && common < to.len() && from[common] == to[common] {
        common += 1;
    }
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend_from_slice(&to[common..]);
    parts.join(&separator.to_string())
}

fn find_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string", "description": "Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'" },
            "path": { "type": "string", "description": "Directory to search in (default: current directory)" },
            "limit": { "type": "number", "description": "Maximum number of results (default: 1000)" },
        },
        "required": ["pattern"],
    })
}

/// Pluggable operations, so the search can be delegated to a remote system.
pub trait FindOperations: Send + Sync {
    fn exists<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, bool>;
    /// Files matching the glob; relative or absolute paths.
    fn glob<'a>(
        &'a self,
        pattern: &'a str,
        cwd: &'a str,
        ignore: &'a [&'a str],
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<String>, String>>;
}

#[derive(Default)]
pub struct FindToolOptions {
    pub operations: Option<Arc<dyn FindOperations>>,
}

pub struct FindToolDefinition {
    cwd: String,
    operations: Option<Arc<dyn FindOperations>>,
    description: String,
    parameters: Value,
}

pub fn create_find_tool_definition(
    cwd: &str,
    options: Option<FindToolOptions>,
) -> FindToolDefinition {
    let options = options.unwrap_or_default();
    FindToolDefinition {
        cwd: cwd.to_owned(),
        operations: options.operations,
        description: format!(
            "Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to {DEFAULT_LIMIT} results or {}KB (whichever is hit first).",
            DEFAULT_MAX_BYTES / 1024
        ),
        parameters: find_schema(),
    }
}

/// Build the shared result payload from the relativized paths.
fn build_result(
    relativized: Vec<String>,
    effective_limit: usize,
    limit_notice: String,
) -> AgentToolResult {
    let result_limit_reached = relativized.len() >= effective_limit;
    let raw_output = relativized.join("\n");
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
    if result_limit_reached {
        notices.push(limit_notice);
        details.insert("resultLimitReached".to_owned(), json!(effective_limit));
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
    AgentToolResult {
        content: vec![TextOrImageContent::Text(TextContent::new(output))],
        details: (!details.is_empty()).then(|| Value::Object(details)),
        usage: None,
        added_tool_names: None,
        terminate: None,
    }
}

fn no_files_found() -> AgentToolResult {
    AgentToolResult {
        content: vec![TextOrImageContent::Text(TextContent::new(
            "No files found matching pattern",
        ))],
        details: None,
        usage: None,
        added_tool_names: None,
        terminate: None,
    }
}

impl ToolDefinition for FindToolDefinition {
    fn name(&self) -> &str {
        "find"
    }

    fn label(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(FIND_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn parameters(&self) -> &Value {
        &self.parameters
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

            let pattern = params
                .get("pattern")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let search_dir = params
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let limit = params
                .get("limit")
                .and_then(Value::as_f64)
                .filter(|limit| limit.is_finite() && *limit >= 0.0)
                .map(|limit| limit as usize);
            let search_path = resolve_to_cwd(
                if search_dir.is_empty() {
                    "."
                } else {
                    search_dir
                },
                &self.cwd,
            );
            let effective_limit = limit.unwrap_or(DEFAULT_LIMIT);

            // Custom operations replace fd entirely.
            if let Some(operations) = &self.operations {
                if !operations.exists(&search_path).await {
                    return Err(ToolExecutionError::new(format!(
                        "Path not found: {search_path}"
                    )));
                }
                if aborted() {
                    return Err(ToolExecutionError::new("Operation aborted"));
                }
                let results = operations
                    .glob(
                        &pattern,
                        &search_path,
                        &["**/node_modules/**", "**/.git/**"],
                        effective_limit,
                    )
                    .await
                    .map_err(ToolExecutionError::new)?;
                if aborted() {
                    return Err(ToolExecutionError::new("Operation aborted"));
                }
                if results.is_empty() {
                    return Ok(no_files_found());
                }
                let relativized = results
                    .iter()
                    .map(|result| relativize_find_result_path(result, &search_path))
                    .collect();
                return Ok(build_result(
                    relativized,
                    effective_limit,
                    format!("{effective_limit} results limit reached"),
                ));
            }

            let Some(fd_path) = ensure_tool(ManagedTool::Fd, true).await else {
                return Err(ToolExecutionError::new(
                    "fd is not available and could not be downloaded",
                ));
            };
            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }

            let mut args: Vec<String> = ["--glob", "--color=never", "--hidden"]
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect();

            // fd ignores .gitignore outside git repos, so --no-require-git is needed
            // there. Inside a repo its default git-aware behaviour stops parent
            // .gitignore rules at nested repo boundaries.
            let mut inside_git_repo = false;
            let mut current = Path::new(&search_path).to_path_buf();
            loop {
                if path_exists(&current.join(".git").to_string_lossy()) {
                    inside_git_repo = true;
                    break;
                }
                let Some(parent) = current.parent().map(Path::to_path_buf) else {
                    break;
                };
                if parent == current {
                    break;
                }
                current = parent;
            }
            if !inside_git_repo {
                args.push("--no-require-git".to_owned());
            }
            args.push("--max-results".to_owned());
            args.push(effective_limit.to_string());

            // `fd --glob` matches the basename unless --full-path is set; in full-path
            // mode a pattern containing a slash needs a leading `**/` to match.
            let mut effective_pattern = pattern.clone();
            if pattern.contains('/') {
                args.push("--full-path".to_owned());
                if !pattern.starts_with('/') && !pattern.starts_with("**/") && pattern != "**" {
                    effective_pattern = format!("**/{pattern}");
                }
                if cfg!(windows) {
                    effective_pattern = effective_pattern.replace('/', r"[/\\]");
                }
            }
            args.push("--".to_owned());
            args.push(effective_pattern);
            args.push(search_path.clone());

            let mut child = tokio::process::Command::new(&fd_path)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| ToolExecutionError::new(format!("Failed to run fd: {error}")))?;

            use tokio::io::AsyncBufReadExt;
            let stdout = child.stdout.take().expect("piped stdout");
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            let mut collected: Vec<String> = Vec::new();
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
                match next {
                    Ok(Some(line)) => collected.push(line),
                    Ok(None) | Err(_) => break,
                }
            }
            let output = child
                .wait_with_output()
                .await
                .map_err(|error| ToolExecutionError::new(format!("Failed to run fd: {error}")))?;
            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }

            let relativized: Vec<String> = collected
                .iter()
                .map(|line| line.trim_end_matches('\r').trim())
                .filter(|line| !line.is_empty())
                .map(|line| relativize_find_result_path(line, &search_path))
                .collect();

            if !output.status.success() && relativized.is_empty() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                let message = if stderr.is_empty() {
                    match output.status.code() {
                        Some(code) => format!("fd exited with code {code}"),
                        None => "fd exited with code null".to_owned(),
                    }
                } else {
                    stderr
                };
                return Err(ToolExecutionError::new(message));
            }
            if relativized.is_empty() {
                return Ok(no_files_found());
            }

            Ok(build_result(
                relativized,
                effective_limit,
                format!(
                    "{effective_limit} results limit reached. Use limit={} for more, or refine pattern",
                    effective_limit * 2
                ),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct GlobOperations {
        results: Vec<String>,
    }

    impl FindOperations for GlobOperations {
        fn exists<'a>(&'a self, _absolute_path: &'a str) -> BoxFuture<'a, bool> {
            Box::pin(async { true })
        }

        fn glob<'a>(
            &'a self,
            _pattern: &'a str,
            _cwd: &'a str,
            ignore: &'a [&'a str],
            limit: usize,
        ) -> BoxFuture<'a, Result<Vec<String>, String>> {
            assert_eq!(ignore, ["**/node_modules/**", "**/.git/**"]);
            Box::pin(async move { Ok(self.results.iter().take(limit).cloned().collect()) })
        }
    }

    struct MissingPathOperations;

    impl FindOperations for MissingPathOperations {
        fn exists<'a>(&'a self, _absolute_path: &'a str) -> BoxFuture<'a, bool> {
            Box::pin(async { false })
        }

        fn glob<'a>(
            &'a self,
            _pattern: &'a str,
            _cwd: &'a str,
            _ignore: &'a [&'a str],
            _limit: usize,
        ) -> BoxFuture<'a, Result<Vec<String>, String>> {
            Box::pin(async { Ok(Vec::new()) })
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

    #[test]
    fn relativizes_results_against_the_search_root() {
        assert_eq!(
            relativize_find_result_path("/root/src/main.rs", "/root"),
            "src/main.rs"
        );
        assert_eq!(relativize_find_result_path("/root/dir/", "/root"), "dir/");
        assert_eq!(
            relativize_find_result_path("src/main.rs", "/root"),
            "src/main.rs"
        );
        assert_eq!(
            relativize_find_result_path("/other/file", "/root"),
            "../other/file"
        );
    }

    #[tokio::test]
    async fn uses_custom_glob_operations_when_provided() {
        let tool = create_find_tool_definition(
            "/root",
            Some(FindToolOptions {
                operations: Some(Arc::new(GlobOperations {
                    results: vec!["/root/a.ts".to_owned(), "/root/b.ts".to_owned()],
                })),
            }),
        );
        let result = tool
            .execute("call-1", json!({ "pattern": "*.ts" }), None, None, None)
            .await
            .expect("find");
        assert_eq!(text_of(&result), "a.ts\nb.ts");
        assert_eq!(result.details, None);
    }

    #[tokio::test]
    async fn reports_the_result_limit() {
        let tool = create_find_tool_definition(
            "/root",
            Some(FindToolOptions {
                operations: Some(Arc::new(GlobOperations {
                    results: (0..5)
                        .map(|index| format!("/root/file-{index}.ts"))
                        .collect(),
                })),
            }),
        );
        let result = tool
            .execute(
                "call-1",
                json!({ "pattern": "*.ts", "limit": 2 }),
                None,
                None,
                None,
            )
            .await
            .expect("find");
        assert_eq!(
            text_of(&result),
            "file-0.ts\nfile-1.ts\n\n[2 results limit reached]"
        );
        assert_eq!(
            result.details.expect("details")["resultLimitReached"],
            json!(2)
        );
    }

    #[tokio::test]
    async fn reports_an_empty_result_and_a_missing_path() {
        let tool = create_find_tool_definition(
            "/root",
            Some(FindToolOptions {
                operations: Some(Arc::new(GlobOperations {
                    results: Vec::new(),
                })),
            }),
        );
        let result = tool
            .execute("call-1", json!({ "pattern": "*.ts" }), None, None, None)
            .await
            .expect("find");
        assert_eq!(text_of(&result), "No files found matching pattern");

        let tool = create_find_tool_definition(
            "/root",
            Some(FindToolOptions {
                operations: Some(Arc::new(MissingPathOperations)),
            }),
        );
        let error = tool
            .execute(
                "call-1",
                json!({ "pattern": "*.ts", "path": "missing" }),
                None,
                None,
                None,
            )
            .await
            .expect_err("missing");
        assert_eq!(error.message, "Path not found: /root/missing");
    }

    #[tokio::test]
    async fn reports_a_missing_fd_binary_in_offline_mode() {
        // SAFETY: restored below; the download path must not run in tests.
        let original = std::env::var("NOTAGENT_OFFLINE").ok();
        unsafe { std::env::set_var("NOTAGENT_OFFLINE", "1") };
        if crate::utils::tools_manager::get_tool_path(ManagedTool::Fd).is_none() {
            let tool = create_find_tool_definition("/tmp", None);
            let error = tool
                .execute("call-1", json!({ "pattern": "*.ts" }), None, None, None)
                .await
                .expect_err("no fd");
            assert_eq!(
                error.message,
                "fd is not available and could not be downloaded"
            );
        }
        unsafe {
            match original {
                Some(value) => std::env::set_var("NOTAGENT_OFFLINE", value),
                None => std::env::remove_var("NOTAGENT_OFFLINE"),
            }
        }
    }

    #[test]
    fn advertises_its_description_and_schema() {
        let tool = create_find_tool_definition("/tmp", None);
        assert_eq!(tool.name(), "find");
        assert!(
            tool.description()
                .contains("truncated to 1000 results or 50KB")
        );
        assert_eq!(
            tool.prompt_snippet(),
            Some("Find files by glob pattern (respects .gitignore)")
        );
        assert_eq!(tool.parameters()["required"], json!(["pattern"]));
    }
}
