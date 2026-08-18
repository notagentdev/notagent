//! Single line text input with horizontal scrolling.
//!
//! 1:1 port of `packages/tui/src/components/input.ts` (447 LOC). Cursor
//! positions are byte offsets instead of UTF-16 indices; all arithmetic stays
//! internally consistent (deviation class 1).

use crate::keybindings::keybindings_match;
use crate::keys::decode_kitty_printable;
use crate::kill_ring::{KillRing, KillRingPushOptions};
use crate::tui::{CURSOR_MARKER, Component, Focusable, Line};
use crate::undo_stack::UndoStack;
use crate::utils::{graphemes, is_whitespace_char, slice_by_column, visible_width};
use crate::word_navigation::{WordNavigationOptions, find_word_backward, find_word_forward};

#[derive(Debug, Clone)]
struct InputState {
    value: String,
    cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastAction {
    Kill,
    Yank,
    TypeWord,
}

/// Callback invoked on submit.
pub type SubmitCallback = Box<dyn FnMut(&str)>;

/// Single line text input.
pub struct Input {
    value: String,
    cursor: usize,
    /// Called when the input is submitted.
    pub on_submit: Option<SubmitCallback>,
    /// Called when the input is cancelled.
    pub on_escape: Option<Box<dyn FnMut()>>,
    focused: bool,
    paste_buffer: String,
    is_in_paste: bool,
    kill_ring: KillRing,
    last_action: Option<LastAction>,
    undo_stack: UndoStack<InputState>,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    /// Empty input.
    pub fn new() -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            on_submit: None,
            on_escape: None,
            focused: false,
            paste_buffer: String::new(),
            is_in_paste: false,
            kill_ring: KillRing::new(),
            last_action: None,
            undo_stack: UndoStack::new(),
        }
    }

    /// Current value.
    pub fn get_value(&self) -> &str {
        &self.value
    }

    /// Replace the value; the cursor is clamped to its end.
    pub fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.cursor.min(self.value.len());
        while self.cursor > 0 && !self.value.is_char_boundary(self.cursor) {
            self.cursor -= 1;
        }
    }

    fn insert_character(&mut self, character: &str) {
        // Undo coalescing: consecutive word characters form one undo unit.
        if is_whitespace_char(character) || self.last_action != Some(LastAction::TypeWord) {
            self.push_undo();
        }
        self.last_action = Some(LastAction::TypeWord);
        self.value.insert_str(self.cursor, character);
        self.cursor += character.len();
    }

    fn handle_backspace(&mut self) {
        self.last_action = None;
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let grapheme_length = graphemes(&self.value[..self.cursor])
            .next_back()
            .map_or(1, str::len);
        let start = self.cursor - grapheme_length;
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn handle_forward_delete(&mut self) {
        self.last_action = None;
        if self.cursor >= self.value.len() {
            return;
        }
        self.push_undo();
        let grapheme_length = graphemes(&self.value[self.cursor..])
            .next()
            .map_or(1, str::len);
        self.value
            .replace_range(self.cursor..self.cursor + grapheme_length, "");
    }

    fn delete_to_line_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let deleted_text = self.value[..self.cursor].to_string();
        self.kill_ring.push(
            &deleted_text,
            KillRingPushOptions {
                prepend: true,
                accumulate: self.last_action == Some(LastAction::Kill),
            },
        );
        self.last_action = Some(LastAction::Kill);
        self.value = self.value[self.cursor..].to_string();
        self.cursor = 0;
    }

    fn delete_to_line_end(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.push_undo();
        let deleted_text = self.value[self.cursor..].to_string();
        self.kill_ring.push(
            &deleted_text,
            KillRingPushOptions {
                prepend: false,
                accumulate: self.last_action == Some(LastAction::Kill),
            },
        );
        self.last_action = Some(LastAction::Kill);
        self.value.truncate(self.cursor);
    }

    fn delete_word_backwards(&mut self) {
        if self.cursor == 0 {
            return;
        }
        // Save lastAction before the cursor move resets it.
        let was_kill = self.last_action == Some(LastAction::Kill);
        self.push_undo();

        let old_cursor = self.cursor;
        self.move_word_backwards();
        let delete_from = self.cursor;
        self.cursor = old_cursor;

        let deleted_text = self.value[delete_from..self.cursor].to_string();
        self.kill_ring.push(
            &deleted_text,
            KillRingPushOptions {
                prepend: true,
                accumulate: was_kill,
            },
        );
        self.last_action = Some(LastAction::Kill);
        self.value.replace_range(delete_from..self.cursor, "");
        self.cursor = delete_from;
    }

    fn delete_word_forward(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        let was_kill = self.last_action == Some(LastAction::Kill);
        self.push_undo();

        let old_cursor = self.cursor;
        self.move_word_forwards();
        let delete_to = self.cursor;
        self.cursor = old_cursor;

        let deleted_text = self.value[self.cursor..delete_to].to_string();
        self.kill_ring.push(
            &deleted_text,
            KillRingPushOptions {
                prepend: false,
                accumulate: was_kill,
            },
        );
        self.last_action = Some(LastAction::Kill);
        self.value.replace_range(self.cursor..delete_to, "");
    }

    fn yank(&mut self) {
        let Some(text) = self.kill_ring.peek().map(str::to_string) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        self.push_undo();
        self.value.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.last_action = Some(LastAction::Yank);
    }

    fn yank_pop(&mut self) {
        if self.last_action != Some(LastAction::Yank) || self.kill_ring.len() <= 1 {
            return;
        }
        self.push_undo();

        // Remove the previously yanked text (still at the end of the ring).
        let prev_text = self.kill_ring.peek().unwrap_or_default().to_string();
        let start = self.cursor - prev_text.len();
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;

        self.kill_ring.rotate();
        let text = self.kill_ring.peek().unwrap_or_default().to_string();
        self.value.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.last_action = Some(LastAction::Yank);
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(InputState {
            value: self.value.clone(),
            cursor: self.cursor,
        });
    }

    fn undo(&mut self) {
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        self.value = snapshot.value;
        self.cursor = snapshot.cursor;
        self.last_action = None;
    }

    fn move_word_backwards(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.last_action = None;
        self.cursor =
            find_word_backward(&self.value, self.cursor, &WordNavigationOptions::default());
    }

    fn move_word_forwards(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.last_action = None;
        self.cursor =
            find_word_forward(&self.value, self.cursor, &WordNavigationOptions::default());
    }

    fn handle_paste(&mut self, pasted_text: &str) {
        self.last_action = None;
        self.push_undo();
        // Newlines are dropped, tabs become four spaces.
        // TS chains four replaces; collapsing the two single-character ones is
        // equivalent because the CRLF pass already removed those pairs.
        let clean_text = pasted_text
            .replace("\r\n", "")
            .replace(['\r', '\n'], "")
            .replace('\t', "    ");
        self.value.insert_str(self.cursor, &clean_text);
        self.cursor += clean_text.len();
    }
}

impl Component for Input {
    fn handle_input(&mut self, data: &str) {
        let mut data = data.to_string();

        // Bracketed paste: \x1b[200~ … \x1b[201~
        if let Some(index) = data.find("\x1b[200~") {
            self.is_in_paste = true;
            self.paste_buffer.clear();
            data.replace_range(index..index + "\x1b[200~".len(), "");
        }

        if self.is_in_paste {
            self.paste_buffer.push_str(&data);
            if let Some(end_index) = self.paste_buffer.find("\x1b[201~") {
                let paste_content = self.paste_buffer[..end_index].to_string();
                self.handle_paste(&paste_content);
                self.is_in_paste = false;
                let remaining = self.paste_buffer[end_index + "\x1b[201~".len()..].to_string();
                self.paste_buffer.clear();
                if !remaining.is_empty() {
                    self.handle_input(&remaining);
                }
            }
            return;
        }

        if keybindings_match(&data, "tui.select.cancel") {
            if let Some(callback) = self.on_escape.as_mut() {
                callback();
            }
            return;
        }
        if keybindings_match(&data, "tui.editor.undo") {
            self.undo();
            return;
        }
        if keybindings_match(&data, "tui.input.submit") || data == "\n" {
            let value = self.value.clone();
            if let Some(callback) = self.on_submit.as_mut() {
                callback(&value);
            }
            return;
        }

        if keybindings_match(&data, "tui.editor.deleteCharBackward") {
            self.handle_backspace();
            return;
        }
        if keybindings_match(&data, "tui.editor.deleteCharForward") {
            self.handle_forward_delete();
            return;
        }
        if keybindings_match(&data, "tui.editor.deleteWordBackward") {
            self.delete_word_backwards();
            return;
        }
        if keybindings_match(&data, "tui.editor.deleteWordForward") {
            self.delete_word_forward();
            return;
        }
        if keybindings_match(&data, "tui.editor.deleteToLineStart") {
            self.delete_to_line_start();
            return;
        }
        if keybindings_match(&data, "tui.editor.deleteToLineEnd") {
            self.delete_to_line_end();
            return;
        }

        if keybindings_match(&data, "tui.editor.yank") {
            self.yank();
            return;
        }
        if keybindings_match(&data, "tui.editor.yankPop") {
            self.yank_pop();
            return;
        }

        if keybindings_match(&data, "tui.editor.cursorLeft") {
            self.last_action = None;
            if self.cursor > 0 {
                let length = graphemes(&self.value[..self.cursor])
                    .next_back()
                    .map_or(1, str::len);
                self.cursor -= length;
            }
            return;
        }
        if keybindings_match(&data, "tui.editor.cursorRight") {
            self.last_action = None;
            if self.cursor < self.value.len() {
                let length = graphemes(&self.value[self.cursor..])
                    .next()
                    .map_or(1, str::len);
                self.cursor += length;
            }
            return;
        }
        if keybindings_match(&data, "tui.editor.cursorLineStart") {
            self.last_action = None;
            self.cursor = 0;
            return;
        }
        if keybindings_match(&data, "tui.editor.cursorLineEnd") {
            self.last_action = None;
            self.cursor = self.value.len();
            return;
        }
        if keybindings_match(&data, "tui.editor.cursorWordLeft") {
            self.move_word_backwards();
            return;
        }
        if keybindings_match(&data, "tui.editor.cursorWordRight") {
            self.move_word_forwards();
            return;
        }

        // Kitty CSI-u printable character (e.g. \x1b[97u for 'a'). Decoded before
        // the control character check because CSI-u contains \x1b.
        if let Some(kitty_printable) = decode_kitty_printable(&data) {
            self.insert_character(&kitty_printable);
            return;
        }

        // Regular input: printable characters including Unicode, but no control
        // characters (C0, DEL, C1).
        let has_control_chars = data.chars().any(|c| {
            let code = u32::from(c);
            code < 32 || code == 0x7f || (0x80..=0x9f).contains(&code)
        });
        if !has_control_chars {
            let data = data.clone();
            self.insert_character(&data);
        }
    }

    fn render(&mut self, width: usize) -> Vec<Line> {
        let prompt = "> ";
        let Some(available_width) = width.checked_sub(prompt.len()).filter(|w| *w > 0) else {
            return vec![Line::from(prompt)];
        };

        let visible_text;
        let mut cursor_display = self.cursor;
        let total_width = visible_width(&self.value);

        if total_width < available_width {
            // Everything fits (with room for the cursor at the end).
            visible_text = self.value.clone();
        } else {
            // Horizontal scrolling; reserve a column when the cursor is at the end.
            let scroll_width = if self.cursor == self.value.len() {
                available_width - 1
            } else {
                available_width
            };
            let cursor_col = visible_width(&self.value[..self.cursor]);

            if scroll_width > 0 {
                let half_width = scroll_width / 2;
                let start_col = if cursor_col < half_width {
                    0
                } else if cursor_col > total_width.saturating_sub(half_width) {
                    total_width.saturating_sub(scroll_width)
                } else {
                    cursor_col.saturating_sub(half_width)
                };
                visible_text = slice_by_column(&self.value, start_col, scroll_width, true);
                let before_cursor = slice_by_column(
                    &self.value,
                    start_col,
                    cursor_col.saturating_sub(start_col),
                    true,
                );
                cursor_display = before_cursor.len();
            } else {
                visible_text = String::new();
                cursor_display = 0;
            }
        }

        let cursor_display = cursor_display.min(visible_text.len());
        let at_cursor = graphemes(&visible_text[cursor_display..])
            .next()
            .unwrap_or(" ")
            .to_string();
        let before_cursor = &visible_text[..cursor_display];
        let after_cursor = if cursor_display + at_cursor.len() <= visible_text.len() {
            &visible_text[cursor_display + at_cursor.len()..]
        } else {
            ""
        };

        // Zero-width hardware cursor marker before the fake cursor (IME positioning).
        let marker = if self.focused { CURSOR_MARKER } else { "" };
        // Reverse video shows the cursor.
        let text_with_cursor =
            format!("{before_cursor}{marker}\x1b[7m{at_cursor}\x1b[27m{after_cursor}");
        let visual_length = visible_width(&text_with_cursor);
        let padding = " ".repeat(available_width.saturating_sub(visual_length));

        vec![Line::from(format!("{prompt}{text_with_cursor}{padding}"))]
    }

    fn invalidate(&mut self) {
        // No cached state to invalidate currently.
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for Input {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}
