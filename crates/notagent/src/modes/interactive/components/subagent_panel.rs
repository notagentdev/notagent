//! The subagents, below the footer.
//!
//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/subagent-panel.ts` (111 LOC).
//!
//! A delegated run is the one thing the agent does that is otherwise invisible:
//! it produces no output until it answers, it can run for minutes, and it is
//! spending money the whole time. The transcript shows that a delegation was
//! started and then nothing until it returns.
//!
//! So each running child gets a row: its name, what it may do, what it was
//! asked, how long it has been at it, and what it has spent. The parent sits
//! above them as the thing they were split off from. A finished child keeps
//! its row for a few seconds so its outcome — the marker turning green or red
//! — registers before the row leaves.

use notagent_tui::tui::{Component, Line, shared_lines};
use notagent_tui::utils::{truncate_to_width_opts, visible_width};

use crate::core::modes::shells::ShellId;
use crate::core::tasks::types::{SubagentTaskInfo, TaskInfo, TaskStatus, is_terminal_task_status};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::tasks_panel::{now_ms, single_line};

/// Most children shown at once, so a wide fan-out cannot take the screen.
const MAX_ROWS: usize = 6;

/// Shortest a task label may be squeezed to before the row gives up on it.
const MIN_LABEL_WIDTH: usize = 12;

/// How long a finished child keeps its row: long enough to register how it
/// ended, short enough that the list keeps showing what is actually running.
const LINGER_MS: i64 = 5_000;

/// Whether the panel lists a subagent at `now`: while it runs, and for
/// [`LINGER_MS`] after it settled so its outcome is seen before the row leaves.
pub fn is_listed_subagent(info: &TaskInfo, now: i64) -> bool {
    match info {
        TaskInfo::Subagent(subagent) => {
            !is_terminal_task_status(subagent.base.status)
                || subagent
                    .base
                    .ended_at
                    .is_some_and(|ended| now.saturating_sub(ended) < LINGER_MS)
        }
        _ => false,
    }
}

fn as_listed_subagent(info: &TaskInfo, now: i64) -> Option<&SubagentTaskInfo> {
    match info {
        TaskInfo::Subagent(subagent) if is_listed_subagent(info, now) => Some(subagent),
        _ => None,
    }
}

/// The running time of a child, between two millisecond stamps.
pub fn format_elapsed(started_at: i64, now: i64) -> String {
    let millis = (now - started_at).max(0) as u64;
    crate::modes::interactive::theme::theme::format_elapsed(std::time::Duration::from_millis(
        millis,
    ))
}

/// Thousands as `17.9k`.
///
/// The decimal is kept well past ten thousand because that is the band a
/// subagent actually lives in, and `17.9k` versus `18.2k` is the difference a
/// reader is looking for. Past a hundred thousand the tenth stops carrying
/// anything and the number gets shorter instead.
pub fn format_tokens(tokens: u64) -> String {
    if tokens < 1000 {
        return tokens.to_string();
    }
    if tokens < 100_000 {
        // `toFixed(1)`; both languages round the same binary double.
        return format!("{:.1}k", tokens as f64 / 1000.0);
    }
    if tokens < 1_000_000 {
        return format!("{}k", (tokens as f64 / 1000.0).round() as u64);
    }
    format!("{:.1}M", tokens as f64 / 1_000_000.0)
}

/// `SubagentRow`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentRow {
    /// The star name the child is shown under. A name distinguishes two
    /// children of the same type, which is the case this panel exists for.
    pub alias: String,
    /// What it may do. Carried so the read-only restriction can be spelled out
    /// after the name.
    pub shell: Option<ShellId>,
    /// Where the child stands, carried by the marker's colour.
    pub status: TaskStatus,
    /// What it was asked to do.
    pub label: String,
    /// How long it has been running, frozen once it ended.
    pub elapsed: String,
    /// What it has spent.
    pub tokens: String,
}

/// The rows for one snapshot, oldest child first so positions stay put.
pub fn build_subagent_rows(tasks: &[TaskInfo], now: i64) -> Vec<SubagentRow> {
    let mut listed: Vec<&SubagentTaskInfo> = tasks
        .iter()
        .filter_map(|info| as_listed_subagent(info, now))
        .collect();
    listed.sort_by_key(|info| info.base.started_at);
    listed
        .into_iter()
        .map(|info| SubagentRow {
            alias: info.alias.clone(),
            shell: ShellId::parse(&info.agent),
            status: info.base.status,
            label: single_line(&info.base.description),
            elapsed: format_elapsed(info.base.started_at, info.base.ended_at.unwrap_or(now)),
            tokens: format_tokens(info.tokens),
        })
        .collect()
}

/// The name as the row shows it: the star, and the restriction when there is
/// one.
///
/// A read-only child says so; a worker shows only its name. Same rule the footer
/// applies to modes (`format_mode_label`) and for the same reason — the
/// restriction is the part worth a column, and "worker" is what a subagent is
/// unless told otherwise.
fn row_name(row: &SubagentRow) -> String {
    match row.shell {
        Some(ShellId::ReadOnly) => format!("{} (read-only)", row.alias),
        _ => row.alias.clone(),
    }
}

/// Colour of the row's marker, carrying the child's state: grey while it runs,
/// green once it completed cleanly, red for everything that ended badly
/// (failed, timed out, killed, lost). The same split the tasks panel uses, so
/// a reader who has learned it once has learned it everywhere.
fn marker_colour(status: TaskStatus) -> ThemeColor {
    match status {
        TaskStatus::Running => ThemeColor::Dim,
        TaskStatus::Completed => ThemeColor::Success,
        _ => ThemeColor::Error,
    }
}

/// The panel itself.
#[derive(Default)]
pub struct SubagentPanel {
    tasks: Vec<TaskInfo>,
}

impl SubagentPanel {
    /// New panel with nothing to show.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the panel.
    pub fn set_tasks(&mut self, tasks: Vec<TaskInfo>) {
        self.tasks = tasks;
    }
}

impl Component for SubagentPanel {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<Line> {
        let theme_instance = theme();
        let rows = build_subagent_rows(&self.tasks, now_ms());
        if rows.is_empty() {
            return Vec::new();
        }

        let shown = &rows[..rows.len().min(MAX_ROWS)];
        // `Math.max(...shown.map((row) => row.name.length))` — JavaScript counts
        // UTF-16 units; the port counts characters, which agrees for every star
        // name and keeps the column aligned for the rest.
        let name_width = shown
            .iter()
            .map(|row| row_name(row).chars().count())
            .max()
            .unwrap_or(0);
        let mut lines: Vec<String> = vec![format!(
            "{} {}",
            theme_instance.fg(ThemeColor::Muted, "●"),
            theme_instance.bold("main")
        )];

        for row in shown {
            let right = format!("{} · ↓ {} tokens", row.elapsed, row.tokens);
            let name = row_name(row);
            let prefix = format!(
                "  {} {}  ",
                theme_instance.fg(marker_colour(row.status), "○"),
                theme_instance.fg(
                    ThemeColor::Accent,
                    &format!(
                        "{name}{}",
                        " ".repeat(name_width.saturating_sub(name.chars().count()))
                    )
                )
            );
            // The label yields to the numbers: how long and how much are the
            // reason to look at the row at all, and both are short. Measured
            // rather than counted, so editing the prefix cannot silently push
            // the row past the terminal edge.
            let label_width = width
                .saturating_sub(visible_width(&prefix))
                .saturating_sub(2)
                .saturating_sub(visible_width(&right))
                .max(MIN_LABEL_WIDTH);
            let label = truncate_to_width_opts(&row.label, label_width, "…", false);
            let padded = format!(
                "{label}{}",
                " ".repeat(label_width.saturating_sub(visible_width(&label)))
            );
            lines.push(truncate_to_width_opts(
                &format!(
                    "{prefix}{}  {}",
                    theme_instance.fg(ThemeColor::Text, &padded),
                    theme_instance.fg(ThemeColor::Muted, &right)
                ),
                width,
                "…",
                false,
            ));
        }

        let hidden = rows.len() - shown.len();
        if hidden > 0 {
            let more = format!("↓ {hidden} more");
            let pad = width.saturating_sub(visible_width(&more));
            lines.push(" ".repeat(pad) + &theme_instance.fg(ThemeColor::Muted, &more));
        }
        shared_lines(lines)
    }
}
