//! Port of `packages/coding-agent/test/task-tool.test.ts`.
//!
//! The delegation tool.
//!
//! The gates are what is asserted: a mode that does not exist, a shell a
//! read-only session may not reach, a repeat refused inside one call, a
//! continuation that must stay with the child it belongs to, and the background
//! flag that is absent from the schema unless the session can observe a task.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notagent::core::modes::shells::{ApprovalLevel, ShellId, tools_for_shell};
use notagent::core::modes::{Mode, ModeSkill};
use notagent::core::tasks::manager::{TaskManager, TaskManagerOptions};
use notagent::core::tasks::store::TaskStore;
use notagent::core::tools::task::{TaskToolSources, create_task_tool_definition};
use notagent::core::tools::tool_definition::ToolDefinition;
use notagent_agent::agent::{Agent, AgentOptions};
use notagent_agent::types::{AgentToolResult, ToolExecutionError};
use notagent_ai::types::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, DoneReason, Modality, Model,
    ModelCost, StopReason, TextContent, TextOrImageContent, Usage,
};
use notagent_ai::utils::event_stream::create_assistant_message_event_stream;
use serde_json::{Value, json};

fn model() -> Model {
    Model {
        id: "mock".to_owned(),
        name: "mock".to_owned(),
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        base_url: "https://example.invalid".to_owned(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 8192,
        max_tokens: 2048,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn assistant(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![AssistantContent::Text(TextContent::new(text))],
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        model: "mock".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

fn mode(id: &str, shell: ShellId) -> Mode {
    Mode {
        id: id.to_owned(),
        shell,
        approval: ApprovalLevel::Manual,
        tools: tools_for_shell(shell),
        subagents: None,
        skills: vec![ModeSkill {
            file_name: "10.md".to_owned(),
            file_path: format!("/modes/{id}/10.md").into(),
            body: format!("{id} guidance"),
        }],
        source_dir: format!("/modes/{id}").into(),
    }
}

fn modes() -> Vec<Mode> {
    vec![
        mode("plan", ShellId::ReadOnly),
        mode("worker", ShellId::Worker),
    ]
}

struct Harness {
    _directory: tempfile::TempDir,
    tool: notagent::core::tools::task::TaskToolDefinition,
    manager: TaskManager,
    calls: Arc<AtomicUsize>,
}

impl Harness {
    async fn run(&self, input: Value) -> Result<AgentToolResult, ToolExecutionError> {
        self.tool.execute("call-1", input, None, None, None).await
    }
}

fn harness(parent_shell: ShellId, background: bool) -> Harness {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let parent = Agent::new(AgentOptions {
        model: Some(model()),
        system_prompt: Some("P".to_owned()),
        tools: Some(Vec::new()),
        get_api_key: Some(Arc::new(|_provider| {
            Box::pin(async { Some("test-key".to_owned()) })
        })),
        stream_fn: Some(Arc::new(move |_model, _context, _options| {
            let index = counter.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                let stream = create_assistant_message_event_stream();
                // Padded past the minimum a handoff must reach: a short answer
                // costs an extra turn by design, and these cases are not about
                // that.
                let message = assistant(&format!("answer {index} {}", "detail ".repeat(40)));
                stream.push(AssistantMessageEvent::Done {
                    reason: DoneReason::Stop,
                    message,
                });
                stream
            })
        })),
        ..AgentOptions::default()
    });

    let directory = tempfile::Builder::new()
        .prefix("task-tool-")
        .tempdir()
        .expect("temp dir");
    let manager = TaskManager::new(
        Arc::new(TaskStore::new(directory.path())),
        TaskManagerOptions::default(),
    );
    let source_manager = manager.clone();
    let tool = create_task_tool_definition(Some(TaskToolSources {
        modes: Arc::new(modes),
        parent_shell: Arc::new(move || Some(parent_shell)),
        parent_mode: None,
        parent: Arc::new(move || Some(Arc::clone(&parent))),
        manager: Some(Arc::new(move || Some(source_manager.clone()))),
        background_allowed: Some(Arc::new(move || background)),
        resolve_tool: None,
        tool_options: None,
        transcripts: Default::default(),
        cwd: None,
    }));
    Harness {
        _directory: directory,
        tool,
        manager,
        calls,
    }
}

fn text_of(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|part| match part {
            TextOrImageContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn results_of(result: &AgentToolResult) -> Vec<Value> {
    result
        .details
        .as_ref()
        .and_then(|details| details.get("results"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn matches_task_id(text: &str, prefix: &str) -> bool {
    text.split_whitespace().any(|word| {
        word.strip_prefix(prefix).is_some_and(|suffix| {
            suffix.len() == 8
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
    })
}

// ── choosing a mode ───────────────────────────────────────────────────

#[tokio::test]
async fn lists_the_available_modes_and_their_tools_in_the_description() {
    let harness = harness(ShellId::Worker, false);
    let description = harness.tool.description().to_owned();
    assert!(description.contains("plan (read-only)"), "{description}");
    assert!(description.contains("worker (worker)"), "{description}");
    assert!(!description.contains("task,"), "{description}");
}

#[tokio::test]
async fn refuses_a_mode_that_does_not_exist_naming_the_ones_that_do() {
    let harness = harness(ShellId::Worker, false);
    let error = harness
        .run(json!({ "tasks": ["t"], "mode": "scout" }))
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("Available: plan, worker"),
        "{}",
        error.message
    );
}

// ── a read-only session ───────────────────────────────────────────────

#[tokio::test]
async fn cannot_delegate_work_that_changes_the_workspace() {
    let harness = harness(ShellId::ReadOnly, false);
    let error = harness
        .run(json!({ "tasks": ["t"], "mode": "worker" }))
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("exceeds the read-only shell"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn can_still_delegate_read_only_work() {
    let harness = harness(ShellId::ReadOnly, false);
    assert!(
        harness
            .run(json!({ "tasks": ["t"], "mode": "plan" }))
            .await
            .is_ok()
    );
}

// ── running tasks ─────────────────────────────────────────────────────

#[tokio::test]
async fn runs_one_subagent_per_task_and_returns_each_answer() {
    let harness = harness(ShellId::Worker, false);
    let result = harness
        .run(json!({ "tasks": ["first task", "second task"], "mode": "worker" }))
        .await
        .expect("ran");
    assert_eq!(harness.calls.load(Ordering::SeqCst), 2);
    let text = text_of(&result);
    assert!(text.contains("answer 1"), "{text}");
    assert!(text.contains("answer 2"), "{text}");
}

#[tokio::test]
async fn labels_each_answer_with_the_mode_and_the_child_it_came_from() {
    let harness = harness(ShellId::Worker, false);
    let result = harness
        .run(json!({ "tasks": ["only task"], "mode": "plan" }))
        .await
        .expect("ran");
    let text = text_of(&result);
    assert!(
        text.contains("<subagent mode=\"plan\" session=\""),
        "{text}"
    );
}

#[tokio::test]
async fn reports_one_result_entry_per_task() {
    let harness = harness(ShellId::Worker, false);
    let result = harness
        .run(json!({ "tasks": ["a task", "b task"], "mode": "worker" }))
        .await
        .expect("ran");
    assert_eq!(results_of(&result).len(), 2);
}

// ── what one call may ask for ─────────────────────────────────────────

#[tokio::test]
async fn refuses_the_same_task_twice_in_one_call() {
    let harness = harness(ShellId::Worker, false);
    let error = harness
        .run(json!({ "tasks": ["inspect the parser", "Inspect the parser."], "mode": "worker" }))
        .await
        .expect_err("refused");
    assert!(error.message.contains("Duplicate"), "{}", error.message);
}

#[tokio::test]
async fn allows_the_same_task_in_a_later_call() {
    let harness = harness(ShellId::Worker, false);
    harness
        .run(json!({ "tasks": ["inspect the parser"], "mode": "worker" }))
        .await
        .expect("ran");
    assert!(
        harness
            .run(json!({ "tasks": ["inspect the parser"], "mode": "worker" }))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn refuses_more_tasks_than_one_call_may_carry() {
    let harness = harness(ShellId::Worker, false);
    let tasks: Vec<String> = (0..9).map(|index| format!("task {index}")).collect();
    let error = harness
        .run(json!({ "tasks": tasks, "mode": "worker" }))
        .await
        .expect_err("refused");
    assert!(error.message.contains("maximum is 8"), "{}", error.message);
}

#[tokio::test]
async fn refuses_an_empty_request() {
    let harness = harness(ShellId::Worker, false);
    let error = harness
        .run(json!({ "tasks": [], "mode": "worker" }))
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("at least one non-empty task"),
        "{}",
        error.message
    );
}

// ── delegating in the background ──────────────────────────────────────

#[tokio::test]
async fn hides_the_flag_entirely_when_the_session_cannot_observe_a_task() {
    let harness = harness(ShellId::Worker, false);
    let properties = harness.tool.parameters()["properties"]
        .as_object()
        .expect("properties");
    assert!(!properties.contains_key("run_in_background"));
    assert!(!harness.tool.description().contains("run_in_background"));
}

#[tokio::test]
async fn offers_the_flag_once_the_session_can() {
    let harness = harness(ShellId::Worker, true);
    assert!(
        harness.tool.parameters()["properties"]
            .as_object()
            .expect("properties")
            .contains_key("run_in_background")
    );
}

#[tokio::test]
async fn refuses_the_flag_when_it_is_not_available() {
    let harness = harness(ShellId::Worker, false);
    let error = harness
        .run(json!({ "tasks": ["t"], "mode": "worker", "run_in_background": true }))
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("not available here"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn returns_a_task_id_immediately_instead_of_the_answer() {
    let harness = harness(ShellId::Worker, true);
    let result = harness
        .run(
            json!({ "tasks": ["long investigation"], "mode": "worker", "run_in_background": true }),
        )
        .await
        .expect("ran");
    let text = text_of(&result);
    assert!(matches_task_id(&text, "agent-"), "{text}");
    assert!(text.contains("status: running"), "{text}");
    assert!(!text.contains("answer 1"), "{text}");
    assert_eq!(harness.manager.list(true, None).len(), 1);
}

#[tokio::test]
async fn refuses_to_detach_and_continue_an_existing_subagent_at_the_same_time() {
    let harness = harness(ShellId::Worker, true);
    let first = harness
        .run(json!({ "tasks": ["start"], "mode": "worker" }))
        .await
        .expect("ran");
    let session_id = results_of(&first)[0]["sessionId"]
        .as_str()
        .expect("session")
        .to_owned();
    let error = harness
        .run(json!({
            "tasks": ["carry on"],
            "mode": "worker",
            "session_id": session_id,
            "run_in_background": true,
        }))
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("cannot be combined"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn names_the_session_id_rather_than_the_task_id_as_the_way_to_continue_it() {
    let harness = harness(ShellId::Worker, true);
    let result = harness
        .run(
            json!({ "tasks": ["long investigation"], "mode": "worker", "run_in_background": true }),
        )
        .await
        .expect("ran");
    assert!(text_of(&result).contains("continue_with: session_id"));
}

// ── continuing a subagent ─────────────────────────────────────────────

#[tokio::test]
async fn continues_the_child_the_id_names() {
    let harness = harness(ShellId::Worker, false);
    let first = harness
        .run(json!({ "tasks": ["start something"], "mode": "worker" }))
        .await
        .expect("ran");
    let session_id = results_of(&first)[0]["sessionId"]
        .as_str()
        .expect("session")
        .to_owned();
    assert!(!session_id.is_empty());

    let second = harness
        .run(json!({ "tasks": ["carry on"], "mode": "worker", "session_id": session_id }))
        .await
        .expect("ran");
    assert_eq!(harness.calls.load(Ordering::SeqCst), 2);
    assert!(
        text_of(&second).contains(&format!("session=\"{session_id}\"")),
        "{}",
        text_of(&second)
    );
}

#[tokio::test]
async fn refuses_an_id_it_never_handed_out() {
    let harness = harness(ShellId::Worker, false);
    let error = harness
        .run(json!({ "tasks": ["t"], "mode": "worker", "session_id": "made-up" }))
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("Unknown subagent session"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn refuses_to_continue_a_child_in_a_different_mode_than_it_ran_in() {
    let harness = harness(ShellId::Worker, false);
    let first = harness
        .run(json!({ "tasks": ["start something"], "mode": "worker" }))
        .await
        .expect("ran");
    let session_id = results_of(&first)[0]["sessionId"]
        .as_str()
        .expect("session")
        .to_owned();
    let error = harness
        .run(json!({ "tasks": ["carry on"], "mode": "plan", "session_id": session_id }))
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("runs in mode \"worker\""),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn refuses_to_continue_with_several_tasks_at_once() {
    let harness = harness(ShellId::Worker, false);
    let error = harness
        .run(json!({ "tasks": ["a", "b"], "mode": "worker", "session_id": "anything" }))
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("takes a single task"),
        "{}",
        error.message
    );
}

// ── without a session ─────────────────────────────────────────────────

#[tokio::test]
async fn says_delegation_is_unavailable_rather_than_failing_obscurely() {
    let tool = create_task_tool_definition(None);
    let error = tool
        .execute(
            "call-1",
            json!({ "tasks": ["t"], "mode": "plan" }),
            None,
            None,
            None,
        )
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("Unknown mode") || error.message.contains("not available"),
        "{}",
        error.message
    );
}
