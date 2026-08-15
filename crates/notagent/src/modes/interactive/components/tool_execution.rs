//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/tool-execution.ts` (377 LOC).
//!
//! One row of the transcript: the tool call, its result and any images it
//! returned. What the row looks like is the tool's business — the component
//! only decides which shell it draws into and what to fall back to when a tool
//! brings no renderer.
//!
//! Deviations (class 1):
//! - TS takes the `TUI` to call `requestRender()`; the port takes that one
//!   callback, because nothing else of the TUI is used.
//! - `ToolRenderContext::invalidate` cannot call back into the row: the closure
//!   would need `&mut` on the component while the renderer holds it. It sets a
//!   dirty flag instead, which the next `render` consumes — observably the same,
//!   since an invalidation only ever shows up in the following frame anyway.
//! - The TS renderers are wrapped in try/catch; a Rust renderer reports failure
//!   by returning `None`, which takes the same fallback path.

use base64::Engine;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use notagent_ai::types::TextOrImageContent;
use notagent_tui::components::box_component::BoxComponent;
use notagent_tui::components::image::{Image, ImageOptions, ImageTheme};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::terminal_image::{ImageProtocol, get_capabilities};
use notagent_tui::tui::{Component, ComponentRef, Container, component_ref};
use serde_json::Value;

use crate::core::tools::render_utils::get_text_output;
use crate::core::tools::tool_definition::{
    RenderShell, ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
    new_tool_render_state,
};
use crate::core::tools::{ToolDef, ToolName, create_all_tool_definitions};
use crate::modes::interactive::theme::theme::{ThemeBg, ThemeColor, theme};

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
    args_complete: bool,
    result: Option<ToolExecutionResult>,
    converted_images: HashMap<usize, (String, String)>,
    hide_component: bool,
    /// Set by the render context's `invalidate`; consumed by the next `render`.
    dirty: Rc<Cell<bool>>,
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
            args_complete: false,
            result: None,
            converted_images: HashMap::new(),
            hide_component: false,
            dirty: Rc::new(Cell::new(false)),
        };

        if component.has_renderer_definition() {
            let shell = component.render_shell();
            let child: ComponentRef = if shell == RenderShell::SelfManaged {
                Rc::clone(&component.self_render_container) as ComponentRef
            } else {
                Rc::clone(&component.content_box) as ComponentRef
            };
            component.container.add_child(child);
        } else {
            component
                .container
                .add_child(Rc::clone(&component.content_text) as ComponentRef);
        }

        component.update_display();
        component
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
    ///
    /// Deviation (class 1): TS picks the *function*
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
    ///
    /// Deviation (class 1): TS converts in a promise because its converter is a
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

        // TS starts at `false`, and both branches below always set it — the
        // `hide_component` case is therefore as unreachable here as it is there
        // (bug-compat, not a simplification).
        #[allow(unused_assignments)]
        let mut has_content = false;
        self.hide_component = false;
        if self.has_renderer_definition() {
            let self_managed = self.render_shell() == RenderShell::SelfManaged;
            if self_managed {
                self.self_render_container.borrow_mut().clear();
            } else {
                let mut content_box = self.content_box.borrow_mut();
                content_box.set_bg_fn(Some(Rc::clone(&bg_fn)));
                content_box.clear();
            }

            let theme_instance = theme();
            let context = self.render_context(self.call_renderer_component.clone());
            let call_component = self.definitions_in_order().find_map(|definition| {
                definition.render_call(&self.args, &theme_instance, &context)
            });
            let call_component = match call_component {
                Some(component) => {
                    self.call_renderer_component = Some(Rc::clone(&component));
                    component
                }
                None => {
                    self.call_renderer_component = None;
                    self.create_call_fallback()
                }
            };
            self.add_to_render_container(self_managed, call_component);
            has_content = true;

            if let Some(result) = self.result.clone() {
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
                        self.add_to_render_container(self_managed, component);
                        has_content = true;
                    }
                    None => {
                        self.result_renderer_component = None;
                        if let Some(component) = self.create_result_fallback() {
                            self.add_to_render_container(self_managed, component);
                            has_content = true;
                        }
                    }
                }
            }
        } else {
            let text = self.format_tool_execution();
            let mut content_text = self.content_text.borrow_mut();
            content_text.set_custom_bg_fn(Some(Rc::clone(&bg_fn)));
            content_text.set_text(text);
            has_content = true;
        }

        for image in std::mem::take(&mut self.image_components) {
            self.container.remove_child(&image);
        }
        for spacer in std::mem::take(&mut self.image_spacers) {
            self.container.remove_child(&spacer);
        }

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

/// `convertToPng(base64Data, mimeType)` (`utils/image-convert.ts:29-49`).
///
/// Lives here until C adds it beside `convert_image_bytes_to_png` in
/// `utils/image.rs`, where its TS file was ported (interface request A-21).
fn convert_to_png(data: &str, mime_type: &str) -> Option<(String, String)> {
    if mime_type == "image/png" {
        return Some((data.to_string(), mime_type.to_string()));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let png = crate::utils::image::convert_image_bytes_to_png(&bytes)?;
    Some((
        base64::engine::general_purpose::STANDARD.encode(&png),
        "image/png".to_string(),
    ))
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
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.dirty.get() {
            self.update_display();
        }
        if self.hide_component {
            return Vec::new();
        }

        if self.has_renderer_definition() && self.render_shell() == RenderShell::SelfManaged {
            let content_lines = self.self_render_container.borrow_mut().render(width);
            if content_lines.is_empty() && self.image_components.is_empty() {
                return Vec::new();
            }

            let mut lines: Vec<String> = Vec::new();
            if !content_lines.is_empty() {
                lines.push(String::new());
                lines.extend(content_lines);
            }
            for (index, image) in self.image_components.iter().enumerate() {
                if let Some(spacer) = self.image_spacers.get(index) {
                    lines.extend(spacer.borrow_mut().render(width));
                }
                lines.extend(image.borrow_mut().render(width));
            }
            return lines;
        }

        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
        self.update_display();
    }
}
