//! The panel that makes background work visible.
//!
//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/tasks-panel.ts` (106 LOC).
//!
//! A detached process the user cannot see is worse than one they had to wait
//! for: it is still spending their machine, still holding their port, and the
//! only evidence it exists is a line of tool output that scrolled away. So a
//! running task is on screen for as long as it runs, above the editor, where
//! the eye already is.
//!
//! It renders nothing at all when there is nothing to show. A panel that
//! occupied a row to say "no tasks" would cost every user a line of terminal
//! for a fact almost all of them do not need.

use notagent_tui::tui::Component;
use notagent_tui::utils::truncate_to_width_opts;

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

fn status_colour(status: TaskStatus) -> ThemeColor {
    match status {
        TaskStatus::Running => ThemeColor::Success,
        TaskStatus::Completed => ThemeColor::Muted,
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

fn elapsed(info: &TaskInfo, now: i64) -> String {
    let base = info.base();
    let seconds = (((base.ended_at.unwrap_or(now) - base.started_at) as f64) / 1000.0)
        .round()
        .max(0.0) as i64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    format!("{}h", minutes / 60)
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
    ///
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

    fn render(&mut self, width: usize) -> Vec<String> {
        let theme_instance = theme();
        let visible = self.visible();
        if visible.is_empty() {
            return Vec::new();
        }

        let now = now_ms();
        let mut rows: Vec<String> = visible
            .iter()
            .take(MAX_ROWS)
            .map(|info| {
                let base = info.base();
                let marker = theme_instance.fg(
                    status_colour(base.status),
                    if base.status == TaskStatus::Running {
                        "●"
                    } else {
                        "○"
                    },
                );
                let id = theme_instance.fg(
                    if matches!(info, TaskInfo::Subagent(_)) {
                        ThemeColor::Accent
                    } else {
                        ThemeColor::ToolTitle
                    },
                    &format!("{:<14}", base.task_id),
                );
                let status = theme_instance.fg(
                    status_colour(base.status),
                    &format!("{:<10}", base.status.as_str()),
                );
                let time =
                    theme_instance.fg(ThemeColor::Muted, &format!("{:>4}", elapsed(info, now)));
                let label = truncate_to_width_opts(
                    &single_line(&base.description),
                    width.saturating_sub(36).max(8),
                    "…",
                    false,
                );
                truncate_to_width_opts(
                    &format!(
                        "{marker} {id} {status} {time}  {}",
                        theme_instance.fg(ThemeColor::Text, &label)
                    ),
                    width,
                    "…",
                    false,
                )
            })
            .collect();

        let hidden = visible.len() - rows.len();
        if hidden > 0 {
            rows.push(theme_instance.fg(ThemeColor::Muted, &format!("  … and {hidden} more")));
        }

        let running = self
            .tasks
            .iter()
            .filter(|info| !is_terminal_task_status(info.base().status))
            .count();
        let heading = theme_instance.fg(
            ThemeColor::Muted,
            &if self.scope == TasksPanelScope::All {
                format!("background tasks ({running} running)")
            } else {
                format!("background tasks ({running})")
            },
        );
        let mut lines = vec![truncate_to_width_opts(&heading, width, "…", false)];
        lines.extend(rows);
        lines
    }
}
