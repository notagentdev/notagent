use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use notagent::core::hooks::Hook;
use notagent::core::hooks::events::HookEvent;
use notagent::core::hooks::payload::HookSessionContext;
use notagent::core::hooks::runtime::{HookRuntime, HookRuntimeOptions};
use notagent::core::modes::shells::{ApprovalLevel, ShellId};
use notagent::core::permissions::chain::build_policy_chain;
use notagent::core::permissions::coordinator::{ApprovalCoordinator, ApprovalPresenter};
use notagent::core::permissions::gate::{PermissionGate, PermissionGateOptions};
use notagent::core::permissions::hook::{
    PermissionCall, PermissionHandler, PermissionHookOptions, PermissionHookResult,
    PermissionSessionState, create_permission_handler,
};
use notagent::core::permissions::request::{ApprovalAnswer, ApprovalRequest};
use notagent::core::tools::tool_definition::ToolDefinition;
use notagent::core::tools::write::create_write_tool_definition;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

struct Workspace {
    path: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("permission-e2e-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        Self { path }
    }

    fn cwd(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    fn join(&self, name: &str) -> String {
        self.path.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn input(arguments: Value) -> Map<String, Value> {
    arguments.as_object().cloned().unwrap_or_default()
}

fn permission_call(tool_name: &str, arguments: &Map<String, Value>) -> PermissionCall {
    PermissionCall {
        tool_call_id: "call-1".to_string(),
        tool_name: tool_name.to_string(),
        input: arguments.clone(),
    }
}

fn state(
    cwd: &str,
    approval: ApprovalLevel,
) -> Arc<dyn Fn() -> PermissionSessionState + Send + Sync> {
    let cwd = cwd.to_string();
    Arc::new(move || PermissionSessionState {
        mode_id: Some("manual".to_string()),
        shell: Some(ShellId::Worker),
        approval,
        cwd: cwd.clone(),
    })
}

/// A presenter that records what it was shown and answers as the test says.
fn recording(answer: ApprovalAnswer) -> (ApprovalPresenter, Arc<Mutex<Vec<String>>>) {
    let shown = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&shown);
    let presenter: ApprovalPresenter = Arc::new(move |request: ApprovalRequest| {
        recorded
            .lock()
            .expect("shown")
            .push(request.target.clone().unwrap_or_default());
        Box::pin(async move { answer }) as BoxFuture<'static, ApprovalAnswer>
    });
    (presenter, shown)
}

/// A dialog that never answers, standing in for one waiting on a human.
fn unanswered() -> ApprovalPresenter {
    Arc::new(|_request| {
        Box::pin(async { std::future::pending::<ApprovalAnswer>().await })
            as BoxFuture<'static, ApprovalAnswer>
    })
}

fn pre_tool_use(command: &str, matcher: Option<&str>) -> Hook {
    Hook {
        event: HookEvent::PreToolUse,
        matcher: matcher.map(str::to_string),
        command: command.to_string(),
        timeout_ms: 10_000,
        source: "e2e".to_string(),
    }
}

struct Harness {
    handler: PermissionHandler,
    shown: Arc<Mutex<Vec<String>>>,
}

fn harness(
    cwd: &str,
    approval: ApprovalLevel,
    answer: ApprovalAnswer,
    hooks: Vec<Hook>,
) -> Harness {
    let (presenter, shown) = recording(answer);
    let coordinator = Arc::new(ApprovalCoordinator::new(presenter));
    let session_cwd = cwd.to_string();
    let runtime = Arc::new(HookRuntime::new(HookRuntimeOptions {
        hooks,
        diagnostics: Vec::new(),
        context: Arc::new(move || HookSessionContext {
            session_id: "e2e".to_string(),
            transcript_path: None,
            cwd: session_cwd.clone(),
        }),
        report: None,
        signal: None,
    }));
    let handler = create_permission_handler(PermissionHookOptions {
        policies: build_policy_chain(&[]),
        coordinator: Arc::clone(&coordinator),
        state: state(cwd, approval),
        decide: Some(Arc::new(move |call: PermissionCall| {
            let runtime = Arc::clone(&runtime);
            Box::pin(async move {
                let fields = input(json!({
                    "tool_call_id": call.tool_call_id,
                    "tool_name": call.tool_name,
                    "tool_input": call.input,
                }));
                runtime
                    .decide(HookEvent::PreToolUse, fields, Some("write"))
                    .await
                    .verdict
            }) as BoxFuture<'static, _>
        })),
    });
    Harness { handler, shown }
}

struct Attempt {
    blocked: bool,
    reason: Option<String>,
    shown: Vec<String>,
}

/// Runs the tool only when the handler allows it, as the runner does.
async fn attempt_write(
    cwd: &str,
    path: &str,
    content: &str,
    approval: ApprovalLevel,
    answer: ApprovalAnswer,
    hooks: Vec<Hook>,
) -> Attempt {
    let harness = harness(cwd, approval, answer, hooks);
    let arguments = input(json!({ "path": path, "content": content }));
    let result = harness
        .handler
        .call(permission_call("write", &arguments), None)
        .await;
    let shown = harness.shown.lock().expect("shown").clone();
    if let Some(PermissionHookResult {
        block: true,
        reason,
    }) = result
    {
        return Attempt {
            blocked: true,
            reason,
            shown,
        };
    }

    let tool = create_write_tool_definition(cwd, None);
    tool.execute("t", Value::Object(arguments), None, None, None)
        .await
        .expect("writes");
    Attempt {
        blocked: false,
        reason: None,
        shown,
    }
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).expect("reads")
}

// ---------------------------------------------------------------------------
// denied calls never touch the file system
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refused_write_leaves_the_file_absent() {
    let workspace = Workspace::new();
    let target = workspace.path.parent().expect("parent").join("outside.txt");
    let target = target.to_string_lossy().into_owned();
    let outcome = attempt_write(
        &workspace.cwd(),
        &target,
        "payload",
        ApprovalLevel::Manual,
        ApprovalAnswer::Deny,
        Vec::new(),
    )
    .await;
    assert!(outcome.blocked);
    assert!(!Path::new(&target).exists());
}

#[tokio::test]
async fn a_refused_overwrite_leaves_the_original_content_intact() {
    let workspace = Workspace::new();
    let target = workspace.join(".env");
    std::fs::write(&target, "ORIGINAL").expect("writes");
    let outcome = attempt_write(
        &workspace.cwd(),
        &target,
        "REPLACED",
        ApprovalLevel::Auto,
        ApprovalAnswer::Deny,
        Vec::new(),
    )
    .await;
    assert!(outcome.blocked);
    assert_eq!(read(&target), "ORIGINAL");
}

#[tokio::test]
async fn the_refusal_names_the_policy_that_caused_it() {
    let workspace = Workspace::new();
    let outcome = attempt_write(
        &workspace.cwd(),
        &workspace.join(".env"),
        "x",
        ApprovalLevel::Auto,
        ApprovalAnswer::Deny,
        Vec::new(),
    )
    .await;
    assert!(
        outcome
            .reason
            .unwrap_or_default()
            .contains("sensitive-file-access-ask")
    );
}

// ---------------------------------------------------------------------------
// approved calls run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_allowed_write_reaches_the_file_system() {
    let workspace = Workspace::new();
    let target = workspace.join("notes.txt");
    let outcome = attempt_write(
        &workspace.cwd(),
        &target,
        "written",
        ApprovalLevel::Manual,
        ApprovalAnswer::ApproveOnce,
        Vec::new(),
    )
    .await;
    assert!(!outcome.blocked);
    assert_eq!(read(&target), "written");
}

#[tokio::test]
async fn an_ordinary_write_inside_the_working_directory_is_not_asked_about_at_all() {
    let workspace = Workspace::new();
    let outcome = attempt_write(
        &workspace.cwd(),
        &workspace.join("src.txt"),
        "x",
        ApprovalLevel::Manual,
        ApprovalAnswer::Deny,
        Vec::new(),
    )
    .await;
    // The answer would have been deny; it is never requested, because the
    // chain approves this case before reaching the asking policies.
    assert!(!outcome.blocked);
    assert!(outcome.shown.is_empty());
}

#[tokio::test]
async fn auto_mode_writes_without_a_prompt() {
    let workspace = Workspace::new();
    let outcome = attempt_write(
        &workspace.cwd(),
        &workspace.join("auto.txt"),
        "x",
        ApprovalLevel::Auto,
        ApprovalAnswer::Deny,
        Vec::new(),
    )
    .await;
    assert!(!outcome.blocked);
    assert!(outcome.shown.is_empty());
}

// ---------------------------------------------------------------------------
// guards bind auto, not yolo
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auto_still_asks_before_touching_a_credentials_file() {
    let workspace = Workspace::new();
    let outcome = attempt_write(
        &workspace.cwd(),
        &workspace.join(".env"),
        "x",
        ApprovalLevel::Auto,
        ApprovalAnswer::ApproveOnce,
        Vec::new(),
    )
    .await;
    assert_eq!(outcome.shown, vec![".env"]);
    assert!(!outcome.blocked);
}

#[tokio::test]
async fn auto_still_asks_before_writing_into_the_git_control_directory() {
    let workspace = Workspace::new();
    let target = workspace
        .path
        .join(".git")
        .join("config")
        .to_string_lossy()
        .into_owned();
    let outcome = attempt_write(
        &workspace.cwd(),
        &target,
        "x",
        ApprovalLevel::Auto,
        ApprovalAnswer::Deny,
        Vec::new(),
    )
    .await;
    assert!(outcome.blocked);
    assert!(!Path::new(&target).exists());
}

#[tokio::test]
async fn yolo_asks_about_nothing_which_is_what_the_name_promises() {
    let workspace = Workspace::new();
    let target = workspace.join(".env");
    let outcome = attempt_write(
        &workspace.cwd(),
        &target,
        "written",
        ApprovalLevel::Yolo,
        ApprovalAnswer::Deny,
        Vec::new(),
    )
    .await;
    assert!(outcome.shown.is_empty());
    assert!(!outcome.blocked);
    assert_eq!(read(&target), "written");
}

// ---------------------------------------------------------------------------
// a user rule written as a hook
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn a_hook_stops_a_write_that_yolo_would_have_permitted() {
    let workspace = Workspace::new();
    let target = workspace.join("generated.txt");
    let outcome = attempt_write(
        &workspace.cwd(),
        &target,
        "x",
        ApprovalLevel::Yolo,
        ApprovalAnswer::ApproveOnce,
        vec![pre_tool_use(
            "echo 'generated files are not editable' >&2; exit 2",
            None,
        )],
    )
    .await;
    assert!(outcome.blocked);
    let reason = outcome.reason.unwrap_or_default();
    assert!(
        reason.contains("generated files are not editable"),
        "{reason}"
    );
    assert!(reason.contains("user-configured-deny"), "{reason}");
    assert!(!Path::new(&target).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_hook_permits_a_credentials_file_the_guard_would_have_asked_about() {
    let workspace = Workspace::new();
    let target = workspace.join(".env");
    let outcome = attempt_write(
        &workspace.cwd(),
        &target,
        "TOKEN=1",
        ApprovalLevel::Manual,
        ApprovalAnswer::Deny,
        vec![pre_tool_use(
            "echo '{\"hook_output\":{\"decision\":\"allow\"}}'",
            None,
        )],
    )
    .await;
    assert!(outcome.shown.is_empty());
    assert_eq!(read(&target), "TOKEN=1");
}

#[cfg(unix)]
#[tokio::test]
async fn a_hook_asks_about_an_ordinary_write_that_nothing_would_have_asked_about() {
    let workspace = Workspace::new();
    let target = workspace.join("src.txt");
    let outcome = attempt_write(
        &workspace.cwd(),
        &target,
        "x",
        ApprovalLevel::Manual,
        ApprovalAnswer::Deny,
        vec![pre_tool_use(
            "echo '{\"hook_output\":{\"decision\":\"ask\",\"reason\":\"review writes\"}}'",
            None,
        )],
    )
    .await;
    assert_eq!(outcome.shown, vec!["src.txt"]);
    assert!(outcome.blocked);
    assert!(!Path::new(&target).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_hook_applies_only_to_the_tools_its_matcher_names() {
    let workspace = Workspace::new();
    let target = workspace.join("kept.txt");
    let outcome = attempt_write(
        &workspace.cwd(),
        &target,
        "x",
        ApprovalLevel::Manual,
        ApprovalAnswer::Deny,
        vec![pre_tool_use("exit 2", Some("edit"))],
    )
    .await;
    assert!(!outcome.blocked);
    assert_eq!(read(&target), "x");
}

#[cfg(unix)]
#[tokio::test]
async fn a_hook_leaves_the_ordinary_path_alone_when_it_abstains() {
    let workspace = Workspace::new();
    let target = workspace.join("kept.txt");
    let outcome = attempt_write(
        &workspace.cwd(),
        &target,
        "x",
        ApprovalLevel::Manual,
        ApprovalAnswer::Deny,
        vec![pre_tool_use("true", None)],
    )
    .await;
    assert!(!outcome.blocked);
    assert_eq!(read(&target), "x");
}

// ---------------------------------------------------------------------------
// abort and interrupting a turn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refuses_a_call_whose_approval_was_pending_when_the_session_aborted() {
    let workspace = Workspace::new();
    let target = workspace.join(".env");
    // because the handler is suspended at its first `await` when `abort()`
    // runs. A Rust future polls straight through that point, so "the approval
    // was still pending" is spelled out with a presenter that has not answered.
    let coordinator = Arc::new(ApprovalCoordinator::new(unanswered()));
    let handler = create_permission_handler(PermissionHookOptions {
        policies: build_policy_chain(&[]),
        coordinator: Arc::clone(&coordinator),
        state: state(&workspace.cwd(), ApprovalLevel::Auto),
        decide: None,
    });
    let arguments = input(json!({ "path": target, "content": "x" }));
    let call = handler.call(permission_call("write", &arguments), None);
    let abort = async {
        tokio::task::yield_now().await;
        coordinator.abort();
    };
    let (result, ()) = tokio::join!(call, abort);
    assert_eq!(result.map(|result| result.block), Some(true));
    assert!(!Path::new(&target).exists());
}

fn unanswered_handler(cwd: &str) -> PermissionHandler {
    create_permission_handler(PermissionHookOptions {
        policies: build_policy_chain(&[]),
        coordinator: Arc::new(ApprovalCoordinator::new(unanswered())),
        state: state(cwd, ApprovalLevel::Auto),
        decide: None,
    })
}

#[tokio::test]
async fn settles_a_prompt_nobody_answered_so_the_interrupt_itself_can_finish() {
    let workspace = Workspace::new();
    let target = workspace.join(".env");
    let handler = unanswered_handler(&workspace.cwd());
    let turn = CancellationToken::new();
    let arguments = input(json!({ "path": target, "content": "x" }));
    let call = handler.call(permission_call("write", &arguments), Some(&turn));
    let cancel = async {
        tokio::task::yield_now().await;
        turn.cancel();
    };
    let (result, ()) = tokio::join!(call, cancel);
    let result = result.expect("blocks");
    assert!(result.block);
    assert!(result.reason.unwrap_or_default().contains("interrupted"));
    assert!(!Path::new(&target).exists());
}

#[tokio::test]
async fn does_not_blame_the_user_for_a_prompt_they_never_saw() {
    let workspace = Workspace::new();
    let handler = unanswered_handler(&workspace.cwd());
    let turn = CancellationToken::new();
    let arguments = input(json!({ "path": workspace.join(".env"), "content": "x" }));
    let call = handler.call(permission_call("write", &arguments), Some(&turn));
    let cancel = async {
        tokio::task::yield_now().await;
        turn.cancel();
    };
    let (result, ()) = tokio::join!(call, cancel);
    assert!(
        !result
            .expect("blocks")
            .reason
            .unwrap_or_default()
            .contains("Denied by the user")
    );
}

#[tokio::test]
async fn refuses_without_asking_once_the_turn_is_already_gone() {
    let workspace = Workspace::new();
    let (presenter, shown) = recording(ApprovalAnswer::ApproveOnce);
    let handler = create_permission_handler(PermissionHookOptions {
        policies: build_policy_chain(&[]),
        coordinator: Arc::new(ApprovalCoordinator::new(presenter)),
        state: state(&workspace.cwd(), ApprovalLevel::Auto),
        decide: None,
    });
    let turn = CancellationToken::new();
    turn.cancel();
    let arguments = input(json!({ "path": workspace.join(".env"), "content": "x" }));
    let result = handler
        .call(permission_call("write", &arguments), Some(&turn))
        .await;
    assert_eq!(result.map(|result| result.block), Some(true));
    assert!(shown.lock().expect("shown").is_empty());
}

#[tokio::test]
async fn leaves_the_next_turn_free_to_ask_again() {
    let workspace = Workspace::new();
    let (presenter, shown) = recording(ApprovalAnswer::ApproveOnce);
    let handler = create_permission_handler(PermissionHookOptions {
        policies: build_policy_chain(&[]),
        coordinator: Arc::new(ApprovalCoordinator::new(presenter)),
        state: state(&workspace.cwd(), ApprovalLevel::Auto),
        decide: None,
    });
    let arguments = input(json!({ "path": workspace.join(".env"), "content": "x" }));
    let gone = CancellationToken::new();
    gone.cancel();
    handler
        .call(permission_call("write", &arguments), Some(&gone))
        .await;
    let fresh = CancellationToken::new();
    assert!(
        handler
            .call(permission_call("write", &arguments), Some(&fresh))
            .await
            .is_none()
    );
    assert_eq!(*shown.lock().expect("shown"), vec![".env"]);
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

fn gate(cwd: &str, approval: ApprovalLevel, presenter: ApprovalPresenter) -> PermissionGate {
    PermissionGate::new(PermissionGateOptions {
        state: state(cwd, approval),
        present: presenter,
        policies: Vec::new(),
        decide: None,
    })
}

#[tokio::test]
async fn the_gate_lets_an_ordinary_in_directory_write_through_untouched() {
    let workspace = Workspace::new();
    let (presenter, shown) = recording(ApprovalAnswer::Deny);
    let gate = gate(&workspace.cwd(), ApprovalLevel::Manual, presenter);
    let arguments = input(json!({ "path": workspace.join("a.txt") }));
    assert!(
        gate.before_tool_call(permission_call("write", &arguments), None)
            .await
            .is_none()
    );
    assert!(shown.lock().expect("shown").is_empty());
}

#[tokio::test]
async fn the_gate_blocks_a_refused_call_and_ends_the_batch() {
    let workspace = Workspace::new();
    let (presenter, _shown) = recording(ApprovalAnswer::Deny);
    let gate = gate(&workspace.cwd(), ApprovalLevel::Auto, presenter);
    let arguments = input(json!({ "path": workspace.join(".env") }));
    let block = gate
        .before_tool_call(permission_call("write", &arguments), None)
        .await
        .expect("blocks");
    assert!(block.terminate);
    assert!(
        block
            .reason
            .unwrap_or_default()
            .contains("sensitive-file-access-ask")
    );
}

#[tokio::test]
async fn the_gate_keeps_a_blocked_write_off_the_file_system() {
    let workspace = Workspace::new();
    let target = workspace.join(".env");
    let (presenter, _shown) = recording(ApprovalAnswer::Deny);
    let gate = gate(&workspace.cwd(), ApprovalLevel::Auto, presenter);
    let arguments = input(json!({ "path": target, "content": "secret" }));
    let block = gate
        .before_tool_call(permission_call("write", &arguments), None)
        .await;
    assert!(block.is_some());
    if block.is_none() {
        let tool = create_write_tool_definition(&workspace.cwd(), None);
        tool.execute("t", Value::Object(arguments), None, None, None)
            .await
            .expect("writes");
    }
    assert!(!Path::new(&target).exists());
}

#[tokio::test]
async fn the_gate_runs_an_approved_write() {
    let workspace = Workspace::new();
    let target = workspace.join(".env");
    let (presenter, _shown) = recording(ApprovalAnswer::ApproveOnce);
    let gate = gate(&workspace.cwd(), ApprovalLevel::Auto, presenter);
    let arguments = input(json!({ "path": target, "content": "ok" }));
    assert!(
        gate.before_tool_call(permission_call("write", &arguments), None)
            .await
            .is_none()
    );
    let tool = create_write_tool_definition(&workspace.cwd(), None);
    tool.execute("t", Value::Object(arguments), None, None, None)
        .await
        .expect("writes");
    assert_eq!(read(&target), "ok");
}

#[tokio::test]
async fn the_gate_refuses_everything_once_aborted() {
    let workspace = Workspace::new();
    let (presenter, shown) = recording(ApprovalAnswer::ApproveOnce);
    let gate = gate(&workspace.cwd(), ApprovalLevel::Auto, presenter);
    gate.abort();
    let arguments = input(json!({ "path": workspace.join(".env") }));
    assert!(
        gate.before_tool_call(permission_call("write", &arguments), None)
            .await
            .is_some()
    );
    assert!(shown.lock().expect("shown").is_empty());
}

#[tokio::test]
async fn the_gate_asks_again_after_a_reset() {
    let workspace = Workspace::new();
    let (presenter, shown) = recording(ApprovalAnswer::ApproveOnce);
    let gate = gate(&workspace.cwd(), ApprovalLevel::Auto, presenter);
    gate.abort();
    gate.reset();
    let arguments = input(json!({ "path": workspace.join(".env") }));
    gate.before_tool_call(permission_call("write", &arguments), None)
        .await;
    assert_eq!(*shown.lock().expect("shown"), vec![".env"]);
}

#[tokio::test]
async fn the_gate_settles_the_prompt_an_interrupted_turn_was_waiting_on() {
    let workspace = Workspace::new();
    let gate = gate(&workspace.cwd(), ApprovalLevel::Auto, unanswered());
    let turn = CancellationToken::new();
    let arguments = input(json!({ "path": workspace.join(".env") }));
    let call = gate.before_tool_call(permission_call("write", &arguments), Some(&turn));
    let cancel = async {
        tokio::task::yield_now().await;
        turn.cancel();
    };
    let (block, ()) = tokio::join!(call, cancel);
    assert!(block.is_some());
}

#[tokio::test]
async fn the_gate_settles_the_calls_queued_behind_it_too() {
    let workspace = Workspace::new();
    let gate = Arc::new(gate(&workspace.cwd(), ApprovalLevel::Auto, unanswered()));
    let turn = CancellationToken::new();
    let first_arguments = input(json!({ "path": workspace.join(".env") }));
    let second_arguments = input(json!({ "path": workspace.join(".netrc") }));
    let first = gate.before_tool_call(permission_call("write", &first_arguments), Some(&turn));
    let second = gate.before_tool_call(permission_call("write", &second_arguments), Some(&turn));
    let cancel = async {
        tokio::task::yield_now().await;
        turn.cancel();
    };
    let (first, second, ()) = tokio::join!(first, second, cancel);
    assert!(first.is_some());
    assert!(second.is_some());
}

#[tokio::test]
async fn the_gate_decides_without_a_signal_as_a_session_that_has_none_does() {
    let workspace = Workspace::new();
    let (presenter, shown) = recording(ApprovalAnswer::ApproveOnce);
    let gate = gate(&workspace.cwd(), ApprovalLevel::Auto, presenter);
    let arguments = input(json!({ "path": workspace.join(".env") }));
    assert!(
        gate.before_tool_call(permission_call("write", &arguments), None)
            .await
            .is_none()
    );
    assert_eq!(*shown.lock().expect("shown"), vec![".env"]);
}

#[tokio::test]
async fn the_gate_denies_what_is_outstanding_when_the_session_is_torn_down() {
    let workspace = Workspace::new();
    let gate = Arc::new(gate(&workspace.cwd(), ApprovalLevel::Auto, unanswered()));
    let arguments = input(json!({ "path": workspace.join(".env") }));
    let call = gate.before_tool_call(permission_call("write", &arguments), None);
    let shutdown = async {
        tokio::task::yield_now().await;
        gate.session_shutdown();
    };
    let (block, ()) = tokio::join!(call, shutdown);
    assert!(block.is_some());
}

#[tokio::test]
async fn the_gate_starts_a_replaced_session_over_rather_than_inheriting_its_answers() {
    let workspace = Workspace::new();
    let (presenter, shown) = recording(ApprovalAnswer::ApproveAlways);
    let gate = gate(&workspace.cwd(), ApprovalLevel::Auto, presenter);
    let arguments = input(json!({ "path": workspace.join(".env") }));
    gate.before_tool_call(permission_call("write", &arguments), None)
        .await;
    gate.before_tool_call(permission_call("write", &arguments), None)
        .await;
    assert_eq!(*shown.lock().expect("shown"), vec![".env"]);

    gate.session_start();
    gate.before_tool_call(permission_call("write", &arguments), None)
        .await;
    assert_eq!(*shown.lock().expect("shown"), vec![".env", ".env"]);
}

#[tokio::test]
async fn the_gate_clears_a_teardown_denial_when_a_session_starts() {
    let workspace = Workspace::new();
    let (presenter, _shown) = recording(ApprovalAnswer::ApproveOnce);
    let gate = gate(&workspace.cwd(), ApprovalLevel::Auto, presenter);
    let arguments = input(json!({ "path": workspace.join(".env") }));
    gate.session_shutdown();
    assert!(
        gate.before_tool_call(permission_call("write", &arguments), None)
            .await
            .is_some()
    );
    gate.session_start();
    assert!(
        gate.before_tool_call(permission_call("write", &arguments), None)
            .await
            .is_none()
    );
}
