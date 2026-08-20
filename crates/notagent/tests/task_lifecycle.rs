//! The transcript lines that report what background work did.
//!
//! The panels show what is running and drop a task the moment it settles, so
//! without these lines a background job simply vanishes and the user is left
//! guessing whether it worked. What is pinned here is which lines appear, that
//! none appears twice, and that work from a previous session stays quiet.

use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::tasks::types::{
    ShellTaskInfo, SubagentTaskInfo, TaskInfo, TaskInfoBase, TaskStatus,
};
use notagent::modes::interactive::components::task_lifecycle::TaskAnnouncer;
use notagent::modes::interactive::theme::theme::{init_theme, theme};
use notagent::utils::ansi::strip_ansi;

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

fn base(task_id: &str, status: TaskStatus, detached: bool) -> TaskInfoBase {
    TaskInfoBase {
        task_id: task_id.to_string(),
        description: "do the thing".to_string(),
        status,
        detached: Some(detached),
        started_at: 0,
        ended_at: Some(3_000),
        stop_reason: None,
        notification_suppressed: None,
        timeout_ms: None,
    }
}

fn subagent(task_id: &str, status: TaskStatus) -> TaskInfo {
    TaskInfo::Subagent(SubagentTaskInfo {
        base: base(task_id, status, true),
        tokens: 0,
        session_id: format!("session-{task_id}"),
        agent: "read-only".to_string(),
        alias: "Vega".to_string(),
    })
}

fn shell(task_id: &str, status: TaskStatus, exit_code: Option<i32>) -> TaskInfo {
    TaskInfo::Shell(ShellTaskInfo {
        base: base(task_id, status, true),
        command: "cargo test --all".to_string(),
        pid: 4242,
        exit_code,
    })
}

fn observe(announcer: &mut TaskAnnouncer, tasks: &[TaskInfo]) -> Vec<String> {
    announcer
        .observe(tasks, &theme(), false)
        .into_iter()
        .map(|line| strip_ansi(&line).trim().to_string())
        .collect()
}

// ── a detached command ────────────────────────────────────────────────

/// The case that prompted all of this: a background command finishes and the
/// transcript says so. The panel cannot, because a settled task is gone from it.
#[test]
fn reports_a_background_command_that_finished() {
    let _guard = theme_lock();
    let mut announcer = TaskAnnouncer::new();

    let running = observe(
        &mut announcer,
        &[shell("shell-1", TaskStatus::Running, None)],
    );
    assert!(
        running.is_empty(),
        "the launch is already in the tool block: {running:?}"
    );

    let done = observe(
        &mut announcer,
        &[shell("shell-1", TaskStatus::Completed, Some(0))],
    );
    assert_eq!(done.len(), 1, "{done:?}");
    assert!(done[0].contains("background done"), "{}", done[0]);
    assert!(done[0].contains("cargo test --all"), "{}", done[0]);
}

/// A non-zero exit is the difference between "it ran" and "it worked", so it
/// goes in the line rather than waiting to be looked up.
#[test]
fn names_the_exit_code_of_a_command_that_failed() {
    let _guard = theme_lock();
    let mut announcer = TaskAnnouncer::new();
    observe(
        &mut announcer,
        &[shell("shell-1", TaskStatus::Running, None)],
    );

    let failed = observe(
        &mut announcer,
        &[shell("shell-1", TaskStatus::Failed, Some(101))],
    );
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("background failed"), "{}", failed[0]);
    assert!(failed[0].contains("exit 101"), "{}", failed[0]);
}

/// A foreground command hands its result back into the block that ran it.
#[test]
fn stays_quiet_about_a_foreground_command() {
    let _guard = theme_lock();
    let mut announcer = TaskAnnouncer::new();
    let attached = TaskInfo::Shell(ShellTaskInfo {
        base: base("shell-1", TaskStatus::Running, false),
        command: "ls".to_string(),
        pid: 1,
        exit_code: None,
    });
    observe(&mut announcer, &[attached]);

    let settled = TaskInfo::Shell(ShellTaskInfo {
        base: base("shell-1", TaskStatus::Completed, false),
        command: "ls".to_string(),
        pid: 1,
        exit_code: Some(0),
    });
    assert!(observe(&mut announcer, &[settled]).is_empty());
}

// ── a subagent ────────────────────────────────────────────────────────

#[test]
fn reports_both_ends_of_a_subagents_life() {
    let _guard = theme_lock();
    let mut announcer = TaskAnnouncer::new();

    let started = observe(&mut announcer, &[subagent("agent-1", TaskStatus::Running)]);
    assert_eq!(started.len(), 1, "{started:?}");
    assert!(started[0].contains("subagent started"), "{}", started[0]);
    assert!(started[0].contains("Vega"), "{}", started[0]);

    let ended = observe(
        &mut announcer,
        &[subagent("agent-1", TaskStatus::Completed)],
    );
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert!(ended[0].contains("subagent done"), "{}", ended[0]);
    assert!(ended[0].contains("Vega"), "{}", ended[0]);
}

#[test]
fn marks_a_subagent_that_ended_badly_as_failed() {
    let _guard = theme_lock();
    let mut announcer = TaskAnnouncer::new();
    observe(&mut announcer, &[subagent("agent-1", TaskStatus::Running)]);
    let ended = observe(&mut announcer, &[subagent("agent-1", TaskStatus::Killed)]);
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert!(ended[0].contains("subagent failed"), "{}", ended[0]);
}

// ── saying it once ────────────────────────────────────────────────────

/// The snapshot arrives every second, and a settled task stays in it.
#[test]
fn says_each_thing_exactly_once() {
    let _guard = theme_lock();
    let mut announcer = TaskAnnouncer::new();
    let running = [
        subagent("agent-1", TaskStatus::Running),
        shell("shell-1", TaskStatus::Running, None),
    ];
    assert_eq!(observe(&mut announcer, &running).len(), 1);
    assert!(observe(&mut announcer, &running).is_empty());

    let settled = [
        subagent("agent-1", TaskStatus::Completed),
        shell("shell-1", TaskStatus::Completed, Some(0)),
    ];
    assert_eq!(observe(&mut announcer, &settled).len(), 2);
    assert!(observe(&mut announcer, &settled).is_empty());
    assert!(observe(&mut announcer, &settled).is_empty());
}

/// A task that was already over the first time it was seen ran in a previous
/// session. Announcing it would tell the user about work they did not start.
#[test]
fn stays_quiet_about_work_from_a_previous_session() {
    let _guard = theme_lock();
    let mut announcer = TaskAnnouncer::new();
    let restored = [
        subagent("agent-1", TaskStatus::Completed),
        shell("shell-1", TaskStatus::Completed, Some(0)),
    ];
    assert!(
        observe(&mut announcer, &restored).is_empty(),
        "{restored:?}"
    );
}
