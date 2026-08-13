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
use crate::layout::{LayoutFrame, get_scroll_view_box, get_scroll_views_at, render_layout_frame};
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
use crate::utils::{
    extract_ansi_code, get_grapheme_cell_range, get_osc8_link_at_column, slice_by_column,
    strip_terminal_sequences, visible_width, word_segments_public as word_segments,
};

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
    selection_anchor: Option<SelectionPoint>,
    selection_focus: Option<SelectionPoint>,
    selection_granularity: SelectionGranularity,
    selection_initial_range: Option<SelectionRange>,
    selection_press_active: bool,
    selection_dragged: bool,
    last_click: Option<ClickTarget>,
    pressed_url: Option<String>,
    clicked_url: Option<String>,
}

/// A point of the current text selection.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectionPoint {
    row: usize,
    col: usize,
    /// Scroll view the point belongs to, identified by pointer identity.
    scroll_view: Option<usize>,
    /// Whether the point lies between cells rather than on one.
    boundary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectionRange {
    start: SelectionPoint,
    end: SelectionPoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionGranularity {
    Character,
    Word,
    Line,
}

#[derive(Debug, Clone)]
struct ClickTarget {
    timestamp: std::time::Instant,
    count: usize,
    row: usize,
    scroll_view: Option<usize>,
    word_start: usize,
    word_end: usize,
}

/// A parsed SGR mouse event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SgrMouseEvent {
    button: i64,
    x: i64,
    y: i64,
    release: bool,
}

const DOUBLE_CLICK_INTERVAL_MS: u64 = 500;

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
            selection_anchor: None,
            selection_focus: None,
            selection_granularity: SelectionGranularity::Character,
            selection_initial_range: None,
            selection_press_active: false,
            selection_dragged: false,
            last_click: None,
            pressed_url: None,
            clicked_url: None,
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

    // === Text selection ===

    /// Parse an SGR mouse event.
    fn parse_sgr_mouse_event(data: &str) -> Option<SgrMouseEvent> {
        let body = data.strip_prefix("\x1b[<")?;
        let (body, release) = if let Some(rest) = body.strip_suffix('M') {
            (rest, false)
        } else if let Some(rest) = body.strip_suffix('m') {
            (rest, true)
        } else {
            return None;
        };
        let mut parts = body.split(';');
        let button: i64 = parts.next()?.parse().ok()?;
        let x: i64 = parts.next()?.parse().ok()?;
        let y: i64 = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(SgrMouseEvent {
            button,
            x: x - 1,
            y: y - 1,
            release,
        })
    }

    fn scroll_view_key(scroll_view: &ScrollStateRef) -> usize {
        Rc::as_ptr(scroll_view).cast::<()>() as usize
    }

    fn scroll_view_by_key(&self, key: usize) -> Option<ScrollStateRef> {
        let layout = self.current_layout.as_ref()?;
        fn visit(layout_box: &crate::layout::LayoutBox, key: usize) -> Option<ScrollStateRef> {
            if let Some(scroll_view) = &layout_box.scroll_view
                && TuiAltScreen::scroll_view_key(scroll_view) == key
            {
                return Some(Rc::clone(scroll_view));
            }
            layout_box
                .children
                .iter()
                .find_map(|child| visit(child, key))
        }
        visit(&layout.root, key)
    }

    /// Point under the pointer, resolved inside `scroll_view` when given.
    fn get_selection_point(
        &self,
        event: SgrMouseEvent,
        scroll_view: Option<usize>,
    ) -> SelectionPoint {
        if let Some(key) = scroll_view
            && let Some(point) = self.get_scroll_selection_point(key, event.x, event.y)
        {
            return point;
        }
        SelectionPoint {
            row: event.y.clamp(0, self.core.rows() as i64 - 1).max(0) as usize,
            col: event.x.clamp(0, self.core.columns() as i64 - 1).max(0) as usize,
            scroll_view: None,
            boundary: false,
        }
    }

    fn get_scroll_selection_point(&self, key: usize, x: i64, y: i64) -> Option<SelectionPoint> {
        let layout = self.current_layout.as_ref()?;
        let scroll_view = self.scroll_view_by_key(key)?;
        let layout_box = get_scroll_view_box(layout, &scroll_view)?;
        if layout_box.rect.height <= 0 || layout_box.clip.height <= 0 {
            return None;
        }
        let visible_top = layout_box.rect.y.max(layout_box.clip.y).max(0);
        let visible_bottom = (self.core.rows() as i64 - 1)
            .min(layout_box.rect.y + layout_box.rect.height - 1)
            .min(layout_box.clip.y + layout_box.clip.height - 1);
        if visible_bottom < visible_top {
            return None;
        }
        let pointer_row = y.clamp(visible_top, visible_bottom);
        let max_content_row = layout_box
            .scroll_content_lines
            .as_ref()
            .map_or(1, Vec::len)
            .saturating_sub(1) as i64;
        let scroll_top = scroll_view.borrow().scroll_top() as i64;
        Some(SelectionPoint {
            row: (scroll_top + pointer_row - layout_box.rect.y)
                .clamp(0, max_content_row)
                .max(0) as usize,
            col: (x - layout_box.rect.x)
                .clamp(0, (layout_box.rect.width - 1).max(0))
                .max(0) as usize,
            scroll_view: Some(key),
            boundary: false,
        })
    }

    fn get_selection_source_line(&self, point: &SelectionPoint) -> String {
        if let Some(key) = point.scroll_view
            && let Some(layout) = &self.current_layout
            && let Some(scroll_view) = self.scroll_view_by_key(key)
            && let Some(layout_box) = get_scroll_view_box(layout, &scroll_view)
            && let Some(lines) = &layout_box.scroll_content_lines
        {
            return lines.get(point.row).cloned().unwrap_or_default();
        }
        self.previous_screen
            .get(point.row)
            .cloned()
            .unwrap_or_default()
    }

    fn get_word_selection(&self, point: &SelectionPoint) -> Option<SelectionRange> {
        let line = strip_terminal_sequences(&self.get_selection_source_line(point));
        let mut start = 0;
        for segment in word_segments(&line) {
            let end = start + visible_width(segment);
            if point.col >= start && point.col < end {
                return Some(SelectionRange {
                    start: SelectionPoint {
                        col: start,
                        ..point.clone()
                    },
                    end: SelectionPoint {
                        col: end,
                        boundary: true,
                        ..point.clone()
                    },
                });
            }
            start = end;
        }
        None
    }

    fn get_line_selection(&self, point: &SelectionPoint) -> SelectionRange {
        SelectionRange {
            start: SelectionPoint {
                col: 0,
                ..point.clone()
            },
            end: SelectionPoint {
                col: visible_width(&self.get_selection_source_line(point)),
                boundary: true,
                ..point.clone()
            },
        }
    }

    fn get_click_count(&mut self, point: &SelectionPoint, word: Option<&SelectionRange>) -> usize {
        let now = std::time::Instant::now();
        let count = match (word, &self.last_click) {
            (Some(word), Some(previous))
                if now.duration_since(previous.timestamp).as_millis()
                    <= u128::from(DOUBLE_CLICK_INTERVAL_MS)
                    && previous.row == point.row
                    && previous.scroll_view == point.scroll_view
                    && previous.word_start == word.start.col
                    && previous.word_end == word.end.col =>
            {
                (previous.count % 3) + 1
            }
            _ => 1,
        };
        self.last_click = word.map(|word| ClickTarget {
            timestamp: now,
            count,
            row: point.row,
            scroll_view: point.scroll_view,
            word_start: word.start.col,
            word_end: word.end.col,
        });
        count
    }

    fn update_selection_focus(&mut self, point: SelectionPoint) {
        if self.selection_granularity == SelectionGranularity::Character
            || self.selection_initial_range.is_none()
        {
            self.selection_focus = Some(point);
            return;
        }
        let range = if self.selection_granularity == SelectionGranularity::Word {
            self.get_word_selection(&point)
        } else {
            Some(self.get_line_selection(&point))
        };
        let Some(range) = range else {
            return;
        };
        let initial = self.selection_initial_range.clone().expect("range present");
        let target_before_initial = range.start.row < initial.start.row
            || (range.start.row == initial.start.row && range.start.col < initial.start.col);
        if target_before_initial {
            self.selection_anchor = Some(initial.end);
            self.selection_focus = Some(range.start);
        } else {
            self.selection_anchor = Some(initial.start);
            self.selection_focus = Some(range.end);
        }
    }

    fn get_selection_bounds(&self) -> Option<SelectionRange> {
        let anchor = self.selection_anchor.clone()?;
        let focus = self.selection_focus.clone()?;
        if anchor.scroll_view != focus.scroll_view {
            return None;
        }
        if anchor.row == focus.row && anchor.col == focus.col {
            return None;
        }
        let anchor_before_focus =
            anchor.row < focus.row || (anchor.row == focus.row && anchor.col < focus.col);
        Some(if anchor_before_focus {
            SelectionRange {
                start: anchor,
                end: focus,
            }
        } else {
            SelectionRange {
                start: focus,
                end: anchor,
            }
        })
    }

    fn get_selection_columns(
        line: &str,
        row: usize,
        selection: &SelectionRange,
        min_column: usize,
        max_column: usize,
    ) -> (usize, usize) {
        let line_width = visible_width(line);
        let mut start = min_column;
        let mut end = line_width.min(max_column);
        if row == selection.start.row {
            start = get_grapheme_cell_range(line, selection.start.col)
                .map_or_else(|| selection.start.col.min(line_width), |range| range.start);
        }
        if row == selection.end.row {
            end = if selection.end.boundary {
                selection.end.col.min(line_width)
            } else {
                get_grapheme_cell_range(line, selection.end.col).map_or_else(
                    || (selection.end.col + 1).min(line_width),
                    |range| range.end,
                )
            };
        }
        (start.max(min_column), end.min(max_column))
    }

    /// Wrap the selected text in reverse video, re-applying it after every SGR.
    fn apply_selection_highlight(text: &str) -> String {
        let mut result = String::from("\x1b[7m");
        let mut index = 0;
        while index < text.len() {
            match extract_ansi_code(text, index) {
                Some(ansi) => {
                    result.push_str(ansi.code);
                    if ansi.code.ends_with('m') {
                        result.push_str("\x1b[7m");
                    }
                    index += ansi.length;
                }
                None => {
                    let ch = text[index..].chars().next().expect("char boundary");
                    result.push(ch);
                    index += ch.len_utf8();
                }
            }
        }
        result.push_str("\x1b[27m");
        result
    }

    fn apply_selection(&self, screen: Vec<String>) -> Vec<String> {
        let Some(selection) = self.get_selection_bounds() else {
            return screen;
        };
        let mut screen_selection = selection.clone();
        let mut min_row = 0;
        let mut max_row = screen.len().saturating_sub(1);
        let mut min_column = 0;
        let mut max_column = self.core.columns();

        if let Some(key) = selection.start.scroll_view {
            let (Some(layout), Some(scroll_view)) =
                (self.current_layout.as_ref(), self.scroll_view_by_key(key))
            else {
                return screen;
            };
            let Some(layout_box) = get_scroll_view_box(layout, &scroll_view) else {
                return screen;
            };
            min_row = layout_box.rect.y.max(layout_box.clip.y).max(0) as usize;
            max_row = (screen.len() as i64 - 1)
                .min(layout_box.rect.y + layout_box.rect.height - 1)
                .min(layout_box.clip.y + layout_box.clip.height - 1)
                .max(0) as usize;
            min_column = layout_box.rect.x.max(layout_box.clip.x).max(0) as usize;
            max_column = (self.core.columns() as i64)
                .min(layout_box.rect.x + layout_box.rect.width)
                .min(layout_box.clip.x + layout_box.clip.width)
                .max(0) as usize;
            let scroll_top = scroll_view.borrow().scroll_top() as i64;
            let to_screen = |point: &SelectionPoint| SelectionPoint {
                row: (layout_box.rect.y + point.row as i64 - scroll_top).max(0) as usize,
                col: (layout_box.rect.x + point.col as i64).max(0) as usize,
                scroll_view: point.scroll_view,
                boundary: point.boundary,
            };
            screen_selection = SelectionRange {
                start: to_screen(&selection.start),
                end: to_screen(&selection.end),
            };
        }

        screen
            .into_iter()
            .enumerate()
            .map(|(row, line)| {
                if row < min_row
                    || row > max_row
                    || row < screen_selection.start.row
                    || row > screen_selection.end.row
                    || is_image_line(&line)
                {
                    return line;
                }
                let line_width = visible_width(&line);
                let (start, end) = Self::get_selection_columns(
                    &line,
                    row,
                    &screen_selection,
                    min_column,
                    max_column,
                );
                if end <= start {
                    return line;
                }
                let before = slice_by_column(&line, 0, start, true);
                let selected = slice_by_column(&line, start, end - start, true);
                let after = slice_by_column(&line, end, line_width.saturating_sub(end), true);
                format!(
                    "{before}{}{after}",
                    Self::apply_selection_highlight(&selected)
                )
            })
            .collect()
    }

    fn copy_selection_to_clipboard(&mut self) {
        let Some(selection) = self.get_selection_bounds() else {
            return;
        };
        let source_lines: Vec<String> = match selection.start.scroll_view {
            None => self.previous_screen.clone(),
            Some(key) => {
                let (Some(layout), Some(scroll_view)) =
                    (self.current_layout.as_ref(), self.scroll_view_by_key(key))
                else {
                    return;
                };
                let Some(lines) = get_scroll_view_box(layout, &scroll_view)
                    .and_then(|layout_box| layout_box.scroll_content_lines.clone())
                else {
                    return;
                };
                lines
            }
        };

        let mut lines: Vec<String> = Vec::new();
        for row in selection.start.row..=selection.end.row {
            let line = source_lines.get(row).cloned().unwrap_or_default();
            let max_column = visible_width(&line);
            let (start, end) = Self::get_selection_columns(&line, row, &selection, 0, max_column);
            let slice = slice_by_column(&line, start, end.saturating_sub(start), true);
            lines.push(strip_terminal_sequences(&slice).trim_end().to_string());
        }
        let text = lines.join("\n");
        if text.is_empty() {
            return;
        }
        let sequence = format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()));
        self.core
            .with_terminal(|terminal| terminal.write(&sequence));
        self.flash("Copied!", None);
    }

    /// Handle a selection mouse event (press, drag, release).
    fn handle_selection_mouse_event(&mut self, event: SgrMouseEvent) {
        if (event.button & 3) != 0 {
            return;
        }
        let anchor_scroll_view = self
            .selection_anchor
            .as_ref()
            .and_then(|anchor| anchor.scroll_view);
        let point = self.get_selection_point(event, anchor_scroll_view);

        if event.release {
            if !self.selection_press_active {
                return;
            }
            self.selection_press_active = false;
            let Some(anchor) = self.selection_anchor.clone() else {
                return;
            };
            self.update_selection_focus(point.clone());
            let clicked_url = if !self.selection_dragged
                && anchor.scroll_view == point.scroll_view
                && anchor.row == point.row
                && anchor.col == point.col
            {
                self.pressed_url.clone()
            } else {
                None
            };
            self.pressed_url = None;
            if let Some(url) = clicked_url {
                self.selection_anchor = None;
                self.selection_focus = None;
                self.clicked_url = Some(url);
                self.core.request_render();
                return;
            }
            self.copy_selection_to_clipboard();
            self.core.request_render();
            return;
        }

        if (event.button & 32) != 0 {
            if !self.selection_press_active || self.selection_anchor.is_none() {
                return;
            }
            self.selection_dragged = true;
            self.last_click = None;
            self.pressed_url = None;
            self.update_selection_focus(point);
            self.core.request_render();
            return;
        }

        self.selection_press_active = true;
        let scroll_view = if !self.core.has_overlay()
            && let Some(layout) = &self.current_layout
        {
            get_scroll_views_at(layout, event.x, event.y)
                .first()
                .map(Self::scroll_view_key)
        } else {
            None
        };
        let anchor = self.get_selection_point(event, scroll_view);
        let word = self.get_word_selection(&anchor);
        let click_count = self.get_click_count(&anchor, word.as_ref());
        let range = match click_count {
            2 => word,
            3 => Some(self.get_line_selection(&anchor)),
            _ => None,
        };
        self.selection_granularity = match (&range, click_count) {
            (Some(_), 2) => SelectionGranularity::Word,
            (Some(_), _) => SelectionGranularity::Line,
            (None, _) => SelectionGranularity::Character,
        };
        self.selection_initial_range = range.clone();
        self.selection_anchor = Some(range.as_ref().map_or(anchor.clone(), |r| r.start.clone()));
        self.selection_focus = Some(range.as_ref().map_or(anchor, |r| r.end.clone()));
        self.selection_dragged = false;
        self.pressed_url = if range.is_some() {
            None
        } else {
            let row = event.y.clamp(0, self.core.rows() as i64 - 1).max(0) as usize;
            let col = event.x.clamp(0, self.core.columns() as i64 - 1).max(0) as usize;
            get_osc8_link_at_column(
                self.previous_screen.get(row).map_or("", String::as_str),
                col,
            )
        };
        self.core.request_render();
    }

    /// URL activated by the last primary click, if any (TS calls `openUrl`).
    pub fn take_clicked_url(&mut self) -> Option<String> {
        self.clicked_url.take()
    }

    /// Handle viewport input (wheel events for now); returns `true` when consumed.
    ///
    /// The TS version registers this as an input listener in the constructor;
    /// here the renderer's input handler calls it before the component dispatch
    /// because the listener needs `&mut self`.
    pub fn handle_viewport_input(&mut self, data: &str) -> bool {
        if data == "\x1b[O" {
            // Focus out discards an active selection.
            let had_active_selection = self.selection_press_active;
            self.selection_press_active = false;
            self.pressed_url = None;
            self.selection_dragged = false;
            if had_active_selection {
                let had_selection = self.get_selection_bounds().is_some();
                self.selection_anchor = None;
                self.selection_focus = None;
                self.selection_granularity = SelectionGranularity::Character;
                self.selection_initial_range = None;
                if had_selection {
                    self.core.request_render();
                }
            }
            self.last_click = None;
            return true;
        }
        if data == "\x1b[I" {
            return true;
        }
        if let Some((direction, x, y)) = Self::parse_wheel_event(data) {
            self.route_wheel(direction, x, y);
            return true;
        }
        if let Some(event) = Self::parse_sgr_mouse_event(data) {
            self.handle_selection_mouse_event(event);
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
        screen = self.apply_selection(screen);
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

/// Base64 for the OSC 52 clipboard payload.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}
