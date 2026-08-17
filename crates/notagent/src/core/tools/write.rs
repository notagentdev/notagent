//! Port of `packages/coding-agent/src/core/tools/write.ts` (tool half).

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_tui::components::text::Text;
use notagent_tui::tui::{ComponentRef, Container};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::tools::file_mutation_queue::with_file_mutation_queue;
use crate::core::tools::path_utils::resolve_to_cwd;
use crate::core::tools::render_utils::{
    call_title, normalize_display_text, render_tool_path, replace_tabs, str_arg,
};
use crate::core::tools::tool_definition::{
    SystemPromptContribution, ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult,
    ToolRenderResultOptions, tool_render_state, wrap_tool_definition,
};
use crate::modes::interactive::components::keybinding_hints::key_hint;
use crate::modes::interactive::theme::theme::{
    Theme, ThemeColor, get_language_from_path, highlight_code,
};

pub const WRITE_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Create or overwrite files",
        guidelines: &["Use write only for new files or complete rewrites."],
    };

const DESCRIPTION: &str = "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.";

fn write_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file to write (relative or absolute)" },
            "content": { "type": "string", "description": "Content to write to the file" },
        },
        "required": ["path", "content"],
    })
}

/// Pluggable operations, so writing can be delegated to a remote system.
pub trait WriteOperations: Send + Sync {
    fn write_file<'a>(
        &'a self,
        absolute_path: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn mkdir<'a>(&'a self, directory: &'a str) -> BoxFuture<'a, Result<(), String>>;
}

pub struct LocalWriteOperations;

impl WriteOperations for LocalWriteOperations {
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
pub struct WriteToolOptions {
    pub operations: Option<Arc<dyn WriteOperations>>,
}

pub struct WriteToolDefinition {
    cwd: String,
    operations: Arc<dyn WriteOperations>,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_write_tool_definition(
    cwd: &str,
    options: Option<WriteToolOptions>,
) -> WriteToolDefinition {
    let options = options.unwrap_or_default();
    WriteToolDefinition {
        cwd: cwd.to_owned(),
        operations: options
            .operations
            .unwrap_or_else(|| Arc::new(LocalWriteOperations)),
        parameters: write_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

// ============================================================================
// Rendering
// ============================================================================

/// The highlighted form of the content being written, kept across renders.
///
/// While the model streams the `content` argument, every render would otherwise
/// re-highlight the whole file. The cache highlights the appended text only and
/// refreshes the first [`WRITE_PARTIAL_FULL_HIGHLIGHT_LINES`] lines, which are
/// the ones a multi-line construct can still change.
struct WriteHighlightCache {
    raw_path: Option<String>,
    lang: String,
    raw_content: String,
    normalized_lines: Vec<String>,
    highlighted_lines: Vec<String>,
}

/// The row state of a `write` call: its component and its highlight cache.
///
/// TypeScript hangs the cache off the component (`WriteCallRenderComponent`);
/// both live for exactly one row, so the port keeps them together in the row
/// state (deviation class 1).
#[derive(Default)]
struct WriteRenderState {
    call: Option<Rc<RefCell<Text>>>,
    cache: Option<WriteHighlightCache>,
    /// The result is a `Text` when there is output and an empty `Container`
    /// otherwise. TypeScript reuses one slot for both and would throw if the
    /// kind ever changed within a row (`Text` has no `clear`); the port keeps
    /// one slot per kind, which is unreachable for `write` because a result is
    /// set once (deviation class 1).
    result_text: Option<Rc<RefCell<Text>>>,
    result_container: Option<Rc<RefCell<Container>>>,
}

const WRITE_PARTIAL_FULL_HIGHLIGHT_LINES: usize = 50;

fn highlight_single_line(line: &str, lang: &str) -> String {
    highlight_code(line, Some(lang))
        .first()
        .cloned()
        .unwrap_or_default()
}

fn refresh_write_highlight_prefix(cache: &mut WriteHighlightCache) {
    let prefix_count = WRITE_PARTIAL_FULL_HIGHLIGHT_LINES.min(cache.normalized_lines.len());
    if prefix_count == 0 {
        return;
    }
    let prefix_source = cache.normalized_lines[..prefix_count].join("\n");
    let prefix_highlighted = highlight_code(&prefix_source, Some(&cache.lang));
    for index in 0..prefix_count {
        cache.highlighted_lines[index] = match prefix_highlighted.get(index) {
            Some(line) => line.clone(),
            None => highlight_single_line(
                cache.normalized_lines.get(index).map_or("", String::as_str),
                &cache.lang,
            ),
        };
    }
}

fn rebuild_write_highlight_cache_full(
    raw_path: Option<&str>,
    file_content: &str,
) -> Option<WriteHighlightCache> {
    let lang = write_language(raw_path)?;
    let display_content = normalize_display_text(file_content);
    let normalized = replace_tabs(&display_content);
    Some(WriteHighlightCache {
        raw_path: raw_path.map(str::to_string),
        highlighted_lines: highlight_code(&normalized, Some(&lang)),
        normalized_lines: normalized.split('\n').map(str::to_string).collect(),
        raw_content: file_content.to_string(),
        lang,
    })
}

/// `rawPath ? getLanguageFromPath(rawPath) : undefined` — an empty path has no
/// language either, because `""` is falsy in TypeScript.
fn write_language(raw_path: Option<&str>) -> Option<String> {
    let raw_path = raw_path.filter(|path| !path.is_empty())?;
    get_language_from_path(raw_path)
}

fn update_write_highlight_cache_incremental(
    cache: Option<WriteHighlightCache>,
    raw_path: Option<&str>,
    file_content: &str,
) -> Option<WriteHighlightCache> {
    let lang = write_language(raw_path)?;
    let Some(mut cache) = cache else {
        return rebuild_write_highlight_cache_full(raw_path, file_content);
    };
    if cache.lang != lang || cache.raw_path.as_deref() != raw_path {
        return rebuild_write_highlight_cache_full(raw_path, file_content);
    }
    if !file_content.starts_with(&cache.raw_content) {
        return rebuild_write_highlight_cache_full(raw_path, file_content);
    }
    if file_content.len() == cache.raw_content.len() {
        return Some(cache);
    }

    let delta_raw = &file_content[cache.raw_content.len()..];
    let delta_normalized = replace_tabs(&normalize_display_text(delta_raw));
    cache.raw_content = file_content.to_string();
    if cache.normalized_lines.is_empty() {
        cache.normalized_lines.push(String::new());
        cache.highlighted_lines.push(String::new());
    }

    let segments: Vec<&str> = delta_normalized.split('\n').collect();
    let last_index = cache.normalized_lines.len() - 1;
    cache.normalized_lines[last_index] += segments[0];
    cache.highlighted_lines[last_index] =
        highlight_single_line(&cache.normalized_lines[last_index].clone(), &cache.lang);
    for segment in &segments[1..] {
        cache.normalized_lines.push((*segment).to_string());
        cache
            .highlighted_lines
            .push(highlight_single_line(segment, &cache.lang));
    }
    refresh_write_highlight_prefix(&mut cache);
    Some(cache)
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

/// `str(args?.file_path ?? args?.path)`
fn write_path_arg(args: &Value) -> Option<String> {
    let raw = args
        .get("file_path")
        .filter(|value| !value.is_null())
        .or_else(|| args.get("path"));
    str_arg(raw)
}

/// Preview rows of the content before the call is expanded.
const WRITE_PREVIEW_LINES: usize = 10;

fn format_write_call(
    args: &Value,
    options: ToolRenderResultOptions,
    theme: &Theme,
    cache: Option<&WriteHighlightCache>,
    cwd: &str,
) -> String {
    let raw_path = write_path_arg(args);
    let file_content = str_arg(args.get("content"));
    let path_display = render_tool_path(raw_path.as_deref(), theme, cwd, None);
    let mut text = format!("{}{path_display}", call_title(theme, "write"));

    let Some(file_content) = file_content else {
        text += &format!(
            "\n\n{}",
            theme.fg(ThemeColor::Error, "[invalid content arg - expected string]")
        );
        return text;
    };
    if file_content.is_empty() {
        return text;
    }

    let lang = write_language(raw_path.as_deref());
    let rendered_lines: Vec<String> = match (&lang, cache) {
        (Some(_), Some(cache)) => cache.highlighted_lines.clone(),
        (Some(lang), None) => highlight_code(
            &replace_tabs(&normalize_display_text(&file_content)),
            Some(lang),
        ),
        (None, _) => normalize_display_text(&file_content)
            .split('\n')
            .map(str::to_string)
            .collect(),
    };
    let lines = trim_trailing_empty_lines(rendered_lines);
    let total_lines = lines.len();
    let max_lines = if options.expanded {
        lines.len()
    } else {
        WRITE_PREVIEW_LINES
    };
    let display_lines = &lines[..max_lines.min(lines.len())];
    let remaining = lines.len() as isize - max_lines as isize;
    text += &format!(
        "\n\n{}",
        display_lines
            .iter()
            .map(|line| match &lang {
                Some(_) => line.clone(),
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
                &format!("\n... ({remaining} more lines, {total_lines} total,")
            ),
            key_hint("app.tools.expand", "to expand"),
            theme.fg(ThemeColor::Muted, ")")
        );
    }
    text
}

/// Only an error is shown; a successful write speaks through its header.
fn format_write_result(
    result: ToolRenderResult<'_>,
    is_error: bool,
    theme: &Theme,
) -> Option<String> {
    if !is_error {
        return None;
    }
    let output = result
        .content
        .iter()
        .filter_map(|block| match block {
            notagent_ai::types::TextOrImageContent::Text(text) => Some(text.text.clone()),
            notagent_ai::types::TextOrImageContent::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if output.is_empty() {
        return None;
    }
    Some(format!("\n{}", theme.fg(ThemeColor::Error, &output)))
}

impl ToolDefinition for WriteToolDefinition {
    fn name(&self) -> &str {
        "write"
    }

    fn label(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(WRITE_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        WRITE_TOOL_SYSTEM_PROMPT_CONTRIBUTION
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
        let mut state = tool_render_state::<WriteRenderState>(&context.state);
        let raw_path = write_path_arg(args);
        let file_content = str_arg(args.get("content"));
        state.cache = match file_content {
            // A complete argument is highlighted in one go; a streaming one
            // extends the cache instead.
            Some(file_content) => {
                if context.args_complete {
                    rebuild_write_highlight_cache_full(raw_path.as_deref(), &file_content)
                } else {
                    update_write_highlight_cache_incremental(
                        state.cache.take(),
                        raw_path.as_deref(),
                        &file_content,
                    )
                }
            }
            None => None,
        };
        let text = format_write_call(
            args,
            ToolRenderResultOptions {
                expanded: context.expanded,
                is_partial: context.is_partial,
            },
            theme,
            state.cache.as_ref(),
            &context.cwd,
        );
        let component = state
            .call
            .get_or_insert_with(|| Rc::new(RefCell::new(Text::new("", 0, 0))));
        component.borrow_mut().set_text(text);
        Some(Rc::clone(component) as ComponentRef)
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        _options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let mut state = tool_render_state::<WriteRenderState>(&context.state);
        let Some(output) = format_write_result(result, context.is_error, theme) else {
            let component = state
                .result_container
                .get_or_insert_with(|| Rc::new(RefCell::new(Container::new())));
            component.borrow_mut().clear();
            return Some(Rc::clone(component) as ComponentRef);
        };
        let component = state
            .result_text
            .get_or_insert_with(|| Rc::new(RefCell::new(Text::new("", 0, 0))));
        component.borrow_mut().set_text(output);
        Some(Rc::clone(component) as ComponentRef)
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
            let path = params
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let content = params
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let absolute_path = resolve_to_cwd(&path, &self.cwd);
            let directory = Path::new(&absolute_path)
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".to_owned());

            with_file_mutation_queue(&absolute_path, async {
                // Do not abort from a listener: that would release the mutation
                // queue while a filesystem call is still in flight. Checking the
                // signal after each await observes the same aborts and keeps the
                // queue locked until the current operation has settled.
                let throw_if_aborted = || -> Result<(), ToolExecutionError> {
                    match &signal {
                        Some(signal) if signal.is_cancelled() => {
                            Err(ToolExecutionError::new("Operation aborted"))
                        }
                        _ => Ok(()),
                    }
                };

                throw_if_aborted()?;
                self.operations
                    .mkdir(&directory)
                    .await
                    .map_err(ToolExecutionError::new)?;
                throw_if_aborted()?;
                self.operations
                    .write_file(&absolute_path, &content)
                    .await
                    .map_err(ToolExecutionError::new)?;
                throw_if_aborted()?;

                // `content.length` in JS counts UTF-16 units, not bytes.
                let length = content.encode_utf16().count();
                Ok(AgentToolResult {
                    content: vec![TextOrImageContent::Text(TextContent::new(format!(
                        "Successfully wrote {length} bytes to {path}"
                    )))],
                    details: None,
                    usage: None,
                    added_tool_names: None,
                    terminate: None,
                })
            })
            .await
        })
    }
}

/// `createCreateWriteTool` — the tool as the agent loop takes it.
pub fn create_write_tool(cwd: &str, options: Option<WriteToolOptions>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_write_tool_definition(cwd, options)), None)
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
                .prefix("notagent-write-tool-")
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
    async fn writes_a_file_and_creates_parent_directories() {
        let directory = TempDir::new();
        let tool = create_write_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute(
                "call-1",
                json!({ "path": "nested/dir/file.txt", "content": "hello" }),
                None,
                None,
                None,
            )
            .await
            .expect("write");
        assert_eq!(
            text_of(&result),
            "Successfully wrote 5 bytes to nested/dir/file.txt"
        );
        let written =
            std::fs::read_to_string(directory.path.join("nested/dir/file.txt")).expect("read");
        assert_eq!(written, "hello");
    }

    #[tokio::test]
    async fn overwrites_an_existing_file() {
        let directory = TempDir::new();
        let file = directory.path.join("file.txt");
        std::fs::write(&file, "old").expect("write");
        let tool = create_write_tool_definition(&directory.cwd(), None);
        tool.execute(
            "call-1",
            json!({ "path": "file.txt", "content": "new" }),
            None,
            None,
            None,
        )
        .await
        .expect("write");
        assert_eq!(std::fs::read_to_string(&file).expect("read"), "new");
    }

    #[tokio::test]
    async fn counts_utf16_units_like_the_javascript_string_length() {
        let directory = TempDir::new();
        let tool = create_write_tool_definition(&directory.cwd(), None);
        // "🎉" is one char, two UTF-16 units and four bytes.
        let result = tool
            .execute(
                "call-1",
                json!({ "path": "emoji.txt", "content": "🎉" }),
                None,
                None,
                None,
            )
            .await
            .expect("write");
        assert_eq!(text_of(&result), "Successfully wrote 2 bytes to emoji.txt");
    }

    #[tokio::test]
    async fn refuses_to_run_when_the_signal_is_already_aborted() {
        let directory = TempDir::new();
        let tool = create_write_tool_definition(&directory.cwd(), None);
        let signal = CancellationToken::new();
        signal.cancel();
        let error = tool
            .execute(
                "call-1",
                json!({ "path": "file.txt", "content": "x" }),
                Some(signal),
                None,
                None,
            )
            .await
            .expect_err("aborted");
        assert_eq!(error.message, "Operation aborted");
        assert!(!directory.path.join("file.txt").exists());
    }

    #[test]
    fn advertises_its_schema_and_prompt_contribution() {
        let tool = create_write_tool_definition("/tmp", None);
        assert_eq!(tool.name(), "write");
        assert_eq!(tool.label(), "write");
        assert_eq!(tool.description(), DESCRIPTION);
        assert_eq!(tool.prompt_snippet(), Some("Create or overwrite files"));
        assert_eq!(
            tool.prompt_guidelines(),
            vec!["Use write only for new files or complete rewrites.".to_owned()]
        );
        assert_eq!(tool.parameters()["required"], json!(["path", "content"]));
    }
}
