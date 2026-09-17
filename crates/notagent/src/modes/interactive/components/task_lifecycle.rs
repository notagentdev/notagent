//! Immutable transcript lines for background work.
//!
//! A running task belongs in the live panels. The transcript only records the
//! two facts that stay true: it started, and later how it ended. Both facts are
//! appended as separate lines so rendering an outcome never reaches back into
//! terminal scrollback.

use super::status_marker::{MarkerState, heading};
use crate::core::tasks::lifecycle::{TaskLifecyclePhase, TaskLifecycleRecord};
use crate::core::tasks::types::{TaskInfo, TaskStatus};
use crate::modes::interactive::components::subagent_panel::{format_elapsed, format_tokens};
use crate::modes::interactive::components::tasks_panel::single_line;
use crate::modes::interactive::theme::theme::{
    BlockStyle, Theme, ThemeBg, ThemeColor, badge, block_style, theme,
};
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, Line};

pub struct TaskLifecycleComponent {
    record: TaskLifecycleRecord,
}

impl TaskLifecycleComponent {
    pub fn new(record: TaskLifecycleRecord) -> Self {
        Self { record }
    }
}

impl Component for TaskLifecycleComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        // Use the tool boxes' outer inset, including continuation lines.
        let style = block_style();
        let line = task_lifecycle_line(&self.record, &theme(), style);
        let inner = super::tool_content_width(width);
        let lines = if style == BlockStyle::Dot {
            // Wrap the text column separately so the marker gutter is counted once.
            let (marker, body) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            Text::new(body, 0, 0)
                .render(inner.saturating_sub(2))
                .into_iter()
                .enumerate()
                .map(|(index, text)| {
                    Line::from(if index == 0 {
                        format!("{marker} {text}")
                    } else {
                        format!("  {text}")
                    })
                })
                .collect()
        } else {
            Text::new(line, 0, 0).render(inner)
        };
        super::indent_lines(lines, width)
    }

    fn invalidate(&mut self) {}
}

pub fn is_background_bash_call(tool_name: &str, args: &serde_json::Value) -> bool {
    tool_name == "bash"
        && args
            .get("run_in_background")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}

pub fn task_lifecycle_line(
    record: &TaskLifecycleRecord,
    theme: &Theme,
    style: BlockStyle,
) -> String {
    let clean = record.task.status() == TaskStatus::Completed;
    // The badge carries where a subagent runs: BG- for a detached child, bare
    // for one the turn is waiting on — the two behave differently, so their
    // lines must be tellable apart at a glance. Shells only ever get lines as
    // background work.
    let (badge_name, identity, work) = match &record.task {
        TaskInfo::Subagent(info) => (
            if info.base.detached == Some(true) {
                "BG-Subagent"
            } else {
                "Subagent"
            },
            format!("{} · {}", info.alias, info.base.task_id),
            single_line(&info.base.description),
        ),
        TaskInfo::Shell(info) => (
            "BG-Bash",
            info.base.task_id.clone(),
            single_line(&info.command),
        ),
    };
    let detail = match record.phase {
        TaskLifecyclePhase::Started => format!("{identity} started · {work}"),
        TaskLifecyclePhase::Ended => {
            let base = record.task.base();
            let elapsed = format_elapsed(base.started_at, base.ended_at.unwrap_or(base.started_at));
            let outcome = if clean { "done" } else { "failed" };
            let extra = match &record.task {
                TaskInfo::Shell(info) => info
                    .exit_code
                    .filter(|code| *code != 0)
                    .map(|code| format!(" · exit {code}"))
                    .unwrap_or_default(),
                // What it spent, in the panel's own spelling, because the
                // panel row is gone five seconds after the child settles.
                TaskInfo::Subagent(info) => format!(" · ↓ {} tokens", format_tokens(info.tokens)),
            };
            format!("{identity} {outcome} · {elapsed}{extra}")
        }
    };

    if style == BlockStyle::Dot {
        let state = if record.phase == TaskLifecyclePhase::Started || clean {
            MarkerState::Success
        } else {
            MarkerState::Error
        };
        return format!(
            "{}  {}",
            heading(badge_name, state),
            theme.fg(ThemeColor::CustomMessageText, &detail)
        );
    }
    if style == BlockStyle::Badge {
        return format!(
            "{}  {}",
            badge(theme, ThemeBg::CustomMessageBg, badge_name),
            theme.fg(ThemeColor::CustomMessageText, &detail)
        );
    }

    format!(
        "{}  {}",
        theme.fg(
            ThemeColor::CustomMessageLabel,
            &format!("\x1b[1m[{badge_name}]\x1b[22m")
        ),
        theme.fg(ThemeColor::CustomMessageText, &detail),
    )
}
