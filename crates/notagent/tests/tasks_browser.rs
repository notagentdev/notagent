//! Port of `packages/coding-agent/test/tasks-browser.test.ts` (283 LOC).
//!
//! The task browser.
//!
//! The cases worth pinning are the ones that decide whether the view can be
//! acted from: what is listed and in what order, that a stop asks before it
//! kills, and that every rendered line is exactly as wide as it was told — a
//! frame that drifts by one column turns the whole layout into noise.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::tasks::types::{
    ShellTaskInfo, SubagentTaskInfo, TaskInfo, TaskInfoBase, TaskStatus,
};
use notagent::modes::interactive::components::tasks_browser::{
    STOP_CONFIRM_TIMEOUT_MS, TaskCounts, TasksBrowserComponent, TasksBrowserProps, TasksFilter,
    count_tasks, visible_tasks,
};
use notagent::modes::interactive::theme::theme::init_theme;
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

const ESCAPE: &str = "\x1b";
const ARROW_UP: &str = "\x1b[A";
const ARROW_DOWN: &str = "\x1b[B";

#[derive(Default)]
struct ShellOverrides {
    description: Option<String>,
    started_at: Option<i64>,
    ended_at: Option<Option<i64>>,
}

fn shell(task_id: &str, status: TaskStatus, overrides: ShellOverrides) -> TaskInfo {
    let started_at = overrides.started_at.unwrap_or(1000);
    TaskInfo::Shell(ShellTaskInfo {
        base: TaskInfoBase {
            task_id: task_id.to_string(),
            description: overrides
                .description
                .unwrap_or_else(|| format!("run {task_id}")),
            status,
            detached: Some(true),
            started_at,
            ended_at: overrides
                .ended_at
                .unwrap_or(if status == TaskStatus::Running {
                    None
                } else {
                    Some(2000)
                }),
            stop_reason: None,
            notification_suppressed: None,
            timeout_ms: None,
        },
        command: "npm test".to_string(),
        pid: 42,
        exit_code: if status == TaskStatus::Completed {
            Some(0)
        } else {
            None
        },
    })
}

fn subagent(task_id: &str, status: TaskStatus) -> TaskInfo {
    TaskInfo::Subagent(SubagentTaskInfo {
        base: TaskInfoBase {
            task_id: task_id.to_string(),
            description: format!("investigate {task_id}"),
            status,
            detached: Some(false),
            started_at: 1000,
            ended_at: if status == TaskStatus::Running {
                None
            } else {
                Some(2000)
            },
            stop_reason: None,
            notification_suppressed: None,
            timeout_ms: None,
        },
        tokens: 17_900,
        session_id: "session-1".to_string(),
        mode_id: "plan".to_string(),
    })
}

#[derive(Default)]
struct PropOverrides {
    filter: Option<TasksFilter>,
    output: Option<String>,
    on_select: Option<Rc<RefCell<Vec<String>>>>,
    on_stop: Option<Rc<RefCell<Vec<String>>>>,
    on_stop_refused: Option<Rc<RefCell<Vec<String>>>>,
    on_close: Option<Rc<RefCell<usize>>>,
    on_toggle_filter: Option<Rc<RefCell<usize>>>,
}

fn props(tasks: Vec<TaskInfo>, overrides: PropOverrides) -> TasksBrowserProps {
    let selected_task_id = tasks.first().map(|task| task.base().task_id.clone());
    let record = |sink: Option<Rc<RefCell<Vec<String>>>>| -> Box<dyn FnMut(&str)> {
        match sink {
            Some(sink) => Box::new(move |id: &str| sink.borrow_mut().push(id.to_string())),
            None => Box::new(|_| {}),
        }
    };
    let count = |sink: Option<Rc<RefCell<usize>>>| -> Box<dyn FnMut()> {
        match sink {
            Some(sink) => Box::new(move || *sink.borrow_mut() += 1),
            None => Box::new(|| {}),
        }
    };
    TasksBrowserProps {
        tasks,
        filter: overrides.filter.unwrap_or(TasksFilter::All),
        selected_task_id,
        output: overrides.output,
        output_loading: false,
        notice: None,
        on_select: record(overrides.on_select),
        on_toggle_filter: count(overrides.on_toggle_filter),
        on_refresh: Box::new(|| {}),
        on_close: count(overrides.on_close),
        on_stop: record(overrides.on_stop),
        on_stop_refused: overrides
            .on_stop_refused
            .map(|sink| -> Box<dyn FnMut(&str)> {
                Box::new(move |id: &str| sink.borrow_mut().push(id.to_string()))
            }),
    }
}

fn ids(tasks: &[TaskInfo]) -> Vec<&str> {
    tasks
        .iter()
        .map(|task| task.base().task_id.as_str())
        .collect()
}

// --- what is listed ------------------------------------------------------------------

#[test]
fn hides_finished_work_under_the_running_filter() {
    let tasks = [
        shell("bash-1", TaskStatus::Running, ShellOverrides::default()),
        shell("bash-2", TaskStatus::Completed, ShellOverrides::default()),
    ];
    assert_eq!(
        ids(&visible_tasks(&tasks, TasksFilter::Running)),
        ["bash-1"]
    );
    assert_eq!(visible_tasks(&tasks, TasksFilter::All).len(), 2);
}

#[test]
fn keeps_running_work_above_finished_work() {
    let order = visible_tasks(
        &[
            shell("bash-1", TaskStatus::Completed, ShellOverrides::default()),
            shell("bash-2", TaskStatus::Running, ShellOverrides::default()),
        ],
        TasksFilter::All,
    );
    assert_eq!(ids(&order), ["bash-2", "bash-1"]);
}

#[test]
fn orders_running_work_oldest_first_so_a_row_does_not_move_while_read() {
    let order = visible_tasks(
        &[
            shell(
                "bash-2",
                TaskStatus::Running,
                ShellOverrides {
                    started_at: Some(2000),
                    ..Default::default()
                },
            ),
            shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides {
                    started_at: Some(1000),
                    ..Default::default()
                },
            ),
        ],
        TasksFilter::All,
    );
    assert_eq!(ids(&order), ["bash-1", "bash-2"]);
}

#[test]
fn orders_finished_work_newest_first_since_the_last_one_to_end_is_the_interesting_one() {
    let order = visible_tasks(
        &[
            shell(
                "bash-1",
                TaskStatus::Completed,
                ShellOverrides {
                    ended_at: Some(Some(1000)),
                    ..Default::default()
                },
            ),
            shell(
                "bash-2",
                TaskStatus::Completed,
                ShellOverrides {
                    ended_at: Some(Some(5000)),
                    ..Default::default()
                },
            ),
        ],
        TasksFilter::All,
    );
    assert_eq!(ids(&order), ["bash-2", "bash-1"]);
}

#[test]
fn includes_work_a_tool_call_is_still_waiting_on() {
    assert_eq!(
        visible_tasks(
            &[subagent("agent-1", TaskStatus::Running)],
            TasksFilter::Running
        )
        .len(),
        1
    );
}

#[test]
fn counts_the_three_outcomes_apart() {
    let counts = count_tasks(&[
        shell("bash-1", TaskStatus::Running, ShellOverrides::default()),
        shell("bash-2", TaskStatus::Completed, ShellOverrides::default()),
        shell("bash-3", TaskStatus::Killed, ShellOverrides::default()),
        shell("bash-4", TaskStatus::TimedOut, ShellOverrides::default()),
    ]);
    assert_eq!(
        counts,
        TaskCounts {
            running: 1,
            completed: 1,
            failed: 2
        }
    );
}

// --- the layout ------------------------------------------------------------------------

#[test]
fn renders_exactly_the_height_it_was_given() {
    let _guard = theme_lock();
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides::default(),
        ),
        24,
    );
    assert_eq!(browser.render(100).len(), 24);
}

#[test]
fn renders_every_line_exactly_the_width_it_was_given() {
    let _guard = theme_lock();
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides::default(),
        ),
        24,
    );
    for line in browser.render(100) {
        assert_eq!(
            strip_ansi(&line).chars().count(),
            100,
            "{:?}",
            strip_ansi(&line)
        );
    }
}

#[test]
fn holds_its_shape_with_a_long_description_and_a_narrow_terminal() {
    let _guard = theme_lock();
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides {
                    description: Some("x".repeat(400)),
                    ..Default::default()
                },
            )],
            PropOverrides::default(),
        ),
        20,
    );
    for line in browser.render(60) {
        assert_eq!(
            strip_ansi(&line).chars().count(),
            60,
            "{:?}",
            strip_ansi(&line)
        );
    }
}

#[test]
fn says_so_rather_than_drawing_a_broken_frame_when_there_is_no_room() {
    let _guard = theme_lock();
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides::default(),
        ),
        4,
    );
    assert!(strip_ansi(&browser.render(100).join("\n")).contains("Terminal too small"));
}

#[test]
fn shows_the_counts_and_the_filter_in_the_header() {
    let _guard = theme_lock();
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![
                shell("bash-1", TaskStatus::Running, ShellOverrides::default()),
                shell("bash-2", TaskStatus::Completed, ShellOverrides::default()),
            ],
            PropOverrides::default(),
        ),
        24,
    );
    let header = strip_ansi(
        browser
            .render(100)
            .first()
            .map(|line| line.as_ref())
            .unwrap_or(""),
    );
    assert!(header.contains("filter=ALL"), "{header}");
    assert!(header.contains("1 running"), "{header}");
    assert!(header.contains("2 total"), "{header}");
}

#[test]
fn says_plainly_when_the_filter_hides_everything() {
    let _guard = theme_lock();
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Completed,
                ShellOverrides::default(),
            )],
            PropOverrides {
                filter: Some(TasksFilter::Running),
                ..Default::default()
            },
        ),
        24,
    );
    assert!(strip_ansi(&browser.render(100).join("\n")).contains("Nothing running"));
}

// --- the detail pane ---------------------------------------------------------------------

#[test]
fn shows_a_shell_tasks_command_and_exit_code() {
    let _guard = theme_lock();
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Completed,
                ShellOverrides::default(),
            )],
            PropOverrides::default(),
        ),
        24,
    );
    let text = strip_ansi(&browser.render(100).join("\n"));
    assert!(text.contains("npm test"), "{text}");
    assert!(text.contains("Exit code:"), "{text}");
}

#[test]
fn shows_a_subagents_session_and_mode_which_is_what_continues_it() {
    let _guard = theme_lock();
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![subagent("agent-1", TaskStatus::Running)],
            PropOverrides::default(),
        ),
        24,
    );
    let text = strip_ansi(&browser.render(100).join("\n"));
    assert!(text.contains("session-1"), "{text}");
    assert!(text.contains("plan"), "{text}");
}

#[test]
fn shows_the_tail_of_the_output_rather_than_its_head() {
    let _guard = theme_lock();
    let output = (0..200)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides {
                output: Some(output),
                ..Default::default()
            },
        ),
        24,
    );
    let text = strip_ansi(&browser.render(100).join("\n"));
    assert!(text.contains("line 199"), "{text}");
    assert!(!text.contains("line 0 "), "{text}");
}

// --- stopping ------------------------------------------------------------------------------

#[test]
fn asks_before_it_kills() {
    let _guard = theme_lock();
    let stopped = Rc::new(RefCell::new(Vec::new()));
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides {
                on_stop: Some(Rc::clone(&stopped)),
                ..Default::default()
            },
        ),
        24,
    );
    browser.handle_input("s");
    assert!(stopped.borrow().is_empty());
    let footer = strip_ansi(
        browser
            .render(100)
            .last()
            .map(|line| line.as_ref())
            .unwrap_or(""),
    );
    assert!(footer.contains("Stop bash-1?"), "{footer}");
}

#[test]
fn kills_on_yes() {
    let _guard = theme_lock();
    let stopped = Rc::new(RefCell::new(Vec::new()));
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides {
                on_stop: Some(Rc::clone(&stopped)),
                ..Default::default()
            },
        ),
        24,
    );
    browser.handle_input("s");
    browser.handle_input("y");
    assert_eq!(*stopped.borrow(), ["bash-1"]);
}

#[test]
fn treats_anything_but_yes_as_no() {
    let _guard = theme_lock();
    let stopped = Rc::new(RefCell::new(Vec::new()));
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides {
                on_stop: Some(Rc::clone(&stopped)),
                ..Default::default()
            },
        ),
        24,
    );
    browser.handle_input("s");
    browser.handle_input("j");
    assert!(stopped.borrow().is_empty());
    // And the key that cancelled it does not also move the selection.
    let footer = strip_ansi(
        browser
            .render(100)
            .last()
            .map(|line| line.as_ref())
            .unwrap_or(""),
    );
    assert!(!footer.contains("Stop bash-1?"), "{footer}");
}

#[test]
fn refuses_on_something_already_finished_and_says_why() {
    let _guard = theme_lock();
    let refused = Rc::new(RefCell::new(Vec::new()));
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Completed,
                ShellOverrides::default(),
            )],
            PropOverrides {
                on_stop_refused: Some(Rc::clone(&refused)),
                ..Default::default()
            },
        ),
        24,
    );
    browser.handle_input("s");
    assert_eq!(*refused.borrow(), ["bash-1"]);
}

#[test]
fn does_not_advertise_the_key_on_something_it_cannot_stop() {
    let _guard = theme_lock();
    let mut running = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides::default(),
        ),
        24,
    );
    let mut finished = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Completed,
                ShellOverrides::default(),
            )],
            PropOverrides::default(),
        ),
        24,
    );
    assert!(
        strip_ansi(
            running
                .render(100)
                .last()
                .map(|line| line.as_ref())
                .unwrap_or("")
        )
        .contains("stop")
    );
    assert!(
        !strip_ansi(
            finished
                .render(100)
                .last()
                .map(|line| line.as_ref())
                .unwrap_or("")
        )
        .contains("stop")
    );
}

#[test]
fn drops_the_question_when_the_task_ends_on_its_own_meanwhile() {
    let _guard = theme_lock();
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides::default(),
        ),
        24,
    );
    browser.handle_input("s");
    browser.set_props(props(
        vec![shell(
            "bash-1",
            TaskStatus::Completed,
            ShellOverrides::default(),
        )],
        PropOverrides::default(),
    ));
    let footer = strip_ansi(
        browser
            .render(100)
            .last()
            .map(|line| line.as_ref())
            .unwrap_or(""),
    );
    assert!(!footer.contains("Stop bash-1?"), "{footer}");
}

#[test]
fn gives_the_question_a_deadline() {
    assert_eq!(STOP_CONFIRM_TIMEOUT_MS, 5_000);
}

// --- navigation ---------------------------------------------------------------------------

#[test]
fn moves_the_selection_and_reports_it() {
    let _guard = theme_lock();
    let selected = Rc::new(RefCell::new(Vec::new()));
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![
                shell(
                    "bash-1",
                    TaskStatus::Running,
                    ShellOverrides {
                        started_at: Some(1),
                        ..Default::default()
                    },
                ),
                shell(
                    "bash-2",
                    TaskStatus::Running,
                    ShellOverrides {
                        started_at: Some(2),
                        ..Default::default()
                    },
                ),
            ],
            PropOverrides {
                on_select: Some(Rc::clone(&selected)),
                ..Default::default()
            },
        ),
        24,
    );
    browser.handle_input(ARROW_DOWN);
    assert_eq!(*selected.borrow(), ["bash-2"]);
}

#[test]
fn stops_at_the_ends_rather_than_wrapping() {
    let _guard = theme_lock();
    let selected = Rc::new(RefCell::new(Vec::new()));
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides {
                on_select: Some(Rc::clone(&selected)),
                ..Default::default()
            },
        ),
        24,
    );
    browser.handle_input(ARROW_UP);
    browser.handle_input(ARROW_DOWN);
    assert_eq!(*selected.borrow(), ["bash-1", "bash-1"]);
}

#[test]
fn closes_on_escape_and_on_q() {
    let _guard = theme_lock();
    let closes = Rc::new(RefCell::new(0usize));
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides {
                on_close: Some(Rc::clone(&closes)),
                ..Default::default()
            },
        ),
        24,
    );
    browser.handle_input(ESCAPE);
    browser.handle_input("q");
    assert_eq!(*closes.borrow(), 2);
}

#[test]
fn toggles_the_filter_on_tab() {
    let _guard = theme_lock();
    let toggles = Rc::new(RefCell::new(0usize));
    let mut browser = TasksBrowserComponent::new(
        props(
            vec![shell(
                "bash-1",
                TaskStatus::Running,
                ShellOverrides::default(),
            )],
            PropOverrides {
                on_toggle_filter: Some(Rc::clone(&toggles)),
                ..Default::default()
            },
        ),
        24,
    );
    browser.handle_input("\t");
    assert_eq!(*toggles.borrow(), 1);
}
