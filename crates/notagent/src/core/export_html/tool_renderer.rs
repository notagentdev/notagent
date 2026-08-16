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

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use notagent_ai::types::TextOrImageContent;
use notagent_tui::tui::ComponentRef;
use serde_json::Value;

use super::ansi_to_html::ansi_lines_to_html;
use crate::core::tools::tool_definition::{
    ToolDefinition, ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
    ToolRenderStateRef, new_tool_render_state,
};
use crate::modes::interactive::theme::theme::Theme;
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

/// `createToolHtmlRenderer(deps)` (`tool-renderer.ts:58-171`).
///
/// Interface request B-8 left the factory to workstream C, because it drives
/// `renderCall`/`renderResult` of a tool definition — the halves that render
/// TUI components and only exist once the interactive components are ported.
/// The seam above and the two helpers stay where B put them.
pub type GetToolDefinition = Box<dyn Fn(&str) -> Option<std::sync::Arc<dyn ToolDefinition>>>;

pub struct ToolDefinitionHtmlRenderer {
    get_tool_definition: GetToolDefinition,
    theme: std::sync::Arc<Theme>,
    cwd: String,
    width: usize,
    /// `renderedArgs`, `renderedStates` and the two component maps of the
    /// TypeScript closure.
    args: RefCell<BTreeMap<String, Value>>,
    states: RefCell<BTreeMap<String, ToolRenderStateRef>>,
    call_components: RefCell<BTreeMap<String, ComponentRef>>,
    result_components: RefCell<BTreeMap<String, ComponentRef>>,
}

/// `width = 100` of `ToolHtmlRendererDeps`.
const DEFAULT_WIDTH: usize = 100;

impl ToolDefinitionHtmlRenderer {
    pub fn new(
        get_tool_definition: GetToolDefinition,
        theme: std::sync::Arc<Theme>,
        cwd: String,
    ) -> Self {
        Self {
            get_tool_definition,
            theme,
            cwd,
            width: DEFAULT_WIDTH,
            args: RefCell::new(BTreeMap::new()),
            states: RefCell::new(BTreeMap::new()),
            call_components: RefCell::new(BTreeMap::new()),
            result_components: RefCell::new(BTreeMap::new()),
        }
    }

    /// `getState(toolCallId)`
    fn state(&self, tool_call_id: &str) -> ToolRenderStateRef {
        let mut states = self.states.borrow_mut();
        Rc::clone(
            states
                .entry(tool_call_id.to_owned())
                .or_insert_with(new_tool_render_state),
        )
    }

    /// `createRenderContext(...)`
    fn context(
        &self,
        tool_call_id: &str,
        last_component: Option<ComponentRef>,
        expanded: bool,
        is_partial: bool,
        is_error: bool,
    ) -> ToolRenderContext {
        ToolRenderContext {
            args: self
                .args
                .borrow()
                .get(tool_call_id)
                .cloned()
                .unwrap_or(Value::Null),
            tool_call_id: tool_call_id.to_owned(),
            invalidate: Rc::new(|| {}),
            last_component,
            state: self.state(tool_call_id),
            cwd: self.cwd.clone(),
            execution_started: true,
            args_complete: true,
            is_partial,
            expanded,
            show_images: false,
            is_error,
        }
    }
}

impl ToolHtmlRenderer for ToolDefinitionHtmlRenderer {
    fn render_call(&self, tool_call_id: &str, tool_name: &str, args: &Value) -> Option<String> {
        self.args
            .borrow_mut()
            .insert(tool_call_id.to_owned(), args.clone());
        let definition = (self.get_tool_definition)(tool_name)?;
        let last = self.call_components.borrow().get(tool_call_id).cloned();
        let context = self.context(tool_call_id, last, false, true, false);
        let component = definition.render_call(args, &self.theme, &context)?;
        self.call_components
            .borrow_mut()
            .insert(tool_call_id.to_owned(), Rc::clone(&component));
        let lines = component.borrow_mut().render(self.width);
        Some(ansi_lines_to_html(&lines))
    }

    fn render_result(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        result: &[Value],
        details: &Value,
        is_error: bool,
    ) -> Option<RenderedToolResult> {
        let definition = (self.get_tool_definition)(tool_name)?;
        // The session file stores the blocks as plain JSON; anything that is not
        // a text or image block is dropped, as the TS cast quietly does.
        let content: Vec<TextOrImageContent> = result
            .iter()
            .filter_map(|block| serde_json::from_value(block.clone()).ok())
            .collect();
        let details = (!details.is_null()).then_some(details);

        let render = |expanded: bool| -> Option<Vec<String>> {
            let last = self.result_components.borrow().get(tool_call_id).cloned();
            let context = self.context(tool_call_id, last, expanded, false, is_error);
            let component = definition.render_result(
                ToolRenderResult {
                    content: &content,
                    details,
                },
                ToolRenderResultOptions {
                    expanded,
                    is_partial: false,
                },
                &self.theme,
                &context,
            )?;
            self.result_components
                .borrow_mut()
                .insert(tool_call_id.to_owned(), Rc::clone(&component));
            Some(component.borrow_mut().render(self.width))
        };

        let collapsed = render(false)?;
        let expanded = render(true)?;
        Some(rendered_result_from_lines(&collapsed, &expanded))
    }
}
