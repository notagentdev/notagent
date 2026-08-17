//! Compact block grouping consecutive search-tool calls.
//!
//! Takeover of `ExploreBlockComponent` from ../notagent-main-rust
//! (`crates/notagent_tui/src/explore_block.rs`), user decision 2026-08-17
//! (v0.1.8), with one deliberate rename: the phase labels read
//! "Searching..." / "Searched" instead of "Exploring..." / "Explored".
//!
//! It groups the search tools — `grep`, `find_filesystem`, `find_codebase` —
//! together with the reads (`read`, `read_minified`), exactly as the reference
//! does (user decision 2026-08-17, v0.1.11; v0.1.8 had left the reads out).
//!
//! The mechanics are the reference's: one row per call id (a call announced
//! before its arguments stays one row), the last four rows as the collapsed
//! preview, ctrl+o to expand, a summary line counting searches and reads
//! separately, and the pending/success/error block background carrying the
//! state.

use std::collections::HashMap;

use notagent_tui::tui::Component;
use notagent_tui::utils::truncate_to_width_opts;

use crate::modes::interactive::components::tasks_panel::single_line;
use crate::modes::interactive::theme::theme::{
    BlockStyle, ThemeBg, ThemeColor, badge, block_style, format_elapsed_live,
    format_elapsed_precise, theme,
};

const PREVIEW_ROWS: usize = 4;

/// Returns whether a tool call belongs in the compact search block.
#[must_use]
pub fn is_search_tool(name: &str) -> bool {
    matches!(
        name,
        "grep" | "find_filesystem" | "find_codebase" | "read" | "read_minified"
    )
}

/// What an entry stands for; the summary counts the two kinds separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchKind {
    Search,
    Read,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchEntry {
    call_id: String,
    label: &'static str,
    detail: String,
    /// The line range of a read, e.g. `L1-200`.
    range: Option<String>,
    kind: SearchKind,
    complete: bool,
    failed: bool,
}

impl SearchEntry {
    fn new(tool_name: &str, call_id: String, args: &serde_json::Value) -> Self {
        let (label, detail, range, kind) = match tool_name {
            "read" | "read_minified" => {
                // The file's name alone: the block is a list of what was
                // looked at, and the full path costs the whole row.
                let path = args
                    .get("file_path")
                    .filter(|value| !value.is_null())
                    .or_else(|| args.get("path"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let name = path
                    .rsplit('/')
                    .next()
                    .filter(|name| !name.is_empty())
                    .unwrap_or(path)
                    .to_string();
                ("Read", name, read_range(args), SearchKind::Read)
            }
            "find_filesystem" => (
                "Searched files",
                args.get("pattern")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                None,
                SearchKind::Search,
            ),
            "find_codebase" => (
                "Searched code",
                args.get("query")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                None,
                SearchKind::Search,
            ),
            "grep" => (
                "Searched text",
                args.get("pattern")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                None,
                SearchKind::Search,
            ),
            _ => ("Searched", String::new(), None, SearchKind::Search),
        };
        // A search pattern or query may span lines; `render_entry` returns one
        // string per entry and the renderer counts it as one row.
        let detail = single_line(&detail);
        Self {
            call_id,
            label,
            detail,
            range,
            kind,
            complete: false,
            failed: false,
        }
    }
}

/// The line range of a read as `L{start}-{end}`, or `L{start}-` when the read
/// runs to the end of the file. `None` when the call reads the whole file.
///
/// The reference reads a `range` object; our read tool takes `offset`/`limit`
/// (`read.rs`), so the range is derived from those the same way the read
/// renderer derives its own header.
fn read_range(args: &serde_json::Value) -> Option<String> {
    let offset = args.get("offset").filter(|value| !value.is_null());
    let limit = args.get("limit").filter(|value| !value.is_null());
    if offset.is_none() && limit.is_none() {
        return None;
    }
    let start = offset
        .and_then(serde_json::Value::as_i64)
        .filter(|start| *start > 0)
        .unwrap_or(1);
    match limit.and_then(serde_json::Value::as_i64) {
        Some(limit) if limit > 0 => Some(format!("L{start}-{}", start + limit - 1)),
        _ => Some(format!("L{start}-")),
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
    /// When the block started collecting calls; drives the badge runtime.
    started: std::time::Instant,
    /// Total runtime, frozen when the block closes. Replayed blocks carry no
    /// meaningful runtime and stay `None`.
    finished: Option<std::time::Duration>,
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
            started: std::time::Instant::now(),
            finished: None,
        }
    }

    /// Whether calls are still running, i.e. the badge runtime still counts.
    #[must_use]
    pub fn is_running(&self) -> bool {
        !self.replayed && (self.open || self.entries.iter().any(|entry| !entry.complete))
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
        if self.finished.is_none() && !self.replayed {
            self.finished = Some(self.started.elapsed());
        }
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
        let searches = self
            .entries
            .iter()
            .filter(|entry| entry.kind == SearchKind::Search)
            .count();
        let reads = self
            .entries
            .iter()
            .filter(|entry| entry.kind == SearchKind::Read)
            .count();
        let failed = self.failed_count();
        let mut parts = Vec::new();
        if searches > 0 {
            parts.push(format!(
                "{searches} search{}",
                if searches == 1 { "" } else { "es" }
            ));
        }
        if reads > 0 {
            parts.push(format!("{reads} read{}", if reads == 1 { "" } else { "s" }));
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
            if let Some(range) = &entry.range {
                line.push(' ');
                line.push_str(&theme_instance.fg(ThemeColor::Dim, range));
            }
            truncate_to_width_opts(&line, width, "…", false)
        };

        // In the badge style the block sheds surface and padding rows; the
        // state moves into the SEARCHING/SEARCHED badge, with the runtime
        // beside it (the reference's `BlockStyle::Bar` rendering).
        let badge_style = block_style() == BlockStyle::Badge;
        let mut lines = if badge_style {
            let label = if self.open || self.entries.iter().any(|entry| !entry.complete) {
                "Searching"
            } else {
                "Searched"
            };
            let mut badge_line = format!(" {}", badge(&theme_instance, self.background(), label));
            let runtime_text = if self.is_running() {
                format_elapsed_live(self.started.elapsed())
            } else {
                self.finished.map(format_elapsed_precise)
            };
            if let Some(runtime_text) = runtime_text {
                badge_line.push_str(&format!(
                    " {}{}{}",
                    theme_instance.fg(ThemeColor::Dim, "("),
                    theme_instance.fg(ThemeColor::Muted, &runtime_text),
                    theme_instance.fg(ThemeColor::Dim, ")")
                ));
            }
            vec![badge_line]
        } else {
            let header_label = if self.open {
                "Searching..."
            } else {
                "Searched"
            };
            vec![
                String::new(),
                format!(" {}", theme_instance.bold(header_label)),
                String::new(),
            ]
        };
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
        let hint = (hidden_entries > 0).then(|| {
            let hint = if self.expanded {
                "(ctrl+o to collapse)".to_string()
            } else {
                format!("({hidden_entries} more, ctrl+o to expand)")
            };
            format!(" {}", theme_instance.fg(ThemeColor::Muted, &hint))
        });
        if !badge_style && let Some(hint) = hint.clone() {
            lines.push(hint);
        }
        let summary = self.summary();
        if !summary.is_empty() {
            lines.push(format!(
                " {}",
                theme_instance.fg(ThemeColor::Muted, &summary)
            ));
        }
        // In the badge style the info line closes the block, below the
        // summary; the filled layout keeps it above, unchanged.
        if badge_style && let Some(hint) = hint {
            lines.push(hint);
        }

        let mut rendered = vec![String::new()];
        if badge_style {
            rendered.extend(lines);
        } else {
            lines.push(String::new());
            // The block surface: every row except the leading spacer is
            // painted to the full width in the state background.
            let background = self.background();
            rendered.extend(
                lines
                    .into_iter()
                    .map(|line| paint_row(&theme_instance, background, &line, width)),
            );
        }
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
    use crate::modes::interactive::theme::theme::{init_theme, set_block_style};
    use crate::utils::ansi::strip_ansi;

    fn theme_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        init_theme(None, false);
        // The block style is a process global (default: badge); the layout
        // tests pin the standard surface unless they say otherwise.
        set_block_style(BlockStyle::Standard);
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
    fn the_search_and_read_tools_and_only_those_are_grouped() {
        for name in [
            "grep",
            "find_filesystem",
            "find_codebase",
            "read",
            "read_minified",
        ] {
            assert!(is_search_tool(name), "{name}");
        }
        for name in ["ls", "bash", "todo_write", "write", "edit"] {
            assert!(!is_search_tool(name), "{name}");
        }
    }

    #[test]
    fn a_read_shows_its_file_name_and_line_range() {
        let _guard = theme_lock();
        let mut block = SearchBlockComponent::new();
        block.push_call(
            "read",
            "read-1".to_string(),
            &serde_json::json!({ "file_path": "/repo/crates/notagent/src/lib.rs", "offset": 1, "limit": 200 }),
        );
        block.complete_call("read-1", false);
        block.close();
        let actual = rendered(&mut block).join("\n");
        // The name alone, not the path, plus the range the reference shows.
        assert!(actual.contains("Read lib.rs L1-200"), "{actual}");
        assert!(!actual.contains("/repo/crates"), "{actual}");
        assert!(actual.contains("1 read"), "{actual}");
    }

    #[test]
    fn a_whole_file_read_shows_no_range_and_an_open_read_shows_one() {
        let _guard = theme_lock();
        let mut whole = SearchBlockComponent::new();
        whole.push_call(
            "read",
            "read-1".to_string(),
            &serde_json::json!({ "file_path": "README.md" }),
        );
        let whole = rendered(&mut whole).join("\n");
        assert!(whole.contains("Read README.md"), "{whole}");
        assert!(!whole.contains(" L"), "{whole}");

        let _guard2 = ();
        let mut open = SearchBlockComponent::new();
        open.push_call(
            "read_minified",
            "read-2".to_string(),
            &serde_json::json!({ "file_path": "README.md", "offset": 40 }),
        );
        let open = rendered(&mut open).join("\n");
        assert!(open.contains("Read README.md L40-"), "{open}");
    }

    #[test]
    fn the_summary_counts_searches_and_reads_separately() {
        let _guard = theme_lock();
        let mut block = SearchBlockComponent::new();
        block.push_call(
            "grep",
            "search-1".to_string(),
            &serde_json::json!({ "pattern": "needle" }),
        );
        block.push_call(
            "read",
            "read-1".to_string(),
            &serde_json::json!({ "file_path": "a.rs" }),
        );
        block.push_call(
            "read",
            "read-2".to_string(),
            &serde_json::json!({ "file_path": "b.rs" }),
        );
        for call_id in ["search-1", "read-1", "read-2"] {
            block.complete_call(call_id, false);
        }
        block.close();
        let actual = rendered(&mut block).join("\n");
        assert!(actual.contains("1 search, 2 reads"), "{actual}");
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
    fn the_badge_style_leads_with_the_state_badge_and_sheds_the_surface() {
        let _guard = theme_lock();
        set_block_style(BlockStyle::Badge);
        let mut block = SearchBlockComponent::new();
        block.push_call(
            "find_codebase",
            "failed".to_string(),
            &serde_json::json!({"query": "workspace lock"}),
        );
        block.complete_call("failed", true);
        block.close();
        let actual = rendered(&mut block);
        // Spacer, badge with the final sub-second runtime, the entry directly
        // below — no surface padding rows.
        let expected = vec![
            "",
            "  SEARCHED  (0ms)",
            " Searched code workspace lock",
            " 1 search, 1 failed",
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn the_badge_says_searching_while_calls_still_run() {
        let _guard = theme_lock();
        set_block_style(BlockStyle::Badge);
        let mut block = SearchBlockComponent::new();
        block.push_call(
            "grep",
            "search-1".to_string(),
            &serde_json::json!({"pattern": "config"}),
        );
        let actual = rendered(&mut block).join("\n");
        assert!(actual.contains("SEARCHING"), "{actual}");
        assert!(!actual.contains("SEARCHED "), "{actual}");
    }

    #[test]
    fn the_badge_style_info_line_closes_the_block_below_the_summary() {
        let _guard = theme_lock();
        set_block_style(BlockStyle::Badge);
        let mut block = SearchBlockComponent::new();
        for index in 0..5 {
            let call_id = format!("call-{index}");
            block.push_call(
                "find_filesystem",
                call_id.clone(),
                &serde_json::json!({"pattern": format!("pattern-{index}")}),
            );
            block.complete_call(&call_id, false);
        }
        block.close();
        let collapsed = rendered(&mut block);
        assert_eq!(
            collapsed.last().map(String::as_str),
            Some(" (1 more, ctrl+o to expand)"),
            "{collapsed:?}"
        );
        assert!(
            collapsed[collapsed.len() - 2].contains("5 searches"),
            "summary right above the info line: {collapsed:?}"
        );
    }

    #[test]
    fn a_replayed_block_shows_no_runtime() {
        let _guard = theme_lock();
        set_block_style(BlockStyle::Badge);
        let mut block = SearchBlockComponent::new();
        block.mark_replayed();
        block.push_call(
            "grep",
            "call-1".to_string(),
            &serde_json::json!({"pattern": "config"}),
        );
        block.complete_call("call-1", false);
        block.close();
        let actual = rendered(&mut block).join("\n");
        assert!(actual.contains("SEARCHED"), "{actual}");
        assert!(!actual.contains("ms)"), "{actual}");
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
