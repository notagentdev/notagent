use std::collections::HashMap;
use std::rc::Rc;

use crate::alt_screen_search::{
    AltScreenSearchComponent, AltScreenSearchMatch, find_alt_screen_search_matches,
    get_alt_screen_search_match_key,
};
use crate::components::alt_screen_flash::AltScreenFlashContainer;
use crate::components::scroll_view::{ScrollView, ScrollViewOptions, ScrollViewState};
use crate::keybindings::keybindings_match;
use crate::keys::is_key_release;
use crate::layout::{
    LayoutFrame, ScrollbarGeometry, get_scroll_view_box, get_scroll_views_at,
    get_scrollbar_geometry, render_layout_frame,
};
use crate::layout_node::ScrollStateRef;
use crate::terminal::Terminal;
use crate::terminal_image::{
    ImageProtocol, TerminalCapabilities, delete_all_kitty_images, delete_all_kitty_placements,
    delete_kitty_image, get_capabilities, get_kitty_image_placement, is_image_line,
    set_capabilities,
};
use crate::tui::{
    CURSOR_MARKER, Component, ComponentRef, Line, OverlayAnchor, OverlayHandle, OverlayMargin,
    OverlayOptions, SizeValue, TuiCore, TuiMode, TuiStopOptions, component_ref,
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
/// Whether `line` starts with an OSC 133;A prompt marker.
fn is_osc133_prompt_start(line: &str) -> bool {
    line.starts_with("\x1b]133;A\x07") || line.starts_with("\x1b]133;A\x1b\\")
}

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
    pub right_click_paste: bool,
}

impl Default for TuiAltScreenOptions {
    fn default() -> Self {
        Self {
            wheel_scroll_lines: 1,
            mouse: true,
            search_match_style: Rc::new(|text| format!("\x1b[4m{text}\x1b[24m")),
            search_current_match_style: Rc::new(|text| format!("\x1b[1;7m{text}\x1b[22;27m")),
            right_click_paste: false,
        }
    }
}

/// Renders into the terminal's alternate screen.
pub struct TuiAltScreen {
    core: TuiCore,
    previous_screen: Vec<Line>,
    last_document: Vec<Line>,
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
    scrollbar_hover: Option<usize>,
    scrollbar_drag: Option<ScrollbarDrag>,
    selection_drag_pointer: Option<(i64, i64)>,
    selection_auto_scroll_direction: i64,
    selection_auto_scroll_deadline: Option<std::time::Instant>,
    right_click_paste_requested: bool,
    active_search: Option<ActiveSearch>,
}

/// The open transcript search.
struct ActiveSearch {
    component: Rc<std::cell::RefCell<AltScreenSearchComponent>>,
    overlay: Option<OverlayHandle>,
    query: String,
    matches: Vec<AltScreenSearchMatch>,
    selected_index: i64,
    selected_key: Option<String>,
    anchor_row: usize,
    selection_mode: SearchSelectionMode,
}

/// How `refresh_search` picks the selected match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchSelectionMode {
    Query,
    Next,
    Previous,
    Retain,
}

/// One highlighted run inside a screen row.
#[derive(Debug, Clone, Copy)]
struct SearchHighlightRange {
    start_col: usize,
    end_col: usize,
    current: bool,
}

/// An in-progress scrollbar thumb drag.
#[derive(Debug, Clone, Copy)]
struct ScrollbarDrag {
    scroll_view: usize,
    grab_offset: i64,
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

/// Rows of overlap kept when paging the viewport.
const PAGE_SCROLL_OVERLAP: i64 = 4;

const FOCUS_IN: &str = "\x1b[I";
const FOCUS_OUT: &str = "\x1b[O";

/// Interval of the selection auto-scroll timer.
const SELECTION_AUTO_SCROLL_INTERVAL_MS: u64 = 50;

struct ImplicitDocument {
    core: TuiCore,
}

impl Component for ImplicitDocument {
    fn render(&mut self, width: usize) -> Vec<Line> {
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
            scrollbar_hover: None,
            scrollbar_drag: None,
            selection_drag_pointer: None,
            selection_auto_scroll_direction: 0,
            selection_auto_scroll_deadline: None,
            right_click_paste_requested: false,
            active_search: None,
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
        self.update_scrollbar_hover(x, y);
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

    // === Transcript search ===

    /// Scroll to the previous or next OSC 133 prompt marker.
    fn scroll_to_prompt(&mut self, direction: i64) {
        let Some(layout) = &self.current_layout else {
            return;
        };
        let scroll_view = self.primary_scroll_state();
        let Some(lines) = get_scroll_view_box(layout, &scroll_view)
            .and_then(|layout_box| layout_box.scroll_content_lines.clone())
        else {
            return;
        };
        let mut row = scroll_view.borrow().scroll_top() as i64 + direction;
        while row >= 0 && row < lines.len() as i64 {
            if is_osc133_prompt_start(&lines[row as usize]) {
                scroll_view.borrow_mut().scroll_to_line(row, false);
                self.core.request_render();
                return;
            }
            row += direction;
        }
    }

    fn open_search(&mut self) {
        if let Some(search) = &self.active_search {
            if let Some(overlay) = &search.overlay {
                overlay.focus();
            }
            return;
        }
        let component = Rc::new(std::cell::RefCell::new(AltScreenSearchComponent::new()));
        let overlay = self.core.show_overlay(
            component.clone(),
            Some(OverlayOptions {
                anchor: Some(OverlayAnchor::TopRight),
                width: Some(SizeValue::Percent(40.0)),
                min_width: Some(24),
                margin: Some(OverlayMargin::all(1)),
                ..OverlayOptions::default()
            }),
        );
        self.active_search = Some(ActiveSearch {
            component,
            overlay: Some(overlay),
            query: String::new(),
            matches: Vec::new(),
            selected_index: -1,
            selected_key: None,
            anchor_row: self.primary_scroll_state().borrow().scroll_top(),
            selection_mode: SearchSelectionMode::Query,
        });
    }

    fn close_search(&mut self) {
        let Some(search) = self.active_search.take() else {
            return;
        };
        if let Some(overlay) = &search.overlay {
            overlay.hide();
        }
        self.core.request_render();
    }

    fn update_search_query(&mut self, query: String) {
        let scroll_top = self.primary_scroll_state().borrow().scroll_top();
        let Some(search) = &mut self.active_search else {
            return;
        };
        if query == search.query {
            return;
        }
        search.anchor_row = search
            .matches
            .get(usize::try_from(search.selected_index).unwrap_or(usize::MAX))
            .and_then(|selected| selected.segments.first())
            .map_or(scroll_top, |segment| segment.row);
        search.query = query;
        search.selection_mode = SearchSelectionMode::Query;
        search.component.borrow_mut().set_result(-1, 0);
        self.core.request_render();
    }

    fn navigate_search(&mut self, direction: i64) {
        let Some(search) = &mut self.active_search else {
            return;
        };
        if search.query.is_empty() {
            return;
        }
        search.selection_mode = if direction < 0 {
            SearchSelectionMode::Previous
        } else {
            SearchSelectionMode::Next
        };
        self.core.request_render();
    }

    /// Recompute the matches for `layout`; returns `true` when the viewport moved.
    fn refresh_search(&mut self, layout: &LayoutFrame) -> bool {
        if self.active_search.is_none() {
            return false;
        }
        let scroll_view = layout
            .primary_scroll_view
            .clone()
            .unwrap_or_else(|| self.implicit_scroll_state.clone());
        let layout_box = get_scroll_view_box(layout, &scroll_view);
        let lines = layout_box.and_then(|layout_box| layout_box.scroll_content_lines.clone());
        let search = self.active_search.as_mut().expect("search present");

        let Some(lines) = lines.filter(|_| !search.query.trim().is_empty()) else {
            search.matches = Vec::new();
            search.selected_index = -1;
            search.selected_key = None;
            search.selection_mode = SearchSelectionMode::Retain;
            search.component.borrow_mut().set_result(-1, 0);
            return false;
        };

        let should_reveal_selection = search.selection_mode != SearchSelectionMode::Retain;
        let matches = find_alt_screen_search_matches(&lines, &search.query);
        let exact_index = search.selected_key.as_ref().map_or(-1, |key| {
            matches
                .iter()
                .position(|search_match| &get_alt_screen_search_match_key(search_match) == key)
                .map_or(-1, |index| index as i64)
        });
        let mut selected_index = -1;
        if !matches.is_empty() {
            let count = matches.len() as i64;
            selected_index = match search.selection_mode {
                SearchSelectionMode::Query => matches
                    .iter()
                    .position(|search_match| {
                        search_match
                            .segments
                            .first()
                            .map_or(0, |segment| segment.row)
                            >= search.anchor_row
                    })
                    .map_or(0, |index| index as i64),
                SearchSelectionMode::Next => {
                    let base_index = if exact_index >= 0 {
                        exact_index
                    } else {
                        search.selected_index.min(count - 1)
                    };
                    if base_index < 0 {
                        0
                    } else {
                        (base_index + 1) % count
                    }
                }
                SearchSelectionMode::Previous => {
                    let base_index = if exact_index >= 0 {
                        exact_index
                    } else {
                        search.selected_index.min(count - 1)
                    };
                    if base_index < 0 {
                        count - 1
                    } else {
                        (base_index - 1 + count) % count
                    }
                }
                SearchSelectionMode::Retain => {
                    if exact_index >= 0 {
                        exact_index
                    } else {
                        search.selected_index.max(0).min(count - 1)
                    }
                }
            };
        }

        search.matches = matches;
        search.selected_index = selected_index;
        search.selected_key = usize::try_from(selected_index)
            .ok()
            .and_then(|index| search.matches.get(index))
            .map(get_alt_screen_search_match_key);
        search.selection_mode = SearchSelectionMode::Retain;
        search
            .component
            .borrow_mut()
            .set_result(selected_index, search.matches.len());
        if !should_reveal_selection {
            return false;
        }

        let selected = usize::try_from(selected_index)
            .ok()
            .and_then(|index| search.matches.get(index));
        let first_segment = selected
            .and_then(|selected| selected.segments.first())
            .copied();
        let last_segment = selected
            .and_then(|selected| selected.segments.last())
            .copied();
        let viewport_height = scroll_view.borrow().viewport_height() as i64;
        let (Some(_), Some(first_segment), Some(last_segment)) =
            (layout_box, first_segment, last_segment)
        else {
            return false;
        };
        if viewport_height <= 0 {
            return false;
        }
        let before = scroll_view.borrow().scroll_top() as i64;
        let visible_bottom = before + viewport_height - 1;
        let mut target = before;
        if (first_segment.row as i64) < before || last_segment.row as i64 > visible_bottom {
            target = first_segment.row as i64 - viewport_height / 3;
        }
        scroll_view.borrow_mut().scroll_to_line(target, true);
        scroll_view.borrow().scroll_top() as i64 != before
    }

    /// Style the plain runs of `text`, leaving embedded ANSI codes intact.
    fn apply_search_text_highlight(&self, text: &str, current: bool) -> String {
        let style = if current {
            &self.options.search_current_match_style
        } else {
            &self.options.search_match_style
        };
        let mut result = String::new();
        let mut plain_start = 0;
        let mut index = 0;
        while index < text.len() {
            let Some(ansi) = extract_ansi_code(text, index) else {
                index += text[index..].chars().next().map_or(1, char::len_utf8);
                continue;
            };
            if index > plain_start {
                result.push_str(&style(&text[plain_start..index]));
            }
            result.push_str(ansi.code);
            index += ansi.length;
            plain_start = index;
        }
        if plain_start < text.len() {
            result.push_str(&style(&text[plain_start..]));
        }
        result
    }

    fn apply_search_highlights(&self, screen: Vec<Line>, layout: &LayoutFrame) -> Vec<Line> {
        let Some(search) = &self.active_search else {
            return screen;
        };
        if search.selected_index < 0 || search.matches.is_empty() {
            return screen;
        }
        let scroll_view = layout
            .primary_scroll_view
            .clone()
            .unwrap_or_else(|| self.implicit_scroll_state.clone());
        let Some(layout_box) = get_scroll_view_box(layout, &scroll_view) else {
            return screen;
        };

        let scrollbar_column = get_scrollbar_geometry(layout_box).map(|geometry| geometry.column);
        let min_row = layout_box.rect.y.max(layout_box.clip.y).max(0);
        let max_row = (screen.len() as i64)
            .min(layout_box.rect.y + layout_box.rect.height)
            .min(layout_box.clip.y + layout_box.clip.height);
        let min_column = layout_box.rect.x.max(layout_box.clip.x).max(0);
        let max_column = (self.core.columns() as i64)
            .min(layout_box.rect.x + layout_box.rect.width)
            .min(layout_box.clip.x + layout_box.clip.width)
            .min(scrollbar_column.unwrap_or(i64::MAX));
        let scroll_top = scroll_view.borrow().scroll_top() as i64;

        let mut ranges_by_row: HashMap<usize, Vec<SearchHighlightRange>> = HashMap::new();
        for (match_index, search_match) in search.matches.iter().enumerate() {
            for segment in &search_match.segments {
                let row = layout_box.rect.y + segment.row as i64 - scroll_top;
                if row < min_row || row >= max_row {
                    continue;
                }
                let start_col = min_column.max(layout_box.rect.x + segment.start_col as i64);
                let end_col = max_column.min(layout_box.rect.x + segment.end_col as i64);
                if end_col <= start_col {
                    continue;
                }
                ranges_by_row
                    .entry(row as usize)
                    .or_default()
                    .push(SearchHighlightRange {
                        start_col: start_col as usize,
                        end_col: end_col as usize,
                        current: match_index as i64 == search.selected_index,
                    });
            }
        }

        let mut result = screen;
        for (row, mut ranges) in ranges_by_row {
            let Some(line) = result.get_mut(row) else {
                continue;
            };
            if is_image_line(line) {
                continue;
            }
            let line_width = visible_width(line);
            ranges.sort_by_key(|range| std::cmp::Reverse(range.start_col));
            for range in ranges {
                let start_col = range.start_col.min(line_width);
                let end_col = range.end_col.min(line_width);
                if end_col <= start_col {
                    continue;
                }
                let before = slice_by_column(line, 0, start_col, true);
                let highlighted = slice_by_column(line, start_col, end_col - start_col, true);
                let after =
                    slice_by_column(line, end_col, line_width.saturating_sub(end_col), true);
                *line = Line::from(format!(
                    "{before}{}{after}",
                    self.apply_search_text_highlight(&highlighted, range.current)
                ));
            }
        }
        result
    }

    // === Scrollbar hover and drag ===

    /// The scrollbar thumb under the pointer, if any.
    fn get_scrollbar_target_at(&self, x: i64, y: i64) -> Option<(usize, ScrollbarGeometry)> {
        if self.core.has_overlay() {
            return None;
        }
        let layout = self.current_layout.as_ref()?;
        get_scroll_views_at(layout, x, y)
            .into_iter()
            .find_map(|scroll_view| {
                let geometry =
                    get_scroll_view_box(layout, &scroll_view).and_then(get_scrollbar_geometry)?;
                (x == geometry.column
                    && y >= geometry.thumb_top
                    && y < geometry.thumb_top + geometry.thumb_height)
                    .then(|| (Self::scroll_view_key(&scroll_view), geometry))
            })
    }

    fn set_scrollbar_hover(&mut self, scroll_view: Option<usize>) {
        if scroll_view == self.scrollbar_hover {
            return;
        }
        if let Some(previous) = self
            .scrollbar_hover
            .and_then(|key| self.scroll_view_by_key(key))
        {
            previous.borrow_mut().set_scrollbar_active_state(false);
        }
        self.scrollbar_hover = scroll_view;
        if let Some(current) = self
            .scrollbar_hover
            .and_then(|key| self.scroll_view_by_key(key))
        {
            current.borrow_mut().set_scrollbar_active_state(true);
        }
    }

    fn update_scrollbar_hover(&mut self, x: i64, y: i64) {
        let target = self.get_scrollbar_target_at(x, y).map(|(key, _)| key);
        self.set_scrollbar_hover(target);
    }

    fn stop_scrollbar_hover(&mut self) {
        self.set_scrollbar_hover(None);
    }

    /// Handle a scrollbar mouse event; returns `true` when consumed.
    fn handle_scrollbar_mouse_event(&mut self, event: SgrMouseEvent) -> bool {
        if let Some(drag) = self.scrollbar_drag {
            if event.release {
                self.stop_scrollbar_drag();
                return true;
            }
            if let Some(scroll_view) = self.scroll_view_by_key(drag.scroll_view)
                && let Some(geometry) = self
                    .current_layout
                    .as_ref()
                    .and_then(|layout| get_scroll_view_box(layout, &scroll_view))
                    .and_then(get_scrollbar_geometry)
            {
                let max_thumb_offset = geometry.track_height - geometry.thumb_height;
                let thumb_offset = (event.y - geometry.track_top - drag.grab_offset)
                    .clamp(0, max_thumb_offset.max(0));
                let scroll_top = if max_thumb_offset == 0 {
                    0
                } else {
                    ((thumb_offset as f64 / max_thumb_offset as f64)
                        * geometry.max_scroll_top as f64)
                        .round() as i64
                };
                scroll_view.borrow_mut().scroll_to_line(scroll_top, false);
            }
            self.core.request_render();
            return true;
        }

        if event.release || (event.button & 32) != 0 || (event.button & 3) != 0 {
            return false;
        }
        let Some((scroll_view, geometry)) = self.get_scrollbar_target_at(event.x, event.y) else {
            return false;
        };
        self.stop_selection_auto_scroll();
        self.selection_press_active = false;
        self.selection_anchor = None;
        self.selection_focus = None;
        self.selection_granularity = SelectionGranularity::Character;
        self.selection_initial_range = None;
        self.last_click = None;
        self.pressed_url = None;
        self.selection_dragged = false;
        self.set_scrollbar_hover(Some(scroll_view));
        self.scrollbar_drag = Some(ScrollbarDrag {
            scroll_view,
            grab_offset: event.y - geometry.thumb_top,
        });
        self.core.request_render();
        true
    }

    fn stop_scrollbar_drag(&mut self) {
        self.scrollbar_drag = None;
    }

    // === Selection auto-scroll ===

    /// Arm or disarm the auto-scroll timer for a drag that left the viewport.
    fn update_selection_auto_scroll(&mut self, event: SgrMouseEvent) {
        let Some(key) = self
            .selection_anchor
            .as_ref()
            .and_then(|anchor| anchor.scroll_view)
        else {
            self.stop_selection_auto_scroll();
            return;
        };
        let bounds = self
            .current_layout
            .as_ref()
            .zip(self.scroll_view_by_key(key))
            .and_then(|(layout, scroll_view)| get_scroll_view_box(layout, &scroll_view))
            .filter(|layout_box| layout_box.rect.height > 0 && layout_box.clip.height > 0)
            .map(|layout_box| {
                (
                    layout_box.rect.y.max(layout_box.clip.y).max(0),
                    (self.core.rows() as i64 - 1)
                        .min(layout_box.rect.y + layout_box.rect.height - 1)
                        .min(layout_box.clip.y + layout_box.clip.height - 1),
                )
            });
        let Some((visible_top, visible_bottom)) = bounds else {
            self.stop_selection_auto_scroll();
            return;
        };
        self.selection_drag_pointer = Some((event.x, event.y));
        self.selection_auto_scroll_direction = if event.y <= visible_top {
            -1
        } else if event.y >= visible_bottom {
            1
        } else {
            0
        };
        if self.selection_auto_scroll_direction == 0 {
            self.stop_selection_auto_scroll();
            return;
        }
        if self.selection_auto_scroll_deadline.is_none() {
            self.selection_auto_scroll_deadline = Some(
                std::time::Instant::now()
                    + std::time::Duration::from_millis(SELECTION_AUTO_SCROLL_INTERVAL_MS),
            );
        }
    }

    /// When the next auto-scroll step is due, or `None` while idle.
    pub fn selection_auto_scroll_deadline(&self) -> Option<std::time::Instant> {
        self.selection_auto_scroll_deadline
    }

    pub fn auto_scroll_selection(&mut self) {
        let scroll_view = self
            .selection_anchor
            .as_ref()
            .and_then(|anchor| anchor.scroll_view)
            .and_then(|key| self.scroll_view_by_key(key).map(|state| (key, state)));
        let direction = self.selection_auto_scroll_direction;
        let (Some((key, state)), Some(pointer)) = (scroll_view, self.selection_drag_pointer) else {
            self.stop_selection_auto_scroll();
            return;
        };
        if direction == 0 {
            self.stop_selection_auto_scroll();
            return;
        }
        // Rearm from the previous deadline, as `setInterval` does.
        self.selection_auto_scroll_deadline = Some(
            self.selection_auto_scroll_deadline
                .unwrap_or_else(std::time::Instant::now)
                + std::time::Duration::from_millis(SELECTION_AUTO_SCROLL_INTERVAL_MS),
        );
        let remaining = state.borrow_mut().scroll_by_lines(direction);
        if remaining == direction {
            self.stop_selection_auto_scroll();
            return;
        }
        if let Some(point) = self.get_scroll_selection_point(key, pointer.0, pointer.1) {
            self.update_selection_focus(point);
        }
        self.core.request_render();
    }

    fn stop_selection_auto_scroll(&mut self) {
        self.selection_auto_scroll_deadline = None;
        self.selection_auto_scroll_direction = 0;
        self.selection_drag_pointer = None;
    }

    /// when consumed. The caller retrieves the request via
    /// [`Self::take_right_click_paste`].
    fn handle_right_click_paste(&mut self, event: SgrMouseEvent) -> bool {
        if !self.options.right_click_paste || !cfg!(windows) || event.release || event.button != 2 {
            return false;
        }
        self.right_click_paste_requested = true;
        true
    }

    pub fn take_right_click_paste(&mut self) -> bool {
        std::mem::take(&mut self.right_click_paste_requested)
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
        let number = |part: Option<&str>| -> Option<i64> {
            let part = part?;
            (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        };
        let button = number(parts.next())?;
        let x = number(parts.next())?;
        let y = number(parts.next())?;
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

    fn get_selection_source_line(&self, point: &SelectionPoint) -> Line {
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

    fn apply_selection(&self, screen: Vec<Line>) -> Vec<Line> {
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
                Line::from(format!(
                    "{before}{}{after}",
                    Self::apply_selection_highlight(&selected)
                ))
            })
            .collect()
    }

    fn copy_selection_to_clipboard(&mut self) {
        let Some(selection) = self.get_selection_bounds() else {
            return;
        };
        let source_lines: Vec<Line> = match selection.start.scroll_view {
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
            self.stop_selection_auto_scroll();
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
            self.update_selection_auto_scroll(event);
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
                self.previous_screen
                    .get(row)
                    .map_or("", |line| line.as_ref()),
                col,
            )
        };
        self.core.request_render();
    }

    pub fn take_clicked_url(&mut self) -> Option<String> {
        self.clicked_url.take()
    }

    /// Handle viewport input (wheel events for now); returns `true` when consumed.
    /// here the renderer's input handler calls it before the component dispatch
    /// because the listener needs `&mut self`.
    pub fn handle_viewport_input(&mut self, data: &str) -> bool {
        if data == FOCUS_OUT {
            let had_active_selection = self.selection_press_active;
            let had_non_empty_active_selection =
                had_active_selection && self.get_selection_bounds().is_some();
            self.selection_press_active = false;
            self.stop_selection_auto_scroll();
            self.stop_scrollbar_hover();
            self.stop_scrollbar_drag();
            self.pressed_url = None;
            self.selection_dragged = false;
            if had_active_selection {
                self.selection_anchor = None;
                self.selection_focus = None;
                self.selection_granularity = SelectionGranularity::Character;
                self.selection_initial_range = None;
                if had_non_empty_active_selection {
                    self.core.request_render();
                }
            }
            self.last_click = None;
            return true;
        }
        if data == FOCUS_IN {
            return true;
        }

        if let Some((direction, x, y)) = Self::parse_wheel_event(data) {
            self.route_wheel(direction, x, y);
            return true;
        }
        if let Some(event) = Self::parse_sgr_mouse_event(data) {
            if self.handle_right_click_paste(event) {
                return true;
            }
            let handled = self.handle_scrollbar_mouse_event(event);
            if self.scrollbar_drag.is_none() {
                self.update_scrollbar_hover(event.x, event.y);
            }
            if !handled {
                self.handle_selection_mouse_event(event);
            }
            return true;
        }
        if Self::is_mouse_sequence(data) {
            return true;
        }

        let is_release = is_key_release(data);
        if keybindings_match(data, "tui.altScreen.search") {
            if !is_release {
                self.open_search();
            }
            return true;
        }
        if self
            .active_search
            .as_ref()
            .and_then(|search| search.overlay.as_ref())
            .is_some_and(OverlayHandle::is_focused)
        {
            if keybindings_match(data, "tui.altScreen.searchNext") {
                if !is_release {
                    self.navigate_search(1);
                }
                return true;
            }
            if keybindings_match(data, "tui.altScreen.searchPrevious") {
                if !is_release {
                    self.navigate_search(-1);
                }
                return true;
            }
            if keybindings_match(data, "tui.altScreen.searchClose") {
                if !is_release {
                    self.close_search();
                }
                return true;
            }
        }
        let viewport_height = self.primary_scroll_state().borrow().viewport_height() as i64;
        if keybindings_match(data, "tui.altScreen.pageUp") {
            if !is_release {
                self.scroll_by(-(viewport_height - PAGE_SCROLL_OVERLAP).max(1));
            }
            return true;
        }
        if keybindings_match(data, "tui.altScreen.pageDown") {
            if !is_release {
                self.scroll_by((viewport_height - PAGE_SCROLL_OVERLAP).max(1));
            }
            return true;
        }
        if keybindings_match(data, "tui.altScreen.halfPageUp") {
            if !is_release {
                self.scroll_by(-(viewport_height / 2).max(1));
            }
            return true;
        }
        if keybindings_match(data, "tui.altScreen.halfPageDown") {
            if !is_release {
                self.scroll_by((viewport_height / 2).max(1));
            }
            return true;
        }
        if keybindings_match(data, "tui.altScreen.lineUp") {
            if !is_release {
                self.scroll_by(-1);
            }
            return true;
        }
        if keybindings_match(data, "tui.altScreen.lineDown") {
            if !is_release {
                self.scroll_by(1);
            }
            return true;
        }
        if keybindings_match(data, "tui.altScreen.previousPrompt") {
            if !is_release {
                self.scroll_to_prompt(-1);
            }
            return true;
        }
        if keybindings_match(data, "tui.altScreen.nextPrompt") {
            if !is_release {
                self.scroll_to_prompt(1);
            }
            return true;
        }
        if keybindings_match(data, "tui.altScreen.top") {
            if !is_release {
                self.scroll_to_top();
            }
            return true;
        }
        if keybindings_match(data, "tui.altScreen.bottom") {
            if !is_release {
                self.scroll_to_bottom();
            }
            return true;
        }
        false
    }

    /// Whether `data` is a mouse report that must not reach the components.
    fn is_mouse_sequence(data: &str) -> bool {
        if Self::parse_sgr_mouse_event(data).is_some() {
            return true;
        }
        data.len() == 6 && data.starts_with("\x1b[M")
    }

    /// Feed terminal input: viewport handling first, then the TUI dispatch.
    pub fn handle_terminal_input(&mut self, data: &str) {
        if self.handle_viewport_input(data) {
            return;
        }
        self.core.handle_terminal_input(data);
        self.poll_search_query();
    }

    /// Adopt a query the search input changed while handling the last input.
    /// `onQueryChange` callback, which would need `&mut` access to the renderer
    /// while the component itself is borrowed.
    fn poll_search_query(&mut self) {
        let Some(search) = &self.active_search else {
            return;
        };
        let changed = search.component.borrow_mut().take_query_changed();
        if !changed {
            return;
        }
        let query = search.component.borrow().query().to_string();
        self.update_search_query(query);
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
    pub fn last_document(&self) -> &[Line] {
        &self.last_document
    }

    fn render_root(&self, width: usize) -> Vec<Line> {
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
    fn prepare_kitty_screen(&mut self, screen: &[Line]) -> (Vec<Line>, String) {
        let mut visible_image_ids: Vec<u32> = Vec::new();
        let lines: Vec<Line> = screen
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
                        Line::from(placement.replacement_line)
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
            let mut document: Vec<Line> = self
                .render_root(width)
                .iter()
                .map(|line| Line::from(strip_osc133_zone_prefix(line).replace(CURSOR_MARKER, "")))
                .collect();
            self.core.apply_line_resets(&mut document);
            self.last_document = document
                .into_iter()
                .map(|line| {
                    if is_image_line(&line) || visible_width(&line) <= width {
                        line
                    } else {
                        Line::from(slice_by_column(&line, 0, width, true))
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

    /// Render the frame the render loop reported as due.
    /// Counterpart of [`TuiCore::wait_until_render_due`]: it consumes the
    /// pending request, so a loop that waits and then calls this cannot spin.
    pub fn render_pending_frame(&mut self) {
        if self.core.begin_frame() {
            self.do_render();
        }
    }

    /// Render the pending frame once its throttle deadline has passed.
    /// Unlike [`TuiCore::wait_until_render_due`] it returns right away when no
    /// frame is pending.
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

    fn composite_flashes(&mut self, screen: Vec<Line>, width: usize, height: usize) -> Vec<Line> {
        self.flashes.borrow_mut().expire();
        let mut flash_lines = self.flashes.borrow_mut().render(width);
        if flash_lines.len() > height {
            flash_lines = flash_lines.split_off(flash_lines.len() - height);
        }
        if flash_lines.is_empty() {
            return screen;
        }
        let mut result = screen;
        result.resize(result.len().max(height), Line::from(""));
        for (row, flash_line) in flash_lines.iter().enumerate() {
            let flash_width = visible_width(flash_line);
            if flash_width == 0 {
                continue;
            }
            result[row] = Line::from(crate::tui::composite_tui_line(
                &result[row],
                flash_line,
                width.saturating_sub(flash_width),
                flash_width,
                width,
            ));
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
        let mut next_layout = render_layout_frame(&root, width, height);
        if self.refresh_search(&next_layout) {
            next_layout = render_layout_frame(&root, width, height);
        }

        // A line without the zone prefix passes through as the same shared
        // line, keeping its identity for the row diff below.
        let mut screen: Vec<Line> = next_layout
            .lines
            .iter()
            .map(|line| {
                let stripped = strip_osc133_zone_prefix(line);
                if stripped.len() == line.len() {
                    line.clone()
                } else {
                    Line::from(stripped)
                }
            })
            .collect();
        screen = self.apply_search_highlights(screen, &next_layout);
        screen = self.core.composite_overlays(screen, width, height);
        if screen.len() > height {
            screen = screen[screen.len() - height..].to_vec();
        }
        screen = self.apply_selection(screen);
        screen = self.composite_flashes(screen, width, height);

        let cursor_pos = self.core.extract_cursor_position(&mut screen, height);
        self.core.apply_line_resets(&mut screen);
        let mut screen: Vec<Line> = screen
            .into_iter()
            .map(|line| {
                if is_image_line(&line) || visible_width(&line) <= width {
                    line
                } else {
                    Line::from(slice_by_column(&line, 0, width, true))
                }
            })
            .collect();
        screen.resize(height, Line::from(""));

        let full_redraw = self.previous_screen.is_empty()
            || self.previous_screen_width != width
            || self.previous_screen_height != height;
        let images_need_redraw = screen.iter().enumerate().any(|(row, line)| {
            let previous = self
                .previous_screen
                .get(row)
                .map_or("", |line| line.as_ref());
            line.as_ref() != previous && (is_image_line(line) || is_image_line(previous))
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
            // Pointer identity first, content comparison as the fallback —
            // same shape as the main screen's diff.
            let new_line = screen.get(row);
            let old_line = self.previous_screen.get(row);
            let unchanged = match (new_line, old_line) {
                (Some(new), Some(old)) => Line::ptr_eq(new, old) || new == old,
                _ => new_line == old_line,
            };
            if !full_redraw && !images_need_redraw && unchanged {
                continue;
            }
            buffer.push_str(&format!(
                "\x1b[{};1H\x1b[2K{}",
                row + 1,
                prepared_lines.get(row).map_or("", |line| line.as_ref())
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

impl crate::tui::RenderLoop for TuiAltScreen {
    fn core(&self) -> &TuiCore {
        &self.core
    }

    fn render_pending_frame(&mut self) {
        TuiAltScreen::render_pending_frame(self);
    }
}
