//! Port of `packages/coding-agent/src/core/tools/read.ts` (tool half).

use std::sync::Arc;

use notagent_agent::types::{
    AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{
    ConstrainedSampling, ImageContent, Modality, Model, TextContent, TextOrImageContent,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::tools::path_utils::resolve_read_path;
use crate::core::tools::tool_definition::{SystemPromptContribution, ToolContext, ToolDefinition};
use crate::core::tools::truncate::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncatedBy, TruncationOptions, format_size,
    truncate_head,
};
use crate::utils::image::{ProcessImageResult, process_image};
use crate::utils::mime::detect_supported_image_mime_type_from_file;

pub const READ_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Read file contents",
        guidelines: &["Use read to examine files instead of cat or sed."],
    };

fn read_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file to read (relative or absolute)" },
            "offset": { "type": "number", "description": "Line number to start reading from (1-indexed)" },
            "limit": { "type": "number", "description": "Maximum number of lines to read" },
        },
        "required": ["path"],
    })
}

/// Pluggable operations, so reading can be delegated to a remote system.
pub trait ReadOperations: Send + Sync {
    fn read_file<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, String>>;
    /// `access(path, R_OK)`; the error message is what TS reports.
    fn access<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<(), String>>;
    fn detect_image_mime_type<'a>(
        &'a self,
        absolute_path: &'a str,
    ) -> BoxFuture<'a, Option<String>>;
}

pub struct LocalReadOperations;

impl ReadOperations for LocalReadOperations {
    fn read_file<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        Box::pin(async move {
            tokio::fs::read(absolute_path)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn access<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let metadata = tokio::fs::metadata(absolute_path)
                .await
                .map_err(|error| error.to_string())?;
            // `access(R_OK)`: opening the file is the portable readability probe.
            if metadata.is_dir() {
                return Err(format!(
                    "EISDIR: illegal operation on a directory, read {absolute_path}"
                ));
            }
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

pub struct ReadToolOptions {
    /// Auto-resize images to 2000×2000. Default: true.
    pub auto_resize_images: bool,
    pub operations: Option<Arc<dyn ReadOperations>>,
}

impl Default for ReadToolOptions {
    fn default() -> Self {
        Self {
            auto_resize_images: true,
            operations: None,
        }
    }
}

pub struct ReadToolDefinition {
    cwd: String,
    auto_resize_images: bool,
    operations: Arc<dyn ReadOperations>,
    description: String,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_read_tool_definition(
    cwd: &str,
    options: Option<ReadToolOptions>,
) -> ReadToolDefinition {
    let options = options.unwrap_or_default();
    ReadToolDefinition {
        cwd: cwd.to_owned(),
        auto_resize_images: options.auto_resize_images,
        operations: options
            .operations
            .unwrap_or_else(|| Arc::new(LocalReadOperations)),
        description: format!(
            "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). Images are sent as attachments. For text files, output is truncated to {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.",
            DEFAULT_MAX_BYTES / 1024
        ),
        parameters: read_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

fn get_non_vision_image_note(model: Option<&Model>) -> Option<&'static str> {
    match model {
        None => None,
        Some(model) if model.input.contains(&Modality::Image) => None,
        Some(_) => Some(
            "[Current model does not support images. The image will be omitted from this request.]",
        ),
    }
}

impl ToolDefinition for ReadToolDefinition {
    fn name(&self) -> &str {
        "read"
    }

    fn label(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(READ_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        READ_TOOL_SYSTEM_PROMPT_CONTRIBUTION
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

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let aborted = || matches!(&signal, Some(signal) if signal.is_cancelled());
            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }

            let path = params
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let offset = params
                .get("offset")
                .and_then(Value::as_f64)
                .filter(|offset| offset.is_finite())
                .map(|offset| offset as i64);
            let limit = params
                .get("limit")
                .and_then(Value::as_f64)
                .filter(|limit| limit.is_finite())
                .map(|limit| limit as i64);

            let absolute_path = resolve_read_path(&path, &self.cwd);
            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }
            self.operations
                .access(&absolute_path)
                .await
                .map_err(ToolExecutionError::new)?;
            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }

            let mime_type = self.operations.detect_image_mime_type(&absolute_path).await;
            let non_vision_note = get_non_vision_image_note(
                context.as_ref().and_then(|context| context.model.as_ref()),
            );

            if let Some(mime_type) = mime_type {
                let bytes = self
                    .operations
                    .read_file(&absolute_path)
                    .await
                    .map_err(ToolExecutionError::new)?;
                let processed = process_image(&bytes, &mime_type, self.auto_resize_images, None);
                let content = match processed {
                    ProcessImageResult::Failed { message } => {
                        let mut note = format!("Read image file [{mime_type}]\n{message}");
                        if let Some(non_vision_note) = non_vision_note {
                            note.push('\n');
                            note.push_str(non_vision_note);
                        }
                        vec![TextOrImageContent::Text(TextContent::new(note))]
                    }
                    ProcessImageResult::Ok {
                        data,
                        mime_type,
                        hints,
                    } => {
                        let mut note = format!("Read image file [{mime_type}]");
                        if !hints.is_empty() {
                            note.push('\n');
                            note.push_str(&hints.join("\n"));
                        }
                        if let Some(non_vision_note) = non_vision_note {
                            note.push('\n');
                            note.push_str(non_vision_note);
                        }
                        vec![
                            TextOrImageContent::Text(TextContent::new(note)),
                            TextOrImageContent::Image(ImageContent { data, mime_type }),
                        ]
                    }
                };
                if aborted() {
                    return Err(ToolExecutionError::new("Operation aborted"));
                }
                return Ok(AgentToolResult {
                    content,
                    details: None,
                    usage: None,
                    added_tool_names: None,
                    terminate: None,
                });
            }

            let bytes = self
                .operations
                .read_file(&absolute_path)
                .await
                .map_err(ToolExecutionError::new)?;
            let text_content = String::from_utf8_lossy(&bytes).into_owned();
            let all_lines: Vec<&str> = text_content.split('\n').collect();
            let total_file_lines = all_lines.len();
            // The offset is 1-indexed on the wire and 0-indexed here.
            let start_line = offset
                .map(|offset| (offset - 1).max(0) as usize)
                .unwrap_or(0);
            let start_line_display = start_line + 1;
            if start_line >= all_lines.len() {
                return Err(ToolExecutionError::new(format!(
                    "Offset {} is beyond end of file ({} lines total)",
                    offset.unwrap_or(0),
                    all_lines.len()
                )));
            }

            // A user limit wins; otherwise `truncate_head` decides.
            let mut user_limited_lines: Option<usize> = None;
            let selected_content = match limit {
                Some(limit) => {
                    let end_line = (start_line + limit.max(0) as usize).min(all_lines.len());
                    user_limited_lines = Some(end_line - start_line);
                    all_lines[start_line..end_line].join("\n")
                }
                None => all_lines[start_line..].join("\n"),
            };

            let truncation = truncate_head(&selected_content, TruncationOptions::default());
            let mut details: Option<Value> = None;
            let output_text = if truncation.first_line_exceeds_limit {
                // Point the model at a bash fallback for a single oversized line.
                let first_line_size = format_size(all_lines[start_line].len());
                details = Some(json!({
                    "truncation": serde_json::to_value(&truncation).expect("truncation"),
                }));
                format!(
                    "[Line {start_line_display} is {first_line_size}, exceeds {} limit. Use bash: sed -n '{start_line_display}p' {path} | head -c {DEFAULT_MAX_BYTES}]",
                    format_size(DEFAULT_MAX_BYTES)
                )
            } else if truncation.truncated {
                let end_line_display = start_line_display + truncation.output_lines - 1;
                let next_offset = end_line_display + 1;
                let notice = if truncation.truncated_by == Some(TruncatedBy::Lines) {
                    format!(
                        "\n\n[Showing lines {start_line_display}-{end_line_display} of {total_file_lines}. Use offset={next_offset} to continue.]"
                    )
                } else {
                    format!(
                        "\n\n[Showing lines {start_line_display}-{end_line_display} of {total_file_lines} ({} limit). Use offset={next_offset} to continue.]",
                        format_size(DEFAULT_MAX_BYTES)
                    )
                };
                details = Some(json!({
                    "truncation": serde_json::to_value(&truncation).expect("truncation"),
                }));
                format!("{}{notice}", truncation.content)
            } else if user_limited_lines.is_some_and(|lines| start_line + lines < all_lines.len()) {
                // The user limit stopped early while the file still has content.
                let user_limited_lines = user_limited_lines.expect("checked");
                let remaining = all_lines.len() - (start_line + user_limited_lines);
                let next_offset = start_line + user_limited_lines + 1;
                format!(
                    "{}\n\n[{remaining} more lines in file. Use offset={next_offset} to continue.]",
                    truncation.content
                )
            } else {
                truncation.content.clone()
            };

            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(output_text))],
                details,
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
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
                .prefix("notagent-read-tool-")
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

    #[tokio::test]
    async fn reads_a_whole_text_file() {
        let directory = TempDir::new();
        directory.write("file.txt", "one\ntwo\nthree\n");
        let tool = create_read_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute("call-1", json!({ "path": "file.txt" }), None, None, None)
            .await
            .expect("read");
        assert_eq!(text_of(&result), "one\ntwo\nthree\n");
        assert_eq!(result.details, None);
    }

    #[tokio::test]
    async fn applies_offset_and_limit() {
        let directory = TempDir::new();
        directory.write("file.txt", "1\n2\n3\n4\n5\n");
        let tool = create_read_tool_definition(&directory.cwd(), None);

        let result = tool
            .execute(
                "call-1",
                json!({ "path": "file.txt", "offset": 3 }),
                None,
                None,
                None,
            )
            .await
            .expect("read");
        assert_eq!(text_of(&result), "3\n4\n5\n");

        let result = tool
            .execute(
                "call-1",
                json!({ "path": "file.txt", "offset": 2, "limit": 2 }),
                None,
                None,
                None,
            )
            .await
            .expect("read");
        // The trailing newline makes the sixth line an empty one, so three remain.
        assert_eq!(
            text_of(&result),
            "2\n3\n\n[3 more lines in file. Use offset=4 to continue.]"
        );
    }

    #[tokio::test]
    async fn rejects_an_offset_beyond_the_end_of_the_file() {
        let directory = TempDir::new();
        directory.write("file.txt", "1\n2\n");
        let tool = create_read_tool_definition(&directory.cwd(), None);
        let error = tool
            .execute(
                "call-1",
                json!({ "path": "file.txt", "offset": 99 }),
                None,
                None,
                None,
            )
            .await
            .expect_err("offset");
        assert_eq!(
            error.message,
            "Offset 99 is beyond end of file (3 lines total)"
        );
    }

    #[tokio::test]
    async fn truncates_at_the_line_limit_with_a_continuation_notice() {
        let directory = TempDir::new();
        let content: String = (1..=2500).map(|index| format!("line {index}\n")).collect();
        directory.write("big.txt", &content);
        let tool = create_read_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute("call-1", json!({ "path": "big.txt" }), None, None, None)
            .await
            .expect("read");
        let text = text_of(&result);
        assert!(
            text.ends_with("[Showing lines 1-2000 of 2501. Use offset=2001 to continue.]"),
            "{}",
            &text[text.len() - 80..]
        );
        let details = result.details.expect("details");
        assert_eq!(details["truncation"]["truncatedBy"], json!("lines"));
    }

    #[tokio::test]
    async fn points_at_bash_when_the_first_line_is_too_large() {
        let directory = TempDir::new();
        directory.write("long.txt", &"x".repeat(60 * 1024));
        let tool = create_read_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute("call-1", json!({ "path": "long.txt" }), None, None, None)
            .await
            .expect("read");
        assert_eq!(
            text_of(&result),
            "[Line 1 is 60.0KB, exceeds 50.0KB limit. Use bash: sed -n '1p' long.txt | head -c 51200]"
        );
        assert_eq!(
            result.details.expect("details")["truncation"]["firstLineExceedsLimit"],
            json!(true)
        );
    }

    #[tokio::test]
    async fn reads_an_image_as_an_attachment() {
        let directory = TempDir::new();
        let image = image::DynamicImage::new_rgb8(8, 8);
        let mut bytes: Vec<u8> = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("encode");
        std::fs::write(directory.path.join("pixel.png"), &bytes).expect("write");

        let tool = create_read_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute("call-1", json!({ "path": "pixel.png" }), None, None, None)
            .await
            .expect("read");
        assert_eq!(text_of(&result), "Read image file [image/png]");
        assert!(matches!(result.content[1], TextOrImageContent::Image(_)));
    }

    #[tokio::test]
    async fn fails_for_a_missing_file() {
        let directory = TempDir::new();
        let tool = create_read_tool_definition(&directory.cwd(), None);
        let error = tool
            .execute("call-1", json!({ "path": "missing.txt" }), None, None, None)
            .await
            .expect_err("missing");
        assert!(error.message.contains("No such file"), "{}", error.message);
    }

    #[tokio::test]
    async fn stops_on_an_aborted_signal() {
        let directory = TempDir::new();
        directory.write("file.txt", "x");
        let tool = create_read_tool_definition(&directory.cwd(), None);
        let signal = CancellationToken::new();
        signal.cancel();
        let error = tool
            .execute(
                "call-1",
                json!({ "path": "file.txt" }),
                Some(signal),
                None,
                None,
            )
            .await
            .expect_err("aborted");
        assert_eq!(error.message, "Operation aborted");
    }

    #[test]
    fn advertises_its_description_and_schema() {
        let tool = create_read_tool_definition("/tmp", None);
        assert_eq!(tool.name(), "read");
        assert!(
            tool.description()
                .contains("truncated to 2000 lines or 50KB")
        );
        assert_eq!(tool.prompt_snippet(), Some("Read file contents"));
        assert_eq!(
            tool.prompt_guidelines(),
            vec!["Use read to examine files instead of cat or sed.".to_owned()]
        );
        assert_eq!(tool.parameters()["required"], json!(["path"]));
    }
}
