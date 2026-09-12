//! Immutable transcript lines for background work.
//!
//! A running task belongs in the live panels. The transcript only records the
//! two facts that stay true: it started, and later how it ended. Both facts are
//! appended as separate lines so rendering an outcome never reaches back into
//! terminal scrollback.

use crate::core::tasks::lifecycle::{TaskLifecyclePhase, TaskLifecycleRecord};
use crate::core::tasks::types::{TaskInfo, TaskStatus};
use crate::modes::interactive::components::subagent_panel::{format_elapsed, format_tokens};
use crate::modes::interactive::components::tasks_panel::single_line;
use crate::modes::interactive::theme::theme::{Theme, ThemeBg, ThemeColor, badge};
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, Line};

pub struct TaskLifecycleComponent {
    text: Text,
}

impl TaskLifecycleComponent {
    pub fn new(line: String) -> Self {
        Self {
            text: Text::new(line, 0, 0),
        }
    }
}

impl Component for TaskLifecycleComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        // Use the tool boxes' outer inset, including continuation lines.
        super::indent_lines(self.text.render(width.saturating_sub(1)), width)
    }

    fn invalidate(&mut self) {
        self.text.invalidate();
    }
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
    badge_style: bool,
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

    if badge_style {
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
