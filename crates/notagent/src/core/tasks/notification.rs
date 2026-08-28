use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use notagent_agent::types::BoxFuture;

use crate::core::tasks::types::{TaskInfo, TaskStatus, is_terminal_task_status};

/// Marks the messages this module creates, in the transcript and in the UI.
pub const TASK_NOTIFICATION_TYPE: &str = "task_notification";

/// Bytes of output carried in a note when no complete log exists.
pub const NOTIFICATION_PREVIEW_BYTES: usize = 3_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskNotificationDetails {
    pub task_id: String,
    pub status: String,
    pub kind: String,
}

/// Identity of one announcement. A task announces once per terminal status.
pub fn notification_key(task_id: &str, status: &str) -> String {
    format!("{task_id} {status}")
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The sentence a completion leads with.
/// A plain shell failure carries no reason — its exit code is the reason — so
/// nothing is invented for it; the code is in the record the model can read.
fn summarise(info: &TaskInfo) -> String {
    let description = info.description();
    match info.status() {
        TaskStatus::TimedOut => {
            return format!("{description} reached its deadline and was stopped.");
        }
        TaskStatus::Lost => {
            return format!(
                "{description} was still running when the previous session ended, and is gone."
            );
        }
        TaskStatus::Killed => {
            return match info.stop_reason() {
                Some(reason) => format!("{description} was stopped: {reason}"),
                None => format!("{description} was stopped."),
            };
        }
        _ => {}
    }
    if info.status() == TaskStatus::Failed
        && let Some(reason) = info.stop_reason()
    {
        return format!("{description} failed: {reason}");
    }
    if info.status() == TaskStatus::Failed
        && let TaskInfo::Shell(shell) = info
        && let Some(exit_code) = shell.exit_code
    {
        return format!("{description} failed with exit code {exit_code}.");
    }
    format!("{description} {}.", info.status())
}

/// What to do about a subagent that did not run to completion.
/// The two identifiers are spelled out because they look alike and only one of
/// them works — a model that passes the task id back gets an unknown-subagent
/// error and usually responds by starting the work over.
fn continuation_hint(info: &TaskInfo) -> Option<String> {
    let TaskInfo::Subagent(subagent) = info else {
        return None;
    };
    if info.status() == TaskStatus::Completed {
        return None;
    }
    Some(
        [
            format!(
                "To continue this subagent instead of starting over, delegate again with session_id \"{}\" and agent \"{}\".",
                subagent.session_id, subagent.agent
            ),
            format!(
                "That is the subagent's own id, not the task id \"{}\" — only the former is accepted.",
                info.task_id()
            ),
            "It keeps everything it had already learned; a tool call that was in flight may need repeating."
                .to_owned(),
        ]
        .join(" "),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NotificationOutput {
    /// Path to the complete log, when one exists.
    pub output_path: Option<String>,
    pub total_bytes: u64,
    pub truncated: bool,
    pub preview: String,
}

/// Renders the block the model reads.
pub fn render_task_notification(info: &TaskInfo, output: Option<&NotificationOutput>) -> String {
    let mut lines = vec![
        format!(
            "<task-finished task_id=\"{}\" kind=\"{}\" status=\"{}\">",
            escape_attribute(info.task_id()),
            info.kind(),
            info.status()
        ),
        summarise(info),
    ];
    if let Some(hint) = continuation_hint(info) {
        lines.push(hint);
    }

    if let Some(output) = output {
        if let Some(output_path) = &output.output_path {
            lines.push(format!(
                "<output-file path=\"{}\" bytes=\"{}\">",
                escape_attribute(output_path),
                output.total_bytes
            ));
            lines.push("Read this file for the complete output.".to_owned());
            lines.push("</output-file>".to_owned());
        } else if !output.preview.is_empty() {
            lines.push(format!(
                "<output-excerpt bytes=\"{}\" truncated=\"{}\">",
                output.total_bytes, output.truncated
            ));
            lines.push(output.preview.clone());
            lines.push("</output-excerpt>".to_owned());
        }
    }
    lines.push("</task-finished>".to_owned());
    lines.join("\n")
}

/// The message that carries a completion into the conversation.
/// building a half-initialised `CustomMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskNotificationMessage {
    pub custom_type: String,
    pub content: String,
    /// Hidden. The block is written for the model — machine-readable tags, a log
    /// path, a resume instruction — and printing it verbatim puts markup in
    /// front of the user that was never addressed to them. What finished is
    /// visible in the task panel instead.
    pub display: bool,
    pub details: TaskNotificationDetails,
}

pub fn task_notification_message(
    info: &TaskInfo,
    output: Option<&NotificationOutput>,
) -> TaskNotificationMessage {
    TaskNotificationMessage {
        custom_type: TASK_NOTIFICATION_TYPE.to_owned(),
        content: render_task_notification(info, output),
        display: false,
        details: TaskNotificationDetails {
            task_id: info.task_id().to_owned(),
            status: info.status().as_str().to_owned(),
            kind: info.kind().as_str().to_owned(),
        },
    }
}

/// How a note is routed into the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TaskNotificationSendOptions {
    pub deliver_as: Option<TaskNotificationDelivery>,
    pub trigger_turn: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskNotificationDelivery {
    Steer,
    FollowUp,
}

/// One transcript message, as much of it as the notifier reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptNotification {
    pub role: String,
    pub custom_type: Option<String>,
    pub details: Option<TaskNotificationDetails>,
}

/// What the notifier needs from the session it announces into.
pub trait TaskNotificationHost: Send + Sync {
    /// Whether a run is in progress right now.
    fn is_streaming(&self) -> bool;
    /// Delivers a message, choosing its own route from the streaming state.
    fn send<'a>(
        &'a self,
        message: TaskNotificationMessage,
        options: TaskNotificationSendOptions,
    ) -> BoxFuture<'a, ()>;
    /// Reads the output a note should carry.
    fn output<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Option<NotificationOutput>>;
    /// The conversation so far, used to tell what has already been announced.
    fn transcript(&self) -> Vec<TranscriptNotification>;
}

/// Announces settled tasks, exactly once each.
/// The delivered set is the fast path; the transcript scan is the correct one.
/// Both are needed: the set is empty after a restart, and the transcript is the
/// only record that survives one.
pub struct TaskNotifier {
    host: Arc<dyn TaskNotificationHost>,
    delivered: Mutex<HashSet<String>>,
    in_flight: Mutex<HashSet<String>>,
    /// Deviation (class 1): the JS promise chain that serialises delivery
    /// becomes a fair async mutex — two tasks finishing at once would otherwise
    /// both read the streaming state before either had delivered, and both
    /// would start a turn.
    queue: tokio::sync::Mutex<()>,
}

impl TaskNotifier {
    pub fn new(host: Arc<dyn TaskNotificationHost>) -> Self {
        TaskNotifier {
            host,
            delivered: Mutex::new(HashSet::new()),
            in_flight: Mutex::new(HashSet::new()),
            queue: tokio::sync::Mutex::new(()),
        }
    }

    /// Records an announcement that reached the conversation by another route.
    pub fn mark_delivered(&self, task_id: &str, status: &str) {
        self.delivered
            .lock()
            .expect("poisoned")
            .insert(notification_key(task_id, status));
    }

    /// Announces a settled task.
    pub async fn notify(&self, info: &TaskInfo) {
        let _guard = self.queue.lock().await;
        self.deliver(info).await;
    }

    async fn deliver(&self, info: &TaskInfo) {
        if !is_terminal_task_status(info.status()) {
            return;
        }
        // A task whose result was handed back directly — stopped from a tool
        // call, or caught by session shutdown — has already been reported.
        if info.base().notification_suppressed == Some(true) {
            return;
        }
        if !info.is_detached() {
            return;
        }

        let status = info.status().as_str();
        let key = notification_key(info.task_id(), status);
        if self.delivered.lock().expect("poisoned").contains(&key)
            || self.in_flight.lock().expect("poisoned").contains(&key)
        {
            return;
        }
        if self.already_in_transcript(info.task_id(), status) {
            self.delivered.lock().expect("poisoned").insert(key);
            return;
        }

        self.in_flight.lock().expect("poisoned").insert(key.clone());
        let output = self.host.output(info.task_id()).await;
        // Reading the output is asynchronous, and a restore pass may have
        // delivered this very note while it was in flight.
        if self.delivered.lock().expect("poisoned").contains(&key)
            || self.already_in_transcript(info.task_id(), status)
        {
            self.in_flight.lock().expect("poisoned").remove(&key);
            return;
        }
        let message = task_notification_message(info, output.as_ref());
        if self.host.is_streaming() {
            // Re-opens the run at the point it would otherwise stop, so the news
            // is acted on inside the run rather than waiting for the next prompt.
            self.host
                .send(
                    message,
                    TaskNotificationSendOptions {
                        deliver_as: Some(TaskNotificationDelivery::FollowUp),
                        trigger_turn: false,
                    },
                )
                .await;
        } else {
            self.host
                .send(
                    message,
                    TaskNotificationSendOptions {
                        deliver_as: None,
                        trigger_turn: true,
                    },
                )
                .await;
        }
        self.delivered.lock().expect("poisoned").insert(key.clone());
        self.in_flight.lock().expect("poisoned").remove(&key);
    }

    fn already_in_transcript(&self, task_id: &str, status: &str) -> bool {
        self.host.transcript().iter().any(|message| {
            if message.role != "custom"
                || message.custom_type.as_deref() != Some(TASK_NOTIFICATION_TYPE)
            {
                return false;
            }
            match &message.details {
                Some(details) => details.task_id == task_id && details.status == status,
                None => false,
            }
        })
    }
}

/// The reminder injected after a compaction.
/// Compaction removes the messages that started the running tasks, so without
/// this the model has no idea they exist and starts them again. The list is the
/// point — a reminder that says "some tasks are running" without saying which
/// is not actionable.
pub fn active_task_reminder(active: &[TaskInfo]) -> Option<String> {
    if active.is_empty() {
        return None;
    }
    let mut lines = vec![
        "The conversation was compacted, so the messages that started these background tasks are gone — but the tasks are still running.".to_owned(),
        "Do not start duplicates. Their completions will arrive on their own.".to_owned(),
        String::new(),
    ];
    for info in active {
        let detail = match info {
            TaskInfo::Shell(shell) => shell.command.clone(),
            TaskInfo::Subagent(subagent) => {
                format!(
                    "{} ({}), session {}",
                    subagent.alias, subagent.agent, subagent.session_id
                )
            }
        };
        lines.push(format!(
            "- {} ({}, {}): {} — {detail}",
            info.task_id(),
            info.kind(),
            info.status(),
            info.description()
        ));
    }
    Some(lines.join("\n"))
}
