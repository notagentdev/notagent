use notagent_tui::components::markdown::Markdown;
use notagent_tui::tui::{Component, Line};
use notagent_tui::utils::visible_width;

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

/// Keep the marker outside Markdown so headings, lists and fenced code retain
/// their meaning, and wrapped lines align with the message text.
pub(super) struct MessageMarker {
    pub content: Markdown,
    pub marker: &'static str,
    pub padding: usize,
}

impl Component for MessageMarker {
    fn render(&mut self, width: usize) -> Vec<Line> {
        if width == 0 {
            return Vec::new();
        }
        let left = self.padding.saturating_add(1).max(2).min(width - 1);
        let right = self.padding.min(width - left - 1);
        let mut marked = false;
        self.content
            .render(width - left - right)
            .into_iter()
            .map(|line| {
                if line.is_empty() {
                    return line;
                }
                let prefix = if !marked && left > 0 && visible_width(&line) > 0 {
                    marked = true;
                    format!(
                        "{}{}",
                        theme().fg(ThemeColor::Text, self.marker),
                        " ".repeat(left - 1)
                    )
                } else {
                    " ".repeat(left)
                };
                Line::from(format!("{prefix}{line}{}", " ".repeat(right)))
            })
            .collect()
    }

    fn invalidate(&mut self) {
        self.content.invalidate();
    }
}
