//! Single line text truncated to the viewport width.
//!
//! 1:1 port of `packages/tui/src/components/truncated-text.ts` (65 LOC).

use crate::tui::{Component, Line, shared_lines};
use crate::utils::{truncate_to_width, visible_width};

/// Text truncated to one line.
pub struct TruncatedText {
    text: String,
    padding_x: usize,
    padding_y: usize,
}

impl TruncatedText {
    /// New truncated text (TS defaults: no padding).
    pub fn new(text: impl Into<String>, padding_x: usize, padding_y: usize) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
        }
    }

    /// Replace the text.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }
}

impl Component for TruncatedText {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let mut result: Vec<String> = Vec::new();
        let empty_line = " ".repeat(width);

        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        let available_width = width.saturating_sub(self.padding_x * 2).max(1);
        // Only the first line is shown.
        let single_line_text = match self.text.find('\n') {
            Some(index) => &self.text[..index],
            None => self.text.as_str(),
        };
        let display_text = truncate_to_width(single_line_text, available_width);

        let padding = " ".repeat(self.padding_x);
        let line_with_padding = format!("{padding}{display_text}{padding}");
        let padding_needed = width.saturating_sub(visible_width(&line_with_padding));
        result.push(line_with_padding + &" ".repeat(padding_needed));

        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        shared_lines(result)
    }

    fn invalidate(&mut self) {
        // No cached state to invalidate currently.
    }
}
