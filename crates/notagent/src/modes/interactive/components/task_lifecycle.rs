//! The transcript lines that report what background work did.
//!
//! A panel shows what is running; it cannot show what happened, because the
//! moment a task settles it drops off the list. So the outcome goes into the
//! transcript, as its own line — never as a correction to the block that
//! launched the work, which by then may sit in rows the terminal has scrolled
//! away and cannot repaint.
//!
//! What gets a line differs by kind, and the difference is deliberate. A
//! subagent gets two, because nothing else in the transcript marks that it
//! started: the `task` call returns immediately and its answer arrives much
//! later. A detached shell command gets one, because the tool block that
//! launched it already recorded the launch — what that block cannot say is how
//! it ended.
//!
//! Only work this session watched running is reported. A task that was already
//! over when the registry was first read belongs to a previous session, and
//! announcing it now would tell the user about something they did not start.

use std::collections::HashSet;

use crate::core::tasks::types::{
    ShellTaskInfo, SubagentTaskInfo, TaskInfo, TaskStatus, is_terminal_task_status,
};
use crate::modes::interactive::components::subagent_panel::format_elapsed;
use crate::modes::interactive::components::tasks_panel::{now_ms, single_line};
use crate::modes::interactive::theme::theme::{Theme, ThemeBg, ThemeColor, badge};

/// Keeps track of what has already been said, so nothing is said twice.
#[derive(Default)]
pub struct TaskAnnouncer {
    /// Tasks seen running. A task absent from this set has no outcome to
    /// report, whatever its status says.
    watched: HashSet<String>,
    reported: HashSet<String>,
}

impl TaskAnnouncer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The lines this snapshot is due, in the order they should be appended.
    ///
    /// Takes the whole snapshot rather than a single task because a settled
    /// task is only visible in a listing that includes settled work — the
    /// caller has to pass one, and passing one task at a time would hide that
    /// requirement.
    pub fn observe(&mut self, tasks: &[TaskInfo], theme: &Theme, badge_style: bool) -> Vec<String> {
        let now = now_ms();
        let mut lines = Vec::new();
        for task in tasks {
            match task {
                TaskInfo::Subagent(info) => {
                    self.observe_subagent(info, theme, badge_style, now, &mut lines)
                }
                TaskInfo::Shell(info) => {
                    self.observe_shell(info, theme, badge_style, now, &mut lines)
                }
            }
        }
        lines
    }

    fn observe_subagent(
        &mut self,
        info: &SubagentTaskInfo,
        theme: &Theme,
        badge_style: bool,
        now: i64,
        lines: &mut Vec<String>,
    ) {
        let base = &info.base;
        if !is_terminal_task_status(base.status) {
            if self.watched.insert(base.task_id.clone()) {
                lines.push(subagent_started_line(info, theme, badge_style));
            }
            return;
        }
        if !self.watched.contains(&base.task_id) || !self.reported.insert(base.task_id.clone()) {
            return;
        }
        lines.push(subagent_ended_line(info, theme, badge_style, now));
    }

    fn observe_shell(
        &mut self,
        info: &ShellTaskInfo,
        theme: &Theme,
        badge_style: bool,
        now: i64,
        lines: &mut Vec<String>,
    ) {
        let base = &info.base;
        // A foreground command hands its result straight back into the block
        // that ran it, so there is nothing left to report.
        if base.detached != Some(true) {
            return;
        }
        if !is_terminal_task_status(base.status) {
            self.watched.insert(base.task_id.clone());
            return;
        }
        if !self.watched.contains(&base.task_id) || !self.reported.insert(base.task_id.clone()) {
            return;
        }
        lines.push(shell_ended_line(info, theme, badge_style, now));
    }
}

fn subagent_started_line(info: &SubagentTaskInfo, theme: &Theme, badge_style: bool) -> String {
    let detail = single_line(&info.base.description);
    if badge_style {
        return format!(
            " {}  {} {}",
            badge(
                theme,
                ThemeBg::ToolPendingBg,
                &format!("subagent {}", info.alias)
            ),
            theme.fg(ThemeColor::Text, "started"),
            theme.fg(ThemeColor::Dim, &detail),
        );
    }
    format!(
        " {} {} {}  {}",
        theme.fg(ThemeColor::Dim, "○"),
        theme.fg(ThemeColor::Text, "subagent started"),
        theme.fg(ThemeColor::Text, &info.alias),
        theme.fg(ThemeColor::Dim, &detail),
    )
}

fn subagent_ended_line(
    info: &SubagentTaskInfo,
    theme: &Theme,
    badge_style: bool,
    now: i64,
) -> String {
    let base = &info.base;
    let clean = base.status == TaskStatus::Completed;
    let elapsed = format_elapsed(base.started_at, base.ended_at.unwrap_or(now));
    if badge_style {
        return format!(
            " {}  {} {}",
            badge(
                theme,
                if clean {
                    ThemeBg::ToolSuccessBg
                } else {
                    ThemeBg::ToolErrorBg
                },
                &format!("subagent {}", info.alias)
            ),
            theme.fg(ThemeColor::Text, if clean { "done" } else { "failed" }),
            theme.fg(ThemeColor::Dim, &elapsed),
        );
    }
    let (marker, label) = if clean {
        (ThemeColor::Success, "subagent done")
    } else {
        (ThemeColor::Error, "subagent failed")
    };
    format!(
        " {} {} {}  {}",
        theme.fg(marker, "○"),
        theme.fg(ThemeColor::Text, label),
        theme.fg(ThemeColor::Text, &info.alias),
        theme.fg(ThemeColor::Dim, &elapsed),
    )
}

fn shell_ended_line(info: &ShellTaskInfo, theme: &Theme, badge_style: bool, now: i64) -> String {
    let base = &info.base;
    let clean = base.status == TaskStatus::Completed;
    let elapsed = format_elapsed(base.started_at, base.ended_at.unwrap_or(now));
    // The command is what the user recognises the job by — the task id is for
    // `task_output`, not for reading. A non-zero exit is the one extra fact
    // worth the space beside it.
    let command = single_line(&info.command);
    let detail = match info.exit_code {
        Some(code) if code != 0 => format!("{command} · exit {code} · {elapsed}"),
        _ => format!("{command} · {elapsed}"),
    };
    if badge_style {
        return format!(
            " {}  {} {}",
            badge(
                theme,
                if clean {
                    ThemeBg::ToolSuccessBg
                } else {
                    ThemeBg::ToolErrorBg
                },
                &format!("background {}", base.task_id)
            ),
            theme.fg(ThemeColor::Text, if clean { "done" } else { "failed" }),
            theme.fg(ThemeColor::Dim, &detail),
        );
    }
    let (marker, label) = if clean {
        (ThemeColor::Success, "background done")
    } else {
        (ThemeColor::Error, "background failed")
    };
    format!(
        " {} {} {}  {}",
        theme.fg(marker, "○"),
        theme.fg(ThemeColor::Text, label),
        theme.fg(ThemeColor::Text, &base.task_id),
        theme.fg(ThemeColor::Dim, &detail),
    )
}
