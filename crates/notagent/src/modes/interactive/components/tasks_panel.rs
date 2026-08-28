use notagent_tui::tui::{Component, Line, shared_lines};
use notagent_tui::utils::{truncate_to_width_opts, visible_width};

use crate::core::tasks::types::{TaskInfo, TaskStatus, is_terminal_task_status};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

/// What the panel shows: only running work, everything, or nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TasksPanelScope {
    /// Unfinished work only.
    #[default]
    Running,
    /// Everything the session knows about.
    All,
    /// Nothing at all.
    Hidden,
}

/// Most rows the panel will ever occupy, so a fan-out cannot eat the screen.
const MAX_ROWS: usize = 8;

/// Left indent of every panel line, matching the reference roster's gutter.
const INDENT: &str = " ";

/// Smallest gap between a row's label and the elapsed figure on its right.
const MIN_GAP: usize = 2;

/// The `○` in front of a row carries the state: grey while running, green once
/// completed cleanly, red for everything that ended badly (failed, timed out,
/// killed, lost). User decision 2026-08-16 (v0.1.5); the reference roster
/// coloured running work green instead.
fn status_colour(status: TaskStatus) -> ThemeColor {
    match status {
        TaskStatus::Running => ThemeColor::Dim,
        TaskStatus::Completed => ThemeColor::Success,
        _ => ThemeColor::Error,
    }
}

/// `text.replace(/\s+/g, " ").trim()`
pub(crate) fn single_line(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_whitespace = false;
    for character in text.chars() {
        if character.is_whitespace() {
            in_whitespace = true;
            continue;
        }
        if in_whitespace && !result.is_empty() {
            result.push(' ');
        }
        in_whitespace = false;
        result.push(character);
    }
    result
}

/// Milliseconds since the epoch, as `Date.now()`.
pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// How long a task has run, or ran.
fn elapsed(info: &TaskInfo, now: i64) -> String {
    let base = info.base();
    crate::modes::interactive::components::subagent_panel::format_elapsed(
        base.started_at,
        base.ended_at.unwrap_or(now),
    )
}

/// The panel itself.
#[derive(Default)]
pub struct TasksPanel {
    scope: TasksPanelScope,
    tasks: Vec<TaskInfo>,
}

impl TasksPanel {
    /// New panel; starts in the `running` scope with nothing to show.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the panel. Called whenever a task starts, settles or is polled.
    pub fn set_tasks(&mut self, tasks: Vec<TaskInfo>) {
        self.tasks = tasks;
    }

    /// The current scope.
    pub fn get_scope(&self) -> TasksPanelScope {
        self.scope
    }

    /// Replace the scope.
    pub fn set_scope(&mut self, scope: TasksPanelScope) {
        self.scope = scope;
    }

    /// Steps through running → all → hidden.
    /// Hidden is part of the ring rather than a separate key: someone who does
    /// not want the panel wants one way to get rid of it, not two keys to learn.
    pub fn cycle_scope(&mut self) -> TasksPanelScope {
        self.scope = match self.scope {
            TasksPanelScope::Running => TasksPanelScope::All,
            TasksPanelScope::All => TasksPanelScope::Hidden,
            TasksPanelScope::Hidden => TasksPanelScope::Running,
        };
        self.scope
    }

    fn visible(&self) -> Vec<&TaskInfo> {
        if self.scope == TasksPanelScope::Hidden {
            return Vec::new();
        }
        let mut shown: Vec<&TaskInfo> = if self.scope == TasksPanelScope::All {
            self.tasks.iter().collect()
        } else {
            self.tasks
                .iter()
                .filter(|info| !is_terminal_task_status(info.base().status))
                .collect()
        };
        // Running first and oldest first within that, so a long-running task keeps
        // its place instead of being pushed around by newer, shorter ones.
        shown.sort_by(|a, b| {
            let a_terminal = is_terminal_task_status(a.base().status);
            let b_terminal = is_terminal_task_status(b.base().status);
            if a_terminal != b_terminal {
                return if a_terminal {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                };
            }
            if a_terminal {
                b.base()
                    .ended_at
                    .unwrap_or(0)
                    .cmp(&a.base().ended_at.unwrap_or(0))
            } else {
                a.base().started_at.cmp(&b.base().started_at)
            }
        });
        shown
    }
}

impl Component for TasksPanel {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<Line> {
        let theme_instance = theme();
        let visible = self.visible();
        if visible.is_empty() {
            return Vec::new();
        }

        let now = now_ms();
        // One row per task, roster style: marker and id on the left, the
        // dimmed label giving way in the middle, the elapsed time flush right.
        let mut rows: Vec<String> = visible
            .iter()
            .take(MAX_ROWS)
            .map(|info| {
                let base = info.base();
                let marker = theme_instance.fg(status_colour(base.status), "○");
                let figures = theme_instance.fg(ThemeColor::Dim, &elapsed(info, now));
                let figures_width = visible_width(&figures);
                let id = theme_instance.fg(ThemeColor::Text, &base.task_id);
                let head = format!("{INDENT}{marker} {id}");
                let head_width = visible_width(&head);

                let label_budget = width
                    .saturating_sub(head_width)
                    .saturating_sub(figures_width)
                    .saturating_sub(MIN_GAP + 1);
                let label = single_line(&base.description);
                let label = if label.is_empty() || label_budget == 0 {
                    String::new()
                } else {
                    format!(
                        " {}",
                        theme_instance.fg(
                            ThemeColor::Dim,
                            &truncate_to_width_opts(&label, label_budget, "…", false)
                        )
                    )
                };

                let left = format!("{head}{label}");
                let gap = width
                    .saturating_sub(visible_width(&left))
                    .saturating_sub(figures_width)
                    .max(MIN_GAP);
                format!("{left}{}{figures}", " ".repeat(gap))
            })
            .collect();

        let hidden = visible.len() - rows.len();
        if hidden > 0 {
            let text = theme_instance.fg(ThemeColor::Dim, &format!("… and {hidden} more"));
            let pad = width.saturating_sub(visible_width(&text));
            rows.push(format!("{}{text}", " ".repeat(pad)));
        }

        let running = self
            .tasks
            .iter()
            .filter(|info| !is_terminal_task_status(info.base().status))
            .count();
        let heading = format!(
            "{INDENT}{} {}",
            theme_instance.fg(ThemeColor::Accent, "●"),
            theme_instance.fg(
                ThemeColor::Text,
                &if self.scope == TasksPanelScope::All {
                    format!("background tasks ({running} running)")
                } else {
                    format!("background tasks ({running})")
                }
            ),
        );
        // The blank line separates the panel from the footer above it.
        let mut lines = vec![
            String::new(),
            truncate_to_width_opts(&heading, width, "…", false),
        ];
        lines.extend(rows);
        shared_lines(lines)
    }
}
