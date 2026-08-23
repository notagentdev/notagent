//! Port of `packages/coding-agent/test/subagent-panel.test.ts` (165 LOC).
//!
//! The subagent rows below the footer.
//!
//! A delegated run produces nothing until it answers, so these two numbers —
//! how long it has been going and what it has spent — are the only evidence it
//! is alive. The cases below pin that they appear, that finished children
//! linger briefly with their outcome and then leave, and that the rows do not
//! reshuffle underneath the eye.

use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::tasks::types::{
    ShellTaskInfo, SubagentTaskInfo, TaskInfo, TaskInfoBase, TaskStatus,
};
use notagent::modes::interactive::components::subagent_panel::{
    SubagentPanel, build_subagent_rows, format_elapsed, format_tokens,
};
use notagent::modes::interactive::theme::theme::{ThemeColor, init_theme, theme};
use notagent::utils::ansi::strip_ansi;
use notagent_tui::tui::Component;

/// The global theme is a process global.
fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    init_theme(None, false);
    guard
}

struct SubagentOverrides {
    status: TaskStatus,
    started_at: i64,
    ended_at: Option<i64>,
    alias: String,
    agent: String,
    tokens: u64,
}

impl Default for SubagentOverrides {
    fn default() -> Self {
        Self {
            status: TaskStatus::Running,
            started_at: 0,
            ended_at: None,
            alias: "Vega".to_string(),
            agent: "worker".to_string(),
            tokens: 0,
        }
    }
}

fn subagent(task_id: &str, description: &str, overrides: SubagentOverrides) -> TaskInfo {
    TaskInfo::Subagent(SubagentTaskInfo {
        base: TaskInfoBase {
            task_id: task_id.to_string(),
            description: description.to_string(),
            status: overrides.status,
            detached: Some(false),
            started_at: overrides.started_at,
            ended_at: overrides.ended_at,
            stop_reason: None,
            notification_suppressed: None,
            timeout_ms: None,
        },
        tokens: overrides.tokens,
        session_id: format!("session-{task_id}"),
        agent: overrides.agent,
        alias: overrides.alias,
    })
}

fn shell(task_id: &str) -> TaskInfo {
    TaskInfo::Shell(ShellTaskInfo {
        base: TaskInfoBase {
            task_id: task_id.to_string(),
            description: "a command".to_string(),
            status: TaskStatus::Running,
            detached: Some(true),
            started_at: 0,
            ended_at: None,
            stop_reason: None,
            notification_suppressed: None,
            timeout_ms: None,
        },
        command: "sleep 1".to_string(),
        pid: 1,
        exit_code: None,
    })
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// --- elapsed time -----------------------------------------------------------------

#[test]
fn counts_seconds_under_a_minute() {
    assert_eq!(format_elapsed(0, 12_000), "12s");
}

#[test]
fn switches_to_minutes_past_one() {
    assert_eq!(format_elapsed(0, 95_000), "1m 35s");
}

#[test]
fn switches_to_hours_past_sixty_minutes() {
    assert_eq!(format_elapsed(0, 3_900_000), "1h 5m");
}

// --- token counts ------------------------------------------------------------------

#[test]
fn prints_small_numbers_exactly() {
    assert_eq!(format_tokens(842), "842");
}

#[test]
fn prints_thousands_with_one_decimal_because_the_digit_stops_mattering() {
    assert_eq!(format_tokens(17_900), "17.9k");
}

#[test]
fn drops_the_decimal_once_past_ten_thousand() {
    assert_eq!(format_tokens(126_400), "126k");
}

#[test]
fn switches_to_millions() {
    assert_eq!(format_tokens(2_300_000), "2.3M");
}

// --- which rows there are ------------------------------------------------------------

#[test]
fn lists_running_subagents() {
    let rows = build_subagent_rows(
        &[subagent(
            "agent-1",
            "Inspect the parser",
            SubagentOverrides::default(),
        )],
        12_000,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "Inspect the parser");
    assert_eq!(rows[0].elapsed, "12s");
}

#[test]
fn ignores_shell_tasks_which_have_their_own_panel() {
    assert!(build_subagent_rows(&[shell("bash-1")], 0).is_empty());
}

#[test]
fn keeps_a_finished_child_briefly_so_its_outcome_is_seen() {
    let rows = build_subagent_rows(
        &[subagent(
            "agent-1",
            "Done",
            SubagentOverrides {
                status: TaskStatus::Completed,
                ended_at: Some(10_000),
                ..Default::default()
            },
        )],
        12_000,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].elapsed, "10s", "frozen at its end, not still counting");
}

#[test]
fn drops_a_finished_child_once_its_linger_has_run_out() {
    let rows = build_subagent_rows(
        &[subagent(
            "agent-1",
            "Done",
            SubagentOverrides {
                status: TaskStatus::Completed,
                ended_at: Some(10_000),
                ..Default::default()
            },
        )],
        16_000,
    );
    assert!(rows.is_empty());
}

#[test]
fn keeps_the_oldest_child_first_so_rows_do_not_move_under_the_eye() {
    let rows = build_subagent_rows(
        &[
            subagent(
                "agent-2",
                "Newer",
                SubagentOverrides {
                    started_at: 2000,
                    ..Default::default()
                },
            ),
            subagent(
                "agent-1",
                "Older",
                SubagentOverrides {
                    started_at: 1000,
                    ..Default::default()
                },
            ),
        ],
        5000,
    );
    assert_eq!(
        rows.iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        ["Older", "Newer"]
    );
}

#[test]
fn carries_the_spend_of_each_child_separately() {
    let rows = build_subagent_rows(
        &[
            subagent(
                "agent-1",
                "A",
                SubagentOverrides {
                    tokens: 17_900,
                    ..Default::default()
                },
            ),
            subagent(
                "agent-2",
                "B",
                SubagentOverrides {
                    tokens: 500,
                    ..Default::default()
                },
            ),
        ],
        0,
    );
    assert_eq!(
        rows.iter()
            .map(|row| row.tokens.as_str())
            .collect::<Vec<_>>(),
        ["17.9k", "500"]
    );
}

// --- rendering --------------------------------------------------------------------------

#[test]
fn takes_no_rows_at_all_when_nothing_is_delegated() {
    let _guard = theme_lock();
    let mut panel = SubagentPanel::new();
    panel.set_tasks(vec![shell("bash-1")]);
    assert!(panel.render(100).is_empty());
}

#[test]
fn puts_the_parent_above_its_children() {
    let _guard = theme_lock();
    let mut panel = SubagentPanel::new();
    panel.set_tasks(vec![subagent(
        "agent-1",
        "Inspect the parser",
        SubagentOverrides {
            started_at: now_ms(),
            ..Default::default()
        },
    )]);
    let lines = panel.render(100);
    assert!(lines[0].contains("main"), "{:?}", lines[0]);
    assert!(lines[1].contains("Inspect the parser"), "{:?}", lines[1]);
}

#[test]
fn shows_the_name_the_elapsed_time_and_the_spend_on_each_row() {
    let _guard = theme_lock();
    let mut panel = SubagentPanel::new();
    panel.set_tasks(vec![subagent(
        "agent-1",
        "Inspect the parser",
        SubagentOverrides {
            alias: "Rigel".to_string(),
            tokens: 17_900,
            started_at: now_ms(),
            ..Default::default()
        },
    )]);
    let row = panel.render(100).get(1).cloned().unwrap_or_default();
    assert!(row.contains("Rigel"), "{row}");
    assert!(row.contains("17.9k tokens"), "{row}");
}

#[test]
fn caps_the_rows_and_says_how_many_it_left_out() {
    let _guard = theme_lock();
    let mut panel = SubagentPanel::new();
    let now = now_ms();
    panel.set_tasks(
        (0..10)
            .map(|index| {
                subagent(
                    &format!("agent-{index}"),
                    &format!("Task {index}"),
                    SubagentOverrides {
                        started_at: now - index,
                        ..Default::default()
                    },
                )
            })
            .collect(),
    );
    let lines = panel.render(100);
    // One parent row, six children, one summary.
    assert_eq!(lines.len(), 8);
    assert!(
        lines.last().unwrap().contains("4 more"),
        "{:?}",
        lines.last()
    );
}

#[test]
fn never_renders_wider_than_it_was_given() {
    let _guard = theme_lock();
    let mut panel = SubagentPanel::new();
    panel.set_tasks(vec![subagent(
        "agent-1",
        &"A very long task description ".repeat(10),
        SubagentOverrides {
            started_at: now_ms(),
            ..Default::default()
        },
    )]);
    for line in panel.render(60) {
        assert!(
            strip_ansi(&line).chars().count() <= 60,
            "{:?}",
            strip_ansi(&line)
        );
    }
}

#[test]
fn spells_out_the_restriction_a_read_only_child_runs_under() {
    let _guard = theme_lock();
    let mut panel = SubagentPanel::new();
    panel.set_tasks(vec![subagent(
        "agent-1",
        "Sweep the renderers",
        SubagentOverrides {
            alias: "Vega".to_string(),
            agent: "read-only".to_string(),
            ..Default::default()
        },
    )]);
    let row = strip_ansi(&panel.render(100).get(1).cloned().unwrap_or_default());
    assert!(row.contains("Vega (read-only)"), "{row}");
}

/// A worker shows only its name. It is the ordinary case, and spelling it out
/// would spend a column on the absence of a restriction.
#[test]
fn leaves_a_worker_unqualified() {
    let _guard = theme_lock();
    let mut panel = SubagentPanel::new();
    panel.set_tasks(vec![subagent(
        "agent-1",
        "Rewrite the parser",
        SubagentOverrides {
            alias: "Rigel".to_string(),
            agent: "worker".to_string(),
            ..Default::default()
        },
    )]);
    let row = strip_ansi(&panel.render(100).get(1).cloned().unwrap_or_default());
    assert!(row.contains("Rigel"), "{row}");
    assert!(!row.contains("read-only"), "{row}");
    assert!(!row.contains("worker"), "{row}");
}

/// The marker carries the child's state: grey while it runs, green once it
/// completed cleanly, red for everything that ended badly — the same split the
/// tasks panel uses.
#[test]
fn colours_the_marker_by_outcome() {
    let _guard = theme_lock();
    let row_for = |status: TaskStatus, ended_at: Option<i64>| {
        let mut panel = SubagentPanel::new();
        panel.set_tasks(vec![subagent(
            "agent-1",
            "Look around",
            SubagentOverrides {
                status,
                started_at: now_ms(),
                ended_at,
                ..Default::default()
            },
        )]);
        panel.render(100).get(1).cloned().unwrap_or_default()
    };
    let running = row_for(TaskStatus::Running, None);
    let completed = row_for(TaskStatus::Completed, Some(now_ms()));
    let failed = row_for(TaskStatus::Failed, Some(now_ms()));
    let killed = row_for(TaskStatus::Killed, Some(now_ms()));
    let theme_instance = theme();
    assert!(
        running.contains(&theme_instance.fg(ThemeColor::Dim, "○")),
        "running is grey: {running}"
    );
    assert!(
        completed.contains(&theme_instance.fg(ThemeColor::Success, "○")),
        "completed is green: {completed}"
    );
    assert!(
        failed.contains(&theme_instance.fg(ThemeColor::Error, "○")),
        "failed is red: {failed}"
    );
    assert!(
        killed.contains(&theme_instance.fg(ThemeColor::Error, "○")),
        "killed is red: {killed}"
    );
}
