//! Port of `packages/coding-agent/test/todo-panel.test.ts` (164 LOC).
//!
//! The task panel above the editor.
//!
//! Two behaviours carry the design and both are pinned here: the window is
//! anchored so the task being worked on is the last visible row, and a list that
//! has just gone all-green stays up briefly instead of vanishing at the moment
//! it was finally worth looking at.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::todos::{Todo, TodoStatus};
use notagent::modes::interactive::components::todo_list::{
    TodoListComponent, TodoListMode, TodoVisibility, build_todo_display,
    format_hidden_todo_summary, format_todo_summary, todo_display_limit,
};
use notagent::modes::interactive::theme::theme::init_theme;
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

fn item(content: &str, status: TodoStatus) -> Todo {
    Todo {
        content: content.to_string(),
        active_form: format!("Doing {content}"),
        status,
    }
}

fn contents(todos: &[Todo]) -> Vec<&str> {
    todos.iter().map(|todo| todo.content.as_str()).collect()
}

// --- how much room the list may take ------------------------------------------

#[test]
fn matches_the_reference_budget() {
    let cases: [(usize, usize); 6] = [(10, 0), (11, 3), (17, 3), (18, 4), (24, 10), (80, 10)];
    for (rows, expected) in cases {
        assert_eq!(todo_display_limit(rows), expected, "rows: {rows}");
    }
}

#[test]
fn shows_nothing_at_all_on_a_terminal_too_small_to_spare_the_rows() {
    let display = build_todo_display(&[item("A", TodoStatus::Pending)], 10);
    assert!(display.visible.is_empty());
    assert_eq!(display.hidden, 1);
}

// --- which part of the list is shown -------------------------------------------

#[test]
fn shows_all_of_it_in_order_when_it_fits() {
    let todos = [
        item("First pending", TodoStatus::Pending),
        item("Active", TodoStatus::InProgress),
        item("Done", TodoStatus::Completed),
    ];
    let display = build_todo_display(&todos, 24);
    assert_eq!(
        contents(&display.visible),
        ["First pending", "Active", "Done"]
    );
    assert_eq!(display.hidden, 0);
    assert_eq!(format_hidden_todo_summary(&display), None);
}

#[test]
fn ends_the_window_on_the_active_task_when_it_has_to_truncate() {
    let todos = [
        item("Done one", TodoStatus::Completed),
        item("Done two", TodoStatus::Completed),
        item("Done three", TodoStatus::Completed),
        item("Done four", TodoStatus::Completed),
        item("Active", TodoStatus::InProgress),
        item("Pending one", TodoStatus::Pending),
    ];
    let display = build_todo_display(&todos, 17);
    assert_eq!(
        contents(&display.visible),
        ["Done three", "Done four", "Active"]
    );
    assert_eq!(display.hidden, 3);
    // The summary counts the whole list, not just what fell outside it.
    assert_eq!(
        format_hidden_todo_summary(&display).as_deref(),
        Some("... +3 1 pending, 4 completed")
    );
}

#[test]
fn stays_at_the_top_when_nothing_is_in_progress() {
    let todos: Vec<Todo> = (0..6)
        .map(|index| item(&format!("Task {index}"), TodoStatus::Pending))
        .collect();
    let display = build_todo_display(&todos, 17);
    assert_eq!(contents(&display.visible), ["Task 0", "Task 1", "Task 2"]);
}

// --- the summary line ----------------------------------------------------------

#[test]
fn omits_the_in_progress_count_when_nothing_is_running() {
    assert_eq!(
        format_todo_summary(&[
            item("A", TodoStatus::Completed),
            item("B", TodoStatus::Pending)
        ]),
        "2 tasks (1 done, 1 open)"
    );
}

#[test]
fn includes_it_when_something_is() {
    assert_eq!(
        format_todo_summary(&[
            item("A", TodoStatus::InProgress),
            item("B", TodoStatus::Pending)
        ]),
        "2 tasks (0 done, 1 in progress, 1 open)"
    );
}

// --- rendering ------------------------------------------------------------------

#[test]
fn takes_no_rows_at_all_when_there_is_nothing_to_show() {
    let _guard = theme_lock();
    let mut panel = TodoListComponent::new(TodoListMode::Status, 24);
    panel.set_todos(Vec::new());
    assert!(panel.render(80).is_empty());
}

#[test]
fn marks_each_status_with_its_own_glyph() {
    let _guard = theme_lock();
    let mut panel = TodoListComponent::new(TodoListMode::Status, 24);
    panel.set_todos(vec![
        item("Pending", TodoStatus::Pending),
        item("Active", TodoStatus::InProgress),
        item("Done", TodoStatus::Completed),
    ]);
    let text = panel.render(80).join("\n");
    assert!(text.contains('○'), "{text}");
    assert!(text.contains('◐'), "{text}");
    assert!(text.contains('●'), "{text}");
}

#[test]
fn shows_the_summary_only_in_standalone_mode() {
    let _guard = theme_lock();
    let todos = vec![
        item("A", TodoStatus::Pending),
        item("B", TodoStatus::Completed),
    ];
    let mut status = TodoListComponent::new(TodoListMode::Status, 24);
    status.set_todos(todos.clone());
    assert!(!status.render(80).join("\n").contains("tasks ("));

    let mut standalone = TodoListComponent::new(TodoListMode::Standalone, 24);
    standalone.set_todos(todos);
    assert!(
        standalone
            .render(80)
            .join("\n")
            .contains("2 tasks (1 done, 1 open)")
    );
}

// --- when the panel is up --------------------------------------------------------

/// The TypeScript cases await the hide timer; the port polls it instead
/// (timers never call back, see `plans/interface-requests.md` A-4). The sleeps
/// keep the same shape so the epoch guard is exercised the same way.
fn sleep(millis: u64) {
    std::thread::sleep(std::time::Duration::from_millis(millis));
}

#[test]
fn stays_up_while_anything_is_unfinished() {
    let mut visibility = TodoVisibility::new(Box::new(|| {}), 20);
    visibility.update(vec![item("Active", TodoStatus::InProgress)], false);
    assert!(visibility.visible());
    sleep(60);
    visibility.tick();
    assert!(visibility.visible());
}

#[test]
fn hides_an_all_completed_list_after_the_delay_having_shown_it_once() {
    let changes = Rc::new(RefCell::new(0usize));
    let sink = Rc::clone(&changes);
    let mut visibility = TodoVisibility::new(Box::new(move || *sink.borrow_mut() += 1), 20);
    visibility.update(vec![item("Done", TodoStatus::Completed)], true);
    assert!(visibility.visible());
    assert!(visibility.hide_deadline().is_some());

    sleep(60);
    assert!(visibility.tick());
    assert!(!visibility.visible());
    assert_eq!(*changes.borrow(), 1);
}

#[test]
fn does_not_raise_a_panel_the_user_already_watched_disappear() {
    let mut visibility = TodoVisibility::new(Box::new(|| {}), 20);
    visibility.update(vec![item("Done", TodoStatus::Completed)], false);
    assert!(!visibility.visible());
}

#[test]
fn goes_down_at_once_on_an_empty_list() {
    let mut visibility = TodoVisibility::new(Box::new(|| {}), 20);
    visibility.update(vec![item("Active", TodoStatus::InProgress)], false);
    visibility.update(Vec::new(), false);
    assert!(!visibility.visible());
}

#[test]
fn does_not_let_a_stale_hide_timer_take_down_a_list_that_changed_meanwhile() {
    let mut visibility = TodoVisibility::new(Box::new(|| {}), 20);
    visibility.update(vec![item("Done", TodoStatus::Completed)], true);
    // New work arrives before the timer fires; the timer must not apply.
    visibility.update(vec![item("Next", TodoStatus::InProgress)], false);
    sleep(60);
    assert!(!visibility.tick());
    assert!(visibility.visible());
}
