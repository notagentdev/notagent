//! Port of the TUI-free half of
//! `packages/coding-agent/src/core/export-html/tool-renderer.ts` (172 LOC).
//!
//! `createToolHtmlRenderer` itself is not here: it calls `renderCall`/
//! `renderResult` of a tool definition, which return TUI components. Those two
//! methods are not on `core::tools::tool_definition::ToolDefinition` — they are
//! wired up with the interactive mode (workstream C, task 13), and the factory
//! belongs to that wiring (interface request B-8). What stays here is the seam
//! the exporter talks to plus the two pure helpers the factory needs: the blank
//! line trimming and, in the sibling module, the ANSI conversion.

use serde_json::Value;

use super::ansi_to_html::ansi_lines_to_html;
use crate::utils::ansi::strip_ansi;

/// The collapsed/expanded pair `renderResult` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderedToolResult {
    pub collapsed: Option<String>,
    pub expanded: Option<String>,
}

/// Interface for rendering custom tools to HTML.
/// Used by agent-session to pre-render extension tool output.
pub trait ToolHtmlRenderer {
    /// Render a tool call to HTML. Returns `None` if the tool has no custom renderer.
    fn render_call(&self, tool_call_id: &str, tool_name: &str, args: &Value) -> Option<String>;
    /// Render a tool result to HTML. Returns collapsed/expanded, or `None` if the
    /// tool has no custom renderer. `details` is `Value::Null` when the message
    /// carries none, which is what the TypeScript passes as `undefined`.
    fn render_result(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        result: &[Value],
        details: &Value,
        is_error: bool,
    ) -> Option<RenderedToolResult>;
}

fn is_blank_rendered_line(line: &str) -> bool {
    strip_ansi(line).trim().is_empty()
}

/// `trimRenderedResultLines(lines)` — drop the leading and trailing spacing
/// lines a TUI component renders around its content.
pub fn trim_rendered_result_lines<S: AsRef<str> + Clone>(lines: &[S]) -> Vec<S> {
    let mut start = 0usize;
    let mut end = lines.len();
    while start < end && is_blank_rendered_line(lines[start].as_ref()) {
        start += 1;
    }
    while end > start && is_blank_rendered_line(lines[end - 1].as_ref()) {
        end -= 1;
    }
    lines[start..end].to_vec()
}

/// The collapsed/expanded pair of a rendered result, built from the two line
/// sets a component produced. Mirrors the tail of `renderResult`: a collapsed
/// rendering equal to the expanded one is dropped.
pub fn rendered_result_from_lines<S: AsRef<str> + Clone>(
    collapsed_lines: &[S],
    expanded_lines: &[S],
) -> RenderedToolResult {
    let collapsed = ansi_lines_to_html(&trim_rendered_result_lines(collapsed_lines));
    let expanded = ansi_lines_to_html(&trim_rendered_result_lines(expanded_lines));
    RenderedToolResult {
        collapsed: (!collapsed.is_empty() && collapsed != expanded).then_some(collapsed),
        expanded: Some(expanded),
    }
}
