//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/visual-truncate.ts` (50 LOC).
//!
//! Shared utility for truncating text to visual lines (accounting for line wrapping).
//! Used by both tool-execution.rs and bash-execution.rs for consistent behavior.

use notagent_tui::components::text::Text;
use notagent_tui::tui::Component;

/// Result of [`truncate_to_visual_lines`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VisualTruncateResult {
    /// The visual lines to display
    pub visual_lines: Vec<String>,
    /// Number of visual lines that were skipped (hidden)
    pub skipped_count: usize,
}

/// Truncate text to a maximum number of visual lines (from the end).
/// This accounts for line wrapping based on terminal width.
///
/// * `text` — The text content (may contain newlines)
/// * `max_visual_lines` — Maximum number of visual lines to show
/// * `width` — Terminal/render width
/// * `padding_x` — Horizontal padding for Text component (default 0).
///   Use 0 when result will be placed in a Box (Box adds its own padding).
///   Use 1 when result will be placed in a plain Container.
pub fn truncate_to_visual_lines(
    text: &str,
    max_visual_lines: usize,
    width: usize,
    padding_x: usize,
) -> VisualTruncateResult {
    if text.is_empty() {
        return VisualTruncateResult {
            visual_lines: Vec::new(),
            skipped_count: 0,
        };
    }

    // Create a temporary Text component to render and get visual lines
    let mut temp_text = Text::new(text, padding_x, 0);
    let all_visual_lines = temp_text.render(width);

    if all_visual_lines.len() <= max_visual_lines {
        return VisualTruncateResult {
            visual_lines: all_visual_lines,
            skipped_count: 0,
        };
    }

    // Take the last N visual lines. `slice(-0)` is `slice(0)` in JavaScript, so
    // a limit of zero keeps every line and still reports them all as skipped
    // (bug-compat).
    let skipped_count = all_visual_lines.len() - max_visual_lines;
    let visual_lines = if max_visual_lines == 0 {
        all_visual_lines
    } else {
        all_visual_lines[skipped_count..].to_vec()
    };

    VisualTruncateResult {
        visual_lines,
        skipped_count,
    }
}
