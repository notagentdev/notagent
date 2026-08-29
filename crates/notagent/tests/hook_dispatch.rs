#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;

use notagent::core::hooks::dispatch::{
    HookDispatcher, RunOutcome, SubagentHookInfo, create_approval_observer, outcome_of,
    render_hook_context,
};
use notagent::core::hooks::events::HookEvent;
use notagent::core::hooks::payload::HookSessionContext;
use notagent::core::hooks::runtime::{HookContextOutput, HookRuntime, HookRuntimeOptions};
use notagent::core::hooks::{HOOKS_FILE_NAME, load_hooks};
use notagent::core::permissions::request::{ApprovalAnswer, ApprovalRequest};
use notagent::core::tasks::types::{SubagentTaskInfo, TaskInfo, TaskInfoBase, TaskStatus};
use notagent_agent::types::AgentMessage;
use notagent_ai::types::{
    AssistantMessage, StopReason, TextContent, TextOrImageContent, Usage, UsageCost,
};
use serde_json::{Map, Value, json};

/// Every event the dispatcher can raise, each appending its payload to one log.
const WATCHED_EVENTS: [&str; 18] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "Stop",
    "StopFailure",
    "Interrupt",
    "Notification",
    "PostToolUse",
    "PostToolUseFailure",
    "PreCompact",
    "PostCompact",
    "PermissionRequest",
    "PermissionResult",
    "TurnStarted",
    "UserPromptQueued",
    "TaskStarted",
    "SubagentStart",
    "SubagentStop",
];

struct Wire {
    directory: PathBuf,
    runtime: Arc<HookRuntime>,
}

impl Wire {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("hooks-wired-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        let log = path.join("log.jsonl");
        let declarations: Vec<Value> = WATCHED_EVENTS
            .iter()
            .map(|event| json!({ "event": event, "command": format!("cat >> {}", log.display()) }))
            .collect();
        std::fs::write(
            path.join(HOOKS_FILE_NAME),
            Value::Array(declarations).to_string(),
        )
        .expect("writes");

        let loaded = load_hooks(std::slice::from_ref(&path));
        assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
        let session_dir = path.to_string_lossy().into_owned();
        let runtime = Arc::new(HookRuntime::new(HookRuntimeOptions {
            hooks: loaded.hooks,
            diagnostics: loaded.diagnostics,
            context: Arc::new(move || HookSessionContext {
                session_id: "wired".to_string(),
                transcript_path: None,
                cwd: session_dir.clone(),
            }),
            report: None,
            signal: None,
        }));
        Self {
            directory: path,
            runtime,
        }
    }

    fn dispatcher(&self) -> HookDispatcher {
        HookDispatcher::new(Arc::clone(&self.runtime))
    }

    fn payloads(&self) -> Vec<Map<String, Value>> {
        let raw = std::fs::read_to_string(self.directory.join("log.jsonl")).unwrap_or_default();
        raw.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<Value>(line)
                    .expect("payload")
                    .as_object()
                    .cloned()
                    .expect("object")
            })
            .collect()
    }

    fn names(&self) -> Vec<String> {
        self.payloads()
            .iter()
            .filter_map(|payload| {
                payload
                    .get("hook_event_name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect()
    }

    /// The payload of the first run of `name`, without the four fields every
    fn fields_of(&self, name: &str) -> Map<String, Value> {
        let mut payload = self
            .payloads()
            .into_iter()
            .find(|payload| payload.get("hook_event_name").and_then(Value::as_str) == Some(name))
            .unwrap_or_default();
        for key in ["session_id", "transcript_path", "cwd", "hook_event_name"] {
            payload.remove(key);
        }
        payload
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn assistant(stop_reason: StopReason) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content: Vec::new(),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "test".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cache_write1h: None,
            reasoning: None,
            total_tokens: Some(0),
            cost: UsageCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                total: 0.0,
            },
        },
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    })
}

// ---------------------------------------------------------------------------
// session events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn maps_a_session_start_carrying_why_it_started() {
    let wire = Wire::new();
    wire.dispatcher().session_start("resume").await;
    assert_eq!(wire.names(), vec!["SessionStart"]);
    assert_eq!(
        wire.fields_of("SessionStart"),
        json!({ "source": "resume" }).as_object().cloned().unwrap()
    );
}

#[tokio::test]
async fn maps_a_shutdown_to_the_end_of_the_session() {
    let wire = Wire::new();
    wire.dispatcher().session_shutdown("quit").await;
    assert_eq!(wire.names(), vec!["SessionEnd"]);
    assert_eq!(
        wire.fields_of("SessionEnd"),
        json!({ "reason": "quit" }).as_object().cloned().unwrap()
    );
}

// ---------------------------------------------------------------------------
// the prompt event
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_prompt_event_fires_before_the_model_starts() {
    let wire = Wire::new();
    assert_eq!(
        wire.dispatcher().before_agent_start("do the thing").await,
        Ok(None)
    );
    assert_eq!(wire.names(), vec!["UserPromptSubmit"]);
    assert_eq!(
        wire.fields_of("UserPromptSubmit"),
        json!({ "prompt": "do the thing" })
            .as_object()
            .cloned()
            .unwrap()
    );
}

#[tokio::test]
async fn what_the_prompt_hooks_wrote_comes_back_wrapped_for_the_model() {
    let wire = Wire::new();
    // The log hook writes nothing to standard output, so there is no context.
    assert_eq!(wire.dispatcher().before_agent_start("x").await, Ok(None));
    assert_eq!(
        render_hook_context(&[
            HookContextOutput {
                event: HookEvent::UserPromptSubmit,
                declaration: 1,
                text: "branch: main".to_string(),
            },
            HookContextOutput {
                event: HookEvent::UserPromptSubmit,
                declaration: 2,
                text: "dirty".to_string(),
            },
        ]),
        "<hook_context>\n<hook_result event=\"UserPromptSubmit\" declaration=\"1\">\nbranch: main\n</hook_result>\n\n<hook_result event=\"UserPromptSubmit\" declaration=\"2\">\ndirty\n</hook_result>\n</hook_context>"
    );
}

// ---------------------------------------------------------------------------
// how a run ended
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stops_normally_when_nothing_went_wrong() {
    let wire = Wire::new();
    let dispatcher = wire.dispatcher();
    dispatcher.agent_end(&[assistant(StopReason::Stop)]);
    dispatcher.agent_settled().await;
    assert_eq!(wire.names(), vec!["Stop"]);
}

#[tokio::test]
async fn separates_an_interrupted_run_and_adds_the_notification() {
    let wire = Wire::new();
    let dispatcher = wire.dispatcher();
    dispatcher.agent_end(&[assistant(StopReason::Aborted)]);
    dispatcher.agent_settled().await;
    assert_eq!(wire.names(), vec!["Interrupt", "Notification"]);
    assert_eq!(
        wire.fields_of("Notification")
            .get("notification_type")
            .and_then(Value::as_str),
        Some("idle_prompt")
    );
}

#[tokio::test]
async fn separates_a_failed_run() {
    let wire = Wire::new();
    let dispatcher = wire.dispatcher();
    dispatcher.agent_end(&[assistant(StopReason::Error)]);
    dispatcher.agent_settled().await;
    assert_eq!(wire.names(), vec!["StopFailure", "Notification"]);
}

#[tokio::test]
async fn does_not_carry_an_outcome_into_the_next_run() {
    let wire = Wire::new();
    let dispatcher = wire.dispatcher();
    dispatcher.agent_end(&[assistant(StopReason::Aborted)]);
    dispatcher.agent_settled().await;
    dispatcher.agent_settled().await;
    assert_eq!(wire.names(), vec!["Interrupt", "Notification", "Stop"]);
}

#[tokio::test]
async fn reports_nothing_until_the_run_has_settled() {
    let wire = Wire::new();
    wire.dispatcher().agent_end(&[assistant(StopReason::Error)]);
    assert!(wire.names().is_empty());
}

#[test]
fn reads_the_outcome_from_the_last_assistant_message() {
    assert_eq!(outcome_of(&[assistant(StopReason::Stop)]), RunOutcome::Ok);
    assert_eq!(
        outcome_of(&[assistant(StopReason::Aborted)]),
        RunOutcome::Interrupted
    );
    assert_eq!(
        outcome_of(&[assistant(StopReason::Error)]),
        RunOutcome::Error
    );
    assert_eq!(outcome_of(&[]), RunOutcome::Ok);
    assert_eq!(
        outcome_of(&[assistant(StopReason::Error), assistant(StopReason::Stop)]),
        RunOutcome::Ok
    );
}

// ---------------------------------------------------------------------------
// tool results
// ---------------------------------------------------------------------------

fn tool_content() -> Vec<TextOrImageContent> {
    vec![TextOrImageContent::Text(TextContent {
        text: "wrote a.ts".to_string(),
        ..TextContent::default()
    })]
}

fn tool_input() -> Map<String, Value> {
    json!({ "path": "a.ts" })
        .as_object()
        .cloned()
        .expect("object")
}

#[tokio::test]
async fn separates_a_failed_call_from_a_successful_one() {
    let ok = Wire::new();
    ok.dispatcher()
        .tool_result("call-ok", "write", &tool_input(), &tool_content(), false)
        .await;
    assert_eq!(ok.names(), vec!["PostToolUse"]);

    let failed = Wire::new();
    failed
        .dispatcher()
        .tool_result("call-failed", "write", &tool_input(), &tool_content(), true)
        .await;
    assert_eq!(failed.names(), vec!["PostToolUseFailure"]);
}

#[tokio::test]
async fn carries_the_tool_its_arguments_and_what_came_back() {
    let wire = Wire::new();
    wire.dispatcher()
        .tool_result("call-1", "write", &tool_input(), &tool_content(), false)
        .await;
    assert_eq!(
        wire.fields_of("PostToolUse"),
        json!({
            "tool_call_id": "call-1",
            "tool_name": "write",
            "tool_input": { "path": "a.ts" },
            "tool_response": { "success": true, "output": "wrote a.ts" },
        })
        .as_object()
        .cloned()
        .unwrap()
    );
}

#[tokio::test]
async fn caps_multibyte_tool_output_including_the_truncation_marker() {
    let wire = Wire::new();
    let content = vec![TextOrImageContent::Text(TextContent {
        text: "é".repeat(3_000),
        ..TextContent::default()
    })];
    wire.dispatcher()
        .tool_result("call-long", "write", &tool_input(), &content, false)
        .await;
    let response = wire
        .fields_of("PostToolUse")
        .remove("tool_response")
        .and_then(|value| value.as_object().cloned())
        .expect("tool response");
    let output = response
        .get("output")
        .and_then(Value::as_str)
        .expect("output");
    assert_eq!(output.encode_utf16().count(), 2_000);
    assert!(output.ends_with('…'));
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(true)
    );
}

#[tokio::test]
async fn passes_the_tool_name_for_matching_so_a_matcher_can_narrow_it() {
    let directory = tempfile::Builder::new()
        .prefix("hooks-matcher-")
        .tempdir()
        .expect("temp dir");
    let path = directory.path().to_path_buf();
    let log = path.join("log.jsonl");
    std::fs::write(
        path.join(HOOKS_FILE_NAME),
        json!([{
            "event": "PostToolUse",
            "matcher": "edit",
            "command": format!("cat >> {}", log.display()),
        }])
        .to_string(),
    )
    .expect("writes");
    let loaded = load_hooks(std::slice::from_ref(&path));
    let runtime = Arc::new(HookRuntime::new(HookRuntimeOptions {
        hooks: loaded.hooks,
        diagnostics: loaded.diagnostics,
        context: Arc::new(HookSessionContext::default),
        report: None,
        signal: None,
    }));
    let dispatcher = HookDispatcher::new(runtime);
    dispatcher
        .tool_result("call-write", "write", &tool_input(), &tool_content(), false)
        .await;
    assert!(!log.exists());
    dispatcher
        .tool_result("call-edit", "edit", &tool_input(), &tool_content(), false)
        .await;
    assert!(log.exists());
}

// ---------------------------------------------------------------------------
// compaction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fires_before_and_after_compaction_telling_manual_from_automatic() {
    let wire = Wire::new();
    let dispatcher = wire.dispatcher();
    dispatcher.session_before_compact("threshold", None).await;
    dispatcher.session_compact("threshold").await;
    assert_eq!(wire.names(), vec!["PreCompact", "PostCompact"]);
    let fields = wire.fields_of("PreCompact");
    assert_eq!(fields.get("trigger").and_then(Value::as_str), Some("auto"));
    assert_eq!(
        fields.get("reason").and_then(Value::as_str),
        Some("threshold")
    );
}

#[tokio::test]
async fn keeps_the_exact_reason_alongside_the_coarser_trigger() {
    let wire = Wire::new();
    wire.dispatcher()
        .session_before_compact("manual", Some("keep the plan"))
        .await;
    assert_eq!(
        wire.fields_of("PreCompact"),
        json!({
            "trigger": "manual",
            "reason": "manual",
            "custom_instructions": "keep the plan",
        })
        .as_object()
        .cloned()
        .unwrap()
    );
}

// ---------------------------------------------------------------------------
// subagent events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn emits_turn_queue_task_and_subagent_lifecycle_payloads() {
    let wire = Wire::new();
    let dispatcher = wire.dispatcher();
    dispatcher.turn_started().await;
    dispatcher.turn_started().await;
    dispatcher
        .user_prompt_queued("next", "follow_up", 2, 3)
        .await;
    let task = TaskInfo::Subagent(SubagentTaskInfo {
        base: TaskInfoBase {
            task_id: "task-1".to_string(),
            description: "inspect hooks".to_string(),
            status: TaskStatus::Completed,
            detached: Some(true),
            started_at: 1_000,
            ended_at: Some(1_250),
            stop_reason: Some("done".to_string()),
            notification_suppressed: None,
            timeout_ms: Some(5_000),
        },
        tokens: 42,
        session_id: "child-1".to_string(),
        agent: "worker".to_string(),
        alias: "reader".to_string(),
    });
    let subagent = SubagentHookInfo {
        task_id: Some("task-1".to_string()),
        child_session_id: "child-1".to_string(),
        agent: "worker".to_string(),
        alias: "reader".to_string(),
        description: "inspect hooks".to_string(),
        prompt: "Inspect every hook boundary.".to_string(),
        detached: true,
    };
    dispatcher.task_started(&task).await;
    dispatcher.subagent_started(&subagent).await;
    dispatcher
        .subagent_stopped(
            &subagent,
            TaskStatus::Completed,
            250,
            Some("done"),
            Some("finished output"),
        )
        .await;

    assert_eq!(
        wire.names(),
        vec![
            "TurnStarted",
            "TurnStarted",
            "UserPromptQueued",
            "TaskStarted",
            "SubagentStart",
            "SubagentStop",
        ]
    );
    let turns = wire
        .payloads()
        .into_iter()
        .filter(|payload| {
            payload.get("hook_event_name").and_then(Value::as_str) == Some("TurnStarted")
        })
        .filter_map(|payload| payload.get("turn_number").and_then(Value::as_u64))
        .collect::<Vec<_>>();
    assert_eq!(turns, vec![1, 2]);
    assert_eq!(
        wire.fields_of("UserPromptQueued"),
        json!({
            "prompt": "next",
            "queue": "follow_up",
            "image_count": 2,
            "queue_length": 3,
        })
        .as_object()
        .cloned()
        .unwrap()
    );
    let stopped = wire.fields_of("SubagentStop");
    assert_eq!(
        wire.fields_of("SubagentStart")
            .get("prompt")
            .and_then(Value::as_str),
        Some("Inspect every hook boundary.")
    );
    assert_eq!(
        stopped.get("duration_ms").and_then(Value::as_i64),
        Some(250)
    );
    assert_eq!(
        stopped.get("status").and_then(Value::as_str),
        Some("completed")
    );
    assert_eq!(
        stopped
            .get("result")
            .and_then(Value::as_object)
            .and_then(|result| result.get("output"))
            .and_then(Value::as_str),
        Some("finished output")
    );
}

#[tokio::test]
async fn a_child_without_a_task_manager_carries_no_fake_task_id() {
    let wire = Wire::new();
    wire.dispatcher()
        .subagent_started(&SubagentHookInfo {
            task_id: None,
            child_session_id: "child-only".to_string(),
            agent: "worker".to_string(),
            alias: "reader".to_string(),
            description: "inspect hooks".to_string(),
            prompt: "Inspect every hook boundary.".to_string(),
            detached: false,
        })
        .await;

    let fields = wire.fields_of("SubagentStart");
    assert_eq!(fields.get("task_id"), Some(&Value::Null));
    assert_eq!(
        fields.get("child_session_id").and_then(Value::as_str),
        Some("child-only")
    );
}

// ---------------------------------------------------------------------------
// from a declaration file to a running process
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runs_the_command_a_project_declared_with_the_event_on_its_input() {
    let wire = Wire::new();
    wire.dispatcher().session_start("startup").await;
    let payload = wire.payloads().into_iter().next().expect("payload");
    assert_eq!(
        payload.get("hook_event_name").and_then(Value::as_str),
        Some("SessionStart")
    );
    assert_eq!(
        payload.get("session_id").and_then(Value::as_str),
        Some("wired")
    );
    assert_eq!(
        payload.get("source").and_then(Value::as_str),
        Some("startup")
    );
}

// ---------------------------------------------------------------------------
// approval prompts
// ---------------------------------------------------------------------------

fn approval_request() -> ApprovalRequest {
    ApprovalRequest {
        tool_call_id: "call-approval".to_string(),
        tool_name: "bash".to_string(),
        target: Some("rm -rf build".to_string()),
        policy_name: "fallback-ask".to_string(),
        reason: Some("No rule covers this call.".to_string()),
        mode_id: Some("manual".to_string()),
        requester: None,
    }
}

#[tokio::test]
async fn reports_the_request_under_both_names() {
    let wire = Wire::new();
    let observer = create_approval_observer(Arc::clone(&wire.runtime));
    observer.requested(approval_request()).await;
    assert_eq!(wire.names(), vec!["PermissionRequest", "Notification"]);
    let notification = wire.fields_of("Notification");
    assert_eq!(
        notification
            .get("notification_type")
            .and_then(Value::as_str),
        Some("permission_prompt")
    );
    assert_eq!(
        notification.get("message").and_then(Value::as_str),
        Some("Approval needed: bash rm -rf build")
    );
    assert_eq!(
        wire.fields_of("PermissionRequest")
            .get("tool_call_id")
            .and_then(Value::as_str),
        Some("call-approval")
    );
}

#[tokio::test]
async fn reports_the_answer_and_whether_it_permitted_the_call() {
    let wire = Wire::new();
    let observer = create_approval_observer(Arc::clone(&wire.runtime));
    observer
        .resolved(approval_request(), ApprovalAnswer::Deny)
        .await;
    let denied = wire.fields_of("PermissionResult");
    assert_eq!(
        denied.get("tool_call_id").and_then(Value::as_str),
        Some("call-approval")
    );
    assert_eq!(denied.get("answer").and_then(Value::as_str), Some("deny"));
    assert_eq!(denied.get("allowed").and_then(Value::as_bool), Some(false));

    let allowed_wire = Wire::new();
    let observer = create_approval_observer(Arc::clone(&allowed_wire.runtime));
    observer
        .resolved(approval_request(), ApprovalAnswer::ApproveAlways)
        .await;
    let allowed = allowed_wire.fields_of("PermissionResult");
    assert_eq!(
        allowed.get("answer").and_then(Value::as_str),
        Some("approve-always")
    );
    assert_eq!(allowed.get("allowed").and_then(Value::as_bool), Some(true));
}
