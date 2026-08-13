//! Alternate-screen renderer with an application-owned viewport.
//!
//! Port of `packages/tui/src/tui-alt-screen.ts` (1291 LOC). This stage covers
//! the renderer core: enter/exit sequences, the CUP+2K line diff, the Kitty
//! placement cache with LRU eviction, scrolling, flash messages and writing the
//! document into the scrollback on exit. Mouse handling and transcript search
//! follow in the next stages (see PARITY.md).

use std::collections::HashMap;
use std::rc::Rc;

use crate::components::alt_screen_flash::AltScreenFlashContainer;
use crate::components::scroll_view::{ScrollView, ScrollViewOptions, ScrollViewState};
use crate::layout::{LayoutFrame, get_scroll_views_at, render_layout_frame};
use crate::layout_node::ScrollStateRef;
use crate::terminal::Terminal;
use crate::terminal_image::{
    ImageProtocol, TerminalCapabilities, delete_all_kitty_images, delete_all_kitty_placements,
    delete_kitty_image, get_capabilities, get_kitty_image_placement, is_image_line,
    set_capabilities,
};
use crate::tui::{
    CURSOR_MARKER, Component, ComponentRef, TuiCore, TuiMode, TuiStopOptions, component_ref,
};
use crate::utils::{slice_by_column, visible_width};

const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";
const EXIT_ALT_SCREEN: &str = "\x1b[?1049l";
const DISABLE_AUTOWRAP: &str = "\x1b[?7l";
const ENABLE_AUTOWRAP: &str = "\x1b[?7h";
const ENABLE_BUTTON_MOTION_MOUSE: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1004h\x1b[?1006h";
const ENABLE_ALL_MOTION_MOUSE: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1004h\x1b[?1006h";
const DISABLE_MOUSE: &str = "\x1b[?1006l\x1b[?1004l\x1b[?1003l\x1b[?1002l\x1b[?1000l";
const BEGIN_SYNCHRONIZED_OUTPUT: &str = "\x1b[?2026h";
const END_SYNCHRONIZED_OUTPUT: &str = "\x1b[?2026l";
const MAX_CACHED_OFFSCREEN_KITTY_IMAGES: usize = 16;
const MAX_CACHED_OFFSCREEN_KITTY_TRANSMISSION_BYTES: u64 = 32 * 1024 * 1024;
const MAX_CACHED_OFFSCREEN_KITTY_DECODED_BYTES: u64 = 64 * 1024 * 1024;

/// Strip leading OSC 133 zone markers.
fn strip_osc133_zone_prefix(line: &str) -> &str {
    let mut rest = line;
    loop {
        let Some(after) = rest.strip_prefix("\x1b]133;") else {
            return rest;
        };
        let Some(kind) = after.chars().next() else {
            return rest;
        };
        if !matches!(kind, 'A' | 'B' | 'C') {
            return rest;
        }
        let after_kind = &after[kind.len_utf8()..];
        if let Some(next) = after_kind.strip_prefix('\x07') {
            rest = next;
        } else if let Some(next) = after_kind.strip_prefix("\x1b\\") {
            rest = next;
        } else {
            return rest;
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CachedKittyImage {
    transmission_generation: u64,
    transmission_bytes: u64,
    estimated_decoded_bytes: u64,
}

/// Options of a [`TuiAltScreen`].
pub struct TuiAltScreenOptions {
    /// Logical lines moved per mouse wheel event.
    pub wheel_scroll_lines: usize,
    /// Capture mouse events for scrolling and text selection.
    pub mouse: bool,
    /// Style of a non-current search match.
    pub search_match_style: Rc<dyn Fn(&str) -> String>,
    /// Style of the current search match.
    pub search_current_match_style: Rc<dyn Fn(&str) -> String>,
}

impl Default for TuiAltScreenOptions {
    fn default() -> Self {
        Self {
            wheel_scroll_lines: 1,
            mouse: true,
            search_match_style: Rc::new(|text| format!("\x1b[4m{text}\x1b[24m")),
            search_current_match_style: Rc::new(|text| format!("\x1b[1;7m{text}\x1b[22;27m")),
        }
    }
}

/// Renders into the terminal's alternate screen.
pub struct TuiAltScreen {
    core: TuiCore,
    previous_screen: Vec<String>,
    last_document: Vec<String>,
    previous_screen_width: usize,
    previous_screen_height: usize,
    layout_root: Option<ComponentRef>,
    current_layout: Option<LayoutFrame>,
    implicit_scroll_view: ComponentRef,
    implicit_scroll_state: Rc<std::cell::RefCell<ScrollViewState>>,
    flashes: Rc<std::cell::RefCell<AltScreenFlashContainer>>,
    alt_screen_active: bool,
    image_protocol: Option<ImageProtocol>,
    saved_capabilities: Option<TerminalCapabilities>,
    uploaded_kitty_images: HashMap<u32, CachedKittyImage>,
    uploaded_kitty_order: Vec<u32>,
    full_redraw_count: usize,
    options: TuiAltScreenOptions,
}

/// Renders the mounted children; the implicit document of the TS version.
struct ImplicitDocument {
    core: TuiCore,
}

impl Component for ImplicitDocument {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.core.render_children(width)
    }

    fn invalidate(&mut self) {
        for child in self.core.children() {
            child.borrow_mut().invalidate();
        }
    }
}

impl TuiAltScreen {
    /// New alternate-screen renderer.
    pub fn new(terminal: Box<dyn Terminal>, options: TuiAltScreenOptions) -> Self {
        let core = TuiCore::new(terminal);
        let document = component_ref(ImplicitDocument { core: core.clone() });
        let scroll_view = ScrollView::new(
            document,
            ScrollViewOptions {
                follow_end: true,
                primary: true,
                ..ScrollViewOptions::default()
            },
        );
        let implicit_scroll_state = scroll_view.state();
        Self {
            core,
            previous_screen: Vec::new(),
            last_document: Vec::new(),
            previous_screen_width: 0,
            previous_screen_height: 0,
            layout_root: None,
            current_layout: None,
            implicit_scroll_view: component_ref(scroll_view),
            implicit_scroll_state,
            flashes: Rc::new(std::cell::RefCell::new(AltScreenFlashContainer::new())),
            alt_screen_active: false,
            image_protocol: None,
            saved_capabilities: None,
            uploaded_kitty_images: HashMap::new(),
            uploaded_kitty_order: Vec::new(),
            full_redraw_count: 0,
            options,
        }
    }

    /// Rendering mode of this renderer.
    pub fn mode(&self) -> TuiMode {
        TuiMode::Fullscreen
    }

    /// Shared TUI state.
    pub fn core(&self) -> &TuiCore {
        &self.core
    }

    /// Number of full redraws performed so far.
    pub fn full_redraws(&self) -> usize {
        self.full_redraw_count
    }

    /// Replace the layout root; `None` falls back to the implicit document.
    pub fn set_layout_root(&mut self, component: Option<ComponentRef>) {
        let same = match (&self.layout_root, &component) {
            (Some(current), Some(next)) => Rc::ptr_eq(current, next),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.layout_root = component;
        self.current_layout = None;
        self.core.request_render();
    }

    /// Scroll offset of the primary scroll view.
    pub fn viewport_top(&self) -> usize {
        self.primary_scroll_state().borrow().scroll_top()
    }

    /// Whether the primary scroll view sticks to the end.
    pub fn is_following_output(&self) -> bool {
        self.primary_scroll_state().borrow().following_end()
    }

    /// Primary scroll view of the current layout, or the implicit one.
    fn primary_scroll_state(&self) -> ScrollStateRef {
        match &self.current_layout {
            Some(layout) => match &layout.primary_scroll_view {
                Some(scroll_view) => Rc::clone(scroll_view),
                None => Rc::clone(&self.implicit_scroll_state) as ScrollStateRef,
            },
            None => Rc::clone(&self.implicit_scroll_state) as ScrollStateRef,
        }
    }

    /// Route a wheel event through the scroll views under the pointer.
    ///
    /// Nested views consume the delta first; a view with `overscroll: contain`
    /// stops the chain, and any remainder falls back to the primary view.
    fn route_wheel(&mut self, direction: i64, x: i64, y: i64) {
        let mut remaining = direction * self.options.wheel_scroll_lines as i64;
        let mut seen: Vec<ScrollStateRef> = Vec::new();
        if let Some(layout) = &self.current_layout {
            for scroll_view in get_scroll_views_at(layout, x, y) {
                seen.push(Rc::clone(&scroll_view));
                let contain = !scroll_view.borrow().overscroll_chain();
                remaining = scroll_view.borrow_mut().scroll_by_lines(remaining);
                if remaining == 0 || contain {
                    break;
                }
            }
        }
        let primary = self.primary_scroll_state();
        let already_seen = seen.iter().any(|view| Rc::ptr_eq(view, &primary));
        if remaining != 0 && !already_seen {
            primary.borrow_mut().scroll_by_lines(remaining);
        }
        self.core.request_render();
    }

    /// Parse an SGR or legacy wheel event; returns `(direction, x, y)`.
    fn parse_wheel_event(data: &str) -> Option<(i64, i64, i64)> {
        if let Some(body) = data
            .strip_prefix("\x1b[<")
            .and_then(|rest| rest.strip_suffix('M').or_else(|| rest.strip_suffix('m')))
        {
            let mut parts = body.split(';');
            let button: i64 = parts.next()?.parse().ok()?;
            let x: i64 = parts.next()?.parse().ok()?;
            let y: i64 = parts.next()?.parse().ok()?;
            if parts.next().is_some() || (button & 64) == 0 {
                return None;
            }
            let direction = match button & 3 {
                0 => -1,
                1 => 1,
                _ => return None,
            };
            return Some((direction, x - 1, y - 1));
        }
        let bytes = data.as_bytes();
        if bytes.len() == 6 && data.starts_with("\x1b[M") {
            let button = i64::from(bytes[3]) - 32;
            if (button & 64) == 0 {
                return None;
            }
            let direction = match button & 3 {
                0 => -1,
                1 => 1,
                _ => return None,
            };
            return Some((
                direction,
                i64::from(bytes[4]) - 33,
                i64::from(bytes[5]) - 33,
            ));
        }
        None
    }

    /// Handle viewport input (wheel events for now); returns `true` when consumed.
    ///
    /// The TS version registers this as an input listener in the constructor;
    /// here the renderer's input handler calls it before the component dispatch
    /// because the listener needs `&mut self`.
    pub fn handle_viewport_input(&mut self, data: &str) -> bool {
        if let Some((direction, x, y)) = Self::parse_wheel_event(data) {
            self.route_wheel(direction, x, y);
            return true;
        }
        false
    }

    /// Feed terminal input: viewport handling first, then the TUI dispatch.
    pub fn handle_terminal_input(&mut self, data: &str) {
        if self.handle_viewport_input(data) {
            return;
        }
        self.core.handle_terminal_input(data);
    }

    /// Scroll the primary view by `lines`.
    pub fn scroll_by(&mut self, lines: i64) {
        self.primary_scroll_state()
            .borrow_mut()
            .scroll_by_lines(lines);
        self.core.request_render();
    }

    /// Scroll the primary view to the top.
    pub fn scroll_to_top(&mut self) {
        self.primary_scroll_state()
            .borrow_mut()
            .scroll_to_start_line();
        self.core.request_render();
    }

    /// Scroll the primary view to the bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.primary_scroll_state()
            .borrow_mut()
            .scroll_to_end_line();
        self.core.request_render();
    }

    /// Show a transient message in the flash stack.
    pub fn flash(&mut self, message: impl Into<String>, duration_ms: Option<u64>) {
        self.flashes.borrow_mut().flash(message, duration_ms);
        self.core.request_render();
    }

    /// The document written into the scrollback on exit.
    pub fn last_document(&self) -> &[String] {
        &self.last_document
    }

    fn render_root(&self, width: usize) -> Vec<String> {
        match &self.layout_root {
            Some(root) => root.borrow_mut().render(width),
            None => self.core.render_children(width),
        }
    }

    fn reset_render_state(&mut self) {
        self.previous_screen = Vec::new();
        self.previous_screen_width = 0;
        self.previous_screen_height = 0;
        self.current_layout = None;
    }

    fn delete_kitty_images(&self) -> String {
        if self.image_protocol == Some(ImageProtocol::Kitty) {
            delete_all_kitty_images()
        } else {
            String::new()
        }
    }

    /// Replace already transmitted images by pure placements and evict the
    /// offscreen cache when it grows past its limits.
    fn prepare_kitty_screen(&mut self, screen: &[String]) -> (Vec<String>, String) {
        let mut visible_image_ids: Vec<u32> = Vec::new();
        let lines: Vec<String> = screen
            .iter()
            .map(|line| {
                let Some(placement) = get_kitty_image_placement(line) else {
                    return line.clone();
                };
                visible_image_ids.push(placement.image_id);
                let cached = self.uploaded_kitty_images.get(&placement.image_id).copied();
                let next = CachedKittyImage {
                    transmission_generation: placement.transmission_generation,
                    transmission_bytes: placement.transmission_bytes as u64,
                    estimated_decoded_bytes: placement.estimated_decoded_bytes,
                };
                if cached.is_some() {
                    self.uploaded_kitty_images.remove(&placement.image_id);
                    self.uploaded_kitty_order
                        .retain(|id| *id != placement.image_id);
                }
                self.uploaded_kitty_images.insert(placement.image_id, next);
                self.uploaded_kitty_order.push(placement.image_id);

                match cached {
                    Some(cached)
                        if cached.transmission_generation == placement.transmission_generation =>
                    {
                        placement.replacement_line
                    }
                    _ => line.clone(),
                }
            })
            .collect();

        let mut offscreen_count = 0;
        let mut offscreen_transmission_bytes = 0;
        let mut offscreen_decoded_bytes = 0;
        for (image_id, cached) in &self.uploaded_kitty_images {
            if visible_image_ids.contains(image_id) {
                continue;
            }
            offscreen_count += 1;
            offscreen_transmission_bytes += cached.transmission_bytes;
            offscreen_decoded_bytes += cached.estimated_decoded_bytes;
        }

        let mut evicted_image_deletion = String::new();
        let order = self.uploaded_kitty_order.clone();
        for image_id in order {
            if offscreen_count <= MAX_CACHED_OFFSCREEN_KITTY_IMAGES
                && offscreen_transmission_bytes <= MAX_CACHED_OFFSCREEN_KITTY_TRANSMISSION_BYTES
                && offscreen_decoded_bytes <= MAX_CACHED_OFFSCREEN_KITTY_DECODED_BYTES
            {
                break;
            }
            if visible_image_ids.contains(&image_id) {
                continue;
            }
            let Some(cached) = self.uploaded_kitty_images.remove(&image_id) else {
                continue;
            };
            self.uploaded_kitty_order.retain(|id| *id != image_id);
            evicted_image_deletion.push_str(&delete_kitty_image(image_id));
            offscreen_count -= 1;
            offscreen_transmission_bytes -= cached.transmission_bytes;
            offscreen_decoded_bytes -= cached.estimated_decoded_bytes;
        }

        (lines, evicted_image_deletion)
    }

    /// Enter the alternate screen and start the terminal.
    pub fn start(&mut self) {
        self.flashes.borrow_mut().dispose();
        self.alt_screen_active = true;
        let capabilities = get_capabilities();
        self.image_protocol = capabilities.images;
        self.uploaded_kitty_images.clear();
        self.uploaded_kitty_order.clear();
        if capabilities.images == Some(ImageProtocol::ITerm2) {
            self.saved_capabilities = Some(capabilities);
            set_capabilities(TerminalCapabilities {
                images: None,
                ..capabilities
            });
            self.core.invalidate();
        }
        self.last_document = Vec::new();
        self.reset_render_state();

        // Multiplexers lag when every pointer movement is forwarded; button
        // motion tracking still preserves clicks, wheel, selection and dragging.
        let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
        let in_multiplexer = std::env::var_os("TMUX").is_some()
            || std::env::var_os("ZELLIJ").is_some()
            || std::env::var_os("STY").is_some()
            || term.starts_with("tmux")
            || term.starts_with("screen");
        let mouse_sequence = if !self.options.mouse {
            ""
        } else if in_multiplexer {
            ENABLE_BUTTON_MOTION_MOUSE
        } else {
            ENABLE_ALL_MOTION_MOUSE
        };
        let enter =
            format!("{ENTER_ALT_SCREEN}{DISABLE_AUTOWRAP}{mouse_sequence}\x1b[2J\x1b[H\x1b[?25l");
        self.core.with_terminal(|terminal| terminal.write(&enter));

        let core = self.core.clone();
        self.core
            .start(Box::new(move |data| core.handle_terminal_input(data)));
    }

    /// Leave the alternate screen and stop the terminal.
    pub fn stop(&mut self, options: TuiStopOptions) {
        self.flashes.borrow_mut().dispose();
        if self.alt_screen_active {
            let leave = format!(
                "{BEGIN_SYNCHRONIZED_OUTPUT}{}{}{ENABLE_AUTOWRAP}{END_SYNCHRONIZED_OUTPUT}",
                self.delete_kitty_images(),
                if self.options.mouse {
                    DISABLE_MOUSE
                } else {
                    ""
                }
            );
            self.core.with_terminal(|terminal| terminal.write(&leave));
            self.uploaded_kitty_images.clear();
            self.uploaded_kitty_order.clear();
        }

        self.core.stop();

        if !self.alt_screen_active {
            return;
        }
        self.alt_screen_active = false;
        if options.preserve_screen {
            let buffer = format!(
                "{BEGIN_SYNCHRONIZED_OUTPUT}{EXIT_ALT_SCREEN}\x1b[?25h{END_SYNCHRONIZED_OUTPUT}"
            );
            self.core.with_terminal(|terminal| terminal.write(&buffer));
        } else {
            let width = self.core.columns().max(1);
            let mut document: Vec<String> = self
                .render_root(width)
                .iter()
                .map(|line| strip_osc133_zone_prefix(line).replace(CURSOR_MARKER, ""))
                .collect();
            self.core.apply_line_resets(&mut document);
            self.last_document = document
                .into_iter()
                .map(|line| {
                    if is_image_line(&line) || visible_width(&line) <= width {
                        line
                    } else {
                        slice_by_column(&line, 0, width, true)
                    }
                })
                .collect();

            let mut buffer =
                format!("{BEGIN_SYNCHRONIZED_OUTPUT}{EXIT_ALT_SCREEN}{DISABLE_AUTOWRAP}");
            for (row, line) in self.last_document.iter().enumerate() {
                if row > 0 {
                    buffer.push_str("\r\n");
                }
                buffer.push_str(&format!("\r\x1b[2K{line}"));
            }
            buffer.push_str(&format!(
                "\x1b[0m{ENABLE_AUTOWRAP}\r\n\x1b[?25h{END_SYNCHRONIZED_OUTPUT}"
            ));
            self.core.with_terminal(|terminal| terminal.write(&buffer));
        }

        if let Some(capabilities) = self.saved_capabilities.take() {
            set_capabilities(capabilities);
        }
    }

    /// Render immediately; `force` resets the differential state first.
    pub fn render_now(&mut self, force: bool) {
        if force {
            self.reset_render_state();
        }
        if !self.core.begin_frame() {
            return;
        }
        self.do_render();
    }

    /// Request a frame; `force` resets the differential state.
    pub fn request_render(&mut self, force: bool) {
        if force {
            self.reset_render_state();
            self.core.request_immediate_render();
            return;
        }
        self.core.request_render();
    }

    /// Render the pending frame once its throttle deadline has passed.
    pub async fn wait_for_render(&mut self) {
        let Some(deadline) = self.core.render_deadline() else {
            return;
        };
        let now = std::time::Instant::now();
        if deadline > now {
            tokio::time::sleep(deadline - now).await;
        }
        if self.core.begin_frame() {
            self.do_render();
        }
    }

    fn composite_flashes(
        &mut self,
        screen: Vec<String>,
        width: usize,
        height: usize,
    ) -> Vec<String> {
        self.flashes.borrow_mut().expire();
        let flash_lines = self.flashes.borrow_mut().render(width);
        if flash_lines.is_empty() {
            return screen;
        }
        let mut result = screen;
        // Flash messages sit at the bottom of the viewport.
        let start = height.saturating_sub(flash_lines.len());
        for (index, flash_line) in flash_lines.iter().enumerate() {
            let row = start + index;
            if row < result.len() {
                result[row] =
                    crate::tui::composite_tui_line(&result[row], flash_line, 0, width, width);
            }
        }
        result
    }

    fn do_render(&mut self) {
        if self.core.is_stopped() || !self.alt_screen_active {
            return;
        }
        let width = self.core.columns().max(1);
        let height = self.core.rows().max(1);
        let root = self
            .layout_root
            .clone()
            .unwrap_or_else(|| self.implicit_scroll_view.clone());
        let next_layout = render_layout_frame(&root, width, height);

        let mut screen: Vec<String> = next_layout
            .lines
            .iter()
            .map(|line| strip_osc133_zone_prefix(line).to_string())
            .collect();
        screen = self.core.composite_overlays(screen, width, height);
        if screen.len() > height {
            screen = screen[screen.len() - height..].to_vec();
        }
        screen = self.composite_flashes(screen, width, height);

        let cursor_pos = self.core.extract_cursor_position(&mut screen, height);
        self.core.apply_line_resets(&mut screen);
        let mut screen: Vec<String> = screen
            .into_iter()
            .map(|line| {
                if is_image_line(&line) || visible_width(&line) <= width {
                    line
                } else {
                    slice_by_column(&line, 0, width, true)
                }
            })
            .collect();
        screen.resize(height, String::new());

        let full_redraw = self.previous_screen.is_empty()
            || self.previous_screen_width != width
            || self.previous_screen_height != height;
        let images_need_redraw = screen.iter().enumerate().any(|(row, line)| {
            let previous = self.previous_screen.get(row).map_or("", String::as_str);
            *line != previous && (is_image_line(line) || is_image_line(previous))
        });
        let redraw_images = full_redraw || images_need_redraw;
        let had_uploaded_kitty_images = !self.uploaded_kitty_images.is_empty();
        let (prepared_lines, evicted_image_deletion) =
            if redraw_images && self.image_protocol == Some(ImageProtocol::Kitty) {
                self.prepare_kitty_screen(&screen)
            } else {
                (screen.clone(), String::new())
            };

        let mut buffer = String::from(BEGIN_SYNCHRONIZED_OUTPUT);
        if full_redraw {
            self.full_redraw_count += 1;
            let clear_images =
                if self.image_protocol == Some(ImageProtocol::Kitty) && had_uploaded_kitty_images {
                    delete_all_kitty_placements()
                } else {
                    self.delete_kitty_images()
                };
            buffer.push_str(&format!("{clear_images}\x1b[2J"));
        } else if images_need_redraw {
            if self.image_protocol == Some(ImageProtocol::ITerm2) {
                buffer.push_str("\x1b[2J");
            } else if self.image_protocol == Some(ImageProtocol::Kitty) {
                buffer.push_str(&delete_all_kitty_placements());
            }
        }
        buffer.push_str(&evicted_image_deletion);

        for row in 0..height {
            if !full_redraw
                && !images_need_redraw
                && screen.get(row) == self.previous_screen.get(row)
            {
                continue;
            }
            buffer.push_str(&format!(
                "\x1b[{};1H\x1b[2K{}",
                row + 1,
                prepared_lines.get(row).map_or("", String::as_str)
            ));
        }

        match cursor_pos {
            Some((row, col)) => {
                buffer.push_str(&format!("\x1b[{};{}H", row + 1, col.min(width) + 1));
                buffer.push_str(if self.core.get_show_hardware_cursor() {
                    "\x1b[?25h"
                } else {
                    "\x1b[?25l"
                });
            }
            None => buffer.push_str("\x1b[?25l"),
        }
        buffer.push_str(END_SYNCHRONIZED_OUTPUT);
        self.core.with_terminal(|terminal| terminal.write(&buffer));

        self.previous_screen = screen;
        self.previous_screen_width = width;
        self.previous_screen_height = height;
        self.current_layout = Some(next_layout);
    }
}
