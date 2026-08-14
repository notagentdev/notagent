//! Port of `packages/coding-agent/src/core/tools/read-minified.ts` (tool half).
//!
//! `renderCall`/`renderResult` need the theme and syntax highlighting and are
//! wired in task 13.

use std::sync::Arc;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::mini_read::minify_for_path;
use crate::core::tools::path_utils::resolve_read_path;
use crate::core::tools::tool_definition::{
    SystemPromptContribution, ToolContext, ToolDefinition, wrap_tool_definition,
};
use crate::core::tools::truncate::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncationOptions, truncate_head,
};
use crate::utils::mime::detect_supported_image_mime_type_from_file;

pub const READ_MINIFIED_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Read a compact, editable view of a source file",
        guidelines: &[
            "Prefer read_minified over read when surveying or navigating code, especially large or heavily commented files.",
        ],
    };

fn read_minified_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file to read (relative or absolute)" },
            "offset": { "type": "number", "description": "Line number to start reading from (1-indexed)" },
            "limit": { "type": "number", "description": "Maximum number of lines to read" },
            "keep_comments": {
                "type": "boolean",
                "description": "Retain comments while still compacting whitespace (default: false)",
            },
        },
        "required": ["path"],
    })
}

fn description() -> String {
    [
        "Reads a source file and returns a compact view of it: comments are removed by default, blank lines are dropped, indentation is collapsed to one space per nesting level, and extra whitespace between tokens shrinks to a single space.".to_owned(),
        String::new(),
        "Prefer this tool over `read` when you want to understand, navigate, or survey code in compact form, especially for large or heavily commented files.".to_owned(),
        String::new(),
        "Usage:".to_owned(),
        "- Supported languages: Rust, Python, JavaScript/JSX, TypeScript/TSX, Go, Java, C, C++, Ruby, Bash, CSS, HTML, JSON. Files in other languages (and files that cannot be parsed) are returned unmodified.".to_owned(),
        "- The compact view is NOT byte-identical to the file on disk: indentation and comments may differ. Never copy text from this view into the old_string of an `edit` call — read the exact region with `read` first.".to_owned(),
        "- Output has no line numbers and line positions do not map back to the original file; use `read` or `grep` when you need precise line references.".to_owned(),
        "- Set keep_comments=true to retain comments while still compacting whitespace — useful when doc comments carry information you need.".to_owned(),
        format!(
            "- Like `read`, output is truncated to {DEFAULT_MAX_LINES} lines or {}KB; an optional offset/limit range is applied before minification.",
            DEFAULT_MAX_BYTES / 1024
        ),
        "- Images and PDFs are rejected; use `read` for visual content.".to_owned(),
    ]
    .join("\n")
}

/// Pluggable operations, mirroring `ReadOperations` in `read.rs`.
pub trait ReadMinifiedOperations: Send + Sync {
    fn read_file<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, String>>;
    /// `access(path, R_OK)`.
    fn access<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<(), String>>;
    fn detect_image_mime_type<'a>(
        &'a self,
        absolute_path: &'a str,
    ) -> BoxFuture<'a, Option<String>>;
}

pub struct LocalReadMinifiedOperations;

impl ReadMinifiedOperations for LocalReadMinifiedOperations {
    fn read_file<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        Box::pin(async move {
            tokio::fs::read(absolute_path)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn access<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            tokio::fs::File::open(absolute_path)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    fn detect_image_mime_type<'a>(
        &'a self,
        absolute_path: &'a str,
    ) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            detect_supported_image_mime_type_from_file(absolute_path)
                .await
                .map(str::to_owned)
        })
    }
}

#[derive(Clone, Default)]
pub struct ReadMinifiedToolOptions {
    pub operations: Option<Arc<dyn ReadMinifiedOperations>>,
}

pub struct ReadMinifiedToolDefinition {
    cwd: String,
    operations: Arc<dyn ReadMinifiedOperations>,
    description: String,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_read_minified_tool_definition(
    cwd: &str,
    options: Option<ReadMinifiedToolOptions>,
) -> ReadMinifiedToolDefinition {
    let options = options.unwrap_or_default();
    ReadMinifiedToolDefinition {
        cwd: cwd.to_owned(),
        operations: options
            .operations
            .unwrap_or_else(|| Arc::new(LocalReadMinifiedOperations)),
        description: description(),
        parameters: read_minified_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

impl ToolDefinition for ReadMinifiedToolDefinition {
    fn name(&self) -> &str {
        "read_minified"
    }

    fn label(&self) -> &str {
        "read minified"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(READ_MINIFIED_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        READ_MINIFIED_TOOL_SYSTEM_PROMPT_CONTRIBUTION
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
        signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let throw_if_aborted = || -> Result<(), ToolExecutionError> {
                match &signal {
                    Some(signal) if signal.is_cancelled() => {
                        Err(ToolExecutionError::new("Operation aborted"))
                    }
                    _ => Ok(()),
                }
            };
            throw_if_aborted()?;

            let path = params
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let offset = params.get("offset").and_then(Value::as_u64);
            let limit = params.get("limit").and_then(Value::as_u64);
            let keep_comments = params
                .get("keep_comments")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            let absolute_path = resolve_read_path(&path, &self.cwd);
            self.operations
                .access(&absolute_path)
                .await
                .map_err(ToolExecutionError::new)?;
            throw_if_aborted()?;

            if self
                .operations
                .detect_image_mime_type(&absolute_path)
                .await
                .is_some()
            {
                return Err(ToolExecutionError::new(format!(
                    "{path} is visual content (image/PDF); minified reads only support text files. Use the read tool instead."
                )));
            }

            let buffer = self
                .operations
                .read_file(&absolute_path)
                .await
                .map_err(ToolExecutionError::new)?;
            throw_if_aborted()?;
            let text_content = String::from_utf8_lossy(&buffer).into_owned();
            let all_lines: Vec<&str> = text_content.split('\n').collect();
            let total_file_lines = all_lines.len();

            // Apply the offset/limit range before minification, matching the
            // reference (the range is resolved against the raw file).
            let start_line = offset.map(|offset| offset.saturating_sub(1)).unwrap_or(0) as usize;
            if start_line >= all_lines.len() {
                return Err(ToolExecutionError::new(format!(
                    "Offset {} is beyond end of file ({} lines total)",
                    offset.unwrap_or_default(),
                    all_lines.len()
                )));
            }
            let selected_content = match limit {
                Some(limit) => {
                    let end = (start_line + limit as usize).min(all_lines.len());
                    all_lines[start_line..end].join("\n")
                }
                None => all_lines[start_line..].join("\n"),
            };

            let truncation = truncate_head(&selected_content, TruncationOptions::default());
            let raw = truncation.content.clone();

            // Unsupported languages, unparseable files, and views that render
            // empty fall back to the raw content, exactly as the reference does.
            let minified = minify_for_path(std::path::Path::new(&path), &raw, keep_comments);
            throw_if_aborted()?;
            let usable = minified.filter(|minified| !minified.trim().is_empty());

            let mut output_text = usable.clone().unwrap_or_else(|| raw.clone());
            if truncation.truncated {
                let start_line_display = start_line + 1;
                let end_line_display = start_line_display + truncation.output_lines - 1;
                output_text.push_str(&format!(
                    "\n\n[Minified from lines {start_line_display}-{end_line_display} of {total_file_lines}. Use offset={} to continue.]",
                    end_line_display + 1
                ));
            }
            if usable.is_none() {
                output_text
                    .push_str("\n\n[Language not supported for minification; raw content shown.]");
            }

            let mut details = Map::new();
            details.insert(
                "truncation".to_owned(),
                serde_json::to_value(&truncation).expect("truncation is serializable"),
            );
            details.insert("minified".to_owned(), Value::Bool(usable.is_some()));
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(output_text))],
                details: Some(Value::Object(details)),
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

/// `createCreateReadMinifiedTool` — the tool as the agent loop takes it.
pub fn create_read_minified_tool(
    cwd: &str,
    options: Option<ReadMinifiedToolOptions>,
) -> Arc<dyn AgentTool> {
    wrap_tool_definition(
        Arc::new(create_read_minified_tool_definition(cwd, options)),
        None,
    )
}
