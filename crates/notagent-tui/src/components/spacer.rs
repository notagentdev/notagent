//! Empty lines.
//!
//! 1:1 port of `packages/tui/src/components/spacer.ts` (28 LOC).

use crate::tui::Component;

/// Renders `lines` empty lines.
pub struct Spacer {
    lines: usize,
}

impl Spacer {
    /// New spacer (TS default: one line).
    pub fn new(lines: usize) -> Self {
        Self { lines }
    }

    /// Change the number of lines.
    pub fn set_lines(&mut self, lines: usize) {
        self.lines = lines;
    }
}

impl Component for Spacer {
    fn render(&mut self, _width: usize) -> Vec<String> {
        vec![String::new(); self.lines]
    }

    fn invalidate(&mut self) {
        // No cached state to invalidate currently.
    }
}
