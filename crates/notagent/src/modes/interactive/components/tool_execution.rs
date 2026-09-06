use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use notagent_ai::types::TextOrImageContent;
use notagent_tui::components::box_component::BoxComponent;
use notagent_tui::components::image::{Image, ImageOptions, ImageTheme};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::terminal_image::{ImageProtocol, get_capabilities};
use notagent_tui::tui::{Component, ComponentRef, Container, Line, component_ref};
use serde_json::Value;
use similar::{ChangeTag, TextDiff};

use crate::core::tools::bash::format_bash_badge_running_suffix;
use crate::core::tools::render_utils::get_text_output;
use crate::core::tools::tool_definition::{
    RenderShell, ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
    new_tool_render_state,
};
use crate::core::tools::{ToolDef, ToolName, create_all_tool_definitions};
use crate::modes::interactive::components::keybinding_hints::key_text;
use crate::modes::interactive::theme::theme::{
    BlockStyle, ThemeBg, ThemeColor, badge, block_style, theme,
};
use crate::utils::image::convert_to_png;

/// The badge-style block header (takeover of the reference's
/// `BadgeCallHeader`): the state badge and the first line of what follows
/// share one row, and the remaining lines follow underneath. Content is
/// wrapped to the space remaining beside the badge so a non-empty first
/// fragment does not move needlessly to the next row. Empty content, or a
/// badge that consumes the full width, falls back to the badge on its own
/// row.
/// The hoisted line is the call when the tool renders one — `BASH ($ ls -la)`
/// — and otherwise the result, which is set without the parentheses because
/// it is a statement rather than an argument.
struct BadgeCallHeader {
    badge: String,
    call: ComponentRef,
    /// Whether the hoisted line is parenthesised — true for a call line.
    parens: bool,
    /// Metadata that follows the complete call, such as diff counts or the
    /// compact running time of a foreground command.
    suffix: String,
}

/// A line below the badge row, moved under the badge's first character.
/// The badge pill carries one space of padding on each side, and every block
/// that stacks content under a badge keeps that column (the explore block
/// prefixes its rows, the thinking block renders at `output_pad`). Continuation
/// lines at the margin would sit one column out of line with the row above.
fn under_badge(line: &Line, width: usize) -> Line {
    use notagent_tui::utils::{slice_by_column, visible_width};
    if width == 0 {
        return Line::from("");
    }
    if visible_width(line) == 0 {
        return Line::clone(line);
    }
    Line::from(format!(
        " {}",
        slice_by_column(line, 0, width.saturating_sub(1), true)
    ))
}

impl Component for BadgeCallHeader {
    fn render(&mut self, width: usize) -> Vec<Line> {
        use notagent_tui::utils::visible_width;
        let theme_instance = theme();
        let suffix = if self.suffix.is_empty() {
            String::new()
        } else {
            format!(" {}", self.suffix)
        };
        // One space after the badge, plus the two parentheses when they are
        // drawn, and the metadata suffix. Render against the remaining width
        // so the suffix stays behind the call instead of displacing its first
        // fragment.
        let overhead =
            visible_width(&self.badge) + if self.parens { 3 } else { 1 } + visible_width(&suffix);
        if let Some(call_width) = width
            .checked_sub(overhead)
            .filter(|call_width| *call_width > 0)
        {
            let mut lines = self.call.borrow_mut().render(call_width);
            let has_first_line = lines
                .first()
                .is_some_and(|first| visible_width(first.trim_end()) > 0);
            if has_first_line {
                let first = lines.remove(0);
                let head = if self.parens {
                    format!(
                        "{} {}{}{}{}",
                        self.badge,
                        theme_instance.fg(ThemeColor::Dim, "("),
                        first.trim_end(),
                        theme_instance.fg(ThemeColor::Dim, ")"),
                        suffix,
                    )
                } else {
                    format!("{} {}{}", self.badge, first.trim_end(), suffix)
                };
                let mut out = vec![Line::from(head)];
                out.extend(lines.iter().map(|line| under_badge(line, width)));
                return out;
            }
        }
        let continuation_width = width.saturating_sub(1);
        let lines = if continuation_width == 0 {
            Vec::new()
        } else {
            self.call.borrow_mut().render(continuation_width)
        };
        let mut out = vec![Line::from(notagent_tui::utils::slice_by_column(
            &self.badge,
            0,
            width,
            true,
        ))];
        if lines.iter().any(|line| visible_width(line.trim_end()) > 0) {
            out.extend(lines.iter().map(|line| under_badge(line, width)));
        }
        if !self.suffix.is_empty() {
            out.push(under_badge(&Line::from(self.suffix.as_str()), width));
        }
        out
    }

    fn invalidate(&mut self) {
        self.call.borrow_mut().invalidate();
    }
}

/// Options of a tool row.
#[derive(Debug, Clone, Default)]
pub struct ToolExecutionOptions {
    /// Whether images in the result are drawn (default `true`).
    pub show_images: Option<bool>,
    /// Width limit for those images in cells (default 60).
    pub image_width_cells: Option<usize>,
}

/// The result half of a tool row.
#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub content: Vec<TextOrImageContent>,
    pub details: Option<Value>,
    pub is_error: bool,
}

/// One tool call in the transcript.
pub struct ToolExecutionComponent {
    container: Container,
    content_box: Rc<RefCell<BoxComponent>>,
    content_text: Rc<RefCell<Text>>,
    self_render_container: Rc<RefCell<Container>>,
    call_renderer_component: Option<ComponentRef>,
    result_renderer_component: Option<ComponentRef>,
    renderer_state: crate::core::tools::tool_definition::ToolRenderStateRef,
    image_components: Vec<ComponentRef>,
    image_spacers: Vec<ComponentRef>,
    tool_name: String,
    tool_call_id: String,
    args: Value,
    expanded: bool,
    show_images: bool,
    image_width_cells: usize,
    is_partial: bool,
    tool_definition: Option<ToolDef>,
    built_in_tool_definition: Option<ToolDef>,
    request_render: Rc<dyn Fn()>,
    cwd: String,
    execution_started: bool,
    execution_started_at: Option<Instant>,
    args_complete: bool,
    result: Option<ToolExecutionResult>,
    converted_images: HashMap<usize, (String, String)>,
    hide_component: bool,
    /// Set by the render context's `invalidate`; consumed by the next `render`.
    dirty: Rc<Cell<bool>>,
    /// The block style the current layout was built for; a mismatch at render
    /// time rebuilds, so a live style switch restyles the row.
    built_style: BlockStyle,
}

fn patch_old_new(args: &Value) -> (String, String) {
    let string_field = |value: &Value, key: &str, alias: &str| {
        value
            .get(key)
            .or_else(|| value.get(alias))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    if let Some(edits) = args.get("edits").and_then(Value::as_array) {
        let old = edits
            .iter()
            .map(|edit| string_field(edit, "old_string", "search"))
            .collect::<Vec<_>>()
            .join("\n");
        let new = edits
            .iter()
            .map(|edit| string_field(edit, "new_string", "content"))
            .collect::<Vec<_>>()
            .join("\n");
        (old, new)
    } else {
        (
            string_field(args, "old_string", "search"),
            string_field(args, "new_string", "content"),
        )
    }
}

impl ToolExecutionComponent {
    /// `new ToolExecutionComponent(toolName, toolCallId, args, options, toolDefinition, ui, cwd)`.
    pub fn new(
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
        args: Value,
        options: ToolExecutionOptions,
        tool_definition: Option<ToolDef>,
        request_render: Rc<dyn Fn()>,
        cwd: impl Into<String>,
    ) -> Self {
        let tool_name = tool_name.into();
        let cwd = cwd.into();
        let built_in_tool_definition = ToolName::parse(&tool_name)
            .and_then(|name| create_all_tool_definitions(&cwd, None).remove(&name));

        // Always create all shell variants. `content_box` is used for default
        // renderer-based composition, `self_render_container` when the tool
        // renders its own framing, and `content_text` is the generic fallback
        // for a tool without any definition.
        let content_box = Rc::new(RefCell::new(BoxComponent::new(
            1,
            1,
            Some(Rc::new(|text: &str| {
                theme().bg(ThemeBg::ToolPendingBg, text)
            })),
        )));
        let content_text = Rc::new(RefCell::new(Text::new("", 1, 1)));
        content_text
            .borrow_mut()
            .set_custom_bg_fn(Some(Rc::new(|text: &str| {
                theme().bg(ThemeBg::ToolPendingBg, text)
            })));
        let self_render_container = Rc::new(RefCell::new(Container::new()));

        let mut container = Container::new();
        container.add_child(component_ref(Spacer::new(1)));

        let mut component = Self {
            container,
            content_box,
            content_text,
            self_render_container,
            call_renderer_component: None,
            result_renderer_component: None,
            renderer_state: new_tool_render_state(),
            image_components: Vec::new(),
            image_spacers: Vec::new(),
            tool_name,
            tool_call_id: tool_call_id.into(),
            args,
            expanded: false,
            show_images: options.show_images.unwrap_or(true),
            image_width_cells: options.image_width_cells.unwrap_or(60),
            is_partial: true,
            tool_definition,
            built_in_tool_definition,
            request_render,
            cwd,
            execution_started: false,
            execution_started_at: None,
            args_complete: false,
            result: None,
            converted_images: HashMap::new(),
            hide_component: false,
            dirty: Rc::new(Cell::new(false)),
            built_style: block_style(),
        };

        component.update_display();
        component
    }

    /// Whether the block renders in the badge style right now. The
    /// self-managed standard shell keeps its own framing; the badge style
    /// routes every tool — the self-framed edit included — through the
    /// badge header (reference `tool_execution.rs`).
    fn badge_style(&self) -> bool {
        block_style() == BlockStyle::Badge
    }

    /// The state badge of this block: the tool's name, uppercased, on the fill
    /// the standard style would wash the whole block with.
    fn badge_for(&self, state: ThemeBg) -> String {
        badge(&theme(), state, &self.tool_name)
    }

    /// Metadata belongs after the call arguments, never inside the tool-name
    /// badge. This keeps every badge row in the same `NAME (arguments) suffix`
    /// order.
    fn badge_suffix(&self) -> String {
        let theme = theme();
        if let Some((added, removed)) = self.diff_stats() {
            let mut parts = Vec::new();
            if added > 0 {
                parts.push(theme.fg(ThemeColor::ToolDiffAdded, &format!("+{added}")));
            }
            if removed > 0 {
                parts.push(theme.fg(ThemeColor::ToolDiffRemoved, &format!("-{removed}")));
            }
            return parts.join(" ");
        }

        let foreground_bash = self.tool_name == "bash"
            && !self
                .args
                .get("run_in_background")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let still_running =
            self.is_partial && !self.result.as_ref().is_some_and(|result| result.is_error);
        if foreground_bash
            && still_running
            && let Some(started_at) = self.execution_started_at
        {
            return theme.fg(
                ThemeColor::Muted,
                &format_bash_badge_running_suffix(&self.args, started_at.elapsed()),
            );
        }
        String::new()
    }

    fn diff_stats(&self) -> Option<(usize, usize)> {
        let (old, new) = match self.tool_name.as_str() {
            "write" => (
                String::new(),
                self.args
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            "patch" | "patch_minified" | "multi_patch" | "multi_patch_minified" => {
                patch_old_new(&self.args)
            }
            _ => return None,
        };

        let mut added = 0;
        let mut removed = 0;
        for change in TextDiff::from_lines(&old, &new).iter_all_changes() {
            match change.tag() {
                ChangeTag::Insert => added += 1,
                ChangeTag::Delete => removed += 1,
                ChangeTag::Equal => {}
            }
        }
        Some((added, removed))
    }

    /// Whether the result holds output the expand toggle controls.
    fn result_is_expandable(&self) -> bool {
        self.result.is_some() && !self.text_output().trim().is_empty()
    }

    fn has_renderer_definition(&self) -> bool {
        self.built_in_tool_definition.is_some() || self.tool_definition.is_some()
    }

    fn render_shell(&self) -> RenderShell {
        match (&self.built_in_tool_definition, &self.tool_definition) {
            (None, Some(definition)) => definition.render_shell(),
            (Some(built_in), None) => built_in.render_shell(),
            (Some(built_in), Some(definition)) => match definition.render_shell() {
                RenderShell::Default => built_in.render_shell(),
                shell => shell,
            },
            (None, None) => RenderShell::Default,
        }
    }

    /// Ask the custom definition first, the built-in one second.
    /// (`toolDefinition.renderCall ?? builtIn.renderCall`) and falls back to the
    /// plain header when the picked one throws. A `dyn ToolDefinition` cannot be
    /// asked whether it overrides a renderer — not implementing it and declining
    /// to draw are the same `None` — so the built-in renderer also steps in for a
    /// custom renderer that declines. Both paths end in a drawn row; only which
    /// of the two draws it differs, and only for a custom tool that overrides a
    /// built-in name and then refuses to render.
    fn definitions_in_order(&self) -> impl Iterator<Item = &ToolDef> {
        self.tool_definition
            .iter()
            .chain(self.built_in_tool_definition.iter())
    }

    fn render_context(&self, last_component: Option<ComponentRef>) -> ToolRenderContext {
        let dirty = Rc::clone(&self.dirty);
        let request_render = Rc::clone(&self.request_render);
        ToolRenderContext {
            args: self.args.clone(),
            tool_call_id: self.tool_call_id.clone(),
            invalidate: Rc::new(move || {
                dirty.set(true);
                request_render();
            }),
            last_component,
            state: Rc::clone(&self.renderer_state),
            cwd: self.cwd.clone(),
            execution_started: self.execution_started,
            args_complete: self.args_complete,
            is_partial: self.is_partial,
            expanded: self.expanded,
            show_images: self.show_images,
            is_error: self.result.as_ref().is_some_and(|result| result.is_error),
        }
    }

    fn create_call_fallback(&self) -> ComponentRef {
        let theme_instance = theme();
        component_ref(Text::new(
            theme_instance.fg(ThemeColor::ToolTitle, &theme_instance.bold(&self.tool_name)),
            0,
            0,
        ))
    }

    fn create_result_fallback(&self) -> Option<ComponentRef> {
        let output = self.text_output();
        if output.is_empty() {
            return None;
        }
        Some(component_ref(Text::new(
            theme().fg(ThemeColor::ToolOutput, &output),
            0,
            0,
        )))
    }

    pub fn update_args(&mut self, args: Value) {
        self.args = args;
        self.update_display();
    }

    pub fn mark_execution_started(&mut self) {
        self.execution_started = true;
        self.execution_started_at.get_or_insert_with(Instant::now);
        self.update_display();
        (self.request_render)();
    }

    pub fn set_args_complete(&mut self) {
        self.args_complete = true;
        self.update_display();
        (self.request_render)();
    }

    pub fn update_result(&mut self, result: ToolExecutionResult, is_partial: bool) {
        self.result = Some(result);
        self.is_partial = is_partial;
        self.update_display();
        self.maybe_convert_images_for_kitty();
    }

    /// Kitty's graphics protocol only takes PNG, so anything else is converted.
    /// WASM worker; here the `image` crate decodes in process, and the row is
    /// redrawn right after, so the conversion runs inline.
    fn maybe_convert_images_for_kitty(&mut self) {
        if get_capabilities().images != Some(ImageProtocol::Kitty) {
            return;
        }
        let Some(result) = self.result.as_ref() else {
            return;
        };

        let mut converted: Vec<(usize, (String, String))> = Vec::new();
        for (index, image) in image_blocks(&result.content).enumerate() {
            if image.mime_type == "image/png" || self.converted_images.contains_key(&index) {
                continue;
            }
            if let Some(png) = convert_to_png(&image.data, &image.mime_type) {
                converted.push((index, png));
            }
        }
        if converted.is_empty() {
            return;
        }
        self.converted_images.extend(converted);
        self.update_display();
        (self.request_render)();
    }

    /// When the row has render work of its own that is due later
    /// (`ToolDefinition::render_deadline`).
    /// The renderers of a row cannot drive their own timers — a callback would
    /// need `&mut` on the row while the renderer holds it — so they report the
    /// moment and the render loop comes back for it, exactly as
    /// `Loader::next_frame_deadline` and `Editor::autocomplete_deadline` do.
    pub fn render_deadline(&self) -> Option<Instant> {
        let context = self.render_context(self.call_renderer_component.clone());
        self.definitions_in_order()
            .filter_map(|definition| definition.render_deadline(&context))
            .min()
    }

    /// The render-side work this row's renderers handed back, detached from
    /// the row.
    /// The component cannot lend out a future that borrows it: the loop would
    /// have to hold the row's `RefCell` borrow across the await, while the work
    /// invalidates that very row when it finishes. `ToolDef` is an `Arc` and the
    /// context is owned, so both travel with the work instead.
    pub fn render_work(&self) -> RowRenderWork {
        RowRenderWork {
            definitions: self.definitions_in_order().cloned().collect(),
            context: self.render_context(self.call_renderer_component.clone()),
        }
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.update_display();
    }

    pub fn set_show_images(&mut self, show: bool) {
        self.show_images = show;
        self.update_display();
    }

    pub fn set_image_width_cells(&mut self, width: usize) {
        self.image_width_cells = width.max(1);
        self.update_display();
    }

    fn update_display(&mut self) {
        self.dirty.set(false);
        self.built_style = block_style();
        let is_partial = self.is_partial;
        let is_error = self.result.as_ref().is_some_and(|result| result.is_error);
        let bg_slot = if is_partial {
            ThemeBg::ToolPendingBg
        } else if is_error {
            ThemeBg::ToolErrorBg
        } else {
            ThemeBg::ToolSuccessBg
        };
        let bg_fn: Rc<dyn Fn(&str) -> String> =
            Rc::new(move |text: &str| theme().bg(bg_slot, text));

        // In the badge style the block sheds surface and padding rows — the
        // state moves entirely into the tool-name badge, which shares its
        // line with the call header (reference `tool_execution.rs`). The
        // container is rebuilt so a live style switch restyles the row.
        let badge_style = self.badge_style();
        self.container.clear();
        self.container.add_child(component_ref(Spacer::new(1)));

        // `hide_component` case is therefore as unreachable here as it is there
        // (bug-compat, not a simplification).
        #[allow(unused_assignments)]
        let mut has_content = false;
        self.hide_component = false;
        if self.has_renderer_definition() {
            // The badge style routes every tool — the self-framed edit
            // included — through the badge header path.
            let self_managed = self.render_shell() == RenderShell::SelfManaged && !badge_style;
            if self_managed {
                self.self_render_container.borrow_mut().clear();
                self.container
                    .add_child(Rc::clone(&self.self_render_container) as ComponentRef);
            } else {
                let mut content_box = self.content_box.borrow_mut();
                // The badge carries the state on its own — no surface, no
                // padding rows, no inset: the badge row starts at the margin.
                content_box.set_padding(
                    if badge_style { 0 } else { 1 },
                    if badge_style { 0 } else { 1 },
                );
                content_box.set_bg_fn(if badge_style {
                    None
                } else {
                    Some(Rc::clone(&bg_fn))
                });
                content_box.clear();
                drop(content_box);
                self.container
                    .add_child(Rc::clone(&self.content_box) as ComponentRef);
            }

            let theme_instance = theme();
            let context = self.render_context(self.call_renderer_component.clone());
            let call_component = self.definitions_in_order().find_map(|definition| {
                definition.render_call(&self.args, &theme_instance, &context)
            });
            let call_component = match call_component {
                Some(component) => {
                    self.call_renderer_component = Some(Rc::clone(&component));
                    Some(component)
                }
                None if badge_style => {
                    // The badge already names the tool; without a rendered
                    // call the badge row takes the result's first line
                    // instead of a second name line.
                    self.call_renderer_component = None;
                    None
                }
                None => {
                    self.call_renderer_component = None;
                    Some(self.create_call_fallback())
                }
            };

            let mut result_component = if let Some(result) = self.result.clone() {
                let context = self.render_context(self.result_renderer_component.clone());
                let rendered = self.definitions_in_order().find_map(|definition| {
                    definition.render_result(
                        ToolRenderResult {
                            content: &result.content,
                            details: result.details.as_ref(),
                        },
                        ToolRenderResultOptions {
                            expanded: self.expanded,
                            is_partial: self.is_partial,
                        },
                        &theme_instance,
                        &context,
                    )
                });
                match rendered {
                    Some(component) => {
                        self.result_renderer_component = Some(Rc::clone(&component));
                        Some(component)
                    }
                    None => {
                        self.result_renderer_component = None;
                        self.create_result_fallback()
                    }
                }
            } else {
                None
            };

            if badge_style {
                // The badge shares its row with the first line of what
                // follows — `BASH ($ ls -la)` for a rendered call, and
                // otherwise the result, so that a tool whose whole output is
                // one line costs one row instead of two.
                let badge = self.badge_for(bg_slot);
                let suffix = self.badge_suffix();
                let head = match call_component {
                    Some(call) => Some((call, true)),
                    None => result_component.take().map(|result| (result, false)),
                };
                match head {
                    Some((content, parens)) => {
                        self.add_to_render_container(
                            self_managed,
                            component_ref(BadgeCallHeader {
                                badge,
                                call: content,
                                parens,
                                suffix,
                            }),
                        );
                    }
                    None => {
                        self.add_to_render_container(
                            self_managed,
                            component_ref(Text::new(badge, 0, 0)),
                        );
                    }
                }
                has_content = true;
            } else if let Some(call) = call_component {
                self.add_to_render_container(self_managed, call);
                has_content = true;
            }

            if let Some(component) = result_component {
                // The result is a sibling of the badge row, so it takes the
                // same column the call's continuation lines get from
                // `under_badge`.
                let component = if badge_style {
                    let mut inset = BoxComponent::new(1, 0, None);
                    inset.add_child(component);
                    component_ref(inset)
                } else {
                    component
                };
                self.add_to_render_container(self_managed, component);
                has_content = true;
            }

            // In the badge style an expanded result closes with its info
            // line (reference `tool_execution.rs`).
            if badge_style && self.expanded && self.result_is_expandable() {
                self.add_to_render_container(
                    self_managed,
                    component_ref(Text::new(
                        theme().fg(
                            ThemeColor::Muted,
                            &format!("({} to collapse)", key_text("app.tools.expand")),
                        ),
                        1,
                        0,
                    )),
                );
            }
        } else if badge_style {
            // No renderer, badge style: the badge takes the first output
            // line onto its row exactly as it does for a tool that renders a
            // call line. The raw argument JSON follows the output instead of
            // preceding it, so the line on the badge row is the answer and
            // not an opening brace (reference `tool_execution.rs`).
            let badge = self.badge_for(bg_slot);
            let output = self.text_output();
            {
                let mut content_box = self.content_box.borrow_mut();
                content_box.set_padding(0, 0);
                content_box.set_bg_fn(None);
                content_box.clear();
                if output.is_empty() {
                    content_box.add_child(component_ref(Text::new(badge, 0, 0)));
                } else {
                    content_box.add_child(component_ref(BadgeCallHeader {
                        badge,
                        call: component_ref(Text::new(
                            theme().fg(ThemeColor::ToolOutput, &output),
                            0,
                            0,
                        )),
                        parens: false,
                        suffix: String::new(),
                    }));
                }
                if self.expanded
                    && let Ok(content) = serde_json::to_string_pretty(&self.args)
                    && !content.is_empty()
                    && content != "null"
                    && content != "{}"
                {
                    content_box.add_child(component_ref(Text::new(format!("\n{content}"), 1, 0)));
                }
            }
            self.container
                .add_child(Rc::clone(&self.content_box) as ComponentRef);
            has_content = true;
        } else {
            let text = self.format_tool_execution();
            let mut content_text = self.content_text.borrow_mut();
            content_text.set_custom_bg_fn(Some(Rc::clone(&bg_fn)));
            content_text.set_text(text);
            drop(content_text);
            self.container
                .add_child(Rc::clone(&self.content_text) as ComponentRef);
            has_content = true;
        }

        self.image_components.clear();
        self.image_spacers.clear();

        if let Some(result) = self.result.clone() {
            let capabilities = get_capabilities();
            for (index, image) in image_blocks(&result.content).enumerate() {
                let Some(kind) = capabilities.images else {
                    continue;
                };
                if !self.show_images || image.data.is_empty() || image.mime_type.is_empty() {
                    continue;
                }
                let converted = self.converted_images.get(&index);
                let image_data = converted.map_or(image.data.as_str(), |(data, _)| data.as_str());
                let image_mime_type =
                    converted.map_or(image.mime_type.as_str(), |(_, mime)| mime.as_str());
                if kind == ImageProtocol::Kitty && image_mime_type != "image/png" {
                    continue;
                }

                let spacer: ComponentRef = component_ref(Spacer::new(1));
                self.container.add_child(Rc::clone(&spacer));
                self.image_spacers.push(spacer);
                let image_component: ComponentRef = component_ref(Image::new(
                    image_data,
                    image_mime_type,
                    ImageTheme {
                        fallback_color: Rc::new(|text: &str| {
                            theme().fg(ThemeColor::ToolOutput, text)
                        }),
                    },
                    ImageOptions {
                        max_width_cells: Some(self.image_width_cells),
                        ..ImageOptions::default()
                    },
                    None,
                ));
                self.container.add_child(Rc::clone(&image_component));
                self.image_components.push(image_component);
            }
        }

        if self.has_renderer_definition() && !has_content && self.image_components.is_empty() {
            self.hide_component = true;
        }
    }

    fn add_to_render_container(&self, self_managed: bool, component: ComponentRef) {
        if self_managed {
            self.self_render_container.borrow_mut().add_child(component);
        } else {
            self.content_box.borrow_mut().add_child(component);
        }
    }

    fn text_output(&self) -> String {
        get_text_output(
            self.result.as_ref().map(|result| result.content.as_slice()),
            self.show_images,
        )
    }

    fn format_tool_execution(&self) -> String {
        let theme_instance = theme();
        let mut text =
            theme_instance.fg(ThemeColor::ToolTitle, &theme_instance.bold(&self.tool_name));
        if let Ok(content) = serde_json::to_string_pretty(&self.args)
            && !content.is_empty()
        {
            text.push_str(&format!("\n\n{content}"));
        }
        let output = self.text_output();
        if !output.is_empty() {
            text.push_str(&format!("\n{output}"));
        }
        text
    }
}

/// The render work of one tool row, ready to be awaited by the render loop.
/// This is the other half of the seam described on
/// future to the loop, which awaits it on the TUI thread. The renderer
/// invalidates the row itself when it is done.
pub struct RowRenderWork {
    definitions: Vec<ToolDef>,
    context: ToolRenderContext,
}

impl RowRenderWork {
    /// Run what the first renderer with pending work handed back, and report
    /// whether anything ran, so the caller knows to ask for a frame.
    pub async fn run(&self) -> bool {
        for definition in &self.definitions {
            if let Some(work) = definition.pump_render(&self.context) {
                work.await;
                return true;
            }
        }
        false
    }
}

fn image_blocks(
    content: &[TextOrImageContent],
) -> impl Iterator<Item = &notagent_ai::types::ImageContent> {
    content.iter().filter_map(|block| match block {
        TextOrImageContent::Image(image) => Some(image),
        TextOrImageContent::Text(_) => None,
    })
}

impl Component for ToolExecutionComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let outer_width = width;
        let width = width.saturating_sub(1);
        // A live style switch restyles rows already on screen (reference
        // pattern: rebuild when the built style no longer matches).
        if self.dirty.get() || self.built_style != block_style() {
            self.update_display();
        }
        if self.hide_component {
            return Vec::new();
        }

        if self.has_renderer_definition()
            && self.render_shell() == RenderShell::SelfManaged
            && !self.badge_style()
        {
            let content_lines = self.self_render_container.borrow_mut().render(width);
            if content_lines.is_empty() && self.image_components.is_empty() {
                return Vec::new();
            }

            let mut lines: Vec<Line> = Vec::new();
            if !content_lines.is_empty() {
                lines.push(Line::from(""));
                lines.extend(content_lines);
            }
            for (index, image) in self.image_components.iter().enumerate() {
                if let Some(spacer) = self.image_spacers.get(index) {
                    lines.extend(spacer.borrow_mut().render(width));
                }
                lines.extend(image.borrow_mut().render(width));
            }
            return super::indent_lines(lines, outer_width);
        }

        super::indent_lines(self.container.render(width), outer_width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
        self.update_display();
    }
}
