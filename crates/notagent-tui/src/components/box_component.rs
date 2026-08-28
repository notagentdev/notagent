use crate::components::text::BackgroundFn;
use crate::tui::{Component, ComponentRef, Container, Line, shared_lines};
use crate::utils::{apply_background_to_line, visible_width};

struct RenderCache {
    /// Cache key: the padded child lines, compared by content each frame.
    child_lines: Vec<String>,
    width: usize,
    bg_sample: Option<String>,
    lines: Vec<Line>,
}

/// Container applying padding and a background to all children.
pub struct BoxComponent {
    container: Container,
    padding_x: usize,
    padding_y: usize,
    bg_fn: Option<BackgroundFn>,
    cache: Option<RenderCache>,
}

impl BoxComponent {
    pub fn new(padding_x: usize, padding_y: usize, bg_fn: Option<BackgroundFn>) -> Self {
        Self {
            container: Container::new(),
            padding_x,
            padding_y,
            bg_fn,
            cache: None,
        }
    }

    /// Append a child.
    pub fn add_child(&mut self, component: ComponentRef) {
        self.container.add_child(component);
        self.cache = None;
    }

    /// Remove a child.
    pub fn remove_child(&mut self, component: &ComponentRef) {
        self.container.remove_child(component);
        self.cache = None;
    }

    /// Drop all children.
    pub fn clear(&mut self) {
        self.container.clear();
        self.cache = None;
    }

    /// Set the background function.
    /// Deliberately does not invalidate: a changed background is detected by
    pub fn set_bg_fn(&mut self, bg_fn: Option<BackgroundFn>) {
        self.bg_fn = bg_fn;
    }

    /// sheds the vertical padding rows of an already-built box.
    pub fn set_padding(&mut self, padding_x: usize, padding_y: usize) {
        if self.padding_x == padding_x && self.padding_y == padding_y {
            return;
        }
        self.padding_x = padding_x;
        self.padding_y = padding_y;
        self.cache = None;
    }

    fn matches_cache(
        &self,
        width: usize,
        child_lines: &[String],
        bg_sample: &Option<String>,
    ) -> bool {
        self.cache.as_ref().is_some_and(|cache| {
            cache.width == width
                && cache.bg_sample == *bg_sample
                && cache.child_lines == child_lines
        })
    }

    fn apply_bg(&self, line: &str, width: usize) -> String {
        let pad_needed = width.saturating_sub(visible_width(line));
        let padded = format!("{line}{}", " ".repeat(pad_needed));
        match &self.bg_fn {
            Some(bg_fn) => apply_background_to_line(&padded, width, |text| bg_fn(text)),
            None => padded,
        }
    }
}

impl Component for BoxComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        if self.container.children.is_empty() {
            return Vec::new();
        }

        let content_width = width.saturating_sub(self.padding_x * 2).max(1);
        let left_pad = " ".repeat(self.padding_x);

        let mut child_lines: Vec<String> = Vec::new();
        for child in &self.container.children {
            for line in child.borrow_mut().render(content_width) {
                // A child that finishes its lines (markdown bakes the segment
                // reset into its cache) would carry a full SGR reset into the
                // middle of this box's background wash and cut it off before
                // the right padding column. Strip it; the paint pass appends
                // it again at the true end of the boxed line.
                let line = line
                    .strip_suffix(crate::tui::SEGMENT_RESET)
                    .unwrap_or(&line);
                child_lines.push(format!("{left_pad}{line}"));
            }
        }
        if child_lines.is_empty() {
            return Vec::new();
        }

        // Detect a changed background function by sampling its output.
        let bg_sample = self.bg_fn.as_ref().map(|bg_fn| bg_fn("test"));

        if self.matches_cache(width, &child_lines, &bg_sample) {
            return self.cache.as_ref().expect("cache matched").lines.clone();
        }

        let mut result: Vec<String> = Vec::new();
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }
        for line in &child_lines {
            result.push(self.apply_bg(line, width));
        }
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }

        let result = shared_lines(result);
        self.cache = Some(RenderCache {
            child_lines,
            width,
            bg_sample,
            lines: result.clone(),
        });
        result
    }

    fn invalidate(&mut self) {
        self.cache = None;
        self.container.invalidate();
    }

    fn as_container(&self) -> Option<&Container> {
        Some(&self.container)
    }
}
