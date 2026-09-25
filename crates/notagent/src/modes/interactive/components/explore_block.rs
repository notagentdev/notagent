//! Compact block grouping consecutive exploration calls.
//! Takeover of `ExploreBlockComponent` from ../notagent-main-rust
//! (`crates/notagent_tui/src/explore_block.rs`), user decision 2026-08-17
//! (v0.1.8), renamed back to the reference's "Exploring..." / "Explored"
//! in v0.1.12.
//! It groups every tool that only looks at the project — the searches
//! (`grep`, `find_filesystem`, `find_codebase`), the reader (`read`)
//! and the listing (`ls`). That is exactly
//! [`crate::core::tools::read_only_tool_names`], and it is the point of the
//! block: looking around is one activity, so it costs one block instead of a
//! row per call. Anything that changes the project keeps its own row. The
//! reference left `list_files` out; the user put it in (v0.1.12).
//! The mechanics are the reference's: one row per call id (a call announced
//! before its arguments stays one row), the last four rows as the collapsed
//! preview, ctrl+o to expand, a summary line counting the kinds separately,
//! and the pending/success/error state carried by the block.

use super::status_marker::{MarkerState, dot, heading};
use std::collections::HashMap;

use notagent_tui::tui::{Component, Line, shared_lines};
use notagent_tui::utils::truncate_to_width_opts;

use crate::modes::interactive::components::tasks_panel::single_line;
use crate::modes::interactive::theme::theme::{
    BlockStyle, ThemeBg, ThemeColor, badge, block_style, format_elapsed_live,
    format_elapsed_precise, theme,
};

const PREVIEW_ROWS: usize = 4;

/// Returns whether a tool call belongs in the compact explore block: every
/// read-only tool does, and only those.
#[must_use]
pub fn is_explore_tool(name: &str) -> bool {
    crate::core::tools::ToolName::parse(name)
        .is_some_and(|name| crate::core::tools::read_only_tool_names().contains(&name))
}

/// What an entry stands for; the summary counts the kinds separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExploreKind {
    Search,
    Read,
    List,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExploreEntry {
    call_id: String,
    label: &'static str,
    detail: String,
    /// The line range of a read, e.g. `L1-200`.
    range: Option<String>,
    kind: ExploreKind,
    complete: bool,
    started: bool,
    failed: bool,
}

impl ExploreEntry {
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
                ("Read", name, read_range(args), ExploreKind::Read)
            }
            "find_filesystem" => (
                "Searched files",
                args.get("pattern")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                None,
                ExploreKind::Search,
            ),
            "find_codebase" => (
                "Searched code",
                args.get("query")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                None,
                ExploreKind::Search,
            ),
            "grep" => (
                "Searched text",
                args.get("pattern")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                None,
                ExploreKind::Search,
            ),
            "ls" => (
                "Listed",
                // The path as given; `.` is what the tool itself falls back
                // to, and it reads better than an empty column.
                match args
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                {
                    "" => ".".to_string(),
                    path => path.to_string(),
                },
                None,
                ExploreKind::List,
            ),
            _ => ("Explored", String::new(), None, ExploreKind::Search),
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
            started: false,
            failed: false,
        }
    }
}

/// The line range of a read as `L{start}-{end}`, or `L{start}-` when the read
/// runs to the end of the file. `None` when the call reads the whole file.
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
pub struct ExploreBlockComponent {
    entries: Vec<ExploreEntry>,
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
    /// Second of the runtime last shown, so a tick redraws only when it moves.
    shown_second: Option<u64>,
}

impl ExploreBlockComponent {
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
            shown_second: None,
        }
    }

    /// Whether the running badge shows a new second. A transcript keeps a
    /// block's render until it reports a change, so without this the runtime
    /// stood still between two search results.
    pub fn tick_runtime(&mut self) -> bool {
        if !self.is_running() {
            return false;
        }
        let second = self.started.elapsed().as_secs();
        if self.shown_second == Some(second) {
            return false;
        }
        self.shown_second = Some(second);
        true
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
    /// A call announced before its arguments are known arrives twice: once to
    /// say it began, then again once they are. The second sighting replaces
    /// the entry rather than adding one, so a single call is a single row.
    pub fn push_call(&mut self, tool_name: &str, call_id: String, args: &serde_json::Value) {
        if let Some(index) = self.entry_by_call_id.get(&call_id).copied() {
            let mut entry = ExploreEntry::new(tool_name, call_id, args);
            entry.started = self.entries[index].started;
            entry.complete = self.entries[index].complete;
            entry.failed = self.entries[index].failed;
            self.entries[index] = entry;
            return;
        }
        let index = self.entries.len();
        self.entry_by_call_id.insert(call_id.clone(), index);
        self.entries
            .push(ExploreEntry::new(tool_name, call_id, args));
    }

    /// Whether this block carries the given call.
    #[must_use]
    pub fn has_call(&self, call_id: &str) -> bool {
        self.entry_by_call_id.contains_key(call_id)
    }

    pub fn mark_execution_started(&mut self, call_id: &str) {
        if let Some(index) = self.entry_by_call_id.get(call_id) {
            self.entries[*index].started = true;
        }
    }

    fn render_dot(&self, width: usize) -> Vec<Line> {
        let inner = super::tool_content_width(width);
        if inner == 0 || self.entries.is_empty() {
            return Vec::new();
        }
        let running = !self.replayed
            && self
                .entries
                .iter()
                .any(|e| e.started && (!e.complete || self.open));
        let settled = !self.open && self.entries.iter().all(|e| e.complete);
        let state = if running {
            MarkerState::Running
        } else if self.failed_count() > 0 {
            MarkerState::Error
        } else if settled {
            MarkerState::Success
        } else {
            MarkerState::Queued
        };
        let label = if settled || self.replayed {
            "Explored"
        } else {
            "Exploring"
        };
        let mut header = if self.expanded {
            format!("  {}", theme().fg(ThemeColor::Muted, label))
        } else {
            heading(label, state)
        };
        let runtime = if self.is_running() {
            format_elapsed_live(self.started.elapsed())
        } else {
            self.finished.map(format_elapsed_precise)
        };
        if let Some(runtime) = runtime {
            header.push_str(&theme().fg(ThemeColor::Muted, &format!(" ({runtime})")));
        }
        let mut lines = vec![String::new(), header];
        let start = if self.expanded {
            0
        } else {
            self.entries.len().saturating_sub(PREVIEW_ROWS)
        };
        if start > 0 {
            lines.push("  ...".to_owned());
        }
        for entry in &self.entries[start..] {
            let prefix = if self.expanded {
                let state = if entry.complete {
                    if entry.failed {
                        MarkerState::Error
                    } else {
                        MarkerState::Success
                    }
                } else if entry.started && !self.replayed {
                    MarkerState::Running
                } else {
                    MarkerState::Queued
                };
                format!("{} ", dot(state))
            } else {
                "  ".to_owned()
            };
            let range = entry
                .range
                .as_ref()
                .map(|r| format!(" {r}"))
                .unwrap_or_default();
            lines.push(format!(
                "{prefix}{} {}{range}",
                theme().fg(ThemeColor::Muted, entry.label),
                theme().fg(ThemeColor::ToolTitle, &entry.detail)
            ));
        }
        lines.push(format!(
            "  {}",
            theme().fg(ThemeColor::Muted, &self.summary())
        ));
        if self.entries.len() > PREVIEW_ROWS {
            let hint = if self.expanded {
                "(ctrl+o to collapse)".to_owned()
            } else {
                format!("({start} more, ctrl+o to expand)")
            };
            lines.push(format!("  {}", theme().fg(ThemeColor::Muted, &hint)));
        }
        super::indent_lines(
            lines
                .into_iter()
                .map(|line| Line::from(truncate_to_width_opts(&line, inner, "", false)))
                .collect(),
            width,
        )
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

    /// Fails every call that never reported back.
    /// (`../notagent-main-rust/crates/notagent_main/src/interactive_mode.rs`,
    /// `pending_explore_tools.drain()` → `complete_call(&call_id, true)`):
    /// there the mode holds a call-id registry and completes each pending call
    /// as failed; here the block knows its incomplete entries directly. Calls
    /// that already completed keep their result, so a block whose calls all
    /// succeeded settles green even on an aborted turn.
    pub fn fail_running_calls(&mut self) {
        for entry in &mut self.entries {
            if !entry.complete {
                entry.complete = true;
                entry.failed = true;
            }
        }
    }

    /// A closed block holding only `call_id`, completed. A result that arrives
    /// after its block settled gets this row, because the settled block may
    /// already be in native scrollback, which cannot be edited.
    #[must_use]
    pub fn detached_result(&self, call_id: &str, failed: bool) -> Option<Self> {
        let index = *self.entry_by_call_id.get(call_id)?;
        let mut entry = self.entries[index].clone();
        entry.started = true;
        entry.complete = true;
        entry.failed = failed;
        let mut block = Self::new();
        block.entry_by_call_id.insert(entry.call_id.clone(), 0);
        block.entries.push(entry);
        block.expanded = self.expanded;
        block.close();
        Some(block)
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

    fn count_of(&self, kind: ExploreKind) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.kind == kind)
            .count()
    }

    fn summary(&self) -> String {
        let searches = self.count_of(ExploreKind::Search);
        let reads = self.count_of(ExploreKind::Read);
        let lists = self.count_of(ExploreKind::List);
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
        if lists > 0 {
            parts.push(format!(
                "{lists} listing{}",
                if lists == 1 { "" } else { "s" }
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

impl Default for ExploreBlockComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for ExploreBlockComponent {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<Line> {
        if block_style() == BlockStyle::Dot {
            return self.render_dot(width);
        }
        let outer_width = width;
        let width = super::tool_content_width(width);
        if self.entries.is_empty() || width == 0 {
            return Vec::new();
        }
        let theme_instance = theme();
        // Same single-space inset as the header and the summary: the entries
        // read as one column, not as a list hanging off it.
        let render_entry = |entry: &ExploreEntry| {
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
        // state moves into the Exploring/Explored badge, with the runtime
        // beside it (the reference's `BlockStyle::Bar` rendering).
        let badge_style = block_style().is_compact();
        let mut lines = if badge_style {
            // The label carries the phase: Exploring while calls still run,
            // Explored once the block is closed and every call settled.
            let label = if self.open || self.entries.iter().any(|entry| !entry.complete) {
                "Exploring"
            } else {
                "Explored"
            };
            let mut badge_line = badge(&theme_instance, self.background(), label);
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
                "Exploring..."
            } else {
                "Explored"
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
        super::indent_lines(shared_lines(rendered), outer_width)
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
    use std::sync::MutexGuard;

    use super::*;
    use crate::modes::interactive::theme::theme::{init_theme, set_block_style};
    use crate::utils::ansi::strip_ansi;

    fn theme_lock() -> MutexGuard<'static, ()> {
        let guard = crate::modes::interactive::theme::theme::test_lock();
        init_theme(None, false);
        // The block style is a process global (default: badge); the layout
        // tests pin the standard surface unless they say otherwise.
        set_block_style(BlockStyle::Standard);
        guard
    }

    fn rendered(component: &mut ExploreBlockComponent) -> Vec<String> {
        component
            .render(100)
            .into_iter()
            .map(|line| strip_ansi(&line).trim_end().to_string())
            .collect()
    }

    #[test]
    fn a_call_announced_before_its_arguments_stays_one_row() {
        let _guard = theme_lock();
        let mut block = ExploreBlockComponent::new();
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
    fn a_late_result_becomes_a_settled_block_of_its_own() {
        let _guard = theme_lock();
        let mut block = ExploreBlockComponent::new();
        block.push_call(
            "grep",
            "call-1".to_string(),
            &serde_json::json!({ "pattern": "a" }),
        );
        block.push_call(
            "grep",
            "call-2".to_string(),
            &serde_json::json!({ "pattern": "late" }),
        );
        block.fail_running_calls();
        block.close();
        let before = rendered(&mut block);

        let mut detached = block
            .detached_result("call-2", false)
            .expect("the block carries the call");
        assert_eq!(
            rendered(&mut block),
            before,
            "the settled block may be in scrollback and must not change"
        );
        assert!(!detached.is_running() && !detached.is_open());
        assert_eq!(detached.failed_count(), 0, "the late result succeeded");
        assert!(detached.has_call("call-2") && !detached.has_call("call-1"));
        assert!(
            rendered(&mut detached)
                .iter()
                .any(|line| line.contains("late"))
        );
        assert!(block.detached_result("call-9", false).is_none());
    }

    /// Every read-only tool is grouped, and nothing that changes the project.
    #[test]
    fn the_read_only_tools_and_only_those_are_grouped() {
        for name in ["grep", "find_filesystem", "find_codebase", "read", "ls"] {
            assert!(is_explore_tool(name), "{name}");
        }
        for name in [
            "bash",
            "todo_write",
            "write",
            "edit",
            "task",
            "skill",
            "read_minified",
        ] {
            assert!(!is_explore_tool(name), "{name}");
        }
        // The grouping is the read-only list itself, so the two never drift.
        for name in crate::core::tools::read_only_tool_names() {
            assert!(is_explore_tool(name.as_str()), "{}", name.as_str());
        }
    }

    #[test]
    fn a_listing_shows_its_path_and_is_counted_as_one() {
        let _guard = theme_lock();
        let mut block = ExploreBlockComponent::new();
        block.push_call(
            "ls",
            "ls-1".to_string(),
            &serde_json::json!({ "path": "crates/notagent" }),
        );
        block.push_call("ls", "ls-2".to_string(), &serde_json::json!({}));
        block.complete_call("ls-1", false);
        block.complete_call("ls-2", false);
        block.close();
        let actual = rendered(&mut block).join("\n");
        assert!(actual.contains("Listed crates/notagent"), "{actual}");
        // A call without a path lists the working directory.
        assert!(actual.contains("Listed ."), "{actual}");
        assert!(actual.contains("2 listings"), "{actual}");
    }

    #[test]
    fn a_read_shows_its_file_name_and_line_range() {
        let _guard = theme_lock();
        let mut block = ExploreBlockComponent::new();
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
        let mut whole = ExploreBlockComponent::new();
        whole.push_call(
            "read",
            "read-1".to_string(),
            &serde_json::json!({ "file_path": "README.md" }),
        );
        let whole = rendered(&mut whole).join("\n");
        assert!(whole.contains("Read README.md"), "{whole}");
        assert!(!whole.contains(" L"), "{whole}");

        let _guard2 = ();
        let mut open = ExploreBlockComponent::new();
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
        let mut block = ExploreBlockComponent::new();
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
        let mut block = ExploreBlockComponent::new();
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
            "  Exploring...",
            "",
            "  Searched files config",
            "  1 search",
            "",
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_closed_block_says_searched_and_counts_failures() {
        let _guard = theme_lock();
        let mut block = ExploreBlockComponent::new();
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
        assert!(actual.contains("Explored"), "{actual}");
        assert!(!actual.contains("Exploring..."), "{actual}");
        assert!(actual.contains("2 searches, 1 failed"), "{actual}");
    }

    #[test]
    fn the_collapsed_block_shows_the_last_four_rows() {
        let _guard = theme_lock();
        let mut block = ExploreBlockComponent::new();
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
            "  Exploring...",
            "",
            "  ...",
            "  Searched files pattern-1",
            "  Searched files pattern-2",
            "  Searched files pattern-3",
            "  Searched files pattern-4",
            "  (1 more, ctrl+o to expand)",
            "  5 searches",
            "",
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn the_expanded_block_shows_all_rows() {
        let _guard = theme_lock();
        let mut block = ExploreBlockComponent::new();
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
            actual.contains(&"  Searched text pattern-0".to_string()),
            "{actual:?}"
        );
        assert_eq!(
            actual.last().map(String::as_str),
            Some(""),
            "the surface ends with a padded blank row"
        );
        assert!(
            actual.contains(&"  (ctrl+o to collapse)".to_string()),
            "{actual:?}"
        );
    }

    #[test]
    fn the_background_carries_the_state() {
        let _guard = theme_lock();
        let mut block = ExploreBlockComponent::new();
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
        let mut block = ExploreBlockComponent::new();
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
            "  Explored  (0ms)",
            "  Searched code workspace lock",
            "  1 search, 1 failed",
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn the_badge_says_searching_while_calls_still_run() {
        let _guard = theme_lock();
        set_block_style(BlockStyle::Badge);
        let mut block = ExploreBlockComponent::new();
        block.push_call(
            "grep",
            "search-1".to_string(),
            &serde_json::json!({"pattern": "config"}),
        );
        let actual = rendered(&mut block).join("\n");
        assert!(actual.contains("Exploring"), "{actual}");
        assert!(!actual.contains("Explored "), "{actual}");
    }

    #[test]
    fn the_badge_style_info_line_closes_the_block_below_the_summary() {
        let _guard = theme_lock();
        set_block_style(BlockStyle::Badge);
        let mut block = ExploreBlockComponent::new();
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
            Some("  (1 more, ctrl+o to expand)"),
            "{collapsed:?}"
        );
        assert!(
            collapsed[collapsed.len() - 2].contains("5 searches"),
            "summary right above the info line: {collapsed:?}"
        );
    }

    /// An aborted turn fails the calls that never reported back (the
    /// reference's `pending_explore_tools.drain()`), so a cut-off call
    /// settles the closed block red and stops it claiming to still explore.
    #[test]
    fn an_aborted_block_with_a_cut_off_call_settles_red() {
        let _guard = theme_lock();
        set_block_style(BlockStyle::Badge);
        let mut block = ExploreBlockComponent::new();
        block.push_call(
            "grep",
            "call-1".to_string(),
            &serde_json::json!({ "pattern": "needle" }),
        );
        // The call never returned — the turn was cut off under it.
        assert!(block.is_running());
        block.fail_running_calls();
        block.close();

        assert!(!block.is_running(), "nothing will complete it now");
        assert_eq!(block.background(), ThemeBg::ToolErrorBg);
        let actual = rendered(&mut block).join("\n");
        assert!(actual.contains("Explored"), "{actual}");
        assert!(!actual.contains("Exploring"), "{actual}");
        assert!(actual.contains("1 failed"), "{actual}");
    }

    /// An abort between calls leaves completed results untouched: a block
    /// whose calls all succeeded settles green, exactly like the reference.
    #[test]
    fn an_abort_after_completed_calls_settles_green() {
        let _guard = theme_lock();
        let mut block = ExploreBlockComponent::new();
        block.push_call(
            "read",
            "call-1".to_string(),
            &serde_json::json!({ "file_path": "a.rs" }),
        );
        block.complete_call("call-1", false);
        block.fail_running_calls();
        block.close();

        assert_eq!(block.background(), ThemeBg::ToolSuccessBg);
    }

    #[test]
    fn a_running_block_asks_for_a_redraw_once_per_second_and_a_settled_one_never() {
        let mut block = ExploreBlockComponent::new();
        block.push_call(
            "grep",
            "call-1".to_string(),
            &serde_json::json!({ "pattern": "needle" }),
        );
        assert!(block.tick_runtime(), "the first second must be shown");
        assert!(
            !block.tick_runtime(),
            "the same second must not redraw the block again"
        );
        block.started -= std::time::Duration::from_secs(1);
        assert!(block.tick_runtime(), "a new second must redraw the block");

        block.complete_call("call-1", false);
        block.close();
        block.started -= std::time::Duration::from_secs(1);
        assert!(
            !block.tick_runtime(),
            "a settled block may be in scrollback and must not change"
        );
    }

    #[test]
    fn a_replayed_block_shows_no_runtime() {
        let _guard = theme_lock();
        set_block_style(BlockStyle::Badge);
        let mut block = ExploreBlockComponent::new();
        block.mark_replayed();
        block.push_call(
            "grep",
            "call-1".to_string(),
            &serde_json::json!({"pattern": "config"}),
        );
        block.complete_call("call-1", false);
        block.close();
        let actual = rendered(&mut block).join("\n");
        assert!(actual.contains("Explored"), "{actual}");
        assert!(!actual.contains("ms)"), "{actual}");
    }

    #[test]
    fn multiline_patterns_collapse_to_one_row() {
        let _guard = theme_lock();
        let mut block = ExploreBlockComponent::new();
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
