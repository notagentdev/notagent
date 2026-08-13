//! Word-wrapping text component.
//!
//! 1:1 port of `packages/tui/src/components/text.ts` (106 LOC).

use std::rc::Rc;

use crate::tui::Component;
use crate::utils::{apply_background_to_line, visible_width, wrap_text_with_ansi};

/// Displays multi-line text with word wrapping, padding and optional background.
pub struct Text {
    text: String,
    padding_x: usize,
    padding_y: usize,
    custom_bg_fn: Option<Rc<dyn Fn(&str) -> String>>,
    cached_text: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<String>>,
}

impl Text {
    /// New text component (TS defaults: `paddingX = 1`, `paddingY = 1`).
    pub fn new(text: impl Into<String>, padding_x: usize, padding_y: usize) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
            custom_bg_fn: None,
            cached_text: None,
            cached_width: None,
            cached_lines: None,
        }
    }

    /// Replace the text.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.clear_cache();
    }

    /// Set or clear the background function.
    pub fn set_custom_bg_fn(&mut self, custom_bg_fn: Option<Rc<dyn Fn(&str) -> String>>) {
        self.custom_bg_fn = custom_bg_fn;
        self.clear_cache();
    }

    fn clear_cache(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }
}

impl Component for Text {
    fn render(&mut self, width: usize) -> Vec<String> {
        if let Some(lines) = &self.cached_lines
            && self.cached_text.as_deref() == Some(self.text.as_str())
            && self.cached_width == Some(width)
        {
            return lines.clone();
        }

        // Nothing to render without actual text.
        if self.text.trim().is_empty() {
            self.cached_text = Some(self.text.clone());
            self.cached_width = Some(width);
            self.cached_lines = Some(Vec::new());
            return Vec::new();
        }

        let normalized_text = self.text.replace('\t', "   ");
        let content_width = width.saturating_sub(self.padding_x * 2).max(1);
        let wrapped_lines = wrap_text_with_ansi(&normalized_text, content_width);

        let margin = " ".repeat(self.padding_x);
        let mut content_lines: Vec<String> = Vec::new();
        for line in wrapped_lines {
            let line_with_margins = format!("{margin}{line}{margin}");
            match &self.custom_bg_fn {
                Some(bg_fn) => content_lines.push(apply_background_to_line(
                    &line_with_margins,
                    width,
                    |text| bg_fn(text),
                )),
                None => {
                    let padding_needed = width.saturating_sub(visible_width(&line_with_margins));
                    content_lines.push(line_with_margins + &" ".repeat(padding_needed));
                }
            }
        }

        let empty_line = " ".repeat(width);
        let mut empty_lines: Vec<String> = Vec::new();
        for _ in 0..self.padding_y {
            empty_lines.push(match &self.custom_bg_fn {
                Some(bg_fn) => apply_background_to_line(&empty_line, width, |text| bg_fn(text)),
                None => empty_line.clone(),
            });
        }

        let mut result = empty_lines.clone();
        result.extend(content_lines);
        result.extend(empty_lines);

        self.cached_text = Some(self.text.clone());
        self.cached_width = Some(width);
        self.cached_lines = Some(result.clone());

        if result.is_empty() {
            vec![String::new()]
        } else {
            result
        }
    }

    fn invalidate(&mut self) {
        self.clear_cache();
    }
}
