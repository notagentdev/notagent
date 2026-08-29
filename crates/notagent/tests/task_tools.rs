use std::sync::{Arc, Mutex};

use notagent::core::tasks::manager::{RegisterTaskOptions, TaskManager, TaskManagerOptions};
use notagent::core::tasks::notification::{
    NotificationOutput, TASK_NOTIFICATION_TYPE, TaskNotificationDelivery, TaskNotificationDetails,
    TaskNotificationHost, TaskNotificationMessage, TaskNotificationSendOptions, TaskNotifier,
    TranscriptNotification, active_task_reminder, render_task_notification,
    task_notification_message,
};
use notagent::core::tasks::store::TaskStore;
use notagent::core::tasks::types::{
    BackgroundTask, ShellTaskInfo, SubagentTaskInfo, TaskInfo, TaskInfoBase, TaskKind,
    TaskSettlement, TaskSettlementStatus, TaskSink, TaskStatus,
};
use notagent::core::tools::task_tools::{
    TaskToolsSources, create_task_list_tool_definition, create_task_output_tool_definition,
    create_task_stop_tool_definition,
};
use notagent::core::tools::tool_definition::ToolDefinition;
use notagent_agent::types::{AgentToolResult, BoxFuture};
use notagent_ai::types::TextOrImageContent;
use serde_json::{Value, json};

fn manager() -> (tempfile::TempDir, TaskManager) {
    let directory = tempfile::Builder::new()
        .prefix("task-tools-")
        .tempdir()
        .expect("temp dir");
    let store = Arc::new(TaskStore::new(directory.path()));
    (
        directory,
        TaskManager::new(store, TaskManagerOptions::default()),
    )
}

fn sources(manager: &TaskManager) -> TaskToolsSources {
    let manager = manager.clone();
    TaskToolsSources {
        manager: Arc::new(move || Some(manager.clone())),
    }
}

/// A shell task that settles at once, so the tools have something to read.
struct Finished {
    description: String,
    output: String,
    exit_code: i32,
}

fn finished(description: &str, output: &str, exit_code: i32) -> Arc<dyn BackgroundTask> {
    Arc::new(Finished {
        description: description.to_owned(),
        output: output.to_owned(),
        exit_code,
    })
}

impl BackgroundTask for Finished {
    fn kind(&self) -> TaskKind {
        TaskKind::Shell
    }

    fn id_prefix(&self) -> &str {
        "bash"
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn start<'a>(&'a self, sink: TaskSink) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if !self.output.is_empty() {
                sink.append_output(&self.output);
            }
            sink.settle(TaskSettlement::new(if self.exit_code == 0 {
                TaskSettlementStatus::Completed
            } else {
                TaskSettlementStatus::Failed
            }))
            .await;
            Ok(())
        })
    }

    fn to_info(&self, base: TaskInfoBase) -> TaskInfo {
        TaskInfo::Shell(ShellTaskInfo {
            base,
            command: "cmd".to_owned(),
            pid: 4242,
            exit_code: Some(self.exit_code),
        })
    }
}

/// A shell task that runs until it is stopped.
struct Running {
    description: String,
}

fn running(description: &str) -> Arc<dyn BackgroundTask> {
    Arc::new(Running {
        description: description.to_owned(),
    })
}

impl BackgroundTask for Running {
    fn kind(&self) -> TaskKind {
        TaskKind::Shell
    }

    fn id_prefix(&self) -> &str {
        "bash"
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn start<'a>(&'a self, sink: TaskSink) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            sink.append_output("starting\n");
            sink.signal.cancelled().await;
            sink.settle(TaskSettlement::new(TaskSettlementStatus::Killed))
                .await;
            Ok(())
        })
    }

    fn to_info(&self, base: TaskInfoBase) -> TaskInfo {
        TaskInfo::Shell(ShellTaskInfo {
            base,
            command: "serve".to_owned(),
            pid: 99,
            exit_code: None,
        })
    }
}

async fn run(definition: &dyn ToolDefinition, input: Value) -> AgentToolResult {
    definition
        .execute("call-1", input, None, None, None)
        .await
        .expect("tool result")
}

fn text_of(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .map(|part| match part {
            TextOrImageContent::Text(text) => text.text.clone(),
            _ => String::new(),
        })
        .collect::<Vec<String>>()
        .join("\n")
}

// ── task_list ─────────────────────────────────────────────────────────

#[tokio::test]
async fn lists_only_running_work_by_default() {
    let (_directory, tasks) = manager();
    let done_id = tasks
        .register(
            finished("the finished one", "", 0),
            RegisterTaskOptions::default(),
        )
        .await
        .expect("registered");
    tasks.wait(&done_id, 2_000, None).await;
    tasks
        .register(running("the running one"), RegisterTaskOptions::default())
        .await
        .expect("registered");

    let listed = text_of(
        &run(
            &create_task_list_tool_definition(Some(sources(&tasks))),
            json!({}),
        )
        .await,
    );
    assert!(listed.contains("the running one"), "{listed}");
    assert!(!listed.contains("the finished one"), "{listed}");
}

#[tokio::test]
async fn includes_finished_work_when_asked() {
    let (_directory, tasks) = manager();
    let done_id = tasks
        .register(
            finished("the finished one", "", 0),
            RegisterTaskOptions::default(),
        )
        .await
        .expect("registered");
    tasks.wait(&done_id, 2_000, None).await;

    let listed = text_of(
        &run(
            &create_task_list_tool_definition(Some(sources(&tasks))),
            json!({ "all": true }),
        )
        .await,
    );
    assert!(listed.contains("the finished one"), "{listed}");
}

#[tokio::test]
async fn says_so_plainly_when_there_is_nothing() {
    let (_directory, tasks) = manager();
    let listed = text_of(
        &run(
            &create_task_list_tool_definition(Some(sources(&tasks))),
            json!({}),
        )
        .await,
    );
    assert!(listed.contains("No matching background tasks"), "{listed}");
}

// ── task_output ───────────────────────────────────────────────────────

#[tokio::test]
async fn reports_a_running_task_as_not_final_with_what_it_has_produced() {
    let (_directory, tasks) = manager();
    let task_id = tasks
        .register(running("a server"), RegisterTaskOptions::default())
        .await
        .expect("registered");
    // The task appends from its own tokio task; the tool reads whatever is
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let result = text_of(
        &run(
            &create_task_output_tool_definition(Some(sources(&tasks))),
            json!({ "task_id": task_id }),
        )
        .await,
    );
    assert!(result.contains("retrieval: in_progress"), "{result}");
    assert!(result.contains("starting"), "{result}");
}

#[tokio::test]
async fn reports_a_settled_task_as_final_with_its_exit_code() {
    let (_directory, tasks) = manager();
    let task_id = tasks
        .register(
            finished("a build", "compiled\n", 2),
            RegisterTaskOptions::default(),
        )
        .await
        .expect("registered");
    tasks.wait(&task_id, 2_000, None).await;
    let result = text_of(
        &run(
            &create_task_output_tool_definition(Some(sources(&tasks))),
            json!({ "task_id": task_id }),
        )
        .await,
    );
    assert!(result.contains("retrieval: final"), "{result}");
    assert!(result.contains("exit_code: 2"), "{result}");
}

#[tokio::test]
async fn names_the_full_log_rather_than_pretending_the_tail_is_everything() {
    let (_directory, tasks) = manager();
    let task_id = tasks
        .register(
            finished("a build", "compiled\n", 0),
            RegisterTaskOptions::default(),
        )
        .await
        .expect("registered");
    tasks.wait(&task_id, 2_000, None).await;
    let result = run(
        &create_task_output_tool_definition(Some(sources(&tasks))),
        json!({ "task_id": task_id }),
    )
    .await;
    assert!(
        result
            .details
            .as_ref()
            .and_then(|details| details.get("outputPath"))
            .and_then(Value::as_str)
            .is_some_and(|path| !path.is_empty())
    );
    assert!(text_of(&result).contains("output_path:"));
}

#[tokio::test]
async fn refuses_an_id_it_does_not_know() {
    let (_directory, tasks) = manager();
    let error = create_task_output_tool_definition(Some(sources(&tasks)))
        .execute(
            "call-1",
            json!({ "task_id": "bash-00000000" }),
            None,
            None,
            None,
        )
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("No background task"),
        "{}",
        error.message
    );
}

// ── task_stop ─────────────────────────────────────────────────────────

#[tokio::test]
async fn stops_a_running_task_and_reports_the_reason() {
    let (_directory, tasks) = manager();
    let task_id = tasks
        .register(running("a server"), RegisterTaskOptions::default())
        .await
        .expect("registered");
    let result = run(
        &create_task_stop_tool_definition(Some(sources(&tasks))),
        json!({ "task_id": task_id, "reason": "port needed" }),
    )
    .await;
    assert!(
        text_of(&result).contains("status: killed"),
        "{}",
        text_of(&result)
    );
    assert!(
        text_of(&result).contains("port needed"),
        "{}",
        text_of(&result)
    );
}

#[tokio::test]
async fn suppresses_the_completion_so_the_model_is_not_told_twice() {
    let (_directory, tasks) = manager();
    let task_id = tasks
        .register(running("a server"), RegisterTaskOptions::default())
        .await
        .expect("registered");
    run(
        &create_task_stop_tool_definition(Some(sources(&tasks))),
        json!({ "task_id": task_id }),
    )
    .await;
    assert_eq!(
        tasks
            .get(&task_id)
            .map(|info| info.base().notification_suppressed),
        Some(Some(true))
    );
}

#[tokio::test]
async fn leaves_an_already_finished_task_alone() {
    let (_directory, tasks) = manager();
    let task_id = tasks
        .register(finished("a build", "", 0), RegisterTaskOptions::default())
        .await
        .expect("registered");
    tasks.wait(&task_id, 2_000, None).await;
    let result = run(
        &create_task_stop_tool_definition(Some(sources(&tasks))),
        json!({ "task_id": task_id }),
    )
    .await;
    assert!(
        text_of(&result).contains("already finished"),
        "{}",
        text_of(&result)
    );
}

// ── the completion note ───────────────────────────────────────────────

fn shell_note(status: TaskStatus) -> TaskInfo {
    TaskInfo::Shell(ShellTaskInfo {
        base: TaskInfoBase {
            task_id: "bash-11112222".to_owned(),
            description: "the test suite".to_owned(),
            status,
            detached: Some(true),
            started_at: 0,
            ended_at: Some(1),
            stop_reason: None,
            notification_suppressed: None,
            timeout_ms: None,
        },
        command: "npm test".to_owned(),
        pid: 1,
        exit_code: Some(0),
    })
}

fn subagent_note(status: TaskStatus) -> TaskInfo {
    TaskInfo::Subagent(SubagentTaskInfo {
        base: TaskInfoBase {
            task_id: "agent-33334444".to_owned(),
            description: "investigate the parser".to_owned(),
            status,
            detached: Some(true),
            started_at: 0,
            ended_at: Some(1),
            stop_reason: None,
            notification_suppressed: None,
            timeout_ms: None,
        },
        tokens: 0,
        session_id: "session-abc".to_owned(),
        agent: "read-only".to_owned(),
        alias: "Vega".to_owned(),
    })
}

#[test]
fn is_not_shown_in_the_transcript_being_written_for_the_model() {
    let message = task_notification_message(&shell_note(TaskStatus::Completed), None);
    assert!(!message.display);
}

#[test]
fn names_the_task_its_kind_and_how_it_ended() {
    let text = render_task_notification(&shell_note(TaskStatus::Completed), None);
    assert!(text.contains("task_id=\"bash-11112222\""), "{text}");
    assert!(text.contains("status=\"completed\""), "{text}");
    assert!(text.contains("the test suite completed."), "{text}");
}

#[test]
fn points_at_the_full_log_when_there_is_one() {
    let text = render_task_notification(
        &shell_note(TaskStatus::Completed),
        Some(&NotificationOutput {
            output_path: Some("/tmp/out.log".to_owned()),
            total_bytes: 900,
            truncated: true,
            preview: "tail".to_owned(),
        }),
    );
    assert!(
        text.contains("<output-file path=\"/tmp/out.log\""),
        "{text}"
    );
    assert!(!text.contains("<output-excerpt"), "{text}");
}

#[test]
fn falls_back_to_an_excerpt_when_no_log_was_kept() {
    let text = render_task_notification(
        &shell_note(TaskStatus::Completed),
        Some(&NotificationOutput {
            output_path: None,
            total_bytes: 4,
            truncated: false,
            preview: "tail".to_owned(),
        }),
    );
    assert!(text.contains("<output-excerpt"), "{text}");
    assert!(text.contains("tail"), "{text}");
}

#[test]
fn tells_a_stopped_subagent_how_to_be_continued() {
    let text = render_task_notification(&subagent_note(TaskStatus::TimedOut), None);
    assert!(text.contains("session_id \"session-abc\""), "{text}");
    assert!(text.contains("not the task id"), "{text}");
}

#[test]
fn says_nothing_about_continuing_a_subagent_that_finished() {
    let text = render_task_notification(&subagent_note(TaskStatus::Completed), None);
    assert!(!text.contains("session_id"), "{text}");
}

// ── delivering a completion ───────────────────────────────────────────

struct Host {
    streaming: bool,
    sent: Mutex<Vec<TaskNotificationSendOptions>>,
    transcript: Mutex<Vec<TranscriptNotification>>,
}

impl TaskNotificationHost for Host {
    fn is_streaming(&self) -> bool {
        self.streaming
    }

    fn send<'a>(
        &'a self,
        message: TaskNotificationMessage,
        options: TaskNotificationSendOptions,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.sent.lock().expect("sent").push(options);
            self.transcript
                .lock()
                .expect("transcript")
                .push(TranscriptNotification {
                    role: "custom".to_owned(),
                    custom_type: Some(message.custom_type),
                    details: Some(message.details),
                });
        })
    }

    fn output<'a>(&'a self, _task_id: &'a str) -> BoxFuture<'a, Option<NotificationOutput>> {
        Box::pin(async move { None })
    }

    fn transcript(&self) -> Vec<TranscriptNotification> {
        self.transcript.lock().expect("transcript").clone()
    }
}

fn host(streaming: bool) -> (Arc<Host>, TaskNotifier) {
    let host = Arc::new(Host {
        streaming,
        sent: Mutex::new(Vec::new()),
        transcript: Mutex::new(Vec::new()),
    });
    let notifier = TaskNotifier::new(Arc::clone(&host) as Arc<dyn TaskNotificationHost>);
    (host, notifier)
}

fn build_info() -> TaskInfo {
    TaskInfo::Shell(ShellTaskInfo {
        base: TaskInfoBase {
            task_id: "bash-55556666".to_owned(),
            description: "a build".to_owned(),
            status: TaskStatus::Completed,
            detached: Some(true),
            started_at: 0,
            ended_at: Some(1),
            stop_reason: None,
            notification_suppressed: None,
            timeout_ms: None,
        },
        command: "make".to_owned(),
        pid: 1,
        exit_code: Some(0),
    })
}

#[tokio::test]
async fn re_opens_a_run_that_is_in_progress_rather_than_waiting_for_the_user() {
    let (host, notifier) = host(true);
    notifier.notify(&build_info()).await;
    assert_eq!(
        *host.sent.lock().expect("sent"),
        vec![TaskNotificationSendOptions {
            deliver_as: Some(TaskNotificationDelivery::FollowUp),
            trigger_turn: false,
        }]
    );
}

#[tokio::test]
async fn starts_a_turn_when_nothing_is_running() {
    let (host, notifier) = host(false);
    notifier.notify(&build_info()).await;
    assert_eq!(
        *host.sent.lock().expect("sent"),
        vec![TaskNotificationSendOptions {
            deliver_as: None,
            trigger_turn: true,
        }]
    );
}

#[tokio::test]
async fn delivers_one_completion_once_however_many_times_it_is_offered() {
    let (host, notifier) = host(false);
    notifier.notify(&build_info()).await;
    notifier.notify(&build_info()).await;
    assert_eq!(host.sent.lock().expect("sent").len(), 1);
}

#[tokio::test]
async fn does_not_repeat_a_completion_already_present_in_the_transcript() {
    let (host, notifier) = host(false);
    host.transcript
        .lock()
        .expect("transcript")
        .push(TranscriptNotification {
            role: "custom".to_owned(),
            custom_type: Some(TASK_NOTIFICATION_TYPE.to_owned()),
            details: Some(TaskNotificationDetails {
                task_id: "bash-55556666".to_owned(),
                status: "completed".to_owned(),
                kind: "shell".to_owned(),
            }),
        });
    notifier.notify(&build_info()).await;
    assert!(host.sent.lock().expect("sent").is_empty());
}

#[tokio::test]
async fn says_nothing_about_a_task_whose_result_was_handed_back_directly() {
    let (host, notifier) = host(false);
    let mut info = build_info();
    info.base_mut().notification_suppressed = Some(true);
    notifier.notify(&info).await;
    assert!(host.sent.lock().expect("sent").is_empty());
}

#[tokio::test]
async fn says_nothing_about_foreground_work() {
    let (host, notifier) = host(false);
    let mut info = build_info();
    info.base_mut().detached = Some(false);
    notifier.notify(&info).await;
    assert!(host.sent.lock().expect("sent").is_empty());
}

// ── the post-compaction reminder ──────────────────────────────────────

#[test]
fn lists_what_is_still_running() {
    let text = active_task_reminder(&[TaskInfo::Shell(ShellTaskInfo {
        base: TaskInfoBase {
            task_id: "bash-77778888".to_owned(),
            description: "the dev server".to_owned(),
            status: TaskStatus::Running,
            detached: Some(true),
            started_at: 0,
            ended_at: None,
            stop_reason: None,
            notification_suppressed: None,
            timeout_ms: None,
        },
        command: "npm run dev".to_owned(),
        pid: 5,
        exit_code: None,
    })])
    .expect("reminder");
    assert!(text.contains("bash-77778888"), "{text}");
    assert!(text.contains("npm run dev"), "{text}");
    assert!(text.contains("Do not start duplicates"), "{text}");
}

#[test]
fn says_nothing_when_nothing_is_running() {
    assert_eq!(active_task_reminder(&[]), None);
}
