use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::tasks::types::{ShellTaskInfo, TaskInfo, TaskInfoBase, TaskStatus};
use notagent::modes::interactive::components::tasks_panel::{TasksPanel, TasksPanelScope};
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

fn shell(task_id: &str, status: TaskStatus, description: &str, started_at: i64) -> TaskInfo {
    TaskInfo::Shell(ShellTaskInfo {
        base: TaskInfoBase {
            task_id: task_id.to_string(),
            description: description.to_string(),
            status,
            detached: Some(true),
            started_at,
            ended_at: if status == TaskStatus::Running {
                None
            } else {
                Some(started_at + 1000)
            },
            stop_reason: None,
            notification_suppressed: None,
            timeout_ms: None,
        },
        command: "cmd".to_string(),
        pid: 1,
        exit_code: if status == TaskStatus::Completed {
            Some(0)
        } else {
            None
        },
    })
}

fn rendered(panel: &mut TasksPanel) -> String {
    panel.render(100).join("\n")
}

// --- what the panel occupies -------------------------------------------------------

#[test]
fn takes_no_rows_at_all_when_nothing_is_running() {
    let _guard = theme_lock();
    let mut panel = TasksPanel::new();
    panel.set_tasks(Vec::new());
    assert!(panel.render(100).is_empty());
}

#[test]
fn takes_no_rows_when_everything_is_finished_and_only_running_work_is_shown() {
    let _guard = theme_lock();
    let mut panel = TasksPanel::new();
    panel.set_tasks(vec![shell(
        "bash-11112222",
        TaskStatus::Completed,
        "a build",
        0,
    )]);
    assert!(panel.render(100).is_empty());
}

#[test]
fn shows_a_running_task_with_its_id_and_description() {
    let _guard = theme_lock();
    let mut panel = TasksPanel::new();
    panel.set_tasks(vec![shell(
        "bash-11112222",
        TaskStatus::Running,
        "the dev server",
        0,
    )]);
    let text = rendered(&mut panel);
    assert!(text.contains("bash-11112222"), "{text}");
    assert!(text.contains("the dev server"), "{text}");
}

#[test]
fn caps_how_much_of_the_screen_a_fan_out_can_take() {
    let _guard = theme_lock();
    let mut panel = TasksPanel::new();
    panel.set_tasks(
        (0..30)
            .map(|index| {
                shell(
                    &format!("bash-1111222{}", index % 10),
                    TaskStatus::Running,
                    &format!("task {index}"),
                    0,
                )
            })
            .collect(),
    );
    // Separator, heading, the capped rows, and one line saying what was left
    // out.
    assert!(panel.render(100).len() <= 11);
    assert!(rendered(&mut panel).contains("more"));
}

#[test]
fn renders_in_the_roster_style_of_the_reference() {
    let _guard = theme_lock();
    let mut panel = TasksPanel::new();
    panel.set_scope(TasksPanelScope::All);
    // Completed, so the elapsed time is ended_at - started_at = 1s and the
    // test does not race the wall clock.
    panel.set_tasks(vec![shell(
        "bash-11112222",
        TaskStatus::Completed,
        "a build",
        0,
    )]);
    let lines = panel.render(60);
    assert_eq!(
        lines[0].as_ref(),
        "",
        "a blank row separates the panel from the footer"
    );
    let heading = strip_ansi(&lines[1]);
    assert!(
        heading.contains("● background tasks"),
        "bulleted head line: {heading}"
    );
    let row = strip_ansi(&lines[2]);
    assert!(
        row.contains("○ bash-11112222") && row.contains("a build"),
        "marker, id and label: {row}"
    );
    assert!(row.trim_end().ends_with("1s"), "elapsed flush right: {row}");
}

#[test]
fn colours_the_marker_grey_running_green_completed_red_failed() {
    let _guard = theme_lock();
    let mut panel = TasksPanel::new();
    panel.set_scope(TasksPanelScope::All);
    panel.set_tasks(vec![
        shell("bash-11112222", TaskStatus::Running, "running", 0),
        shell("bash-33334444", TaskStatus::Completed, "done", 0),
        shell("bash-55556666", TaskStatus::Killed, "killed", 0),
        shell("bash-77778888", TaskStatus::Failed, "failed", 0),
    ]);
    let lines = panel.render(100).join("\n");
    let marker = |colour: ThemeColor| theme().fg(colour, "○");
    // The default test theme emits real colours; without this the containment
    // checks below would pass vacuously.
    assert_ne!(
        marker(ThemeColor::Dim),
        marker(ThemeColor::Success),
        "theme must render distinguishable colours"
    );
    let row = |id: &str| {
        lines
            .lines()
            .find(|line| line.contains(id))
            .unwrap_or_else(|| panic!("row {id} is drawn:\n{lines}"))
            .to_owned()
    };
    // User decision 2026-08-16: grey while running, green on clean completion,
    // red for failed and killed alike.
    assert!(row("bash-11112222").contains(&marker(ThemeColor::Dim)));
    assert!(row("bash-33334444").contains(&marker(ThemeColor::Success)));
    assert!(row("bash-55556666").contains(&marker(ThemeColor::Error)));
    assert!(row("bash-77778888").contains(&marker(ThemeColor::Error)));
}

// --- the scope ring -----------------------------------------------------------------

#[test]
fn steps_through_running_all_and_hidden() {
    let _guard = theme_lock();
    let mut panel = TasksPanel::new();
    assert_eq!(panel.get_scope(), TasksPanelScope::Running);
    assert_eq!(panel.cycle_scope(), TasksPanelScope::All);
    assert_eq!(panel.cycle_scope(), TasksPanelScope::Hidden);
    assert_eq!(panel.cycle_scope(), TasksPanelScope::Running);
}

#[test]
fn shows_finished_work_only_in_the_all_scope() {
    let _guard = theme_lock();
    let mut panel = TasksPanel::new();
    panel.set_tasks(vec![shell(
        "bash-11112222",
        TaskStatus::Completed,
        "a build",
        0,
    )]);
    assert!(panel.render(100).is_empty());
    panel.set_scope(TasksPanelScope::All);
    assert!(rendered(&mut panel).contains("a build"));
}

#[test]
fn shows_nothing_at_all_once_hidden_whatever_is_running() {
    let _guard = theme_lock();
    let mut panel = TasksPanel::new();
    panel.set_tasks(vec![shell(
        "bash-11112222",
        TaskStatus::Running,
        "the dev server",
        0,
    )]);
    panel.set_scope(TasksPanelScope::Hidden);
    assert!(panel.render(100).is_empty());
}

// --- ordering -------------------------------------------------------------------------

#[test]
fn keeps_running_work_above_finished_work() {
    let _guard = theme_lock();
    let mut panel = TasksPanel::new();
    panel.set_scope(TasksPanelScope::All);
    panel.set_tasks(vec![
        shell("bash-11112222", TaskStatus::Completed, "done first", 0),
        shell("bash-33334444", TaskStatus::Running, "still going", 0),
    ]);
    let lines = panel.render(100);
    let running_row = lines
        .iter()
        .position(|line| line.contains("still going"))
        .expect("the running row is drawn");
    let finished_row = lines
        .iter()
        .position(|line| line.contains("done first"))
        .expect("the finished row is drawn");
    assert!(running_row < finished_row);
}

#[test]
fn keeps_the_oldest_running_task_in_place_rather_than_reshuffling_on_each_new_one() {
    let _guard = theme_lock();
    let mut panel = TasksPanel::new();
    panel.set_tasks(vec![
        shell("bash-33334444", TaskStatus::Running, "newer", 2000),
        shell("bash-11112222", TaskStatus::Running, "older", 1000),
    ]);
    let lines = panel.render(100);
    let older = lines.iter().position(|line| line.contains("older"));
    let newer = lines.iter().position(|line| line.contains("newer"));
    assert!(older < newer, "{older:?} {newer:?}");
}
