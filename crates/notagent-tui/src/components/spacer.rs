use crate::tui::{Component, Line};

/// Renders `lines` empty lines.
pub struct Spacer {
    lines: usize,
}

impl Spacer {
    pub fn new(lines: usize) -> Self {
        Self { lines }
    }

    /// Change the number of lines.
    pub fn set_lines(&mut self, lines: usize) {
        self.lines = lines;
    }
}

impl Component for Spacer {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        vec![Line::from(""); self.lines]
    }

    fn invalidate(&mut self) {
        // No cached state to invalidate currently.
    }
}
