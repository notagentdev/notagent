use std::sync::{Arc, Mutex};
use std::time::Duration;

use notagent::core::tasks::manager::{RegisterTaskOptions, TaskManager, TaskManagerOptions};
use notagent::core::tasks::output::OUTPUT_RING_BYTES;
use notagent::core::tasks::store::TaskStore;
use notagent::core::tasks::subagent_task::{SubagentRunResult, SubagentTask, SubagentTaskOptions};
use notagent::core::tasks::types::{
    BackgroundTask, ForegroundRelease, ShellTaskInfo, TaskInfo, TaskInfoBase, TaskKind,
    TaskSettlement, TaskSettlementStatus, TaskSink, TaskStatus,
};
use notagent_agent::types::BoxFuture;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn workspace() -> (tempfile::TempDir, Arc<TaskStore>) {
    let directory = tempfile::Builder::new()
        .prefix("tasks-")
        .tempdir()
        .expect("temp dir");
    let store = Arc::new(TaskStore::new(directory.path()));
    (directory, store)
}

/// A task that does nothing until the test tells it to, so timing is explicit.
struct Controllable {
    sink: Mutex<Option<oneshot::Sender<TaskSink>>>,
    forced: Arc<Mutex<bool>>,
    detached: Arc<Mutex<bool>>,
}

struct Controls {
    task: Arc<Controllable>,
    sink: Mutex<Option<oneshot::Receiver<TaskSink>>>,
}

impl Controls {
    async fn sink(&self) -> TaskSink {
        let receiver = self.sink.lock().expect("sink").take().expect("once");
        receiver.await.expect("started")
    }

    fn force_stopped(&self) -> bool {
        *self.task.forced.lock().expect("forced")
    }

    fn detached(&self) -> bool {
        *self.task.detached.lock().expect("detached")
    }

    fn task(&self) -> Arc<dyn BackgroundTask> {
        Arc::clone(&self.task) as Arc<dyn BackgroundTask>
    }
}

fn controllable() -> Controls {
    let (sender, receiver) = oneshot::channel();
    Controls {
        task: Arc::new(Controllable {
            sink: Mutex::new(Some(sender)),
            forced: Arc::new(Mutex::new(false)),
            detached: Arc::new(Mutex::new(false)),
        }),
        sink: Mutex::new(Some(receiver)),
    }
}

impl BackgroundTask for Controllable {
    fn kind(&self) -> TaskKind {
        TaskKind::Shell
    }

    fn id_prefix(&self) -> &str {
        "bash"
    }

    fn description(&self) -> String {
        "controllable".to_owned()
    }

    fn start<'a>(&'a self, sink: TaskSink) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let Some(sender) = self.sink.lock().expect("sink").take() {
                let _ = sender.send(sink);
            }
            // Never settles on its own: every terminal state in these tests is
            // one the test asked for.
            std::future::pending::<()>().await;
            Ok(())
        })
    }

    fn on_detach(&self) {
        *self.detached.lock().expect("detached") = true;
    }

    fn force_stop<'a>(&'a self) -> Option<BoxFuture<'a, ()>> {
        Some(Box::pin(async move {
            *self.forced.lock().expect("forced") = true;
        }))
    }

    fn to_info(&self, base: TaskInfoBase) -> TaskInfo {
        TaskInfo::Shell(ShellTaskInfo {
            base,
            command: "sleep".to_owned(),
            pid: 1,
            exit_code: None,
        })
    }
}

/// A task that settles as soon as it starts.
struct Immediate {
    status: TaskSettlementStatus,
    output: String,
}

fn immediate(status: TaskSettlementStatus, output: &str) -> Arc<dyn BackgroundTask> {
    Arc::new(Immediate {
        status,
        output: output.to_owned(),
    })
}

impl BackgroundTask for Immediate {
    fn kind(&self) -> TaskKind {
        TaskKind::Shell
    }

    fn id_prefix(&self) -> &str {
        "bash"
    }

    fn description(&self) -> String {
        "immediate".to_owned()
    }

    fn start<'a>(&'a self, sink: TaskSink) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if !self.output.is_empty() {
                sink.append_output(&self.output);
            }
            sink.settle(TaskSettlement::new(self.status)).await;
            Ok(())
        })
    }

    fn to_info(&self, base: TaskInfoBase) -> TaskInfo {
        TaskInfo::Shell(ShellTaskInfo {
            base,
            command: "true".to_owned(),
            pid: 2,
            exit_code: Some(if self.status == TaskSettlementStatus::Completed {
                0
            } else {
                1
            }),
        })
    }
}

/// Work that fails out of `start` instead of settling.
struct Throws;

impl BackgroundTask for Throws {
    fn kind(&self) -> TaskKind {
        TaskKind::Shell
    }

    fn id_prefix(&self) -> &str {
        "bash"
    }

    fn description(&self) -> String {
        "throws".to_owned()
    }

    fn start<'a>(&'a self, _sink: TaskSink) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { Err("spawn failed".to_owned()) })
    }

    fn to_info(&self, base: TaskInfoBase) -> TaskInfo {
        TaskInfo::Shell(ShellTaskInfo {
            base,
            command: "x".to_owned(),
            pid: 0,
            exit_code: None,
        })
    }
}

/// Settles as killed the moment the signal fires, as ordinary work does.
fn settle_on_abort(sink: TaskSink) {
    tokio::spawn(async move {
        sink.signal.cancelled().await;
        sink.settle(TaskSettlement::new(TaskSettlementStatus::Killed))
            .await;
    });
}

// ── registering work ──────────────────────────────────────────────────

#[tokio::test]
async fn announces_detached_work_as_started_and_lists_it() {
    let (_directory, store) = workspace();
    let started: Arc<Mutex<Vec<TaskInfo>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&started);
    let manager = TaskManager::new(
        store,
        TaskManagerOptions {
            on_started: Some(Arc::new(move |info| {
                recorder.lock().expect("started").push(info)
            })),
            ..TaskManagerOptions::default()
        },
    );
    let task_id = manager
        .register(controllable().task(), RegisterTaskOptions::default())
        .await
        .expect("registered");

    assert_eq!(
        started
            .lock()
            .expect("started")
            .iter()
            .map(|info| info.task_id().to_owned())
            .collect::<Vec<String>>(),
        vec![task_id.clone()]
    );
    assert_eq!(
        manager
            .list(true, None)
            .iter()
            .map(|info| info.task_id().to_owned())
            .collect::<Vec<String>>(),
        vec![task_id.clone()]
    );
    assert_eq!(
        manager.get(&task_id).map(|info| info.is_detached()),
        Some(true)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn announces_a_foreground_subagent_at_both_ends_of_its_life() {
    let (_directory, store) = workspace();
    let started: Arc<Mutex<Vec<TaskInfo>>> = Arc::new(Mutex::new(Vec::new()));
    let terminated: Arc<Mutex<Vec<TaskInfo>>> = Arc::new(Mutex::new(Vec::new()));
    let start_recorder = Arc::clone(&started);
    let end_recorder = Arc::clone(&terminated);
    let manager = TaskManager::new(
        store,
        TaskManagerOptions {
            on_started: Some(Arc::new(move |info| {
                start_recorder.lock().expect("started").push(info)
            })),
            on_terminated: Some(Arc::new(move |info, _tail| {
                end_recorder.lock().expect("terminated").push(info)
            })),
            ..TaskManagerOptions::default()
        },
    );

    let (sender, receiver) = oneshot::channel::<SubagentRunResult>();
    let subagent: Arc<dyn BackgroundTask> = Arc::new(SubagentTask::new(SubagentTaskOptions {
        description: "investigate".to_owned(),
        tokens: None,
        session_id: "session-1".to_owned(),
        agent: "read-only".to_owned(),
        alias: "Vega".to_owned(),
        run: receiver,
        cancel: Arc::new(|| {}),
    }));
    let task_id = manager
        .register(
            subagent,
            RegisterTaskOptions {
                detached: false,
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");

    assert_eq!(
        started
            .lock()
            .expect("started")
            .iter()
            .map(|info| (info.task_id().to_owned(), info.is_detached()))
            .collect::<Vec<_>>(),
        vec![(task_id.clone(), false)],
        "a foreground subagent announces its start, unlike a foreground shell"
    );

    let _ = sender.send(SubagentRunResult {
        text: "answer".to_owned(),
        failed: false,
    });
    assert!(
        manager.wait(&task_id, 2_000, None).await.is_some(),
        "the subagent must settle"
    );
    assert_eq!(
        terminated
            .lock()
            .expect("terminated")
            .iter()
            .map(|info| info.task_id().to_owned())
            .collect::<Vec<_>>(),
        vec![task_id],
        "its end is announced even though it never detached"
    );
}

#[tokio::test]
async fn does_not_announce_foreground_work_which_its_tool_call_already_reports() {
    let (_directory, store) = workspace();
    let started: Arc<Mutex<Vec<TaskInfo>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&started);
    let manager = TaskManager::new(
        store,
        TaskManagerOptions {
            on_started: Some(Arc::new(move |info| {
                recorder.lock().expect("started").push(info)
            })),
            ..TaskManagerOptions::default()
        },
    );
    manager
        .register(
            controllable().task(),
            RegisterTaskOptions {
                detached: false,
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");
    assert!(started.lock().expect("started").is_empty());
}

#[tokio::test]
async fn registers_foreground_work_once_even_when_it_later_detaches() {
    let (_directory, store) = workspace();
    let registered: Arc<Mutex<Vec<TaskInfo>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&registered);
    let manager = TaskManager::new(
        store,
        TaskManagerOptions {
            on_registered: Some(Arc::new(move |info| {
                let recorder = Arc::clone(&recorder);
                Box::pin(async move {
                    recorder.lock().expect("registered").push(info);
                })
            })),
            ..TaskManagerOptions::default()
        },
    );
    let task_id = manager
        .register(
            controllable().task(),
            RegisterTaskOptions {
                detached: false,
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");
    tokio::task::yield_now().await;
    manager.detach(&task_id).expect("detached");

    let registered = registered.lock().expect("registered");
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0].task_id(), task_id);
    assert!(!registered[0].is_detached());
}

#[tokio::test]
async fn refuses_to_exceed_a_running_cap_naming_the_numbers() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(
        store,
        TaskManagerOptions {
            max_running_tasks: Some(Arc::new(|| Some(1))),
            ..TaskManagerOptions::default()
        },
    );
    manager
        .register(controllable().task(), RegisterTaskOptions::default())
        .await
        .expect("registered");
    let error = manager
        .register(controllable().task(), RegisterTaskOptions::default())
        .await
        .expect_err("refused");
    assert_eq!(error.running, 1);
    assert_eq!(error.max, 1);
    assert!(
        error
            .to_string()
            .contains("Too many background tasks are already running: 1 of 1."),
        "{error}"
    );
}

#[tokio::test]
async fn does_not_count_foreground_work_against_the_cap() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(
        store,
        TaskManagerOptions {
            max_running_tasks: Some(Arc::new(|| Some(1))),
            ..TaskManagerOptions::default()
        },
    );
    manager
        .register(controllable().task(), RegisterTaskOptions::default())
        .await
        .expect("registered");
    assert!(
        manager
            .register(
                controllable().task(),
                RegisterTaskOptions {
                    detached: false,
                    ..RegisterTaskOptions::default()
                },
            )
            .await
            .is_ok()
    );
}

// ── settling ──────────────────────────────────────────────────────────

#[tokio::test]
async fn reports_the_terminal_status_with_a_tail_of_what_was_produced() {
    let (_directory, store) = workspace();
    type Terminal = Arc<Mutex<Vec<(TaskInfo, Option<String>)>>>;
    let terminal: Terminal = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&terminal);
    let manager = TaskManager::new(
        store,
        TaskManagerOptions {
            on_terminated: Some(Arc::new(move |info, tail| {
                recorder.lock().expect("terminal").push((info, tail))
            })),
            ..TaskManagerOptions::default()
        },
    );
    let task_id = manager
        .register(
            immediate(TaskSettlementStatus::Completed, "hello"),
            RegisterTaskOptions::default(),
        )
        .await
        .expect("registered");
    manager.wait(&task_id, 2_000, None).await;

    let terminal = terminal.lock().expect("terminal");
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].0.status(), TaskStatus::Completed);
    assert_eq!(terminal[0].1.as_deref(), Some("hello"));
}

#[tokio::test]
async fn settles_work_that_threw_instead_of_reporting() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let task_id = manager
        .register(Arc::new(Throws), RegisterTaskOptions::default())
        .await
        .expect("registered");
    let info = manager.wait(&task_id, 2_000, None).await.expect("info");
    assert_eq!(info.status(), TaskStatus::Failed);
    assert!(
        info.stop_reason()
            .unwrap_or_default()
            .contains("spawn failed"),
        "{:?}",
        info.stop_reason()
    );
}

#[tokio::test]
async fn leaves_finished_background_work_out_of_the_active_list() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let task_id = manager
        .register(
            immediate(TaskSettlementStatus::Completed, ""),
            RegisterTaskOptions::default(),
        )
        .await
        .expect("registered");
    manager.wait(&task_id, 2_000, None).await;

    assert!(manager.list(true, None).is_empty());
    assert_eq!(
        manager
            .list(false, None)
            .iter()
            .map(|info| info.task_id().to_owned())
            .collect::<Vec<String>>(),
        vec![task_id]
    );
}

#[tokio::test]
async fn keeps_finished_foreground_work_out_of_both_lists() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let task_id = manager
        .register(
            immediate(TaskSettlementStatus::Completed, ""),
            RegisterTaskOptions {
                detached: false,
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");
    manager.wait_for_foreground_release(&task_id).await;

    assert!(manager.list(true, None).is_empty());
    assert!(manager.list(false, None).is_empty());
}

// ── stopping ──────────────────────────────────────────────────────────

#[tokio::test]
async fn signals_first_and_settles_as_killed_when_the_work_responds() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let controls = controllable();
    let task_id = manager
        .register(controls.task(), RegisterTaskOptions::default())
        .await
        .expect("registered");
    settle_on_abort(controls.sink().await);

    let info = manager
        .stop(&task_id, Some("no longer needed"))
        .await
        .expect("info");
    assert_eq!(info.status(), TaskStatus::Killed);
    assert_eq!(info.stop_reason(), Some("no longer needed"));
    assert!(!controls.force_stopped());
}

#[tokio::test(flavor = "multi_thread")]
async fn forces_work_that_ignores_the_signal_once_the_grace_window_has_passed() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let controls = controllable();
    let task_id = manager
        .register(controls.task(), RegisterTaskOptions::default())
        .await
        .expect("registered");
    controls.sink().await;

    // The grace window is real time; wait it out rather than faking it, so the
    // ordering under test is the one that runs in production.
    let info = manager.stop(&task_id, Some("stuck")).await.expect("info");

    assert_eq!(info.status(), TaskStatus::Killed);
    assert!(controls.force_stopped());
}

#[tokio::test]
async fn treats_a_stop_after_the_deadline_fired_as_a_timeout() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let controls = controllable();
    let task_id = manager
        .register(
            controls.task(),
            RegisterTaskOptions {
                timeout_ms: Some(40),
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");
    settle_on_abort(controls.sink().await);

    let info = manager.wait(&task_id, 5_000, None).await.expect("info");
    assert_eq!(info.status(), TaskStatus::TimedOut);
}

#[tokio::test]
async fn reports_an_unknown_id_as_absent_rather_than_failing() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    assert!(manager.stop("bash-00000000", None).await.is_none());
}

// ── moving work to the background ─────────────────────────────────────

#[tokio::test]
async fn releases_the_waiting_tool_call_and_tells_the_task_it_was_detached() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let controls = controllable();
    let task_id = manager
        .register(
            controls.task(),
            RegisterTaskOptions {
                detached: false,
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");
    controls.sink().await;

    let waiting = {
        let manager = manager.clone();
        let task_id = task_id.clone();
        tokio::spawn(async move { manager.wait_for_foreground_release(&task_id).await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    manager.detach(&task_id);

    assert_eq!(
        waiting.await.expect("joined"),
        Some(ForegroundRelease::Detached)
    );
    assert!(controls.detached());
    assert_eq!(
        manager.get(&task_id).map(|info| info.is_detached()),
        Some(true)
    );
}

#[tokio::test]
async fn detaches_on_the_deadline_instead_of_stopping_when_that_was_asked_for() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let controls = controllable();
    let task_id = manager
        .register(
            controls.task(),
            RegisterTaskOptions {
                detached: false,
                timeout_ms: Some(40),
                detach_timeout_ms: Some(60_000),
                auto_background_on_timeout: true,
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");
    controls.sink().await;

    assert_eq!(
        manager.wait_for_foreground_release(&task_id).await,
        Some(ForegroundRelease::TimeoutDetached)
    );
    assert_eq!(
        manager.get(&task_id).map(|info| info.status()),
        Some(TaskStatus::Running)
    );
}

#[tokio::test]
async fn stops_on_the_deadline_when_backgrounding_was_not_asked_for() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let controls = controllable();
    let task_id = manager
        .register(
            controls.task(),
            RegisterTaskOptions {
                detached: false,
                timeout_ms: Some(40),
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");
    settle_on_abort(controls.sink().await);

    assert_eq!(
        manager.wait_for_foreground_release(&task_id).await,
        Some(ForegroundRelease::Terminal)
    );
    assert_eq!(
        manager.get(&task_id).map(|info| info.status()),
        Some(TaskStatus::TimedOut)
    );
}

#[tokio::test]
async fn stops_foreground_work_when_the_turn_waiting_on_it_is_interrupted() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let controls = controllable();
    let turn = CancellationToken::new();
    let task_id = manager
        .register(
            controls.task(),
            RegisterTaskOptions {
                detached: false,
                signal: Some(turn.clone()),
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");
    settle_on_abort(controls.sink().await);

    turn.cancel();
    let info = manager.wait(&task_id, 5_000, None).await.expect("info");
    assert_eq!(info.status(), TaskStatus::Killed);
}

#[tokio::test]
async fn leaves_detached_work_alone_when_the_turn_that_started_it_ends() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let controls = controllable();
    let turn = CancellationToken::new();
    let task_id = manager
        .register(
            controls.task(),
            RegisterTaskOptions {
                detached: false,
                signal: Some(turn.clone()),
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");
    controls.sink().await;
    manager.detach(&task_id);

    turn.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        manager.get(&task_id).map(|info| info.status()),
        Some(TaskStatus::Running)
    );
}

// ── output ────────────────────────────────────────────────────────────

#[tokio::test]
async fn returns_the_complete_log_for_detached_work_and_says_it_is_complete() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(Arc::clone(&store), TaskManagerOptions::default());
    let task_id = manager
        .register(
            immediate(TaskSettlementStatus::Completed, "line one\nline two\n"),
            RegisterTaskOptions::default(),
        )
        .await
        .expect("registered");
    manager.wait(&task_id, 2_000, None).await;

    let snapshot = manager.output_snapshot(&task_id, 1024).await;
    assert!(snapshot.full_output_available);
    assert_eq!(snapshot.preview, "line one\nline two\n");
    assert_eq!(
        std::fs::read_to_string(store.log_path(&task_id).expect("path")).expect("log"),
        "line one\nline two\n"
    );
}

#[tokio::test]
async fn writes_no_log_for_foreground_work_that_stayed_small() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(Arc::clone(&store), TaskManagerOptions::default());
    let task_id = manager
        .register(
            immediate(TaskSettlementStatus::Completed, "small"),
            RegisterTaskOptions {
                detached: false,
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");
    manager.wait_for_foreground_release(&task_id).await;

    assert!(!store.log_exists(&task_id));
}

#[tokio::test(flavor = "multi_thread")]
async fn spills_the_whole_stream_to_disk_once_foreground_output_outgrows_the_buffer() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(Arc::clone(&store), TaskManagerOptions::default());
    let controls = controllable();
    let task_id = manager
        .register(
            controls.task(),
            RegisterTaskOptions {
                detached: false,
                ..RegisterTaskOptions::default()
            },
        )
        .await
        .expect("registered");
    let sink = controls.sink().await;

    sink.append_output("HEAD");
    sink.append_output(&"x".repeat(OUTPUT_RING_BYTES + 1));
    manager.stop(&task_id, Some("enough")).await;

    let written = std::fs::read_to_string(store.log_path(&task_id).expect("path")).expect("log");
    assert!(written.starts_with("HEAD"));
    assert!(written.len() > OUTPUT_RING_BYTES);
}

#[tokio::test]
async fn reports_a_truncated_preview_as_truncated() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let task_id = manager
        .register(
            immediate(TaskSettlementStatus::Completed, "abcdefghij"),
            RegisterTaskOptions::default(),
        )
        .await
        .expect("registered");
    manager.wait(&task_id, 2_000, None).await;

    let snapshot = manager.output_snapshot(&task_id, 4).await;
    assert!(snapshot.truncated);
    assert_eq!(snapshot.preview, "ghij");
    assert_eq!(snapshot.total_bytes, 10);
}

// ── across a restart ──────────────────────────────────────────────────

#[tokio::test]
async fn reports_work_the_previous_process_left_running_as_lost() {
    let (_directory, store) = workspace();
    let first = TaskManager::new(Arc::clone(&store), TaskManagerOptions::default());
    let task_id = first
        .register(controllable().task(), RegisterTaskOptions::default())
        .await
        .expect("registered");
    tokio::time::sleep(Duration::from_millis(20)).await;

    let second = TaskManager::new(
        Arc::new(TaskStore::new(store.directory())),
        TaskManagerOptions::default(),
    );
    let lost = second.reconcile().await;

    assert_eq!(
        lost.iter()
            .map(|info| info.task_id().to_owned())
            .collect::<Vec<String>>(),
        vec![task_id.clone()]
    );
    assert_eq!(
        second.get(&task_id).map(|info| info.status()),
        Some(TaskStatus::Lost)
    );
}

#[tokio::test]
async fn keeps_a_task_that_had_already_settled_at_the_status_it_settled_with() {
    let (_directory, store) = workspace();
    let first = TaskManager::new(Arc::clone(&store), TaskManagerOptions::default());
    let task_id = first
        .register(
            immediate(TaskSettlementStatus::Failed, ""),
            RegisterTaskOptions::default(),
        )
        .await
        .expect("registered");
    first.wait(&task_id, 2_000, None).await;

    let second = TaskManager::new(
        Arc::new(TaskStore::new(store.directory())),
        TaskManagerOptions::default(),
    );
    second.reconcile().await;
    assert_eq!(
        second.get(&task_id).map(|info| info.status()),
        Some(TaskStatus::Failed)
    );
}

#[tokio::test]
async fn writes_the_record_while_the_task_is_alive_not_only_when_it_ends() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(Arc::clone(&store), TaskManagerOptions::default());
    let task_id = manager
        .register(controllable().task(), RegisterTaskOptions::default())
        .await
        .expect("registered");
    tokio::time::sleep(Duration::from_millis(20)).await;

    let record = store.read_record(&task_id).await.expect("record");
    assert_eq!(record.status(), TaskStatus::Running);
}

// ── session shutdown ──────────────────────────────────────────────────

#[tokio::test]
async fn stops_everything_and_suppresses_the_notes_nobody_is_left_to_read() {
    let (_directory, store) = workspace();
    let terminal: Arc<Mutex<Vec<TaskInfo>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&terminal);
    let manager = TaskManager::new(
        store,
        TaskManagerOptions {
            on_terminated: Some(Arc::new(move |info, _tail| {
                recorder.lock().expect("terminal").push(info)
            })),
            ..TaskManagerOptions::default()
        },
    );
    let controls = controllable();
    let task_id = manager
        .register(controls.task(), RegisterTaskOptions::default())
        .await
        .expect("registered");
    settle_on_abort(controls.sink().await);

    manager.shutdown(None).await;
    assert_eq!(
        manager.get(&task_id).map(|info| info.status()),
        Some(TaskStatus::Killed)
    );
    assert_eq!(
        manager
            .get(&task_id)
            .map(|info| info.base().notification_suppressed),
        Some(Some(true))
    );
    // The listener still fires; suppression is read by the delivery layer,
    // which is what must not announce it.
    assert_eq!(
        terminal
            .lock()
            .expect("terminal")
            .iter()
            .map(|info| info.base().notification_suppressed)
            .collect::<Vec<Option<bool>>>(),
        vec![Some(true)]
    );
}

async fn wait_until_released(task: &std::sync::Weak<dyn BackgroundTask>) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while task.strong_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("finished tasks must release their runtime objects");
}

#[tokio::test]
async fn finished_tasks_release_their_runtime_and_keep_output_and_wait_semantics() {
    for detached in [false, true] {
        let (_directory, store) = workspace();
        let manager = TaskManager::new(Arc::clone(&store), TaskManagerOptions::default());
        let task = immediate(TaskSettlementStatus::Completed, "full answer");
        let weak = Arc::downgrade(&task);
        let id = manager
            .register(
                task,
                RegisterTaskOptions {
                    detached,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        manager.wait(&id, 2000, None).await;
        wait_until_released(&weak).await;
        assert_eq!(manager.get(&id).unwrap().status(), TaskStatus::Completed);
        assert_eq!(
            manager.wait_for_foreground_release(&id).await,
            Some(ForegroundRelease::Terminal)
        );
        assert_eq!(
            manager.wait(&id, 2000, None).await.unwrap().status(),
            TaskStatus::Completed
        );
        assert_eq!(
            manager.stop(&id, None).await.unwrap().status(),
            TaskStatus::Completed
        );
        assert_eq!(manager.detach(&id).unwrap().status(), TaskStatus::Completed);
        let snapshot = manager.output_snapshot(&id, 6).await;
        assert_eq!(snapshot.preview, "answer");
        assert_eq!(snapshot.total_bytes, 11);
        assert!(snapshot.truncated);
        assert_eq!(snapshot.full_output_available, detached);
        assert_eq!(store.log_exists(&id), detached);
        assert_eq!(manager.read_output(&id, None).await, "full answer");
        assert_eq!(
            manager.read_output(&id, Some(6)).await,
            "answer",
            "a tail is the last bytes of the output, from the log or from memory"
        );
        manager.suppress_notification(&id).await;
        assert_eq!(
            manager.get(&id).unwrap().base().notification_suppressed,
            Some(true)
        );
    }
}

#[tokio::test]
async fn retirement_keeps_registration_order_and_does_not_duplicate_reconciled_tasks() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let mut ids = Vec::new();
    for _ in 0..8 {
        let task = immediate(TaskSettlementStatus::Completed, "done");
        let weak = Arc::downgrade(&task);
        let id = manager
            .register(task, RegisterTaskOptions::default())
            .await
            .unwrap();
        wait_until_released(&weak).await;
        ids.push(id);
    }
    manager.reconcile().await;
    assert_eq!(
        manager
            .list(false, None)
            .iter()
            .map(|info| info.task_id().to_owned())
            .collect::<Vec<_>>(),
        ids
    );
    assert!(manager.list(true, None).is_empty());
}

#[tokio::test]
async fn an_archive_failure_keeps_the_runtime_output_readable() {
    let directory = tempfile::tempdir().unwrap();
    let blocked = directory.path().join("blocked");
    std::fs::write(&blocked, "not a directory").unwrap();
    let manager = TaskManager::new(
        Arc::new(TaskStore::new(blocked)),
        TaskManagerOptions::default(),
    );
    let task = immediate(TaskSettlementStatus::Completed, "keep this answer");
    let weak = Arc::downgrade(&task);
    let id = manager
        .register(
            task,
            RegisterTaskOptions {
                detached: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    manager.wait_for_foreground_release(&id).await;
    assert_eq!(manager.read_output(&id, None).await, "keep this answer");
    assert!(
        weak.upgrade().is_some(),
        "failed storage must not discard the only output copy"
    );
}

struct SettledButBusy {
    settled: tokio_util::sync::CancellationToken,
    finish: tokio_util::sync::CancellationToken,
}

impl BackgroundTask for SettledButBusy {
    fn kind(&self) -> TaskKind {
        TaskKind::Shell
    }
    fn id_prefix(&self) -> &str {
        "bash"
    }
    fn description(&self) -> String {
        "settled but unwinding".to_owned()
    }
    fn to_info(&self, base: TaskInfoBase) -> TaskInfo {
        TaskInfo::Shell(ShellTaskInfo {
            base,
            command: "true".to_owned(),
            pid: 3,
            exit_code: Some(0),
        })
    }
    fn start<'a>(&'a self, sink: TaskSink) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            sink.append_output("final");
            sink.settle(TaskSettlement::new(TaskSettlementStatus::Completed))
                .await;
            sink.append_output("late output must not change a terminal result");
            self.settled.cancel();
            self.finish.cancelled().await;
            Ok(())
        })
    }
}

#[tokio::test]
async fn a_terminal_status_does_not_retire_work_that_has_not_returned() {
    let (_directory, store) = workspace();
    let manager = TaskManager::new(store, TaskManagerOptions::default());
    let settled = CancellationToken::new();
    let finish = CancellationToken::new();
    let task: Arc<dyn BackgroundTask> = Arc::new(SettledButBusy {
        settled: settled.clone(),
        finish: finish.clone(),
    });
    let weak = Arc::downgrade(&task);
    let id = manager
        .register(task, RegisterTaskOptions::default())
        .await
        .unwrap();
    settled.cancelled().await;
    assert!(
        weak.upgrade().is_some(),
        "the still-running future owns its resources"
    );
    assert_eq!(manager.read_output(&id, None).await, "final");
    finish.cancel();
    wait_until_released(&weak).await;
    assert_eq!(manager.read_output(&id, None).await, "final");
}

#[tokio::test]
async fn cancellation_before_start_releases_the_unused_task() {
    let (_directory, store) = workspace();
    let owner = Arc::new(Mutex::new(None::<TaskManager>));
    let callback_owner = Arc::clone(&owner);
    let manager = TaskManager::new(
        store,
        TaskManagerOptions {
            on_registered: Some(Arc::new(move |info| {
                let manager = callback_owner.lock().unwrap().as_ref().unwrap().clone();
                Box::pin(async move {
                    manager.stop(info.task_id(), None).await;
                })
            })),
            ..Default::default()
        },
    );
    *owner.lock().unwrap() = Some(manager.clone());
    let task = immediate(TaskSettlementStatus::Completed, "must never run");
    let weak = Arc::downgrade(&task);
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        manager.register(task, RegisterTaskOptions::default()),
    )
    .await;
    owner.lock().unwrap().take();
    let id = result.unwrap().unwrap();
    wait_until_released(&weak).await;
    assert_eq!(manager.get(&id).unwrap().status(), TaskStatus::Killed);
    assert_eq!(manager.read_output(&id, None).await, "");
}
