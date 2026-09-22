use std::sync::Arc;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{
    ConstrainedSampling, ImageContent, Modality, Model, TextContent, TextOrImageContent,
};
use notagent_tui::tui::ComponentRef;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::mini_read::minify_for_path;
use crate::core::tools::path_utils::resolve_read_path;
use crate::core::tools::path_utils::resolve_to_cwd;
use crate::core::tools::render_utils::{
    call_title, get_text_output, render_tool_path, replace_tabs, str_arg,
};
use crate::core::tools::tool_definition::{
    SystemPromptContribution, ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult,
    ToolRenderResultOptions, display_arg, render_text_call, render_text_result,
    wrap_tool_definition,
};
use crate::core::tools::truncate::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncatedBy, TruncationOptions, format_size,
    truncate_head, truncation_from_details,
};
use crate::modes::interactive::components::keybinding_hints::{key_hint, key_text};
use crate::modes::interactive::theme::theme::{
    Theme, ThemeColor, get_language_from_path, highlight_code,
};
use crate::utils::image::{ProcessImageResult, process_image};
use crate::utils::mime::detect_supported_image_mime_type_from_file;

pub const READ_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Read files (compact source by default); set keep_comments=true when comments matter",
        guidelines: &[
            "When read is attached, use it for file reads instead of cat or sed. Source files are compact by default: comments are omitted. You must set keep_comments=true when comments, docstrings, documentation comments, or commented instructions matter; this preserves comments while still compacting whitespace. Use view=original only for exact original line references, formatting-sensitive work, or the exact source needed for a necessary patch fallback, and supply a concrete reason. Request only the needed offset/limit range. Do not bypass compact source reads with shell commands, unless no available tool can read the requested oversized line.",
        ],
    };

fn read_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file to read (relative or absolute)" },
            "offset": { "type": "number", "description": "Line number to start reading from (1-indexed)" },
            "limit": { "type": "number", "description": "Maximum number of lines to read" },
            "keep_comments": {
                "type": "boolean",
                "description": "Default false: comments are omitted from compact source. Set true when comments, docstrings, documentation comments, or commented instructions matter. Whitespace is still compacted.",
            },
            "view": {
                "type": "string",
                "enum": ["compact", "original"],
                "description": "Default compact. Use original only for exact source line references, formatting-sensitive work, or a necessary plain-patch fallback. Original requires a concrete reason; use the smallest needed offset/limit range.",
            },
            "reason": {
                "type": "string",
                "description": "Required when view=original: explain why compact output with keep_comments=true cannot serve this read.",
            },
        },
        "required": ["path"],
    })
}

/// Pluggable operations, so reading can be delegated to a remote system.
pub trait ReadOperations: Send + Sync {
    fn read_file<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, String>>;
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

#[derive(Clone)]
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
        description: description(),
        parameters: read_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

fn description() -> String {
    [
        "Read file contents. Source files are compact by default: comments and blank lines are removed, indentation is reduced, and extra whitespace is collapsed. Other text files are returned unchanged; if compaction is unavailable, source text is returned unchanged. Images (jpg, png, gif, webp, bmp) are detected automatically and sent as attachments. PDFs are not supported.".to_owned(),
        "Comments are omitted by default. You must set keep_comments=true when comments, docstrings, documentation comments, or commented instructions are relevant. This preserves comments while still compacting whitespace; needing comments is not a reason to switch to original view.".to_owned(),
        "Supported compact languages: Rust, Python, JavaScript/JSX, TypeScript/TSX, Go, Java, C, C++, Ruby, Bash, CSS, HTML, JSON.".to_owned(),
        "Use view=original only for exact original line references, formatting-sensitive work, or the exact source needed for a necessary plain patch fallback. A non-empty reason explaining the need is required. Request the smallest relevant offset/limit range. Unsupported languages already return unchanged text and do not require original view.".to_owned(),
        "Compact output is not byte-identical to the file and its line positions do not map to original lines. Edit it with patch_minified or multi_patch_minified. Never copy compact output into plain patch's old_string; for a necessary fallback, first read the exact region with view=original and a reason.".to_owned(),
        format!("Text output is truncated to {DEFAULT_MAX_LINES} lines or {}KB. offset (1-indexed) and limit select original source lines before compaction. Continuation offsets always refer to the original file. Use keep_comments=true to include comments in compact output.", DEFAULT_MAX_BYTES / 1024),
        "Use this tool for file reads; do not use shell commands to bypass the compact default, unless no available tool can read the requested oversized line.".to_owned(),
    ].join("\n\n")
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

// ============================================================================
// Rendering
// ============================================================================

/// What a compact `read` header shows instead of the plain path.
struct CompactReadClassification {
    kind: CompactReadKind,
    label: String,
}

#[derive(PartialEq, Eq)]
enum CompactReadKind {
    Docs,
    Resource,
    Skill,
}

impl CompactReadKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Docs => "docs",
            Self::Resource => "resource",
            Self::Skill => "skill",
        }
    }
}

const COMPACT_RESOURCE_FILE_NAMES: [&str; 5] = [
    "AGENTS.override.md",
    "AGENTS.md",
    "AGENTS.MD",
    "CLAUDE.md",
    "CLAUDE.MD",
];

/// `:12-40` after the path, when the call asks for a line range.
fn format_read_line_range(args: &Value, theme: &Theme) -> String {
    let offset = args.get("offset");
    let limit = args.get("limit");
    if offset.is_none() && limit.is_none() {
        return String::new();
    }
    // `args.offset ?? 1` — a null offset falls back to the first line.
    let offset = offset.filter(|value| !value.is_null());
    let start_display = offset.map_or_else(|| "1".to_string(), display_arg);
    let start_number = offset.map_or(Some(1.0), Value::as_f64);
    // `startLine + limit - 1`, then `endLine ? … : ""` — 0 and NaN are falsy, so
    // a limit that is not a number leaves the range open-ended. Deviation
    let end_line = match (start_number, limit) {
        (Some(start), Some(limit)) => limit
            .as_f64()
            .map(|limit| start + limit - 1.0)
            .filter(|end| *end != 0.0 && !end.is_nan()),
        _ => None,
    };
    let range = match end_line {
        Some(end) => format!("-{}", notagent_ai::utils::js_number::to_js_string(end)),
        None => String::new(),
    };
    theme.fg(ThemeColor::Warning, &format!(":{start_display}{range}"))
}

fn format_read_call(args: &Value, theme: &Theme, cwd: &str) -> String {
    let raw_path = read_path_arg(args);
    let path_display = render_tool_path(raw_path.as_deref(), theme, cwd, None);
    format!(
        "{}{path_display}{}",
        call_title(theme, "read"),
        format_read_line_range(args, theme)
    )
}

/// `str(args?.file_path ?? args?.path)`
fn read_path_arg(args: &Value) -> Option<String> {
    let raw = args
        .get("file_path")
        .filter(|value| !value.is_null())
        .or_else(|| args.get("path"));
    str_arg(raw)
}

fn trim_trailing_empty_lines(lines: Vec<String>) -> Vec<String> {
    let mut end = lines.len();
    while end > 0 && lines[end - 1].is_empty() {
        end -= 1;
    }
    let mut lines = lines;
    lines.truncate(end);
    lines
}

fn to_posix_path(file_path: &str) -> String {
    file_path
        .split(std::path::MAIN_SEPARATOR)
        .collect::<Vec<_>>()
        .join("/")
}

/// The docs, README and examples shipped with the package render compactly.
fn get_pi_docs_classification(absolute_path: &str) -> Option<CompactReadClassification> {
    let package_root = crate::config::get_readme_path()
        .parent()?
        .to_string_lossy()
        .into_owned();
    // `relative === ""` (the package root itself) is not a docs path; every
    // escaping path is filtered by `get_cwd_relative_path` returning `None`.
    let relative_path =
        crate::utils::paths::get_cwd_relative_path(absolute_path, &package_root).ok()??;
    if relative_path == "." {
        return None;
    }

    let label = to_posix_path(&relative_path);
    if label == "README.md" || label.starts_with("docs/") || label.starts_with("examples/") {
        return Some(CompactReadClassification {
            kind: CompactReadKind::Docs,
            label,
        });
    }
    None
}

fn get_compact_read_classification(args: &Value, cwd: &str) -> Option<CompactReadClassification> {
    let raw_path = read_path_arg(args).filter(|path| !path.is_empty())?;

    let absolute_path = resolve_to_cwd(&raw_path, cwd);
    let file_name = std::path::Path::new(&absolute_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if file_name == "SKILL.md" {
        let parent = std::path::Path::new(&absolute_path)
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Some(CompactReadClassification {
            kind: CompactReadKind::Skill,
            // `basename(dirname(path)) || fileName`
            label: if parent.is_empty() { file_name } else { parent },
        });
    }

    if let Some(docs_classification) = get_pi_docs_classification(&absolute_path) {
        return Some(docs_classification);
    }

    if COMPACT_RESOURCE_FILE_NAMES.contains(&file_name.as_str()) {
        return Some(CompactReadClassification {
            kind: CompactReadKind::Resource,
            label: crate::utils::paths::format_path_relative_to_cwd_or_absolute(
                &absolute_path,
                cwd,
            )
            .ok()?,
        });
    }

    None
}

fn format_compact_read_call(
    classification: &CompactReadClassification,
    args: &Value,
    theme: &Theme,
) -> String {
    let expand_hint = theme.fg(
        ThemeColor::Dim,
        &format!(" ({} to expand)", key_text("app.tools.expand")),
    );
    if classification.kind == CompactReadKind::Skill {
        return theme.fg(ThemeColor::CustomMessageLabel, "\x1b[1m[skill]\x1b[22m ")
            + &theme.fg(ThemeColor::CustomMessageText, &classification.label)
            + &format_read_line_range(args, theme)
            + &expand_hint;
    }

    // Badge style: the badge above already says READ, so the header keeps
    // only the kind word (`docs README* …` instead of `read docs README* …`).
    let title = if crate::modes::interactive::theme::theme::block_style().is_compact() {
        theme.fg(
            ThemeColor::ToolTitle,
            &theme.bold(classification.kind.as_str()),
        )
    } else {
        theme.fg(
            ThemeColor::ToolTitle,
            &theme.bold(&format!("read {}", classification.kind.as_str())),
        )
    };
    title
        + " "
        + &theme.fg(ThemeColor::Accent, &classification.label)
        + &format_read_line_range(args, theme)
        + &expand_hint
}

/// Preview rows of the result before it is expanded.
const READ_PREVIEW_LINES: usize = 10;

fn format_read_result(
    args: &Value,
    result: ToolRenderResult<'_>,
    options: ToolRenderResultOptions,
    theme: &Theme,
    show_images: bool,
    is_error: bool,
) -> String {
    if !options.expanded && !is_error {
        return String::new();
    }

    let raw_path = read_path_arg(args);
    let output = get_text_output(Some(result.content), show_images);
    let lang = match (is_error, &raw_path) {
        (false, Some(raw_path)) if !raw_path.is_empty() => get_language_from_path(raw_path),
        _ => None,
    };
    let rendered_lines = match &lang {
        Some(lang) => highlight_code(&replace_tabs(&output), Some(lang)),
        None => output.split('\n').map(str::to_string).collect(),
    };
    let lines = trim_trailing_empty_lines(rendered_lines);
    let max_lines = if options.expanded {
        lines.len()
    } else {
        READ_PREVIEW_LINES
    };
    let display_lines = &lines[..max_lines.min(lines.len())];
    let remaining = lines.len() as isize - max_lines as isize;
    let mut text = format!(
        "\n{}",
        display_lines
            .iter()
            .map(|line| match &lang {
                Some(_) => replace_tabs(line),
                None => theme.fg(ThemeColor::ToolOutput, &replace_tabs(line)),
            })
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

    if let Some(truncation) = truncation_from_details(result.details).filter(|t| t.truncated) {
        let warning = if truncation.first_line_exceeds_limit {
            format!(
                "[First line exceeds {} limit]",
                format_size(truncation.max_bytes)
            )
        } else if truncation.truncated_by == Some(TruncatedBy::Lines) {
            format!(
                "[Truncated: showing {} of {} lines ({} line limit)]",
                truncation.output_lines, truncation.total_lines, truncation.max_lines
            )
        } else {
            format!(
                "[Truncated: {} lines shown ({} limit)]",
                truncation.output_lines,
                format_size(truncation.max_bytes)
            )
        };
        text += &format!("\n{}", theme.fg(ThemeColor::Warning, &warning));
    }
    if !is_error
        && result.details.is_some_and(|details| {
            details.get("view").and_then(Value::as_str) == Some("compact")
                && details.get("minified").and_then(Value::as_bool) == Some(false)
        })
    {
        text += &format!(
            "\n{}",
            theme.fg(
                ThemeColor::Dim,
                "[Compaction unavailable; original text shown]"
            )
        );
    }
    text
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

    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let classification = if context.expanded {
            None
        } else {
            get_compact_read_classification(args, &context.cwd)
        };
        let mut text = match &classification {
            Some(classification) => format_compact_read_call(classification, args, theme),
            None => format_read_call(args, theme, &context.cwd),
        };
        if args.get("view").and_then(Value::as_str) == Some("original") {
            text += &theme.fg(ThemeColor::Dim, " [original]");
        } else if args.get("keep_comments").and_then(Value::as_bool) == Some(true) {
            text += &theme.fg(ThemeColor::Dim, " +comments");
        }
        Some(render_text_call(context, &text))
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
            &format_read_result(
                &context.args,
                result,
                options,
                theme,
                context.show_images,
                context.is_error,
            ),
        ))
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

            let original = match params.get("view").and_then(Value::as_str) {
                None | Some("compact") => false,
                Some("original") => true,
                Some(value) => {
                    return Err(ToolExecutionError::new(format!(
                        "Unknown read view {value:?}; use compact or original"
                    )));
                }
            };
            if original
                && params
                    .get("reason")
                    .and_then(Value::as_str)
                    .is_none_or(|reason| reason.trim().is_empty())
            {
                return Err(ToolExecutionError::new(
                    "view=original requires a concrete reason. For comments, use keep_comments=true with the default compact view.",
                ));
            }
            let keep_comments = params
                .get("keep_comments")
                .and_then(Value::as_bool)
                .unwrap_or(false);

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
            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }
            if bytes.starts_with(b"%PDF-") {
                return Err(ToolExecutionError::new(
                    "PDF reading is not supported. Provide extracted text or rendered page images.",
                ));
            }
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
            let compact = if original {
                None
            } else {
                minify_for_path(
                    std::path::Path::new(&path),
                    &truncation.content,
                    keep_comments,
                )
            };
            let minified = compact.is_some();
            let content = compact.as_deref().unwrap_or(&truncation.content);
            let mut details = json!({
                "minified": minified,
                "view": if original { "original" } else { "compact" },
                "startLine": start_line_display,
                "endLine": start_line + truncation.output_lines,
                "totalFileLines": total_file_lines,
            });
            let output_text = if truncation.first_line_exceeds_limit {
                // Point the model at a bash fallback for a single oversized line.
                let first_line_size = format_size(all_lines[start_line].len());
                details["truncation"] = serde_json::to_value(&truncation)
                    .map_err(|error| ToolExecutionError::new(error.to_string()))?;
                format!(
                    "[Line {start_line_display} is {first_line_size}, exceeds {} limit. Only if no available tool can read this oversized line, use bash: sed -n '{start_line_display}p' {path} | head -c {DEFAULT_MAX_BYTES}]",
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
                details["truncation"] = serde_json::to_value(&truncation)
                    .map_err(|error| ToolExecutionError::new(error.to_string()))?;
                format!("{content}{notice}")
            } else if user_limited_lines.is_some_and(|lines| start_line + lines < all_lines.len()) {
                // The user limit stopped early while the file still has content.
                let user_limited_lines = user_limited_lines.expect("checked");
                let remaining = all_lines.len() - (start_line + user_limited_lines);
                let next_offset = start_line + user_limited_lines + 1;
                format!(
                    "{}\n\n[{remaining} more lines in file. Use offset={next_offset} to continue.]",
                    content
                )
            } else {
                content.to_owned()
            };

            if aborted() {
                return Err(ToolExecutionError::new("Operation aborted"));
            }
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(output_text))],
                details: Some(details),
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

/// `createCreateReadTool` — the tool as the agent loop takes it.
pub fn create_read_tool(cwd: &str, options: Option<ReadToolOptions>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_read_tool_definition(cwd, options)), None)
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
        assert_eq!(result.details.as_ref().unwrap()["minified"], json!(false));
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
            "[Line 1 is 60.0KB, exceeds 50.0KB limit. Only if no available tool can read this oversized line, use bash: sed -n '1p' long.txt | head -c 51200]"
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
    fn read_headers_show_the_comment_switch_for_source_and_documentation() {
        let _guard = crate::modes::interactive::theme::theme::test_lock();
        crate::modes::interactive::theme::theme::init_theme(Some("dark"), false);
        let theme = crate::modes::interactive::theme::theme::theme();
        let tool = create_read_tool_definition("/tmp", None);
        for path in ["main.rs", "AGENTS.md", "example/SKILL.md"] {
            for expanded in [false, true] {
                let args = json!({"path": path, "keep_comments": true});
                let mut context = ToolRenderContext::new("read", args.clone(), "/tmp");
                context.expanded = expanded;
                let component = tool.render_call(&args, &theme, &context).unwrap();
                let rows = component.borrow_mut().render(160);
                assert!(
                    rows.iter().any(|row| row.contains("+comments")),
                    "the comment switch must remain visible for {path}, expanded={expanded}: {rows:?}"
                );
            }
        }
    }

    #[test]
    fn read_results_distinguish_fallback_text_from_requested_originals_and_images() {
        let _guard = crate::modes::interactive::theme::theme::test_lock();
        crate::modes::interactive::theme::theme::init_theme(Some("dark"), false);
        let theme = crate::modes::interactive::theme::theme::theme();
        let content = vec![TextOrImageContent::Text(TextContent::new("example"))];
        for (details, should_mark) in [
            (Some(json!({"view": "compact", "minified": false})), true),
            (Some(json!({"view": "compact", "minified": true})), false),
            (Some(json!({"view": "original", "minified": false})), false),
            (None, false),
        ] {
            let output = format_read_result(
                &json!({"path": "notes.txt"}),
                ToolRenderResult {
                    content: &content,
                    details: details.as_ref(),
                },
                ToolRenderResultOptions {
                    expanded: true,
                    is_partial: false,
                },
                &theme,
                true,
                false,
            );
            assert_eq!(
                output.contains("Compaction unavailable"),
                should_mark,
                "only a compact read falling back to original text needs a marker: {output}"
            );
        }
    }

    #[test]
    fn advertises_its_description_and_schema() {
        let tool = create_read_tool_definition("/tmp", None);
        assert_eq!(tool.name(), "read");
        assert!(
            tool.description()
                .contains("truncated to 2000 lines or 50KB")
        );
        assert!(
            tool.prompt_snippet()
                .unwrap()
                .contains("keep_comments=true")
        );
        assert!(
            tool.prompt_guidelines()
                .join(" ")
                .contains("must set keep_comments=true")
        );
        assert!(tool.description().contains("must set keep_comments=true"));
        assert!(
            tool.parameters()["properties"]["keep_comments"]["description"]
                .as_str()
                .unwrap()
                .contains("comments")
        );
        assert_eq!(tool.parameters()["required"], json!(["path"]));
    }
}
