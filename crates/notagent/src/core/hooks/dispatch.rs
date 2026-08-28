use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use notagent_agent::types::AgentMessage;
use notagent_ai::types::{StopReason, TextOrImageContent};
use serde_json::{Map, Value, json};

use super::events::HookEvent;
use super::payload::{
    NOTIFICATION_IDLE_PROMPT, NOTIFICATION_PERMISSION_PROMPT, summarize_tool_response,
};
use super::runtime::HookRuntime;
use crate::core::permissions::coordinator::ApprovalObserver;
use crate::core::permissions::request::{ApprovalAnswer, ApprovalRequest, format_request_summary};

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
pub fn render_hook_context(outputs: &[String]) -> String {
    format!("<hook_context>\n{}\n</hook_context>", outputs.join("\n\n"))
}

fn fields(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

/// The text parts of a tool result, joined — `resultText` of the extension.
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

pub struct HookDispatcher {
    runtime: Arc<HookRuntime>,
    /// Recorded at agent_end and read at agent_settled: the outcome is known when
    /// the loop ends, but "the agent has stopped" is only true once no retry,
    /// compaction or queued continuation will follow.
    last_outcome: Mutex<RunOutcome>,
}

impl HookDispatcher {
    pub fn new(runtime: Arc<HookRuntime>) -> Self {
        Self {
            runtime,
            last_outcome: Mutex::new(RunOutcome::Ok),
        }
    }

    pub fn runtime(&self) -> Arc<HookRuntime> {
        Arc::clone(&self.runtime)
    }

    pub async fn session_start(&self, reason: &str) {
        self.runtime
            .emit(
                HookEvent::SessionStart,
                fields([("source", json!(reason))]),
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

    /// `before_agent_start`. Returns the body of the hidden `hook_context`
    /// message, or `None` when no hook wrote anything.
    /// Delivered as a message rather than folded into the prompt, so what
    /// the user typed stays exactly what they typed. The wrapper names the
    /// source, since text of unclear origin is the thing a model is most
    /// likely to mistake for an instruction from the user.
    pub async fn before_agent_start(&self, prompt: &str) -> Option<String> {
        let context = self
            .runtime
            .emit(
                HookEvent::UserPromptSubmit,
                fields([("prompt", json!(prompt))]),
                None,
            )
            .await;
        (!context.is_empty()).then(|| render_hook_context(&context))
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

        // A run that ended badly emits a name the reference's consumers do not
        // watch for, so their working indicator would spin forever. The idle
        // notification is what those consumers already use to recover, and it
        // costs nothing when nobody listens.
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
                    // The reference distinguishes only manual from automatic; ours knows
                    // why it was automatic, so both are sent rather than losing one.
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

/// Reports approval prompts as hook events.
/// Both names fire: PermissionRequest and PermissionResult carry the detail, and
/// Notification carries the same moment under the name an external supervisor
/// already watches for. That duplication is the point — a consumer built against
/// the reference needs no change to see that the agent is waiting for a human.
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
