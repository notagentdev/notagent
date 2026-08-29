//! The immutable transcript record around background work.

mod suite;

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use notagent::core::agent_session::AgentSessionEvent;
use notagent::core::session_manager::{SessionManager, session_entry_to_context_messages};
use notagent::core::tasks::lifecycle::{
    TASK_LIFECYCLE_ENTRY_TYPE, TaskLifecyclePhase, TaskLifecycleRecord,
};
use notagent::core::tasks::manager::RegisterTaskOptions;
use notagent::core::tasks::types::{
    BackgroundTask, ShellTaskInfo, SubagentTaskInfo, TaskInfo, TaskInfoBase, TaskKind,
    TaskSettlement, TaskSettlementStatus, TaskSink, TaskStatus,
};
use notagent::modes::interactive::components::task_lifecycle::{
    is_background_bash_call, task_lifecycle_line,
};
use notagent::modes::interactive::theme::theme::{ThemeBg, badge, init_theme, theme};
use notagent::utils::ansi::strip_ansi;
use notagent_agent::types::BoxFuture;
use serde_json::json;
use suite::{HarnessOptions, create_harness};

const UNSTREAMED_OUTPUT: &str = "background output stays in the task log";

struct ImmediateShell;

impl BackgroundTask for ImmediateShell {
    fn kind(&self) -> TaskKind {
        TaskKind::Shell
    }

    fn id_prefix(&self) -> &str {
        "shell"
    }

    fn description(&self) -> String {
        "finish immediately".to_owned()
    }

    fn start<'a>(&'a self, sink: TaskSink) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            sink.append_output(UNSTREAMED_OUTPUT);
            sink.settle(TaskSettlement::new(TaskSettlementStatus::Completed))
                .await;
            Ok(())
        })
    }

    fn to_info(&self, base: TaskInfoBase) -> TaskInfo {
        TaskInfo::Shell(ShellTaskInfo {
            base,
            command: "true".to_owned(),
            pid: 42,
            exit_code: Some(0),
        })
    }
}

fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    init_theme(None, false);
    guard
}

fn base(task_id: &str, status: TaskStatus) -> TaskInfoBase {
    TaskInfoBase {
        task_id: task_id.to_owned(),
        description: "inspect the lifecycle".to_owned(),
        status,
        detached: Some(true),
        started_at: 1_000,
        ended_at: (status != TaskStatus::Running).then_some(4_000),
        stop_reason: None,
        notification_suppressed: None,
        timeout_ms: None,
    }
}

fn subagent(status: TaskStatus) -> TaskInfo {
    TaskInfo::Subagent(SubagentTaskInfo {
        base: base("agent-1", status),
        tokens: 12,
        session_id: "session-1".to_owned(),
        agent: "read-only".to_owned(),
        alias: "Vega".to_owned(),
    })
}

fn shell(status: TaskStatus, exit_code: Option<i32>) -> TaskInfo {
    TaskInfo::Shell(ShellTaskInfo {
        base: base("shell-1", status),
        command: "cargo test --all".to_owned(),
        pid: 42,
        exit_code,
    })
}

fn line(record: &TaskLifecycleRecord, badge_style: bool) -> String {
    strip_ansi(&task_lifecycle_line(record, &theme(), badge_style))
        .trim()
        .to_owned()
}

#[test]
fn a_background_subagent_gets_separate_start_and_end_lines() {
    let _guard = theme_lock();
    let started = TaskLifecycleRecord::started(subagent(TaskStatus::Running));
    let ended = TaskLifecycleRecord::ended(subagent(TaskStatus::Completed));

    let start_line = line(&started, true);
    let end_line = line(&ended, true);

    assert!(start_line.contains("BG-SUBAGENT"), "{start_line}");
    assert!(start_line.contains("Vega"), "{start_line}");
    assert!(start_line.contains("agent-1 started"), "{start_line}");
    assert!(end_line.contains("BG-SUBAGENT"), "{end_line}");
    assert!(end_line.contains("Vega"), "{end_line}");
    assert!(end_line.contains("agent-1 done"), "{end_line}");
    assert_ne!(start_line, end_line, "the outcome is a new transcript line");
}

#[test]
fn a_background_bash_gets_separate_start_and_end_lines() {
    let _guard = theme_lock();
    let started = TaskLifecycleRecord::started(shell(TaskStatus::Running, None));
    let ended = TaskLifecycleRecord::ended(shell(TaskStatus::Failed, Some(101)));

    let start_line = line(&started, false);
    let end_line = line(&ended, false);

    assert!(start_line.contains("[BG-Bash]"), "{start_line}");
    assert!(start_line.contains("shell-1 started"), "{start_line}");
    assert!(start_line.contains("cargo test --all"), "{start_line}");
    assert!(end_line.contains("[BG-Bash]"), "{end_line}");
    assert!(end_line.contains("shell-1 failed"), "{end_line}");
    assert!(end_line.contains("exit 101"), "{end_line}");
}

#[test]
fn every_background_badge_uses_the_compaction_colour() {
    let _guard = theme_lock();
    let current_theme = theme();
    for (label, record) in [
        (
            "BG-Bash",
            TaskLifecycleRecord::started(shell(TaskStatus::Running, None)),
        ),
        (
            "BG-Subagent",
            TaskLifecycleRecord::ended(subagent(TaskStatus::Completed)),
        ),
    ] {
        let rendered = task_lifecycle_line(&record, &current_theme, true);
        let expected = badge(&current_theme, ThemeBg::CustomMessageBg, label);
        assert!(
            rendered.starts_with(&expected),
            "{label} must use the same badge fill as compaction: {rendered:?}"
        );
    }
}

#[test]
fn a_background_bash_call_uses_only_the_lifecycle_rows() {
    assert!(is_background_bash_call(
        "bash",
        &json!({ "command": "serve", "run_in_background": true })
    ));
    assert!(!is_background_bash_call(
        "bash",
        &json!({ "command": "serve" })
    ));
}

#[test]
fn lifecycle_entries_survive_resume_without_entering_model_context() {
    let record = TaskLifecycleRecord::started(shell(TaskStatus::Running, None));
    let data = serde_json::to_value(&record)
        .unwrap_or_else(|error| panic!("the lifecycle record must be serializable: {error}"));
    let mut manager = SessionManager::in_memory(Some("/workspace"), None)
        .unwrap_or_else(|error| panic!("the in-memory session must open: {error}"));
    manager
        .append_custom_entry(TASK_LIFECYCLE_ENTRY_TYPE, Some(data))
        .unwrap_or_else(|error| panic!("the lifecycle entry must append: {error}"));

    let entries = manager.get_branch(None);
    assert_eq!(entries.len(), 1, "one chat fact was stored: {entries:?}");
    assert_eq!(
        TaskLifecycleRecord::from_session_entry(entries[0]),
        Some(record),
        "resume can rebuild the exact chat line"
    );
    assert!(
        session_entry_to_context_messages(entries[0]).is_empty(),
        "a display record must not become another instruction to the model"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fast_background_terminal_emits_one_ordered_session_pair() {
    let harness = create_harness(HarnessOptions::default());
    let manager = harness
        .session
        .task_manager()
        .unwrap_or_else(|| panic!("the harness session must have a task manager"));
    let task_id = manager
        .register(Arc::new(ImmediateShell), RegisterTaskOptions::default())
        .unwrap_or_else(|error| panic!("the task must register: {error}"));
    let settled = manager.wait(&task_id, 2_000, None).await;
    assert!(settled.is_some(), "the immediate task must settle");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let (stored, emitted) = loop {
        let stored = harness.session.with_session_manager(|session| {
            session
                .get_branch(None)
                .into_iter()
                .filter_map(TaskLifecycleRecord::from_session_entry)
                .collect::<Vec<_>>()
        });
        let emitted = harness
            .events()
            .into_iter()
            .filter_map(|event| match event {
                AgentSessionEvent::EntryAppended { entry } => {
                    TaskLifecycleRecord::from_session_entry(&entry)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if stored.len() == 2 && emitted.len() == 2 {
            break (stored, emitted);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the session never delivered both lifecycle entries; stored={stored:?}, emitted={emitted:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    };

    for (source, records) in [("session", stored), ("events", emitted)] {
        let serialized = serde_json::to_string(&records)
            .unwrap_or_else(|error| panic!("the records must serialize: {error}"));
        assert!(
            !serialized.contains(UNSTREAMED_OUTPUT),
            "{source} must not stream bash output into lifecycle chat rows"
        );
        assert_eq!(
            records
                .iter()
                .map(|record| record.phase)
                .collect::<Vec<_>>(),
            vec![TaskLifecyclePhase::Started, TaskLifecyclePhase::Ended],
            "{source} must contain one immutable start/end pair: {records:?}"
        );
        assert_eq!(
            records[0].task.status(),
            TaskStatus::Running,
            "{source} must capture the start before the task can settle"
        );
        assert!(
            records
                .iter()
                .all(|record| record.task.task_id() == task_id),
            "{source} must keep both lines attached to {task_id}: {records:?}"
        );
    }
}
