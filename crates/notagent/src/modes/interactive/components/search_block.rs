//! Compact block grouping consecutive search-tool calls.
//!
//! Takeover of `ExploreBlockComponent` from ../notagent-main-rust
//! (`crates/notagent_tui/src/explore_block.rs`), user decision 2026-08-17
//! (v0.1.8), with two deliberate renames and one scope change:
//! - The phase labels read "Searching..." / "Searched" instead of
//!   "Exploring..." / "Explored".
//! - It groups the search tools — `grep`, `find_filesystem`, `find_codebase` —
//!   and only those. The reference also folded `read`/`read_minified` in; the
//!   user scoped this block to searches.
//!
//! The mechanics are the reference's: one row per call id (a call announced
//! before its arguments stays one row), the last four rows as the collapsed
//! preview, ctrl+o to expand, a summary line, and the pending/success/error
//! block background carrying the state.

use std::collections::HashMap;

use notagent_tui::tui::Component;
use notagent_tui::utils::truncate_to_width_opts;

use crate::modes::interactive::components::tasks_panel::single_line;
use crate::modes::interactive::theme::theme::{ThemeBg, ThemeColor, theme};

const PREVIEW_ROWS: usize = 4;

/// Returns whether a tool call belongs in the compact search block.
#[must_use]
pub fn is_search_tool(name: &str) -> bool {
    matches!(name, "grep" | "find_filesystem" | "find_codebase")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchEntry {
    call_id: String,
    label: &'static str,
    detail: String,
    complete: bool,
    failed: bool,
}

impl SearchEntry {
    fn new(tool_name: &str, call_id: String, args: &serde_json::Value) -> Self {
        let (label, detail) = match tool_name {
            "find_filesystem" => (
                "Searched files",
                args.get("pattern")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
            "find_codebase" => (
                "Searched code",
                args.get("query")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
            "grep" => (
                "Searched text",
                args.get("pattern")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
            _ => ("Searched", String::new()),
        };
        // A search pattern or query may span lines; `render_entry` returns one
        // string per entry and the renderer counts it as one row.
        let detail = single_line(&detail);
        Self {
            call_id,
            label,
            detail,
            complete: false,
            failed: false,
        }
    }
}

/// Compact TUI component grouping consecutive search calls.
pub struct SearchBlockComponent {
    entries: Vec<SearchEntry>,
    entry_by_call_id: HashMap<String, usize>,
    open: bool,
    expanded: bool,
    /// Replayed blocks come from a restored session: closed on arrival.
    replayed: bool,
}

impl SearchBlockComponent {
    /// Creates an empty open search block.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            entry_by_call_id: HashMap::new(),
            open: true,
            expanded: false,
            replayed: false,
        }
    }

    /// Marks the block as replayed history.
    pub fn mark_replayed(&mut self) {
        self.replayed = true;
    }

    /// Appends one search call to the block, or updates it.
    ///
    /// A call announced before its arguments are known arrives twice: once to
    /// say it began, then again once they are. The second sighting replaces
    /// the entry rather than adding one, so a single call is a single row.
    pub fn push_call(&mut self, tool_name: &str, call_id: String, args: &serde_json::Value) {
        if let Some(index) = self.entry_by_call_id.get(&call_id).copied() {
            self.entries[index] = SearchEntry::new(tool_name, call_id, args);
            return;
        }
        let index = self.entries.len();
        self.entry_by_call_id.insert(call_id.clone(), index);
        self.entries
            .push(SearchEntry::new(tool_name, call_id, args));
    }

    /// Whether this block carries the given call.
    #[must_use]
    pub fn has_call(&self, call_id: &str) -> bool {
        self.entry_by_call_id.contains_key(call_id)
    }

    /// Marks a grouped call complete and records whether it failed.
    pub fn complete_call(&mut self, call_id: &str, failed: bool) {
        let Some(index) = self.entry_by_call_id.get(call_id).copied() else {
            return;
        };
        let entry = &mut self.entries[index];
        entry.complete = true;
        entry.failed = failed;
    }

    /// Closes the block so its final success or error background is shown.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Returns whether this block is still accepting consecutive search calls.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Toggles the expanded view; the mode calls this on the expand key.
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    fn failed_count(&self) -> usize {
        self.entries.iter().filter(|entry| entry.failed).count()
    }

    fn summary(&self) -> String {
        let searches = self.entries.len();
        let failed = self.failed_count();
        let mut parts = Vec::new();
        if searches > 0 {
            parts.push(format!(
                "{searches} search{}",
                if searches == 1 { "" } else { "es" }
            ));
        }
        if failed > 0 {
            parts.push(format!("{failed} failed"));
        }
        parts.join(", ")
    }

    fn background(&self) -> ThemeBg {
        if self.open || self.entries.iter().any(|entry| !entry.complete) {
            ThemeBg::ToolPendingBg
        } else if self.failed_count() > 0 {
            ThemeBg::ToolErrorBg
        } else {
            ThemeBg::ToolSuccessBg
        }
    }
}

impl Default for SearchBlockComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for SearchBlockComponent {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<String> {
        if self.entries.is_empty() || width == 0 {
            return Vec::new();
        }
        let theme_instance = theme();
        // Same single-space inset as the header and the summary: the entries
        // read as one column, not as a list hanging off it.
        let render_entry = |entry: &SearchEntry| {
            let mut line = format!(" {}", theme_instance.fg(ThemeColor::Muted, entry.label));
            if !entry.detail.is_empty() {
                line.push(' ');
                line.push_str(&theme_instance.fg(ThemeColor::ToolTitle, &entry.detail));
            }
            truncate_to_width_opts(&line, width, "…", false)
        };

        let header_label = if self.open {
            "Searching..."
        } else {
            "Searched"
        };
        let mut lines = vec![
            String::new(),
            format!(" {}", theme_instance.bold(header_label)),
            String::new(),
        ];
        let visible_entries = if self.expanded {
            &self.entries[..]
        } else {
            let start = self.entries.len().saturating_sub(PREVIEW_ROWS);
            &self.entries[start..]
        };
        let hidden_entries = self.entries.len().saturating_sub(PREVIEW_ROWS);
        if hidden_entries > 0 && !self.expanded {
            lines.push(format!(" {}", theme_instance.fg(ThemeColor::Muted, "...")));
        }
        lines.extend(visible_entries.iter().map(render_entry));
        if hidden_entries > 0 {
            let hint = if self.expanded {
                "(ctrl+o to collapse)".to_string()
            } else {
                format!("({hidden_entries} more, ctrl+o to expand)")
            };
            lines.push(format!(" {}", theme_instance.fg(ThemeColor::Muted, &hint)));
        }
        let summary = self.summary();
        if !summary.is_empty() {
            lines.push(format!(
                " {}",
                theme_instance.fg(ThemeColor::Muted, &summary)
            ));
        }
        lines.push(String::new());

        // The block surface: every row except the leading spacer is painted
        // to the full width in the state background.
        let background = self.background();
        let mut rendered = vec![String::new()];
        rendered.extend(
            lines
                .into_iter()
                .map(|line| paint_row(&theme_instance, background, &line, width)),
        );
        rendered
    }

    fn handle_input(&mut self, data: &str) {
        if data == "\x0f" {
            self.expanded = !self.expanded;
        }
    }
}

/// Paints one row of the block surface: the text, padded to `width`, on the
/// block background (the reference's `BlockPaint`).
fn paint_row(
    theme_instance: &crate::modes::interactive::theme::theme::Theme,
    background: ThemeBg,
    line: &str,
    width: usize,
) -> String {
    let visible = notagent_tui::utils::visible_width(line);
    let padded = format!("{line}{}", " ".repeat(width.saturating_sub(visible)));
    theme_instance.bg(background, &padded)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use super::*;
    use crate::modes::interactive::theme::theme::init_theme;
    use crate::utils::ansi::strip_ansi;

    fn theme_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        init_theme(None, false);
        guard
    }

    fn rendered(component: &mut SearchBlockComponent) -> Vec<String> {
        component
            .render(100)
            .into_iter()
            .map(|line| strip_ansi(&line).trim_end().to_string())
            .collect()
    }

    #[test]
    fn a_call_announced_before_its_arguments_stays_one_row() {
        let _guard = theme_lock();
        let mut block = SearchBlockComponent::new();
        block.push_call("grep", "call-1".to_string(), &serde_json::json!({}));
        block.push_call(
            "grep",
            "call-1".to_string(),
            &serde_json::json!({ "pattern": "needle" }),
        );
        assert_eq!(block.entries.len(), 1);
        assert!(
            rendered(&mut block)
                .iter()
                .any(|line| line.contains("needle")),
            "the row shows the arguments that arrived last"
        );
    }

    #[test]
    fn the_search_tools_and_only_those_are_grouped() {
        for name in ["grep", "find_filesystem", "find_codebase"] {
            assert!(is_search_tool(name), "{name}");
        }
        for name in ["read", "read_minified", "ls", "bash", "todo_write"] {
            assert!(!is_search_tool(name), "{name}");
        }
    }

    #[test]
    fn a_running_block_says_searching_and_lists_the_calls() {
        let _guard = theme_lock();
        let mut block = SearchBlockComponent::new();
        block.push_call(
            "find_filesystem",
            "search-1".to_string(),
            &serde_json::json!({"pattern": "config"}),
        );
        block.complete_call("search-1", false);
        let actual = rendered(&mut block);
        let expected = vec![
            "",
            "",
            " Searching...",
            "",
            " Searched files config",
            " 1 search",
            "",
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_closed_block_says_searched_and_counts_failures() {
        let _guard = theme_lock();
        let mut block = SearchBlockComponent::new();
        block.push_call(
            "find_codebase",
            "search-1".to_string(),
            &serde_json::json!({"query": "workspace lock"}),
        );
        block.push_call(
            "grep",
            "search-2".to_string(),
            &serde_json::json!({"pattern": "lock"}),
        );
        block.complete_call("search-1", false);
        block.complete_call("search-2", true);
        block.close();
        let actual = rendered(&mut block).join("\n");
        assert!(actual.contains("Searched"), "{actual}");
        assert!(!actual.contains("Searching..."), "{actual}");
        assert!(actual.contains("2 searches, 1 failed"), "{actual}");
    }

    #[test]
    fn the_collapsed_block_shows_the_last_four_rows() {
        let _guard = theme_lock();
        let mut block = SearchBlockComponent::new();
        for index in 0..5 {
            block.push_call(
                "find_filesystem",
                format!("call-{index}"),
                &serde_json::json!({"pattern": format!("pattern-{index}")}),
            );
        }
        let actual = rendered(&mut block);
        let expected = vec![
            "",
            "",
            " Searching...",
            "",
            " ...",
            " Searched files pattern-1",
            " Searched files pattern-2",
            " Searched files pattern-3",
            " Searched files pattern-4",
            " (1 more, ctrl+o to expand)",
            " 5 searches",
            "",
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn the_expanded_block_shows_all_rows() {
        let _guard = theme_lock();
        let mut block = SearchBlockComponent::new();
        for index in 0..5 {
            let call_id = format!("call-{index}");
            block.push_call(
                "grep",
                call_id.clone(),
                &serde_json::json!({"pattern": format!("pattern-{index}")}),
            );
            block.complete_call(&call_id, false);
        }
        block.close();
        block.set_expanded(true);
        let actual = rendered(&mut block);
        assert!(
            actual.contains(&" Searched text pattern-0".to_string()),
            "{actual:?}"
        );
        assert_eq!(
            actual.last().map(String::as_str),
            Some(""),
            "the surface ends with a padded blank row"
        );
        assert!(
            actual.contains(&" (ctrl+o to collapse)".to_string()),
            "{actual:?}"
        );
    }

    #[test]
    fn the_background_carries_the_state() {
        let _guard = theme_lock();
        let mut block = SearchBlockComponent::new();
        block.push_call(
            "find_codebase",
            "failed".to_string(),
            &serde_json::json!({"query": "workspace lock"}),
        );
        // Open (pending), even though the call completed.
        block.complete_call("failed", true);
        assert_eq!(block.background(), ThemeBg::ToolPendingBg);
        block.close();
        assert_eq!(block.background(), ThemeBg::ToolErrorBg);
    }

    #[test]
    fn multiline_patterns_collapse_to_one_row() {
        let _guard = theme_lock();
        let mut block = SearchBlockComponent::new();
        block.push_call(
            "grep",
            "call-1".to_string(),
            &serde_json::json!({"pattern": "first\nsecond"}),
        );
        let lines = block.render(100);
        assert_eq!(
            lines
                .iter()
                .filter(|line| strip_ansi(line).contains("first"))
                .count(),
            1
        );
        assert!(
            rendered(&mut block)
                .iter()
                .any(|line| line.contains("first second")),
            "newlines collapse to spaces"
        );
    }
}
