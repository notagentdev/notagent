//! Multi-line editor with autocomplete, kill ring, undo and paste markers.
//!
//! Port of `packages/tui/src/components/editor.ts` (2363 LOC).
//!
//! Deviation class 1: `cursorCol` and all string indices are byte offsets
//! instead of UTF-16 code units; every slice happens on grapheme boundaries, so
//! only the numeric values of the accessors differ.

use std::rc::Rc;

use crate::autocomplete::{
    AbortController, AutocompleteItem, AutocompleteProvider, AutocompleteSuggestions,
    SuggestionOptions,
};
use crate::components::select_list::{
    SelectItem, SelectList, SelectListLayoutOptions, SelectListTheme,
};
use crate::keybindings::{keybinding_keys, keybindings_match};
use crate::keys::{decode_printable_key, matches_key};
use crate::kill_ring::{KillRing, KillRingPushOptions};
use crate::tui::{CURSOR_MARKER, Component, Focusable, Line, TuiCore, shared_lines};
use crate::undo_stack::UndoStack;
use crate::utils::{
    graphemes, is_cjk_break, is_whitespace_char, slice_by_column, visible_width, word_segments,
};
use crate::word_navigation::{WordNavigationOptions, find_word_backward, find_word_forward};

/// Whether `segment` is a paste marker such as `[paste #1 +12 lines]`.
pub fn is_paste_marker(segment: &str) -> bool {
    segment.len() >= 10 && parse_paste_marker(segment).is_some()
}

/// Parse a complete paste marker into its id and optional suffix.
fn parse_paste_marker(segment: &str) -> Option<(u64, String)> {
    let rest = segment.strip_prefix("[paste #")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let id: u64 = digits.parse().ok()?;
    let rest = &rest[digits.len()..];
    let body = rest.strip_suffix(']')?;
    if body.is_empty() {
        return Some((id, String::new()));
    }
    let detail = body.strip_prefix(' ')?;
    let valid = detail
        .strip_prefix('+')
        .and_then(|value| value.strip_suffix(" lines"))
        .is_some_and(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
        || detail
            .strip_suffix(" chars")
            .is_some_and(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()));
    valid.then(|| (id, body.to_string()))
}

/// Byte spans of all paste markers in `text` whose id is valid.
fn paste_marker_spans(text: &str, valid_ids: &[u64]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut search_from = 0;
    while let Some(relative) = text[search_from..].find("[paste #") {
        let start = search_from + relative;
        let Some(end_relative) = text[start..].find(']') else {
            break;
        };
        let end = start + end_relative + 1;
        if let Some((id, _)) = parse_paste_marker(&text[start..end])
            && valid_ids.contains(&id)
        {
            spans.push((start, end));
            search_from = end;
            continue;
        }
        search_from = start + "[paste #".len();
    }
    spans
}

/// One segment with its byte offset in the source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Byte offset in the source text.
    pub index: usize,
    /// The segment itself.
    pub segment: String,
}

/// Segment `text`, merging valid paste markers into single atomic segments.
fn segment_with_markers(text: &str, word_granularity: bool, valid_ids: &[u64]) -> Vec<Segment> {
    let base = |text: &str| -> Vec<Segment> {
        if word_granularity {
            word_segments(text)
                .scan(0usize, |offset, segment| {
                    let index = *offset;
                    *offset += segment.len();
                    Some(Segment {
                        index,
                        segment: segment.to_string(),
                    })
                })
                .collect()
        } else {
            graphemes(text)
                .scan(0usize, |offset, segment| {
                    let index = *offset;
                    *offset += segment.len();
                    Some(Segment {
                        index,
                        segment: segment.to_string(),
                    })
                })
                .collect()
        }
    };

    if valid_ids.is_empty() || !text.contains("[paste #") {
        return base(text);
    }
    let markers = paste_marker_spans(text, valid_ids);
    if markers.is_empty() {
        return base(text);
    }

    let mut result: Vec<Segment> = Vec::new();
    let mut marker_index = 0;
    for segment in base(text) {
        while marker_index < markers.len() && markers[marker_index].1 <= segment.index {
            marker_index += 1;
        }
        let marker = markers.get(marker_index).copied();
        match marker {
            Some((start, end)) if segment.index >= start && segment.index < end => {
                if segment.index == start {
                    result.push(Segment {
                        index: start,
                        segment: text[start..end].to_string(),
                    });
                }
            }
            _ => result.push(segment),
        }
    }
    result
}

/// A word-wrapped chunk with its position in the source line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChunk {
    /// The chunk text.
    pub text: String,
    /// First byte index in the source line.
    pub start_index: usize,
    /// Byte index after the chunk in the source line.
    pub end_index: usize,
}

/// Split a line into word-wrapped chunks.
///
/// Wraps at word boundaries where possible and falls back to grapheme-level
/// wrapping for words wider than `max_width`.
pub fn word_wrap_line(
    line: &str,
    max_width: usize,
    pre_segmented: Option<&[Segment]>,
) -> Vec<TextChunk> {
    if line.is_empty() || max_width == 0 {
        return vec![TextChunk {
            text: String::new(),
            start_index: 0,
            end_index: 0,
        }];
    }

    if visible_width(line) <= max_width {
        return vec![TextChunk {
            text: line.to_string(),
            start_index: 0,
            end_index: line.len(),
        }];
    }

    let owned_segments;
    let segments: &[Segment] = match pre_segmented {
        Some(segments) => segments,
        None => {
            owned_segments = segment_with_markers(line, false, &[]);
            &owned_segments
        }
    };

    let mut chunks: Vec<TextChunk> = Vec::new();
    let mut current_width = 0usize;
    let mut chunk_start = 0usize;
    let mut wrap_opp_index: Option<usize> = None;
    let mut wrap_opp_width = 0usize;

    for index in 0..segments.len() {
        let segment = &segments[index];
        let grapheme = segment.segment.as_str();
        let grapheme_width = visible_width(grapheme);
        let char_index = segment.index;
        let is_ws = !is_paste_marker(grapheme) && is_whitespace_char(grapheme);

        if current_width + grapheme_width > max_width {
            if let Some(opportunity) = wrap_opp_index
                && current_width - wrap_opp_width + grapheme_width <= max_width
            {
                chunks.push(TextChunk {
                    text: line[chunk_start..opportunity].to_string(),
                    start_index: chunk_start,
                    end_index: opportunity,
                });
                chunk_start = opportunity;
                current_width -= wrap_opp_width;
            } else if chunk_start < char_index {
                chunks.push(TextChunk {
                    text: line[chunk_start..char_index].to_string(),
                    start_index: chunk_start,
                    end_index: char_index,
                });
                chunk_start = char_index;
                current_width = 0;
            }
            wrap_opp_index = None;
        }

        if grapheme_width > max_width {
            // An atomic segment wider than the line is re-wrapped visually; it
            // stays one unit for cursor movement and editing.
            let sub_chunks = word_wrap_line(grapheme, max_width, None);
            for sub_chunk in &sub_chunks[..sub_chunks.len() - 1] {
                chunks.push(TextChunk {
                    text: sub_chunk.text.clone(),
                    start_index: char_index + sub_chunk.start_index,
                    end_index: char_index + sub_chunk.end_index,
                });
            }
            let last = &sub_chunks[sub_chunks.len() - 1];
            chunk_start = char_index + last.start_index;
            current_width = visible_width(&last.text);
            wrap_opp_index = None;
            continue;
        }

        current_width += grapheme_width;

        let next = segments.get(index + 1);
        if is_ws
            && let Some(next) = next
            && (is_paste_marker(&next.segment) || !is_whitespace_char(&next.segment))
        {
            wrap_opp_index = Some(next.index);
            wrap_opp_width = current_width;
        } else if !is_ws
            && let Some(next) = next
            && !is_whitespace_char(&next.segment)
        {
            let is_cjk = !is_paste_marker(grapheme) && is_cjk_break(grapheme);
            let next_is_cjk = !is_paste_marker(&next.segment) && is_cjk_break(&next.segment);
            if is_cjk || next_is_cjk {
                wrap_opp_index = Some(next.index);
                wrap_opp_width = current_width;
            }
        }
    }

    chunks.push(TextChunk {
        text: line[chunk_start..].to_string(),
        start_index: chunk_start,
        end_index: line.len(),
    });

    chunks
}

/// Text state of the editor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct EditorState {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

/// Undo snapshot: text state plus the paste registry.
#[derive(Clone, Debug)]
struct EditorSnapshot {
    state: EditorState,
    pastes: Vec<(u64, String)>,
    paste_counter: u64,
}

/// One laid out visual line.
struct LayoutLine {
    text: String,
    has_cursor: bool,
    cursor_pos: Option<usize>,
}

/// A visual line of the wrap map.
#[derive(Clone, Copy, Debug)]
struct VisualLine {
    logical_line: usize,
    start_col: usize,
    length: usize,
}

/// Colouring of the editor.
pub struct EditorTheme {
    /// Applied to the horizontal borders.
    pub border_color: Rc<dyn Fn(&str) -> String>,
    /// Theme of the autocomplete list.
    pub select_list: Rc<dyn Fn() -> SelectListTheme>,
}

/// Construction options.
#[derive(Clone, Copy, Debug, Default)]
pub struct EditorOptions {
    /// Horizontal padding in cells.
    pub padding_x: Option<usize>,
    /// Maximum number of visible autocomplete entries.
    pub autocomplete_max_visible: Option<usize>,
}

const ATTACHMENT_AUTOCOMPLETE_DEBOUNCE_MS: u64 = 20;
const DEFAULT_AUTOCOMPLETE_TRIGGER_CHARACTERS: [char; 2] = ['@', '#'];

fn slash_command_select_list_layout() -> SelectListLayoutOptions {
    SelectListLayoutOptions {
        min_primary_column_width: Some(12),
        max_primary_column_width: Some(32),
        truncate_primary: None,
    }
}

fn create_scroll_border(direction: &str, hidden_line_count: usize, width: usize) -> String {
    let indicator = format!("─── {direction} {hidden_line_count} more ");
    let indicator_width = visible_width(&indicator);
    if width >= indicator_width {
        return format!("{indicator}{}", "─".repeat(width - indicator_width));
    }
    let ellipsis: String = "...".chars().take(width).collect();
    let truncated_width = width - visible_width(&ellipsis);
    format!(
        "{}{ellipsis}",
        slice_by_column(&indicator, 0, truncated_width, true)
    )
}

/// State of the pending autocomplete request.
#[derive(Clone, Debug)]
struct PendingAutocomplete {
    start_token: u64,
    force: bool,
    explicit_tab: bool,
    due_at: Option<std::time::Instant>,
}

/// Which trigger opened the autocomplete list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutocompleteState {
    Regular,
    Force,
}

/// The editor component.
pub struct Editor {
    state: EditorState,
    /// Focus flag maintained by the TUI.
    focused: bool,
    core: TuiCore,
    theme: EditorTheme,
    padding_x: usize,
    last_width: usize,
    scroll_offset: usize,
    /// Border colour; may be replaced at runtime.
    pub border_color: Rc<dyn Fn(&str) -> String>,
    autocomplete_provider: Option<Rc<dyn AutocompleteProvider>>,
    autocomplete_trigger_characters: Vec<char>,
    autocomplete_list: Option<SelectList>,
    autocomplete_state: Option<AutocompleteState>,
    autocomplete_prefix: String,
    autocomplete_max_visible: usize,
    autocomplete_abort: Option<AbortController>,
    autocomplete_start_token: u64,
    autocomplete_request_id: u64,
    pending_autocomplete: Option<PendingAutocomplete>,
    pastes: Vec<(u64, String)>,
    paste_counter: u64,
    paste_buffer: String,
    is_in_paste: bool,
    history: Vec<String>,
    history_index: i64,
    history_draft: Option<EditorState>,
    kill_ring: KillRing,
    last_action: Option<LastAction>,
    jump_mode: Option<JumpDirection>,
    preferred_visual_col: Option<usize>,
    snapped_from_cursor_col: Option<usize>,
    undo_stack: UndoStack<EditorSnapshot>,
    /// Submitted text (`onSubmit`), polled by the owner.
    submitted: Vec<String>,
    /// Text after every change (`onChange`), polled by the owner.
    changes: Vec<String>,
    /// Blocks submitting.
    pub disable_submit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LastAction {
    Kill,
    Yank,
    TypeWord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JumpDirection {
    Forward,
    Backward,
}

impl Editor {
    /// New editor bound to `core`.
    pub fn new(core: TuiCore, theme: EditorTheme, options: EditorOptions) -> Self {
        let border_color = Rc::clone(&theme.border_color);
        let padding_x = options.padding_x.unwrap_or(0);
        let autocomplete_max_visible = options.autocomplete_max_visible.unwrap_or(5).clamp(3, 20);
        Self {
            state: EditorState {
                lines: vec![String::new()],
                cursor_line: 0,
                cursor_col: 0,
            },
            focused: false,
            core,
            theme,
            padding_x,
            last_width: 80,
            scroll_offset: 0,
            border_color,
            autocomplete_provider: None,
            autocomplete_trigger_characters: DEFAULT_AUTOCOMPLETE_TRIGGER_CHARACTERS.to_vec(),
            autocomplete_list: None,
            autocomplete_state: None,
            autocomplete_prefix: String::new(),
            autocomplete_max_visible,
            autocomplete_abort: None,
            autocomplete_start_token: 0,
            autocomplete_request_id: 0,
            pending_autocomplete: None,
            pastes: Vec::new(),
            paste_counter: 0,
            paste_buffer: String::new(),
            is_in_paste: false,
            history: Vec::new(),
            history_index: -1,
            history_draft: None,
            kill_ring: KillRing::new(),
            last_action: None,
            jump_mode: None,
            preferred_visual_col: None,
            snapped_from_cursor_col: None,
            undo_stack: UndoStack::new(),
            submitted: Vec::new(),
            changes: Vec::new(),
            disable_submit: false,
        }
    }

    /// Ids of the currently valid pastes.
    fn valid_paste_ids(&self) -> Vec<u64> {
        self.pastes.iter().map(|(id, _)| *id).collect()
    }

    /// Segment `text` with paste-marker awareness.
    fn segment(&self, text: &str, word_granularity: bool) -> Vec<Segment> {
        segment_with_markers(text, word_granularity, &self.valid_paste_ids())
    }

    /// Horizontal padding.
    pub fn get_padding_x(&self) -> usize {
        self.padding_x
    }

    /// Change the horizontal padding.
    pub fn set_padding_x(&mut self, padding: usize) {
        if self.padding_x != padding {
            self.padding_x = padding;
            self.core.request_render();
        }
    }

    /// Maximum number of visible autocomplete entries.
    pub fn get_autocomplete_max_visible(&self) -> usize {
        self.autocomplete_max_visible
    }

    /// Change the maximum number of visible autocomplete entries.
    pub fn set_autocomplete_max_visible(&mut self, max_visible: usize) {
        let next = max_visible.clamp(3, 20);
        if self.autocomplete_max_visible != next {
            self.autocomplete_max_visible = next;
            self.core.request_render();
        }
    }

    /// Install the autocomplete provider.
    pub fn set_autocomplete_provider(&mut self, provider: Rc<dyn AutocompleteProvider>) {
        self.cancel_autocomplete();
        let trigger_characters = provider.trigger_characters();
        self.autocomplete_provider = Some(provider);
        self.set_autocomplete_trigger_characters(&trigger_characters);
    }

    /// Add a prompt to the history (after a successful submit).
    pub fn add_to_history(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.history.first().is_some_and(|entry| entry == trimmed) {
            return;
        }
        self.history.insert(0, trimmed.to_string());
        if self.history.len() > 100 {
            self.history.pop();
        }
    }

    /// Text submitted since the last call (`onSubmit`).
    pub fn take_submitted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.submitted)
    }

    /// Text of every change since the last call (`onChange`).
    pub fn take_changes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.changes)
    }

    fn emit_change(&mut self) {
        let text = self.get_text();
        self.changes.push(text);
    }

    fn is_editor_empty(&self) -> bool {
        self.state.lines.len() == 1 && self.state.lines[0].is_empty()
    }

    fn is_on_first_visual_line(&self) -> bool {
        let visual_lines = self.build_visual_line_map(self.last_width);
        self.find_current_visual_line(&visual_lines) == 0
    }

    fn is_on_last_visual_line(&self) -> bool {
        let visual_lines = self.build_visual_line_map(self.last_width);
        self.find_current_visual_line(&visual_lines) == visual_lines.len() - 1
    }

    /// Full text.
    pub fn get_text(&self) -> String {
        self.state.lines.join("\n")
    }

    /// Text with all paste markers expanded.
    pub fn get_expanded_text(&self) -> String {
        self.expand_paste_markers(&self.state.lines.join("\n"))
    }

    /// The logical lines.
    pub fn get_lines(&self) -> Vec<String> {
        self.state.lines.clone()
    }

    /// Cursor position as `(line, column)`.
    pub fn get_cursor(&self) -> (usize, usize) {
        (self.state.cursor_line, self.state.cursor_col)
    }

    /// Whether the autocomplete list is open.
    pub fn is_showing_autocomplete(&self) -> bool {
        self.autocomplete_state.is_some()
    }

    fn expand_paste_markers(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (paste_id, paste_content) in &self.pastes {
            let mut expanded = String::with_capacity(result.len());
            let mut rest = result.as_str();
            loop {
                let Some(relative) = rest.find(&format!("[paste #{paste_id}")) else {
                    expanded.push_str(rest);
                    break;
                };
                let start = relative;
                let Some(end_relative) = rest[start..].find(']') else {
                    expanded.push_str(rest);
                    break;
                };
                let end = start + end_relative + 1;
                match parse_paste_marker(&rest[start..end]) {
                    Some((id, _)) if id == *paste_id => {
                        expanded.push_str(&rest[..start]);
                        expanded.push_str(paste_content);
                        rest = &rest[end..];
                    }
                    _ => {
                        expanded.push_str(&rest[..start + "[paste #".len()]);
                        rest = &rest[start + "[paste #".len()..];
                    }
                }
            }
            result = expanded;
        }
        result
    }

    /// Replace the whole text.
    pub fn set_text(&mut self, text: &str) {
        self.cancel_autocomplete();
        self.last_action = None;
        self.exit_history_browsing();
        let normalized = normalize_text(text);
        if self.get_text() != normalized {
            self.push_undo_snapshot();
        }
        self.pastes.clear();
        self.paste_counter = 0;
        self.set_text_internal(&normalized, false);
    }

    /// Insert `text` at the cursor as one undoable step.
    pub fn insert_text_at_cursor(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.cancel_autocomplete();
        self.push_undo_snapshot();
        self.last_action = None;
        self.exit_history_browsing();
        self.insert_text_at_cursor_internal(text);
    }

    fn set_text_internal(&mut self, text: &str, cursor_at_start: bool) {
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        self.state.lines = if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };
        self.state.cursor_line = if cursor_at_start {
            0
        } else {
            self.state.lines.len() - 1
        };
        let column = if cursor_at_start {
            0
        } else {
            self.state.lines[self.state.cursor_line].len()
        };
        self.set_cursor_col(column);
        self.scroll_offset = 0;
        self.emit_change();
    }

    fn insert_text_at_cursor_internal(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let normalized = normalize_text(text);
        let inserted_lines: Vec<&str> = normalized.split('\n').collect();

        let current_line = self.current_line();
        let before_cursor = current_line[..self.state.cursor_col].to_string();
        let after_cursor = current_line[self.state.cursor_col..].to_string();

        if inserted_lines.len() == 1 {
            self.state.lines[self.state.cursor_line] =
                format!("{before_cursor}{normalized}{after_cursor}");
            self.set_cursor_col(self.state.cursor_col + normalized.len());
        } else {
            let mut lines: Vec<String> = self.state.lines[..self.state.cursor_line].to_vec();
            lines.push(format!("{before_cursor}{}", inserted_lines[0]));
            for line in &inserted_lines[1..inserted_lines.len() - 1] {
                lines.push((*line).to_string());
            }
            let last = inserted_lines[inserted_lines.len() - 1];
            lines.push(format!("{last}{after_cursor}"));
            lines.extend_from_slice(&self.state.lines[self.state.cursor_line + 1..]);
            self.state.lines = lines;
            self.state.cursor_line += inserted_lines.len() - 1;
            self.set_cursor_col(last.len());
        }

        self.emit_change();
    }

    fn current_line(&self) -> String {
        self.state
            .lines
            .get(self.state.cursor_line)
            .cloned()
            .unwrap_or_default()
    }

    fn set_cursor_col(&mut self, col: usize) {
        self.state.cursor_col = col;
        self.preferred_visual_col = None;
        self.snapped_from_cursor_col = None;
    }

    fn push_undo_snapshot(&mut self) {
        self.undo_stack.push(EditorSnapshot {
            state: self.state.clone(),
            pastes: self.pastes.clone(),
            paste_counter: self.paste_counter,
        });
    }

    fn undo(&mut self) {
        self.exit_history_browsing();
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        self.state = snapshot.state;
        self.pastes = snapshot.pastes;
        self.paste_counter = snapshot.paste_counter;
        self.last_action = None;
        self.preferred_visual_col = None;
        self.emit_change();
    }

    fn exit_history_browsing(&mut self) {
        self.history_index = -1;
        self.history_draft = None;
    }

    fn navigate_history(&mut self, direction: i64) {
        self.last_action = None;
        if self.history.is_empty() {
            return;
        }

        let new_index = self.history_index - direction;
        if new_index < -1 || new_index >= self.history.len() as i64 {
            return;
        }

        if self.history_index == -1 && new_index >= 0 {
            self.push_undo_snapshot();
            self.history_draft = Some(self.state.clone());
        }

        self.history_index = new_index;

        if self.history_index == -1 {
            match self.history_draft.take() {
                Some(draft) => {
                    self.state = draft;
                    self.preferred_visual_col = None;
                    self.snapped_from_cursor_col = None;
                    self.scroll_offset = 0;
                    self.emit_change();
                }
                None => self.set_text_internal("", false),
            }
        } else {
            let entry = self.history[self.history_index as usize].clone();
            self.set_text_internal(&entry, direction == -1);
        }
    }
}

/// Normalize line endings and expand tabs, as the TS editor does on input.
fn normalize_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ")
}

impl Editor {
    // === Visual line map ===

    fn build_visual_line_map(&self, width: usize) -> Vec<VisualLine> {
        let mut visual_lines: Vec<VisualLine> = Vec::new();
        for (index, line) in self.state.lines.iter().enumerate() {
            let line_visible_width = visible_width(line);
            if line.is_empty() {
                visual_lines.push(VisualLine {
                    logical_line: index,
                    start_col: 0,
                    length: 0,
                });
            } else if line_visible_width <= width {
                visual_lines.push(VisualLine {
                    logical_line: index,
                    start_col: 0,
                    length: line.len(),
                });
            } else {
                let segments = self.segment(line, false);
                for chunk in word_wrap_line(line, width, Some(&segments)) {
                    visual_lines.push(VisualLine {
                        logical_line: index,
                        start_col: chunk.start_index,
                        length: chunk.end_index - chunk.start_index,
                    });
                }
            }
        }
        visual_lines
    }

    fn find_visual_line_at(visual_lines: &[VisualLine], line: usize, col: usize) -> usize {
        for (index, visual_line) in visual_lines.iter().enumerate() {
            if visual_line.logical_line != line {
                continue;
            }
            if col < visual_line.start_col {
                continue;
            }
            let offset = col - visual_line.start_col;
            let is_last_segment_of_line = index == visual_lines.len() - 1
                || visual_lines[index + 1].logical_line != visual_line.logical_line;
            if offset < visual_line.length
                || (is_last_segment_of_line && offset == visual_line.length)
            {
                return index;
            }
        }
        visual_lines.len() - 1
    }

    fn find_current_visual_line(&self, visual_lines: &[VisualLine]) -> usize {
        Self::find_visual_line_at(visual_lines, self.state.cursor_line, self.state.cursor_col)
    }

    // === Cursor movement ===

    fn move_to_visual_line(
        &mut self,
        visual_lines: &[VisualLine],
        current_visual_line: usize,
        target_visual_line: usize,
    ) {
        let (Some(current_vl), Some(target_vl)) = (
            visual_lines.get(current_visual_line).copied(),
            visual_lines.get(target_visual_line).copied(),
        ) else {
            return;
        };

        // A cursor snapped to a segment start keeps its pre-snap position so
        // the next vertical move resolves the right visual column.
        let current_visual_col = match self.snapped_from_cursor_col {
            Some(snapped_from) => {
                let vl_index =
                    Self::find_visual_line_at(visual_lines, current_vl.logical_line, snapped_from);
                snapped_from - visual_lines[vl_index].start_col
            }
            None => self.state.cursor_col - current_vl.start_col,
        };

        let is_last_source_segment = current_visual_line == visual_lines.len() - 1
            || visual_lines[current_visual_line + 1].logical_line != current_vl.logical_line;
        let source_max_visual_col = if is_last_source_segment {
            current_vl.length
        } else {
            current_vl.length.saturating_sub(1)
        };

        let is_last_target_segment = target_visual_line == visual_lines.len() - 1
            || visual_lines[target_visual_line + 1].logical_line != target_vl.logical_line;
        let target_max_visual_col = if is_last_target_segment {
            target_vl.length
        } else {
            target_vl.length.saturating_sub(1)
        };

        let move_to_visual_col = self.compute_vertical_move_column(
            current_visual_col,
            source_max_visual_col,
            target_max_visual_col,
        );

        self.state.cursor_line = target_vl.logical_line;
        let target_col = target_vl.start_col + move_to_visual_col;
        let logical_line = self.state.lines[target_vl.logical_line].clone();
        self.state.cursor_col = target_col.min(logical_line.len());

        // Snap onto atomic segment boundaries (paste markers) so the cursor
        // never lands inside a multi-grapheme unit.
        for segment in self.segment(&logical_line, false) {
            if segment.index > self.state.cursor_col {
                break;
            }
            if segment.segment.chars().count() <= 1 {
                continue;
            }
            if self.state.cursor_col < segment.index + segment.segment.len() {
                let is_continuation = segment.index < target_vl.start_col;
                let is_moving_down = target_visual_line > current_visual_line;

                if is_continuation && is_moving_down {
                    // The segment started on an earlier visual line and was
                    // already visited: skip its remaining continuation lines.
                    let segment_end = segment.index + segment.segment.len();
                    let mut next = target_visual_line + 1;
                    while next < visual_lines.len()
                        && visual_lines[next].logical_line == target_vl.logical_line
                        && visual_lines[next].start_col < segment_end
                    {
                        next += 1;
                    }
                    if next < visual_lines.len() {
                        self.move_to_visual_line(visual_lines, current_visual_line, next);
                        return;
                    }
                }

                self.snapped_from_cursor_col = Some(self.state.cursor_col);
                self.state.cursor_col = segment.index;
                return;
            }
        }

        self.snapped_from_cursor_col = None;
    }

    /// Sticky-column decision table of the TS version.
    fn compute_vertical_move_column(
        &mut self,
        current_visual_col: usize,
        source_max_visual_col: usize,
        target_max_visual_col: usize,
    ) -> usize {
        let has_preferred = self.preferred_visual_col.is_some();
        let cursor_in_middle = current_visual_col < source_max_visual_col;
        let target_too_short = target_max_visual_col < current_visual_col;

        if !has_preferred || cursor_in_middle {
            if target_too_short {
                self.preferred_visual_col = Some(current_visual_col);
                return target_max_visual_col;
            }
            self.preferred_visual_col = None;
            return current_visual_col;
        }

        let preferred = self.preferred_visual_col.expect("preferred column set");
        let target_cant_fit_preferred = target_max_visual_col < preferred;
        if target_too_short || target_cant_fit_preferred {
            return target_max_visual_col;
        }

        self.preferred_visual_col = None;
        preferred
    }

    fn move_cursor(&mut self, delta_line: i64, delta_col: i64) {
        self.last_action = None;
        let visual_lines = self.build_visual_line_map(self.last_width);
        let current_visual_line = self.find_current_visual_line(&visual_lines);

        if delta_line != 0 {
            let target_visual_line = current_visual_line as i64 + delta_line;
            if target_visual_line >= 0 && target_visual_line < visual_lines.len() as i64 {
                self.move_to_visual_line(
                    &visual_lines,
                    current_visual_line,
                    target_visual_line as usize,
                );
            }
        }

        if delta_col != 0 {
            let current_line = self.current_line();

            if delta_col > 0 {
                if self.state.cursor_col < current_line.len() {
                    let after_cursor = &current_line[self.state.cursor_col..];
                    let first = self
                        .segment(after_cursor, false)
                        .first()
                        .map_or(1, |segment| segment.segment.len());
                    self.set_cursor_col(self.state.cursor_col + first);
                } else if self.state.cursor_line < self.state.lines.len() - 1 {
                    self.state.cursor_line += 1;
                    self.set_cursor_col(0);
                } else if let Some(current_vl) = visual_lines.get(current_visual_line) {
                    // At the very end: remember the column for vertical moves.
                    self.preferred_visual_col = Some(self.state.cursor_col - current_vl.start_col);
                }
            } else if self.state.cursor_col > 0 {
                let before_cursor = &current_line[..self.state.cursor_col];
                let last = self
                    .segment(before_cursor, false)
                    .last()
                    .map_or(1, |segment| segment.segment.len());
                self.set_cursor_col(self.state.cursor_col - last);
            } else if self.state.cursor_line > 0 {
                self.state.cursor_line -= 1;
                let previous_line = self.current_line();
                self.set_cursor_col(previous_line.len());
            }
        }

        // Keep an open picker in sync: the text before the cursor changed.
        if self.autocomplete_state.is_some() {
            self.update_autocomplete();
        }
    }

    fn page_scroll(&mut self, direction: i64) {
        self.last_action = None;
        let page_size = (self.core.rows() * 3 / 10).max(5) as i64;
        let visual_lines = self.build_visual_line_map(self.last_width);
        let current_visual_line = self.find_current_visual_line(&visual_lines);
        let target_visual_line = (current_visual_line as i64 + direction * page_size)
            .clamp(0, visual_lines.len() as i64 - 1) as usize;
        self.move_to_visual_line(&visual_lines, current_visual_line, target_visual_line);
    }

    fn move_to_line_start(&mut self) {
        self.last_action = None;
        self.set_cursor_col(0);
    }

    fn move_to_line_end(&mut self) {
        self.last_action = None;
        let current_line = self.current_line();
        self.set_cursor_col(current_line.len());
    }

    fn word_navigation_segments(&self, text: &str) -> Vec<(usize, String)> {
        self.segment(text, true)
            .into_iter()
            .map(|segment| (segment.index, segment.segment))
            .collect()
    }

    fn move_word_backwards(&mut self) {
        self.last_action = None;
        let current_line = self.current_line();

        if self.state.cursor_col == 0 {
            if self.state.cursor_line > 0 {
                self.state.cursor_line -= 1;
                let previous_line = self.current_line();
                self.set_cursor_col(previous_line.len());
            }
            return;
        }

        let segment_fn = |text: &str| self.word_navigation_segments(text);
        let column = find_word_backward(
            &current_line,
            self.state.cursor_col,
            &WordNavigationOptions {
                segment: Some(&segment_fn),
                is_atomic_segment: Some(&is_paste_marker),
            },
        );
        self.set_cursor_col(column);
    }

    fn move_word_forwards(&mut self) {
        self.last_action = None;
        let current_line = self.current_line();

        if self.state.cursor_col >= current_line.len() {
            if self.state.cursor_line < self.state.lines.len() - 1 {
                self.state.cursor_line += 1;
                self.set_cursor_col(0);
            }
            return;
        }

        let segment_fn = |text: &str| self.word_navigation_segments(text);
        let column = find_word_forward(
            &current_line,
            self.state.cursor_col,
            &WordNavigationOptions {
                segment: Some(&segment_fn),
                is_atomic_segment: Some(&is_paste_marker),
            },
        );
        self.set_cursor_col(column);
    }

    /// Jump to the next or previous occurrence of `character`.
    fn jump_to_char(&mut self, character: &str, direction: JumpDirection) {
        self.last_action = None;
        let is_forward = direction == JumpDirection::Forward;
        let line_count = self.state.lines.len() as i64;
        let end = if is_forward { line_count } else { -1 };
        let step = if is_forward { 1 } else { -1 };

        let mut line_index = self.state.cursor_line as i64;
        while line_index != end {
            let line = self.state.lines[line_index as usize].clone();
            let is_current_line = line_index == self.state.cursor_line as i64;

            let found = if is_forward {
                let from = if is_current_line {
                    self.state.cursor_col + 1
                } else {
                    0
                };
                if from > line.len() {
                    None
                } else {
                    line[from..].find(character).map(|index| from + index)
                }
            } else {
                // `lastIndexOf(char, from)` matches at positions <= from.
                let limit = if is_current_line {
                    if self.state.cursor_col == 0 {
                        None
                    } else {
                        Some(self.state.cursor_col - 1)
                    }
                } else {
                    Some(line.len())
                };
                match limit {
                    None => None,
                    Some(limit) => {
                        let bound = (limit + character.len()).min(line.len());
                        line[..bound].rfind(character)
                    }
                }
            };

            if let Some(index) = found {
                self.state.cursor_line = line_index as usize;
                self.set_cursor_col(index);
                return;
            }
            line_index += step;
        }
    }

    // === Editing ===

    fn insert_character(&mut self, character: &str, skip_undo_coalescing: bool) {
        self.exit_history_browsing();

        // Undo coalescing: consecutive word characters merge into one unit, a
        // space captures the state before itself.
        if !skip_undo_coalescing {
            if is_whitespace_char(character) || self.last_action != Some(LastAction::TypeWord) {
                self.push_undo_snapshot();
            }
            self.last_action = Some(LastAction::TypeWord);
        }

        let line = self.current_line();
        let before = &line[..self.state.cursor_col];
        let after = &line[self.state.cursor_col..];
        self.state.lines[self.state.cursor_line] = format!("{before}{character}{after}");
        self.set_cursor_col(self.state.cursor_col + character.len());
        self.emit_change();

        if self.autocomplete_state.is_none() {
            let single_char = character.chars().next();
            if character == "/" && self.is_at_start_of_message() {
                self.try_trigger_autocomplete();
            } else if single_char
                .is_some_and(|character| self.autocomplete_trigger_characters.contains(&character))
            {
                let current_line = self.current_line();
                let text_before_cursor = &current_line[..self.state.cursor_col];
                let char_before_symbol = text_before_cursor
                    [..text_before_cursor.len() - character.len()]
                    .chars()
                    .next_back();
                if text_before_cursor.len() == character.len()
                    || char_before_symbol == Some(' ')
                    || char_before_symbol == Some('\t')
                {
                    self.try_trigger_autocomplete();
                }
            } else if single_char.is_some_and(|character| {
                character.is_ascii_alphanumeric()
                    || character == '.'
                    || character == '-'
                    || character == '_'
            }) {
                let current_line = self.current_line();
                let text_before_cursor = current_line[..self.state.cursor_col].to_string();
                // Slash command or symbol completion context.
                if self.is_in_slash_command_context(&text_before_cursor)
                    || self.matches_trigger_pattern(&text_before_cursor)
                {
                    self.try_trigger_autocomplete();
                }
            }
        } else {
            self.update_autocomplete();
        }
    }

    fn add_new_line(&mut self) {
        self.cancel_autocomplete();
        self.exit_history_browsing();
        self.last_action = None;
        self.push_undo_snapshot();

        let current_line = self.current_line();
        let before = current_line[..self.state.cursor_col].to_string();
        let after = current_line[self.state.cursor_col..].to_string();

        self.state.lines[self.state.cursor_line] = before;
        self.state.lines.insert(self.state.cursor_line + 1, after);
        self.state.cursor_line += 1;
        self.set_cursor_col(0);
        self.emit_change();
    }

    fn submit_value(&mut self) {
        self.cancel_autocomplete();
        let result = self
            .expand_paste_markers(&self.state.lines.join("\n"))
            .trim()
            .to_string();

        self.state = EditorState {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
        };
        self.pastes.clear();
        self.paste_counter = 0;
        self.exit_history_browsing();
        self.scroll_offset = 0;
        self.undo_stack.clear();
        self.last_action = None;

        self.changes.push(String::new());
        self.submitted.push(result);
    }

    fn should_submit_on_backslash_enter(&self, data: &str) -> bool {
        if self.disable_submit {
            return false;
        }
        if !matches_key(data, "enter") {
            return false;
        }
        let submit_keys = keybinding_keys("tui.input.submit");
        let has_shift_enter = submit_keys.iter().any(|key| key == "shift+enter")
            || submit_keys.iter().any(|key| key == "shift+return");
        if !has_shift_enter {
            return false;
        }
        let current_line = self.current_line();
        self.state.cursor_col > 0 && current_line[..self.state.cursor_col].ends_with('\\')
    }

    fn handle_backspace(&mut self) {
        self.exit_history_browsing();
        self.last_action = None;

        if self.state.cursor_col > 0 {
            self.push_undo_snapshot();

            let line = self.current_line();
            let before_cursor = &line[..self.state.cursor_col];
            let graphemes_before = self.segment(before_cursor, false);
            let last_grapheme = graphemes_before
                .last()
                .map(|segment| segment.segment.clone())
                .unwrap_or_default();
            let grapheme_length = if last_grapheme.is_empty() {
                1
            } else {
                last_grapheme.len()
            };

            if let Some((target_id, _)) = parse_paste_marker(&last_grapheme) {
                self.pastes.retain(|(id, _)| *id != target_id);
                self.paste_counter = self.paste_counter.saturating_sub(1);

                // Shift the registry down in ascending id order.
                let mut higher_ids: Vec<u64> = self
                    .pastes
                    .iter()
                    .map(|(id, _)| *id)
                    .filter(|id| *id > target_id)
                    .collect();
                higher_ids.sort_unstable();
                for id in higher_ids {
                    if let Some(entry) = self.pastes.iter_mut().find(|(entry, _)| *entry == id) {
                        entry.0 = id - 1;
                    }
                }

                // Renumber markers with a higher id in the text.
                self.state.lines = self
                    .state
                    .lines
                    .iter()
                    .map(|line| renumber_paste_markers(line, target_id))
                    .collect();
            }

            let line = self.current_line();
            let before = &line[..self.state.cursor_col - grapheme_length];
            let after = &line[self.state.cursor_col..];
            self.state.lines[self.state.cursor_line] = format!("{before}{after}");
            self.set_cursor_col(self.state.cursor_col - grapheme_length);
        } else if self.state.cursor_line > 0 {
            self.push_undo_snapshot();

            let current_line = self.current_line();
            let previous_line = self.state.lines[self.state.cursor_line - 1].clone();
            self.state.lines[self.state.cursor_line - 1] = format!("{previous_line}{current_line}");
            self.state.lines.remove(self.state.cursor_line);
            self.state.cursor_line -= 1;
            self.set_cursor_col(previous_line.len());
        }

        self.emit_change();

        if self.autocomplete_state.is_some() {
            self.update_autocomplete();
        } else {
            let current_line = self.current_line();
            let text_before_cursor = current_line[..self.state.cursor_col].to_string();
            if self.is_in_slash_command_context(&text_before_cursor)
                || self.matches_trigger_pattern(&text_before_cursor)
            {
                self.try_trigger_autocomplete();
            }
        }
    }

    fn handle_forward_delete(&mut self) {
        self.exit_history_browsing();
        self.last_action = None;

        let current_line = self.current_line();
        if self.state.cursor_col < current_line.len() {
            self.push_undo_snapshot();

            let after_cursor = &current_line[self.state.cursor_col..];
            let grapheme_length = self
                .segment(after_cursor, false)
                .first()
                .map_or(1, |segment| segment.segment.len());

            let before = &current_line[..self.state.cursor_col];
            let after = &current_line[self.state.cursor_col + grapheme_length..];
            self.state.lines[self.state.cursor_line] = format!("{before}{after}");
        } else if self.state.cursor_line < self.state.lines.len() - 1 {
            self.push_undo_snapshot();

            let next_line = self.state.lines[self.state.cursor_line + 1].clone();
            self.state.lines[self.state.cursor_line] = format!("{current_line}{next_line}");
            self.state.lines.remove(self.state.cursor_line + 1);
        }

        self.emit_change();

        if self.autocomplete_state.is_some() {
            self.update_autocomplete();
        } else {
            let current_line = self.current_line();
            let text_before_cursor = current_line[..self.state.cursor_col].to_string();
            if self.is_in_slash_command_context(&text_before_cursor)
                || self.matches_trigger_pattern(&text_before_cursor)
            {
                self.try_trigger_autocomplete();
            }
        }
    }

    fn delete_to_start_of_line(&mut self) {
        self.exit_history_browsing();
        let current_line = self.current_line();

        if self.state.cursor_col > 0 {
            self.push_undo_snapshot();
            let deleted_text = current_line[..self.state.cursor_col].to_string();
            self.kill_ring.push(
                &deleted_text,
                KillRingPushOptions {
                    prepend: true,
                    accumulate: self.last_action == Some(LastAction::Kill),
                },
            );
            self.last_action = Some(LastAction::Kill);
            self.state.lines[self.state.cursor_line] =
                current_line[self.state.cursor_col..].to_string();
            self.set_cursor_col(0);
        } else if self.state.cursor_line > 0 {
            self.push_undo_snapshot();
            self.kill_ring.push(
                "\n",
                KillRingPushOptions {
                    prepend: true,
                    accumulate: self.last_action == Some(LastAction::Kill),
                },
            );
            self.last_action = Some(LastAction::Kill);

            let previous_line = self.state.lines[self.state.cursor_line - 1].clone();
            self.state.lines[self.state.cursor_line - 1] = format!("{previous_line}{current_line}");
            self.state.lines.remove(self.state.cursor_line);
            self.state.cursor_line -= 1;
            self.set_cursor_col(previous_line.len());
        }

        self.emit_change();
    }

    fn delete_to_end_of_line(&mut self) {
        self.exit_history_browsing();
        let current_line = self.current_line();

        if self.state.cursor_col < current_line.len() {
            self.push_undo_snapshot();
            let deleted_text = current_line[self.state.cursor_col..].to_string();
            self.kill_ring.push(
                &deleted_text,
                KillRingPushOptions {
                    prepend: false,
                    accumulate: self.last_action == Some(LastAction::Kill),
                },
            );
            self.last_action = Some(LastAction::Kill);
            self.state.lines[self.state.cursor_line] =
                current_line[..self.state.cursor_col].to_string();
        } else if self.state.cursor_line < self.state.lines.len() - 1 {
            self.push_undo_snapshot();
            self.kill_ring.push(
                "\n",
                KillRingPushOptions {
                    prepend: false,
                    accumulate: self.last_action == Some(LastAction::Kill),
                },
            );
            self.last_action = Some(LastAction::Kill);

            let next_line = self.state.lines[self.state.cursor_line + 1].clone();
            self.state.lines[self.state.cursor_line] = format!("{current_line}{next_line}");
            self.state.lines.remove(self.state.cursor_line + 1);
        }

        self.emit_change();
    }

    fn delete_word_backwards(&mut self) {
        self.exit_history_browsing();
        let current_line = self.current_line();

        if self.state.cursor_col == 0 {
            if self.state.cursor_line > 0 {
                self.push_undo_snapshot();
                self.kill_ring.push(
                    "\n",
                    KillRingPushOptions {
                        prepend: true,
                        accumulate: self.last_action == Some(LastAction::Kill),
                    },
                );
                self.last_action = Some(LastAction::Kill);

                let previous_line = self.state.lines[self.state.cursor_line - 1].clone();
                self.state.lines[self.state.cursor_line - 1] =
                    format!("{previous_line}{current_line}");
                self.state.lines.remove(self.state.cursor_line);
                self.state.cursor_line -= 1;
                self.set_cursor_col(previous_line.len());
            }
        } else {
            self.push_undo_snapshot();
            let was_kill = self.last_action == Some(LastAction::Kill);

            let old_cursor_col = self.state.cursor_col;
            self.move_word_backwards();
            let delete_from = self.state.cursor_col;
            self.set_cursor_col(old_cursor_col);

            let deleted_text = current_line[delete_from..self.state.cursor_col].to_string();
            self.kill_ring.push(
                &deleted_text,
                KillRingPushOptions {
                    prepend: true,
                    accumulate: was_kill,
                },
            );
            self.last_action = Some(LastAction::Kill);

            self.state.lines[self.state.cursor_line] = format!(
                "{}{}",
                &current_line[..delete_from],
                &current_line[self.state.cursor_col..]
            );
            self.set_cursor_col(delete_from);
        }

        self.emit_change();
    }

    fn delete_word_forward(&mut self) {
        self.exit_history_browsing();
        let current_line = self.current_line();

        if self.state.cursor_col >= current_line.len() {
            if self.state.cursor_line < self.state.lines.len() - 1 {
                self.push_undo_snapshot();
                self.kill_ring.push(
                    "\n",
                    KillRingPushOptions {
                        prepend: false,
                        accumulate: self.last_action == Some(LastAction::Kill),
                    },
                );
                self.last_action = Some(LastAction::Kill);

                let next_line = self.state.lines[self.state.cursor_line + 1].clone();
                self.state.lines[self.state.cursor_line] = format!("{current_line}{next_line}");
                self.state.lines.remove(self.state.cursor_line + 1);
            }
        } else {
            self.push_undo_snapshot();
            let was_kill = self.last_action == Some(LastAction::Kill);

            let old_cursor_col = self.state.cursor_col;
            self.move_word_forwards();
            let delete_to = self.state.cursor_col;
            self.set_cursor_col(old_cursor_col);

            let deleted_text = current_line[self.state.cursor_col..delete_to].to_string();
            self.kill_ring.push(
                &deleted_text,
                KillRingPushOptions {
                    prepend: false,
                    accumulate: was_kill,
                },
            );
            self.last_action = Some(LastAction::Kill);

            self.state.lines[self.state.cursor_line] = format!(
                "{}{}",
                &current_line[..self.state.cursor_col],
                &current_line[delete_to..]
            );
        }

        self.emit_change();
    }

    // === Kill ring ===

    fn yank(&mut self) {
        if self.kill_ring.is_empty() {
            return;
        }
        self.push_undo_snapshot();
        let text = self
            .kill_ring
            .peek()
            .expect("kill ring not empty")
            .to_string();
        self.insert_yanked_text(&text);
        self.last_action = Some(LastAction::Yank);
    }

    fn yank_pop(&mut self) {
        if self.last_action != Some(LastAction::Yank) || self.kill_ring.len() <= 1 {
            return;
        }
        self.push_undo_snapshot();
        self.delete_yanked_text();
        self.kill_ring.rotate();
        let text = self
            .kill_ring
            .peek()
            .expect("kill ring not empty")
            .to_string();
        self.insert_yanked_text(&text);
        self.last_action = Some(LastAction::Yank);
    }

    fn insert_yanked_text(&mut self, text: &str) {
        self.exit_history_browsing();
        let lines: Vec<&str> = text.split('\n').collect();

        let current_line = self.current_line();
        let before = current_line[..self.state.cursor_col].to_string();
        let after = current_line[self.state.cursor_col..].to_string();

        if lines.len() == 1 {
            self.state.lines[self.state.cursor_line] = format!("{before}{text}{after}");
            self.set_cursor_col(self.state.cursor_col + text.len());
        } else {
            self.state.lines[self.state.cursor_line] = format!("{before}{}", lines[0]);
            for (offset, line) in lines[1..lines.len() - 1].iter().enumerate() {
                self.state
                    .lines
                    .insert(self.state.cursor_line + offset + 1, (*line).to_string());
            }
            let last_line_index = self.state.cursor_line + lines.len() - 1;
            let last = lines[lines.len() - 1];
            self.state
                .lines
                .insert(last_line_index, format!("{last}{after}"));
            self.state.cursor_line = last_line_index;
            self.set_cursor_col(last.len());
        }

        self.emit_change();
    }

    fn delete_yanked_text(&mut self) {
        let Some(yanked_text) = self.kill_ring.peek().map(str::to_string) else {
            return;
        };
        let yank_lines: Vec<&str> = yanked_text.split('\n').collect();

        if yank_lines.len() == 1 {
            let current_line = self.current_line();
            let delete_len = yanked_text.len();
            let before = current_line[..self.state.cursor_col - delete_len].to_string();
            let after = current_line[self.state.cursor_col..].to_string();
            self.state.lines[self.state.cursor_line] = format!("{before}{after}");
            self.set_cursor_col(self.state.cursor_col - delete_len);
        } else {
            let start_line = self.state.cursor_line - (yank_lines.len() - 1);
            let start_col = self.state.lines[start_line].len() - yank_lines[0].len();
            let after_cursor =
                self.state.lines[self.state.cursor_line][self.state.cursor_col..].to_string();
            let before_yank = self.state.lines[start_line][..start_col].to_string();

            self.state.lines.splice(
                start_line..start_line + yank_lines.len(),
                [format!("{before_yank}{after_cursor}")],
            );

            self.state.cursor_line = start_line;
            self.set_cursor_col(start_col);
        }

        self.emit_change();
    }

    // === Paste ===

    fn handle_paste(&mut self, pasted_text: &str) {
        self.cancel_autocomplete();
        self.exit_history_browsing();
        self.last_action = None;
        self.push_undo_snapshot();

        // Some terminals re-encode control bytes inside bracketed paste as
        // CSI-u Ctrl+<letter>; decode them back to the literal byte.
        let decoded_text = decode_csi_u_control_bytes(pasted_text);
        let clean_text = normalize_text(&decoded_text);

        let mut filtered_text: String = clean_text
            .chars()
            .filter(|character| *character == '\n' || (*character as u32) >= 32)
            .collect();

        // A pasted path directly after a word character gets a leading space.
        if filtered_text.starts_with(['/', '~', '.']) {
            let current_line = self.current_line();
            let char_before_cursor = if self.state.cursor_col > 0 {
                current_line[..self.state.cursor_col].chars().next_back()
            } else {
                None
            };
            if char_before_cursor
                .is_some_and(|character| character.is_alphanumeric() || character == '_')
            {
                filtered_text = format!(" {filtered_text}");
            }
        }

        let pasted_lines: Vec<&str> = filtered_text.split('\n').collect();
        let total_chars = filtered_text.chars().count();

        if pasted_lines.len() > 10 || total_chars > 1000 {
            self.paste_counter += 1;
            let paste_id = self.paste_counter;
            self.pastes.push((paste_id, filtered_text.clone()));

            let marker = if pasted_lines.len() > 10 {
                format!("[paste #{paste_id} +{} lines]", pasted_lines.len())
            } else {
                format!("[paste #{paste_id} {total_chars} chars]")
            };
            self.insert_text_at_cursor_internal(&marker);
            return;
        }

        self.insert_text_at_cursor_internal(&filtered_text);
    }

    // === Autocomplete ===

    fn is_slash_menu_allowed(&self) -> bool {
        self.state.cursor_line == 0
    }

    fn is_at_start_of_message(&self) -> bool {
        if !self.is_slash_menu_allowed() {
            return false;
        }
        let current_line = self.current_line();
        let before_cursor = current_line[..self.state.cursor_col].trim();
        before_cursor.is_empty() || before_cursor == "/"
    }

    fn is_in_slash_command_context(&self, text_before_cursor: &str) -> bool {
        self.is_slash_menu_allowed() && text_before_cursor.trim_start().starts_with('/')
    }

    /// `autocompleteTriggerPattern`: a trigger character at a token boundary
    /// followed by non-whitespace up to the cursor.
    fn matches_trigger_pattern(&self, text_before_cursor: &str) -> bool {
        let mut token_start = 0;
        for (index, character) in text_before_cursor.char_indices() {
            if character.is_whitespace() {
                token_start = index + character.len_utf8();
            }
        }
        let token = &text_before_cursor[token_start..];
        let Some(first) = token.chars().next() else {
            return false;
        };
        self.autocomplete_trigger_characters.contains(&first)
            && !token[first.len_utf8()..].chars().any(char::is_whitespace)
    }

    /// `autocompleteDebouncePattern`: `@` (optionally quoted) or another
    /// trigger character at a space/tab boundary.
    fn matches_debounce_pattern(&self, text_before_cursor: &str) -> bool {
        let mut token_start = 0;
        for (index, character) in text_before_cursor.char_indices() {
            if character == ' ' || character == '\t' {
                token_start = index + character.len_utf8();
            }
        }
        let token = &text_before_cursor[token_start..];
        let Some(first) = token.chars().next() else {
            return false;
        };
        if first == '@' {
            let rest = &token[1..];
            if let Some(quoted) = rest.strip_prefix('"') {
                return !quoted.contains('"');
            }
            return !rest.chars().any(char::is_whitespace);
        }
        self.autocomplete_trigger_characters.contains(&first)
            && first != '@'
            && !token[first.len_utf8()..].chars().any(char::is_whitespace)
    }

    fn set_autocomplete_trigger_characters(&mut self, trigger_characters: &[String]) {
        let mut next = DEFAULT_AUTOCOMPLETE_TRIGGER_CHARACTERS.to_vec();
        for character in trigger_characters {
            let mut chars = character.chars();
            let (Some(single), None) = (chars.next(), chars.next()) else {
                continue;
            };
            if single == '/' || is_whitespace_char(character) || next.contains(&single) {
                continue;
            }
            next.push(single);
        }
        self.autocomplete_trigger_characters = next;
    }

    fn get_best_autocomplete_match_index(
        items: &[AutocompleteItem],
        prefix: &str,
    ) -> Option<usize> {
        if prefix.is_empty() {
            return None;
        }
        let mut first_prefix_index = None;
        for (index, item) in items.iter().enumerate() {
            if item.value == prefix {
                return Some(index);
            }
            if first_prefix_index.is_none() && item.value.starts_with(prefix) {
                first_prefix_index = Some(index);
            }
        }
        first_prefix_index
    }

    fn create_autocomplete_list(&self, prefix: &str, items: &[AutocompleteItem]) -> SelectList {
        let layout = if prefix.starts_with('/') {
            slash_command_select_list_layout()
        } else {
            SelectListLayoutOptions::default()
        };
        SelectList::new(
            items
                .iter()
                .map(|item| SelectItem {
                    value: item.value.clone(),
                    label: item.label.clone(),
                    description: item.description.clone(),
                })
                .collect(),
            self.autocomplete_max_visible,
            (self.theme.select_list)(),
            layout,
        )
    }

    fn try_trigger_autocomplete(&mut self) {
        self.request_autocomplete(false, false);
    }

    fn handle_tab_completion(&mut self) {
        if self.autocomplete_provider.is_none() {
            return;
        }
        let current_line = self.current_line();
        let before_cursor = current_line[..self.state.cursor_col].to_string();

        if self.is_in_slash_command_context(&before_cursor)
            && !before_cursor.trim_start().contains(' ')
        {
            self.request_autocomplete(false, true);
        } else {
            self.request_autocomplete(true, true);
        }
    }

    fn request_autocomplete(&mut self, force: bool, explicit_tab: bool) {
        let Some(provider) = self.autocomplete_provider.clone() else {
            return;
        };

        if force
            && !provider.should_trigger_file_completion(
                &self.state.lines,
                self.state.cursor_line,
                self.state.cursor_col,
            )
        {
            return;
        }

        self.cancel_autocomplete_request();
        self.autocomplete_start_token += 1;
        let start_token = self.autocomplete_start_token;

        let debounce_ms = self.get_autocomplete_debounce_ms(force, explicit_tab);
        self.pending_autocomplete = Some(PendingAutocomplete {
            start_token,
            force,
            explicit_tab,
            due_at: (debounce_ms > 0)
                .then(|| std::time::Instant::now() + std::time::Duration::from_millis(debounce_ms)),
        });
    }

    fn get_autocomplete_debounce_ms(&self, force: bool, explicit_tab: bool) -> u64 {
        if explicit_tab || force {
            return 0;
        }
        let current_line = self.current_line();
        let text_before_cursor = &current_line[..self.state.cursor_col];
        if self.matches_debounce_pattern(text_before_cursor) {
            ATTACHMENT_AUTOCOMPLETE_DEBOUNCE_MS
        } else {
            0
        }
    }

    /// When the pending autocomplete request becomes due, if any.
    ///
    /// Deviation class 1: the TS version schedules the debounce with
    /// `setTimeout` and resolves the request in a promise chain; the port
    /// reports the deadline and runs the request in [`Self::pump_autocomplete`],
    /// because a callback would need `&mut` access to the editor.
    pub fn autocomplete_deadline(&self) -> Option<std::time::Instant> {
        self.pending_autocomplete
            .as_ref()
            .map(|pending| pending.due_at.unwrap_or_else(std::time::Instant::now))
    }

    /// Whether a request is waiting to run.
    pub fn has_pending_autocomplete(&self) -> bool {
        self.pending_autocomplete.is_some()
    }

    fn apply_autocomplete_suggestions(
        &mut self,
        suggestions: &AutocompleteSuggestions,
        state: AutocompleteState,
    ) {
        self.autocomplete_prefix = suggestions.prefix.clone();
        let mut list = self.create_autocomplete_list(&suggestions.prefix, &suggestions.items);
        if let Some(index) =
            Self::get_best_autocomplete_match_index(&suggestions.items, &suggestions.prefix)
        {
            list.set_selected_index(index);
        }
        self.autocomplete_list = Some(list);
        self.autocomplete_state = Some(state);
    }

    fn cancel_autocomplete_request(&mut self) {
        self.autocomplete_start_token += 1;
        self.pending_autocomplete = None;
        if let Some(controller) = self.autocomplete_abort.take() {
            controller.abort();
        }
    }

    fn clear_autocomplete_ui(&mut self) {
        self.autocomplete_state = None;
        self.autocomplete_list = None;
        self.autocomplete_prefix.clear();
    }

    fn cancel_autocomplete(&mut self) {
        self.cancel_autocomplete_request();
        self.clear_autocomplete_ui();
    }

    fn update_autocomplete(&mut self) {
        if self.autocomplete_state.is_none() || self.autocomplete_provider.is_none() {
            return;
        }
        let force = self.autocomplete_state == Some(AutocompleteState::Force);
        self.request_autocomplete(force, false);
    }

    fn apply_selected_completion(&mut self, selected: &SelectItem) {
        let Some(provider) = self.autocomplete_provider.clone() else {
            return;
        };
        self.push_undo_snapshot();
        self.last_action = None;
        let item = AutocompleteItem {
            value: selected.value.clone(),
            label: selected.label.clone(),
            description: selected.description.clone(),
        };
        let result = provider.apply_completion(
            &self.state.lines,
            self.state.cursor_line,
            self.state.cursor_col,
            &item,
            &self.autocomplete_prefix,
        );
        self.state.lines = result.lines;
        self.state.cursor_line = result.cursor_line;
        self.set_cursor_col(result.cursor_col);
    }
}

/// Decode `ESC [ <code> ; 5 u` back to its literal control byte.
fn decode_csi_u_control_bytes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("\x1b[") {
        result.push_str(&rest[..index]);
        let tail = &rest[index..];
        let body = &tail[2..];
        let digits: String = body.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() && body[digits.len()..].starts_with(";5u") {
            let code: u32 = digits.parse().unwrap_or(0);
            let replacement = if (97..=122).contains(&code) {
                Some((code - 96) as u8 as char)
            } else if (65..=90).contains(&code) {
                Some((code - 64) as u8 as char)
            } else {
                None
            };
            match replacement {
                Some(character) => {
                    result.push(character);
                    rest = &body[digits.len() + 3..];
                    continue;
                }
                None => {
                    let matched_len = 2 + digits.len() + 3;
                    result.push_str(&tail[..matched_len]);
                    rest = &tail[matched_len..];
                    continue;
                }
            }
        }
        result.push_str("\x1b[");
        rest = body;
    }
    result.push_str(rest);
    result
}

/// Renumber paste markers with an id greater than `target_id`.
fn renumber_paste_markers(line: &str, target_id: u64) -> String {
    let mut result = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(index) = rest.find("[paste #") {
        result.push_str(&rest[..index]);
        let tail = &rest[index..];
        let Some(end_relative) = tail.find(']') else {
            result.push_str(tail);
            return result;
        };
        let end = end_relative + 1;
        match parse_paste_marker(&tail[..end]) {
            Some((id, suffix)) if id > target_id => {
                if suffix.is_empty() {
                    result.push_str(&format!("[paste #{}]", id - 1));
                } else {
                    result.push_str(&format!("[paste #{}{suffix}]", id - 1));
                }
            }
            _ => result.push_str(&tail[..end]),
        }
        rest = &tail[end..];
    }
    result.push_str(rest);
    result
}

impl Editor {
    // === Layout and rendering ===

    fn layout_text(&self, content_width: usize) -> Vec<LayoutLine> {
        let mut layout_lines: Vec<LayoutLine> = Vec::new();

        if self.state.lines.is_empty()
            || (self.state.lines.len() == 1 && self.state.lines[0].is_empty())
        {
            layout_lines.push(LayoutLine {
                text: String::new(),
                has_cursor: true,
                cursor_pos: Some(0),
            });
            return layout_lines;
        }

        for (index, line) in self.state.lines.iter().enumerate() {
            let is_current_line = index == self.state.cursor_line;
            if visible_width(line) <= content_width {
                layout_lines.push(LayoutLine {
                    text: line.clone(),
                    has_cursor: is_current_line,
                    cursor_pos: is_current_line.then_some(self.state.cursor_col),
                });
                continue;
            }

            let segments = self.segment(line, false);
            let chunks = word_wrap_line(line, content_width, Some(&segments));
            let chunk_count = chunks.len();
            for (chunk_index, chunk) in chunks.into_iter().enumerate() {
                let cursor_pos = self.state.cursor_col;
                let is_last_chunk = chunk_index == chunk_count - 1;

                let mut has_cursor_in_chunk = false;
                let mut adjusted_cursor_pos = 0;
                if is_current_line {
                    if is_last_chunk {
                        has_cursor_in_chunk = cursor_pos >= chunk.start_index;
                        adjusted_cursor_pos = cursor_pos.saturating_sub(chunk.start_index);
                    } else {
                        has_cursor_in_chunk =
                            cursor_pos >= chunk.start_index && cursor_pos < chunk.end_index;
                        if has_cursor_in_chunk {
                            adjusted_cursor_pos = cursor_pos - chunk.start_index;
                            // The cursor may sit in trimmed whitespace.
                            adjusted_cursor_pos = adjusted_cursor_pos.min(chunk.text.len());
                        }
                    }
                }

                layout_lines.push(LayoutLine {
                    text: chunk.text,
                    has_cursor: has_cursor_in_chunk,
                    cursor_pos: has_cursor_in_chunk.then_some(adjusted_cursor_pos),
                });
            }
        }

        layout_lines
    }

    /// Run the pending autocomplete request (the TS promise chain).
    pub async fn pump_autocomplete(&mut self) {
        let Some(pending) = self.pending_autocomplete.clone() else {
            return;
        };
        if let Some(due_at) = pending.due_at {
            let now = std::time::Instant::now();
            if due_at > now {
                tokio::time::sleep(due_at - now).await;
            }
        }
        if self
            .pending_autocomplete
            .as_ref()
            .is_none_or(|current| current.start_token != pending.start_token)
        {
            return;
        }
        self.pending_autocomplete = None;

        if pending.start_token != self.autocomplete_start_token {
            return;
        }
        let Some(provider) = self.autocomplete_provider.clone() else {
            return;
        };

        let controller = AbortController::new();
        self.autocomplete_abort = Some(controller.clone());
        self.autocomplete_request_id += 1;
        let request_id = self.autocomplete_request_id;
        let snapshot_text = self.get_text();
        let snapshot_line = self.state.cursor_line;
        let snapshot_col = self.state.cursor_col;

        let suggestions = provider
            .get_suggestions(
                &self.state.lines,
                self.state.cursor_line,
                self.state.cursor_col,
                SuggestionOptions {
                    signal: controller.signal(),
                    force: pending.force,
                },
            )
            .await;

        let is_current = !controller.signal().aborted()
            && request_id == self.autocomplete_request_id
            && self.get_text() == snapshot_text
            && self.state.cursor_line == snapshot_line
            && self.state.cursor_col == snapshot_col;
        if !is_current {
            return;
        }

        self.autocomplete_abort = None;

        let Some(suggestions) = suggestions.filter(|suggestions| !suggestions.items.is_empty())
        else {
            self.cancel_autocomplete();
            self.core.request_render();
            return;
        };

        if pending.force && pending.explicit_tab && suggestions.items.len() == 1 {
            let item = suggestions.items[0].clone();
            self.push_undo_snapshot();
            self.last_action = None;
            let result = provider.apply_completion(
                &self.state.lines,
                self.state.cursor_line,
                self.state.cursor_col,
                &item,
                &suggestions.prefix,
            );
            self.state.lines = result.lines;
            self.state.cursor_line = result.cursor_line;
            self.set_cursor_col(result.cursor_col);
            self.emit_change();
            self.core.request_render();
            return;
        }

        let state = if pending.force {
            AutocompleteState::Force
        } else {
            AutocompleteState::Regular
        };
        self.apply_autocomplete_suggestions(&suggestions, state);
        self.core.request_render();
    }
}

impl Component for Editor {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let max_padding = width.saturating_sub(1) / 2;
        let padding_x = self.padding_x.min(max_padding);
        let content_width = width.saturating_sub(padding_x * 2).max(1);

        // With padding the cursor may spill into it; without padding one column
        // stays reserved for the cursor.
        let layout_width = content_width
            .saturating_sub(if padding_x > 0 { 0 } else { 1 })
            .max(1);
        self.last_width = layout_width;

        let horizontal = (self.border_color)("─");
        let layout_lines = self.layout_text(layout_width);

        let terminal_rows = self.core.rows();
        let max_visible_lines = (terminal_rows * 3 / 10).max(5);

        let cursor_line_index = layout_lines
            .iter()
            .position(|line| line.has_cursor)
            .unwrap_or(0);

        if cursor_line_index < self.scroll_offset {
            self.scroll_offset = cursor_line_index;
        } else if cursor_line_index >= self.scroll_offset + max_visible_lines {
            self.scroll_offset = cursor_line_index - max_visible_lines + 1;
        }
        let max_scroll_offset = layout_lines.len().saturating_sub(max_visible_lines);
        self.scroll_offset = self.scroll_offset.min(max_scroll_offset);

        let visible_lines = &layout_lines
            [self.scroll_offset..(self.scroll_offset + max_visible_lines).min(layout_lines.len())];

        let mut result: Vec<String> = Vec::new();
        let left_padding = " ".repeat(padding_x);
        let right_padding = left_padding.clone();

        if self.scroll_offset > 0 {
            result.push((self.border_color)(&create_scroll_border(
                "↑",
                self.scroll_offset,
                width,
            )));
        } else {
            result.push(horizontal.repeat(width));
        }

        // The hardware cursor marker is emitted while focused so the TUI can
        // place the cursor for IME candidate windows.
        let emit_cursor_marker = self.focused;

        for layout_line in visible_lines {
            let mut display_text = layout_line.text.clone();
            let mut line_visible_width = visible_width(&layout_line.text);
            let mut cursor_in_padding = false;

            if let Some(cursor_pos) = layout_line.cursor_pos.filter(|_| layout_line.has_cursor) {
                let before = &display_text[..cursor_pos.min(display_text.len())];
                let after = &display_text[cursor_pos.min(display_text.len())..];
                let marker = if emit_cursor_marker {
                    CURSOR_MARKER
                } else {
                    ""
                };

                if after.is_empty() {
                    let cursor = "\x1b[7m \x1b[0m";
                    let next = format!("{before}{marker}{cursor}");
                    display_text = next;
                    line_visible_width += 1;
                    if line_visible_width > content_width && padding_x > 0 {
                        cursor_in_padding = true;
                    }
                } else {
                    let first_grapheme = self
                        .segment(after, false)
                        .first()
                        .map(|segment| segment.segment.clone())
                        .unwrap_or_default();
                    let rest_after = &after[first_grapheme.len()..];
                    let cursor = format!("\x1b[7m{first_grapheme}\x1b[0m");
                    display_text = format!("{before}{marker}{cursor}{rest_after}");
                }
            }

            let padding = " ".repeat(content_width.saturating_sub(line_visible_width));
            let line_right_padding = if cursor_in_padding {
                &right_padding[1.min(right_padding.len())..]
            } else {
                right_padding.as_str()
            };
            result.push(format!(
                "{left_padding}{display_text}{padding}{line_right_padding}"
            ));
        }

        let lines_below = layout_lines
            .len()
            .saturating_sub(self.scroll_offset + visible_lines.len());
        if lines_below > 0 {
            result.push((self.border_color)(&create_scroll_border(
                "↓",
                lines_below,
                width,
            )));
        } else {
            result.push(horizontal.repeat(width));
        }

        if self.autocomplete_state.is_some()
            && let Some(list) = &mut self.autocomplete_list
        {
            for line in list.render(content_width) {
                let line_width = visible_width(&line);
                let line_padding = " ".repeat(content_width.saturating_sub(line_width));
                result.push(format!("{left_padding}{line}{line_padding}{right_padding}"));
            }
        }

        shared_lines(result)
    }

    fn invalidate(&mut self) {}

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }

    fn handle_input(&mut self, data: &str) {
        // Character jump mode: the next key names the target character.
        if let Some(jump_mode) = self.jump_mode {
            if keybindings_match(data, "tui.editor.jumpForward")
                || keybindings_match(data, "tui.editor.jumpBackward")
            {
                self.jump_mode = None;
                return;
            }

            let printable = decode_printable_key(data).or_else(|| {
                data.chars()
                    .next()
                    .filter(|character| (*character as u32) >= 32)
                    .map(|_| data.to_string())
            });
            if let Some(printable) = printable {
                self.jump_mode = None;
                self.jump_to_char(&printable, jump_mode);
                return;
            }

            // A control character cancels the mode and falls through.
            self.jump_mode = None;
        }

        // Bracketed paste.
        let mut data = data.to_string();
        if data.contains("\x1b[200~") {
            self.is_in_paste = true;
            self.paste_buffer.clear();
            data = data.replacen("\x1b[200~", "", 1);
        }

        if self.is_in_paste {
            self.paste_buffer.push_str(&data);
            if let Some(end_index) = self.paste_buffer.find("\x1b[201~") {
                let paste_content = self.paste_buffer[..end_index].to_string();
                if !paste_content.is_empty() {
                    self.handle_paste(&paste_content);
                }
                self.is_in_paste = false;
                let remaining = self.paste_buffer[end_index + 6..].to_string();
                self.paste_buffer.clear();
                if !remaining.is_empty() {
                    self.handle_input(&remaining);
                }
            }
            return;
        }
        let data = data.as_str();

        // Ctrl+C is handled by the owner (exit/clear).
        if keybindings_match(data, "tui.input.copy") {
            return;
        }

        if keybindings_match(data, "tui.editor.undo") {
            self.undo();
            return;
        }

        if self.autocomplete_state.is_some() && self.autocomplete_list.is_some() {
            if keybindings_match(data, "tui.select.cancel") {
                self.cancel_autocomplete();
                return;
            }

            if keybindings_match(data, "tui.select.up")
                || keybindings_match(data, "tui.select.down")
            {
                if let Some(list) = &mut self.autocomplete_list {
                    list.handle_input(data);
                }
                return;
            }

            if keybindings_match(data, "tui.input.tab") {
                let selected = self
                    .autocomplete_list
                    .as_ref()
                    .and_then(SelectList::get_selected_item);
                if let Some(selected) = selected
                    && self.autocomplete_provider.is_some()
                {
                    self.apply_selected_completion(&selected);
                    self.cancel_autocomplete();
                    self.emit_change();
                }
                return;
            }

            if keybindings_match(data, "tui.select.confirm") {
                let selected = self
                    .autocomplete_list
                    .as_ref()
                    .and_then(SelectList::get_selected_item);
                if let Some(selected) = selected
                    && self.autocomplete_provider.is_some()
                {
                    self.apply_selected_completion(&selected);
                    let was_slash_command = self.autocomplete_prefix.starts_with('/');
                    self.cancel_autocomplete();
                    if !was_slash_command {
                        self.emit_change();
                        return;
                    }
                    // A slash command falls through to submit.
                }
            }
        }

        if keybindings_match(data, "tui.input.tab") && self.autocomplete_state.is_none() {
            self.handle_tab_completion();
            return;
        }

        if keybindings_match(data, "tui.editor.deleteToLineEnd") {
            self.delete_to_end_of_line();
            return;
        }
        if keybindings_match(data, "tui.editor.deleteToLineStart") {
            self.delete_to_start_of_line();
            return;
        }
        if keybindings_match(data, "tui.editor.deleteWordBackward") {
            self.delete_word_backwards();
            return;
        }
        if keybindings_match(data, "tui.editor.deleteWordForward") {
            self.delete_word_forward();
            return;
        }
        if keybindings_match(data, "tui.editor.deleteCharBackward")
            || matches_key(data, "shift+backspace")
        {
            self.handle_backspace();
            return;
        }
        if keybindings_match(data, "tui.editor.deleteCharForward")
            || matches_key(data, "shift+delete")
        {
            self.handle_forward_delete();
            return;
        }

        if keybindings_match(data, "tui.editor.yank") {
            self.yank();
            return;
        }
        if keybindings_match(data, "tui.editor.yankPop") {
            self.yank_pop();
            return;
        }

        // Dedicated history keys always browse instead of moving the cursor.
        if keybindings_match(data, "tui.editor.historyPrevious") {
            self.cancel_autocomplete();
            self.navigate_history(-1);
            return;
        }
        if keybindings_match(data, "tui.editor.historyNext") {
            self.cancel_autocomplete();
            self.navigate_history(1);
            return;
        }

        if keybindings_match(data, "tui.editor.cursorLineStart") {
            self.move_to_line_start();
            return;
        }
        if keybindings_match(data, "tui.editor.cursorLineEnd") {
            self.move_to_line_end();
            return;
        }
        if keybindings_match(data, "tui.editor.cursorWordLeft") {
            self.move_word_backwards();
            return;
        }
        if keybindings_match(data, "tui.editor.cursorWordRight") {
            self.move_word_forwards();
            return;
        }

        if keybindings_match(data, "tui.input.newLine")
            || (data.starts_with('\n') && data.len() > 1)
            || data == "\x1b\r"
            || data == "\x1b[13;2~"
            || (data.len() > 1 && data.contains('\x1b') && data.contains('\r'))
            || data == "\n"
        {
            if self.should_submit_on_backslash_enter(data) {
                self.handle_backspace();
                self.submit_value();
                return;
            }
            self.add_new_line();
            return;
        }

        if keybindings_match(data, "tui.input.submit") {
            if self.disable_submit {
                return;
            }
            // Terminals without Shift+Enter: a trailing backslash becomes a
            // newline instead of a submit.
            let current_line = self.current_line();
            if self.state.cursor_col > 0 && current_line[..self.state.cursor_col].ends_with('\\') {
                self.handle_backspace();
                self.add_new_line();
                return;
            }
            self.submit_value();
            return;
        }

        if keybindings_match(data, "tui.editor.cursorUp") {
            if self.is_on_first_visual_line()
                && (self.is_editor_empty() || self.history_index > -1 || self.state.cursor_col == 0)
            {
                self.navigate_history(-1);
            } else if self.is_on_first_visual_line() {
                self.move_to_line_start();
            } else {
                self.move_cursor(-1, 0);
            }
            return;
        }
        if keybindings_match(data, "tui.editor.cursorDown") {
            if self.history_index > -1 && self.is_on_last_visual_line() {
                self.navigate_history(1);
            } else if self.is_on_last_visual_line() {
                self.move_to_line_end();
            } else {
                self.move_cursor(1, 0);
            }
            return;
        }
        if keybindings_match(data, "tui.editor.cursorRight") {
            self.move_cursor(0, 1);
            return;
        }
        if keybindings_match(data, "tui.editor.cursorLeft") {
            self.move_cursor(0, -1);
            return;
        }

        if keybindings_match(data, "tui.editor.pageUp") {
            self.page_scroll(-1);
            return;
        }
        if keybindings_match(data, "tui.editor.pageDown") {
            self.page_scroll(1);
            return;
        }

        if keybindings_match(data, "tui.editor.jumpForward") {
            self.jump_mode = Some(JumpDirection::Forward);
            return;
        }
        if keybindings_match(data, "tui.editor.jumpBackward") {
            self.jump_mode = Some(JumpDirection::Backward);
            return;
        }

        if matches_key(data, "shift+space") {
            self.insert_character(" ", false);
            return;
        }

        if let Some(printable) = decode_printable_key(data) {
            self.insert_character(&printable, false);
            return;
        }

        if data.chars().next().is_some_and(|c| (c as u32) >= 32) {
            self.insert_character(data, false);
        }
    }
}

impl Focusable for Editor {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}

impl crate::editor_component::EditorComponent for Editor {
    fn get_text(&self) -> String {
        Self::get_text(self)
    }

    fn set_text(&mut self, text: &str) {
        Self::set_text(self, text);
    }

    fn take_submitted(&mut self) -> Vec<String> {
        Self::take_submitted(self)
    }

    fn take_changes(&mut self) -> Vec<String> {
        Self::take_changes(self)
    }

    fn add_to_history(&mut self, text: &str) {
        Self::add_to_history(self, text);
    }

    fn insert_text_at_cursor(&mut self, text: &str) {
        Self::insert_text_at_cursor(self, text);
    }

    fn get_expanded_text(&self) -> String {
        Self::get_expanded_text(self)
    }

    fn set_autocomplete_provider(&mut self, provider: Rc<dyn AutocompleteProvider>) {
        Self::set_autocomplete_provider(self, provider);
    }

    fn set_border_color(&mut self, border_color: Rc<dyn Fn(&str) -> String>) {
        self.border_color = border_color;
    }

    fn set_padding_x(&mut self, padding: usize) {
        Self::set_padding_x(self, padding);
    }

    fn set_autocomplete_max_visible(&mut self, max_visible: usize) {
        Self::set_autocomplete_max_visible(self, max_visible);
    }
}
