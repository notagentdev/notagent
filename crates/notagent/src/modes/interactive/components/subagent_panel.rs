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
//! So each running child gets a row: which mode it runs in, what it was asked,
//! how long it has been at it, and what it has spent. The parent sits above them
//! as the thing they were split off from.

use notagent_tui::tui::Component;
use notagent_tui::utils::{truncate_to_width_opts, visible_width};

use crate::core::tasks::types::{SubagentTaskInfo, TaskInfo, is_terminal_task_status};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::tasks_panel::{now_ms, single_line};

/// Most children shown at once, so a wide fan-out cannot take the screen.
const MAX_ROWS: usize = 6;

/// Shortest a task label may be squeezed to before the row gives up on it.
const MIN_LABEL_WIDTH: usize = 12;

fn as_running_subagent(info: &TaskInfo) -> Option<&SubagentTaskInfo> {
    match info {
        TaskInfo::Subagent(subagent) if !is_terminal_task_status(subagent.base.status) => {
            Some(subagent)
        }
        _ => None,
    }
}

/// Seconds, then minutes: a child that ran an hour is a different problem.
pub fn format_elapsed(started_at: i64, now: i64) -> String {
    let seconds = (((now - started_at) as f64) / 1000.0).round().max(0.0) as i64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {}s", seconds % 60);
    }
    format!("{}h {}m", minutes / 60, minutes % 60)
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
    /// The mode the child runs in.
    pub mode_id: String,
    /// What it was asked to do.
    pub label: String,
    /// How long it has been running.
    pub elapsed: String,
    /// What it has spent.
    pub tokens: String,
}

/// The rows for one snapshot, oldest child first so positions stay put.
pub fn build_subagent_rows(tasks: &[TaskInfo], now: i64) -> Vec<SubagentRow> {
    let mut running: Vec<&SubagentTaskInfo> =
        tasks.iter().filter_map(as_running_subagent).collect();
    running.sort_by_key(|info| info.base.started_at);
    running
        .into_iter()
        .map(|info| SubagentRow {
            mode_id: info.mode_id.clone(),
            label: single_line(&info.base.description),
            elapsed: format_elapsed(info.base.started_at, now),
            tokens: format_tokens(info.tokens),
        })
        .collect()
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

    fn render(&mut self, width: usize) -> Vec<String> {
        let theme_instance = theme();
        let rows = build_subagent_rows(&self.tasks, now_ms());
        if rows.is_empty() {
            return Vec::new();
        }

        let shown = &rows[..rows.len().min(MAX_ROWS)];
        // `Math.max(...shown.map((row) => row.modeId.length))` — JavaScript
        // counts UTF-16 units; the port counts characters, which agrees for
        // every mode id and keeps the column aligned for the rest.
        let mode_width = shown
            .iter()
            .map(|row| row.mode_id.chars().count())
            .max()
            .unwrap_or(0);
        let mut lines: Vec<String> = vec![format!(
            "{} {}",
            theme_instance.fg(ThemeColor::Muted, "●"),
            theme_instance.bold("main")
        )];

        for row in shown {
            let right = format!("{} · ↓ {} tokens", row.elapsed, row.tokens);
            let prefix = format!(
                "  {} {}  ",
                theme_instance.fg(ThemeColor::Success, "○"),
                theme_instance.fg(
                    ThemeColor::Accent,
                    &format!(
                        "{}{}",
                        row.mode_id,
                        " ".repeat(mode_width.saturating_sub(row.mode_id.chars().count()))
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
        lines
    }
}
