use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use notagent_agent::types::AgentMessage;
use notagent_ai::types::{StopReason, TextOrImageContent};
use serde_json::{Map, Value, json};

use super::events::HookEvent;
use super::payload::{
    NOTIFICATION_IDLE_PROMPT, NOTIFICATION_PERMISSION_PROMPT, summarize_output,
    summarize_tool_response,
};
use super::runner::{HookVerdict, hook_context};
use super::runtime::{HookContextOutput, HookRuntime};
use crate::core::permissions::coordinator::ApprovalObserver;
use crate::core::permissions::request::{ApprovalAnswer, ApprovalRequest, format_request_summary};
use crate::core::tasks::types::{TaskInfo, TaskStatus};

/// How a completed run ended, which decides which of the three names fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunOutcome {
    #[default]
    Ok,
    Error,
    Interrupted,
}

pub fn outcome_of(messages: &[AgentMessage]) -> RunOutcome {
    for message in messages.iter().rev() {
        let AgentMessage::Assistant(message) = message else {
            continue;
        };
        return match message.stop_reason {
            StopReason::Aborted => RunOutcome::Interrupted,
            StopReason::Error => RunOutcome::Error,
            _ => RunOutcome::Ok,
        };
    }
    RunOutcome::Ok
}

/// Wraps what hooks printed so the model can tell it from conversation text.
/// Named as machine-supplied, because a line whose origin is unclear is the one
/// a model is most likely to treat as an instruction from the user.
pub fn render_hook_context(outputs: &[HookContextOutput]) -> String {
    let body = outputs
        .iter()
        .map(|output| {
            format!(
                "<hook_result event=\"{}\" declaration=\"{}\">\n{}\n</hook_result>",
                output.event, output.declaration, output.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("<hook_context>\n{body}\n</hook_context>")
}

fn fields(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

/// The text parts of a tool result, joined for a bounded hook payload.
pub fn result_text(content: &[TextOrImageContent]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            TextOrImageContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentHookInfo {
    pub task_id: Option<String>,
    pub child_session_id: String,
    pub agent: String,
    pub alias: String,
    pub description: String,
    pub prompt: String,
    pub detached: bool,
}

pub struct HookDispatcher {
    runtime: Arc<HookRuntime>,
    /// Recorded at agent_end and read at agent_settled: the outcome is known when
    /// the loop ends, but "the agent has stopped" is only true once no retry,
    /// compaction or queued continuation will follow.
    last_outcome: Mutex<RunOutcome>,
    turn_number: AtomicU64,
}

impl HookDispatcher {
    pub fn new(runtime: Arc<HookRuntime>) -> Self {
        Self {
            runtime,
            last_outcome: Mutex::new(RunOutcome::Ok),
            turn_number: AtomicU64::new(0),
        }
    }

    pub fn runtime(&self) -> Arc<HookRuntime> {
        Arc::clone(&self.runtime)
    }

    pub async fn session_start(&self, reason: &str) {
        self.turn_number.store(0, Ordering::SeqCst);
        self.runtime
            .emit(
                HookEvent::SessionStart,
                fields([("source", json!(reason))]),
                None,
            )
            .await;
    }

    pub async fn turn_started(&self) {
        let turn_number = self.turn_number.fetch_add(1, Ordering::SeqCst) + 1;
        self.runtime
            .emit(
                HookEvent::TurnStarted,
                fields([("turn_number", json!(turn_number))]),
                None,
            )
            .await;
    }

    pub async fn user_prompt_queued(
        &self,
        prompt: &str,
        queue: &str,
        image_count: usize,
        queue_length: usize,
    ) {
        self.runtime
            .emit(
                HookEvent::UserPromptQueued,
                fields([
                    ("prompt", json!(prompt)),
                    ("queue", json!(queue)),
                    ("image_count", json!(image_count)),
                    ("queue_length", json!(queue_length)),
                ]),
                None,
            )
            .await;
    }

    pub async fn task_started(&self, info: &TaskInfo) {
        self.runtime
            .emit(
                HookEvent::TaskStarted,
                fields([
                    ("task_id", json!(info.task_id())),
                    ("kind", json!(info.kind().as_str())),
                    ("description", json!(info.description())),
                    ("detached", json!(info.is_detached())),
                ]),
                None,
            )
            .await;
    }

    pub async fn subagent_started(&self, info: &SubagentHookInfo) {
        self.runtime
            .emit(
                HookEvent::SubagentStart,
                fields([
                    ("task_id", json!(info.task_id)),
                    ("child_session_id", json!(info.child_session_id)),
                    ("agent", json!(info.agent)),
                    ("alias", json!(info.alias)),
                    ("description", json!(info.description)),
                    ("prompt", json!(info.prompt)),
                    ("detached", json!(info.detached)),
                ]),
                None,
            )
            .await;
    }

    pub async fn subagent_stopped(
        &self,
        info: &SubagentHookInfo,
        status: TaskStatus,
        duration_ms: u64,
        stop_reason: Option<&str>,
        result: Option<&str>,
    ) {
        self.runtime
            .emit(
                HookEvent::SubagentStop,
                fields([
                    ("task_id", json!(info.task_id)),
                    ("child_session_id", json!(info.child_session_id)),
                    ("agent", json!(info.agent)),
                    ("alias", json!(info.alias)),
                    ("status", json!(status.as_str())),
                    ("duration_ms", json!(duration_ms)),
                    (
                        "stop_reason",
                        stop_reason.map_or(Value::Null, |reason| json!(reason)),
                    ),
                    (
                        "result",
                        result.map_or(Value::Null, |text| Value::Object(summarize_output(text))),
                    ),
                    ("detached", json!(info.detached)),
                ]),
                None,
            )
            .await;
    }

    pub async fn session_shutdown(&self, reason: &str) {
        self.runtime
            .emit(
                HookEvent::SessionEnd,
                fields([("reason", json!(reason))]),
                None,
            )
            .await;
    }

    /// Decides a submitted prompt before transcript or model state is changed.
    /// Accepted context is delivered as a hidden message so the user's text
    /// stays byte-for-byte what they submitted.
    pub async fn before_agent_start(&self, prompt: &str) -> Result<Option<String>, String> {
        let outcome = self
            .runtime
            .decide(
                HookEvent::UserPromptSubmit,
                fields([("prompt", json!(prompt))]),
                None,
            )
            .await;
        if outcome.cancelled {
            return Err("Prompt hook was cancelled".to_string());
        }
        if let HookVerdict::Deny { reason } = outcome.verdict {
            return Err(reason);
        }
        let context = outcome
            .results
            .iter()
            .enumerate()
            .filter_map(|(index, result)| {
                hook_context(result).map(|text| HookContextOutput {
                    event: HookEvent::UserPromptSubmit,
                    declaration: index + 1,
                    text,
                })
            })
            .collect::<Vec<_>>();
        Ok((!context.is_empty()).then(|| render_hook_context(&context)))
    }

    pub fn agent_end(&self, messages: &[AgentMessage]) {
        *self.last_outcome.lock().expect("hook outcome") = outcome_of(messages);
    }

    pub async fn agent_settled(&self) {
        let outcome = {
            let mut last = self.last_outcome.lock().expect("hook outcome");
            std::mem::replace(&mut *last, RunOutcome::Ok)
        };
        let event = match outcome {
            RunOutcome::Interrupted => HookEvent::Interrupt,
            RunOutcome::Error => HookEvent::StopFailure,
            RunOutcome::Ok => HookEvent::Stop,
        };
        self.runtime
            .emit(event, fields([("stop_hook_active", json!(false))]), None)
            .await;

        // Failure names carry detail while the idle notification closes the
        // coarse working state maintained by notification-only consumers.
        if outcome != RunOutcome::Ok {
            let message = if outcome == RunOutcome::Interrupted {
                "The turn was interrupted."
            } else {
                "The turn ended with an error."
            };
            self.runtime
                .emit(
                    HookEvent::Notification,
                    fields([
                        ("notification_type", json!(NOTIFICATION_IDLE_PROMPT)),
                        ("message", json!(message)),
                    ]),
                    None,
                )
                .await;
        }
    }

    pub async fn tool_result(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        input: &Map<String, Value>,
        content: &[TextOrImageContent],
        is_error: bool,
    ) {
        let event = if is_error {
            HookEvent::PostToolUseFailure
        } else {
            HookEvent::PostToolUse
        };
        let mut payload = fields([
            ("tool_call_id", json!(tool_call_id)),
            ("tool_name", json!(tool_name)),
            ("tool_input", Value::Object(input.clone())),
        ]);
        payload.extend(summarize_tool_response(&result_text(content), is_error));
        self.runtime.emit(event, payload, Some(tool_name)).await;
    }

    pub async fn session_before_compact(&self, reason: &str, custom_instructions: Option<&str>) {
        self.runtime
            .emit(
                HookEvent::PreCompact,
                fields([
                    // `trigger` is coarse for consumers that only branch on user
                    // intent; `reason` preserves the exact local cause.
                    (
                        "trigger",
                        json!(if reason == "manual" { "manual" } else { "auto" }),
                    ),
                    ("reason", json!(reason)),
                    (
                        "custom_instructions",
                        json!(custom_instructions.unwrap_or("")),
                    ),
                ]),
                None,
            )
            .await;
    }

    pub async fn session_compact(&self, reason: &str) {
        self.runtime
            .emit(
                HookEvent::PostCompact,
                fields([
                    (
                        "trigger",
                        json!(if reason == "manual" { "manual" } else { "auto" }),
                    ),
                    ("reason", json!(reason)),
                ]),
                None,
            )
            .await;
    }
}

/// Reports approval prompts as detailed permission events and as coarse
/// notifications for consumers that only track whether the session is waiting.
pub struct HookApprovalObserver {
    runtime: Arc<HookRuntime>,
}

pub fn create_approval_observer(runtime: Arc<HookRuntime>) -> Arc<dyn ApprovalObserver> {
    Arc::new(HookApprovalObserver { runtime })
}

fn optional(value: &Option<String>) -> Value {
    value.as_ref().map_or(Value::Null, |value| json!(value))
}

impl ApprovalObserver for HookApprovalObserver {
    fn requested(&self, request: ApprovalRequest) -> BoxFuture<'static, ()> {
        let runtime = Arc::clone(&self.runtime);
        Box::pin(async move {
            runtime
                .emit(
                    HookEvent::PermissionRequest,
                    fields([
                        ("tool_call_id", json!(request.tool_call_id)),
                        ("tool_name", json!(request.tool_name)),
                        ("target", optional(&request.target)),
                        ("policy", json!(request.policy_name)),
                        ("reason", optional(&request.reason)),
                        ("mode", optional(&request.mode_id)),
                    ]),
                    Some(&request.tool_name),
                )
                .await;
            runtime
                .emit(
                    HookEvent::Notification,
                    fields([
                        ("notification_type", json!(NOTIFICATION_PERMISSION_PROMPT)),
                        (
                            "message",
                            json!(format!(
                                "Approval needed: {}",
                                format_request_summary(&request)
                            )),
                        ),
                        ("tool_name", json!(request.tool_name)),
                    ]),
                    None,
                )
                .await;
        })
    }

    fn resolved(&self, request: ApprovalRequest, answer: ApprovalAnswer) -> BoxFuture<'static, ()> {
        let runtime = Arc::clone(&self.runtime);
        Box::pin(async move {
            runtime
                .emit(
                    HookEvent::PermissionResult,
                    fields([
                        ("tool_call_id", json!(request.tool_call_id)),
                        ("tool_name", json!(request.tool_name)),
                        ("target", optional(&request.target)),
                        ("policy", json!(request.policy_name)),
                        ("answer", json!(answer.as_str())),
                        ("allowed", json!(answer != ApprovalAnswer::Deny)),
                    ]),
                    Some(&request.tool_name),
                )
                .await;
        })
    }
}
