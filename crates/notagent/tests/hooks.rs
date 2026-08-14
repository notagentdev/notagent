//! Port of `packages/coding-agent/test/hooks.test.ts`, `hooks-runner.test.ts`
//! and `hooks-runtime.test.ts`.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use notagent::core::hooks::events::{HOOK_EVENTS, HookEvent};
use notagent::core::hooks::payload::HookSessionContext;
use notagent::core::hooks::runner::{
    HookVerdict, decide_tool_call, describe_refusal, is_hook_fault, read_verdict, run_hook,
    run_hooks,
};
use notagent::core::hooks::runtime::{HookReportLevel, HookRuntime, HookRuntimeOptions};
use notagent::core::hooks::{
    DEFAULT_HOOK_TIMEOUT_MS, HOOKS_FILE_NAME, Hook, HookDiagnostic, MAX_HOOK_TIMEOUT_MS,
    is_blocking_hook, load_hooks, matches_tool, select_hooks,
};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let directory = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn dir_with(content: Value) -> TempDir {
    let directory = TempDir::new("hooks-");
    std::fs::write(directory.path.join(HOOKS_FILE_NAME), content.to_string()).expect("writes");
    directory
}

/// The declaration every case starts from: a PreToolUse hook that echoes.
fn declaration(overrides: Value) -> Value {
    let mut entry = json!({ "event": "PreToolUse", "command": "echo hi" });
    let object = entry.as_object_mut().expect("object");
    for (key, value) in overrides.as_object().cloned().unwrap_or_default() {
        object.insert(key, value);
    }
    entry
}

fn hook(command: &str) -> Hook {
    Hook {
        event: HookEvent::PreToolUse,
        matcher: None,
        command: command.to_string(),
        timeout_ms: 10_000.0,
        source: "test".to_string(),
    }
}

fn hook_with(command: &str, event: HookEvent, timeout_ms: f64, matcher: Option<&str>) -> Hook {
    Hook {
        event,
        matcher: matcher.map(str::to_string),
        command: command.to_string(),
        timeout_ms,
        source: "test".to_string(),
    }
}

fn messages(diagnostics: &[HookDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// event vocabulary
// ---------------------------------------------------------------------------

#[test]
fn carries_all_sixteen_reference_events_each_once() {
    assert_eq!(HOOK_EVENTS.len(), 16);
    let mut names: Vec<&str> = HOOK_EVENTS.iter().map(|event| event.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 16);
}

#[test]
fn uses_user_prompt_submit_rather_than_the_consumers_spelling() {
    assert!(HookEvent::parse("UserPromptSubmit").is_some());
    assert!(HookEvent::parse("beforeSubmitPrompt").is_none());
}

#[test]
fn rejects_anything_not_in_the_list() {
    assert!(HookEvent::parse("PreToolUseX").is_none());
    assert!(HookEvent::parse("42").is_none());
}

// ---------------------------------------------------------------------------
// loading declarations
// ---------------------------------------------------------------------------

#[test]
fn reads_a_plain_array_and_an_object_with_a_hooks_key_alike() {
    let array = dir_with(json!([declaration(json!({}))]));
    let object = dir_with(json!({ "hooks": [declaration(json!({}))] }));
    assert_eq!(load_hooks(std::slice::from_ref(&array.path)).hooks.len(), 1);
    assert_eq!(
        load_hooks(std::slice::from_ref(&object.path)).hooks.len(),
        1
    );
}

#[test]
fn applies_the_default_timeout_when_none_is_given() {
    let directory = dir_with(json!([declaration(json!({}))]));
    assert_eq!(
        load_hooks(std::slice::from_ref(&directory.path)).hooks[0].timeout_ms,
        DEFAULT_HOOK_TIMEOUT_MS
    );
}

#[test]
fn caps_an_excessive_timeout_and_says_so() {
    let directory = dir_with(json!([declaration(json!({ "timeout": 999_999 }))]));
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert_eq!(result.hooks[0].timeout_ms, MAX_HOOK_TIMEOUT_MS);
    assert!(messages(&result.diagnostics).contains("capped"));
}

#[test]
fn names_the_valid_options_when_the_event_is_unknown() {
    let directory = dir_with(json!([declaration(json!({ "event": "Whenever" }))]));
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert!(result.hooks.is_empty());
    assert!(result.diagnostics[0].message.contains("PreToolUse"));
}

#[test]
fn rejects_a_hook_without_a_command() {
    let directory = dir_with(json!([declaration(json!({ "command": "  " }))]));
    assert!(
        load_hooks(std::slice::from_ref(&directory.path))
            .hooks
            .is_empty()
    );
}

#[test]
fn reports_invalid_json_rather_than_failing() {
    let directory = TempDir::new("hooks-");
    std::fs::write(directory.path.join(HOOKS_FILE_NAME), "{ not json").expect("writes");
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert!(result.hooks.is_empty());
    assert!(result.diagnostics[0].message.contains("invalid JSON"));
}

#[test]
fn ignores_a_directory_without_a_hooks_file() {
    let directory = TempDir::new("hooks-");
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert!(result.hooks.is_empty());
    assert!(result.diagnostics.is_empty());
}

#[test]
fn appends_across_directories_instead_of_replacing() {
    let user = dir_with(json!([declaration(json!({ "command": "user" }))]));
    let project = dir_with(json!([declaration(json!({ "command": "project" }))]));
    let commands: Vec<String> = load_hooks(&[user.path.clone(), project.path.clone()])
        .hooks
        .into_iter()
        .map(|hook| hook.command)
        .collect();
    assert_eq!(commands, vec!["user", "project"]);
}

#[test]
fn warns_that_a_matcher_on_a_non_tool_event_can_never_apply() {
    let directory = dir_with(json!([declaration(
        json!({ "event": "SessionStart", "matcher": "write" })
    )]));
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert!(messages(&result.diagnostics).contains("only tool events"));
}

#[test]
fn accepts_an_unemitted_event_but_says_it_will_not_fire() {
    let event = HOOK_EVENTS
        .iter()
        .find(|event| event.is_unemitted())
        .expect("one is unemitted");
    let directory = dir_with(json!([declaration(json!({ "event": event.as_str() }))]));
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert_eq!(result.hooks.len(), 1);
    assert!(messages(&result.diagnostics).contains("never fires yet"));
}

// ---------------------------------------------------------------------------
// matching
// ---------------------------------------------------------------------------

#[test]
fn matches_a_listed_tool_and_rejects_others() {
    assert!(matches_tool("write, edit", "write"));
    assert!(!matches_tool("write, edit", "read"));
}

#[test]
fn treats_a_star_as_every_tool() {
    assert!(matches_tool("*", "anything"));
}

#[test]
fn is_not_a_pattern_language_so_a_partial_name_does_not_match() {
    assert!(!matches_tool("write", "writer"));
    assert!(!matches_tool("wri*", "write"));
}

#[test]
fn selects_by_event_and_narrows_by_tool() {
    let directory = dir_with(json!([
        declaration(json!({ "command": "all" })),
        declaration(json!({ "command": "writes", "matcher": "write" })),
        declaration(json!({ "event": "SessionStart", "command": "start" })),
    ]));
    let hooks = load_hooks(std::slice::from_ref(&directory.path)).hooks;
    let commands = |event, tool| -> Vec<String> {
        select_hooks(&hooks, event, tool)
            .into_iter()
            .map(|hook| hook.command.clone())
            .collect()
    };
    assert_eq!(
        commands(HookEvent::PreToolUse, Some("write")),
        vec!["all", "writes"]
    );
    assert_eq!(commands(HookEvent::PreToolUse, Some("read")), vec!["all"]);
    assert_eq!(commands(HookEvent::SessionStart, None), vec!["start"]);
}

#[test]
fn marks_only_the_pre_tool_event_as_able_to_refuse() {
    let directory = dir_with(json!([
        declaration(json!({})),
        declaration(json!({ "event": "PostToolUse" })),
    ]));
    let hooks = load_hooks(std::slice::from_ref(&directory.path)).hooks;
    assert_eq!(
        hooks.iter().map(is_blocking_hook).collect::<Vec<_>>(),
        vec![true, false]
    );
}

// ---------------------------------------------------------------------------
// running one hook
// ---------------------------------------------------------------------------

const EMPTY: Value = Value::Null;

#[tokio::test]
async fn succeeds_on_a_zero_exit_and_captures_output() {
    let result = run_hook(&hook("echo hello"), &EMPTY, None).await;
    assert!(result.ok);
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout.trim(), "hello");
}

#[tokio::test]
async fn fails_on_a_non_zero_exit_and_keeps_what_was_written() {
    let result = run_hook(&hook("echo nope >&2; exit 3"), &EMPTY, None).await;
    assert!(!result.ok);
    assert_eq!(result.exit_code, Some(3));
    assert_eq!(result.stderr.trim(), "nope");
}

#[tokio::test]
async fn delivers_the_payload_as_json_on_standard_input() {
    let payload = json!({ "toolName": "write", "path": "a.ts" });
    let result = run_hook(&hook("cat"), &payload, None).await;
    assert_eq!(
        serde_json::from_str::<Value>(result.stdout.trim()).expect("json"),
        payload
    );
}

#[tokio::test]
async fn kills_a_hook_that_exceeds_its_timeout_and_reports_the_timeout() {
    let result = run_hook(
        &hook_with("sleep 30", HookEvent::PreToolUse, 200.0, None),
        &EMPTY,
        None,
    )
    .await;
    assert!(result.timed_out);
    assert!(!result.ok);
    assert!(result.duration_ms < 10_000);
}

#[tokio::test]
async fn reports_a_command_that_cannot_run_instead_of_failing() {
    let result = run_hook(&hook("this-command-does-not-exist-xyz"), &EMPTY, None).await;
    assert!(!result.ok);
}

#[tokio::test]
async fn does_not_wedge_when_the_hook_ignores_its_input() {
    let payload = json!({ "large": "x".repeat(200_000) });
    let result = run_hook(&hook("echo done"), &payload, None).await;
    assert!(result.ok);
    assert_eq!(result.stdout.trim(), "done");
}

#[tokio::test]
async fn stops_early_when_the_run_is_aborted() {
    let token = CancellationToken::new();
    let hook = hook_with("sleep 30", HookEvent::PreToolUse, 30_000.0, None);
    let run = run_hook(&hook, &EMPTY, Some(&token));
    let abort = async {
        tokio::task::yield_now().await;
        token.cancel();
    };
    let (result, ()) = tokio::join!(run, abort);
    assert!(!result.ok);
    assert!(result.duration_ms < 10_000);
}

// ---------------------------------------------------------------------------
// running several hooks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runs_them_in_declaration_order_one_after_another() {
    let first = hook("echo first");
    let second = hook("echo second");
    let results = run_hooks(&[&first, &second], &EMPTY, None).await;
    let outputs: Vec<String> = results
        .iter()
        .map(|result| result.stdout.trim().to_string())
        .collect();
    assert_eq!(outputs, vec!["first", "second"]);
}

#[tokio::test]
async fn keeps_going_after_one_fails() {
    let failing = hook("exit 1");
    let after = hook("echo after");
    let results = run_hooks(&[&failing, &after], &EMPTY, None).await;
    assert_eq!(
        results.iter().map(|result| result.ok).collect::<Vec<_>>(),
        vec![false, true]
    );
}

// ---------------------------------------------------------------------------
// reading what a hook decided
// ---------------------------------------------------------------------------

async fn verdict_of(command: &str) -> HookVerdict {
    read_verdict(&run_hook(&hook(command), &EMPTY, None).await)
}

#[tokio::test]
async fn abstains_on_a_plain_success() {
    assert_eq!(verdict_of("echo done").await, HookVerdict::Abstain);
}

#[tokio::test]
async fn denies_on_exit_two_the_status_a_hook_refuses_with() {
    assert!(matches!(
        verdict_of("exit 2").await,
        HookVerdict::Deny { .. }
    ));
}

#[tokio::test]
async fn treats_any_other_non_zero_exit_as_a_broken_hook_not_a_refusal() {
    // A typo in a command name leaves 127. Reading that as "no" would block
    // every tool call in the session while reporting nothing more useful
    // than the number.
    for code in [1, 3, 126, 127] {
        assert_eq!(
            verdict_of(&format!("exit {code}")).await,
            HookVerdict::Abstain
        );
    }
    assert_eq!(
        verdict_of("this-command-does-not-exist-xyz").await,
        HookVerdict::Abstain
    );
}

#[tokio::test]
async fn separates_a_fault_from_a_decision() {
    assert!(!is_hook_fault(
        &run_hook(&hook("exit 2"), &EMPTY, None).await
    ));
    assert!(is_hook_fault(
        &run_hook(&hook("exit 127"), &EMPTY, None).await
    ));
    assert!(!is_hook_fault(&run_hook(&hook("true"), &EMPTY, None).await));
}

#[tokio::test]
async fn still_denies_on_a_timeout_deliberately_unlike_the_reference() {
    let result = run_hook(
        &hook_with("sleep 30", HookEvent::PreToolUse, 200.0, None),
        &EMPTY,
        None,
    )
    .await;
    assert!(matches!(read_verdict(&result), HookVerdict::Deny { .. }));
    assert!(is_hook_fault(&result));
}

#[tokio::test]
async fn lets_a_broken_hook_still_state_a_decision_it_managed_to_print() {
    let verdict =
        verdict_of("echo '{\"permission\":\"deny\",\"reason\":\"policy\"}'; exit 127").await;
    assert_eq!(
        verdict,
        HookVerdict::Deny {
            reason: "policy".to_string()
        }
    );
}

#[tokio::test]
async fn reads_a_stated_decision_from_json_on_standard_output() {
    assert_eq!(
        verdict_of("echo '{\"permission\":\"allow\"}'").await,
        HookVerdict::Allow { reason: None }
    );
    assert_eq!(
        verdict_of("echo '{\"permission\":\"ask\",\"reason\":\"check first\"}'").await,
        HookVerdict::Ask {
            reason: Some("check first".to_string())
        }
    );
}

#[tokio::test]
async fn accepts_the_reference_spellings() {
    assert_eq!(
        verdict_of("echo '{\"decision\":\"approve\"}'").await,
        HookVerdict::Allow { reason: None }
    );
    assert_eq!(
        verdict_of("echo '{\"decision\":\"block\",\"reason\":\"no\"}'").await,
        HookVerdict::Deny {
            reason: "no".to_string()
        }
    );
}

#[tokio::test]
async fn finds_the_decision_after_other_output() {
    let verdict =
        verdict_of("echo working; echo '{\"permission\":\"deny\",\"reason\":\"nope\"}'").await;
    assert_eq!(
        verdict,
        HookVerdict::Deny {
            reason: "nope".to_string()
        }
    );
}

#[tokio::test]
async fn lets_a_stated_denial_override_a_zero_exit() {
    let verdict = verdict_of("echo '{\"permission\":\"deny\",\"reason\":\"no\"}'; exit 0").await;
    assert!(matches!(verdict, HookVerdict::Deny { .. }));
}

#[tokio::test]
async fn abstains_on_output_that_is_not_a_decision() {
    assert_eq!(
        verdict_of("echo '{\"unrelated\":true}'").await,
        HookVerdict::Abstain
    );
    assert_eq!(
        verdict_of("echo not json at all").await,
        HookVerdict::Abstain
    );
}

// ---------------------------------------------------------------------------
// deciding a tool call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn abstains_when_every_hook_abstains() {
    let first = hook("true");
    let second = hook("true");
    let outcome = decide_tool_call(&[&first, &second], &EMPTY, None).await;
    assert_eq!(outcome.verdict, HookVerdict::Abstain);
}

#[tokio::test]
async fn denies_on_a_refusal_and_names_the_hook() {
    let refusing = hook("echo not allowed >&2; exit 2");
    let outcome = decide_tool_call(&[&refusing], &EMPTY, None).await;
    let HookVerdict::Deny { reason } = &outcome.verdict else {
        panic!("expected a denial")
    };
    assert!(reason.contains("not allowed"));
    assert!(
        outcome
            .hook
            .expect("names the hook")
            .command
            .contains("not allowed")
    );
}

#[tokio::test]
async fn skips_later_hooks_once_one_refused() {
    let refusing = hook("exit 2");
    let never = hook("echo never");
    let outcome = decide_tool_call(&[&refusing, &never], &EMPTY, None).await;
    assert!(matches!(outcome.verdict, HookVerdict::Deny { .. }));
    assert_eq!(outcome.results.len(), 1);
}

#[tokio::test]
async fn keeps_going_past_an_allow_so_an_earlier_hook_cannot_suppress_a_later_denial() {
    let allowing = hook("echo '{\"permission\":\"allow\"}'");
    let refusing = hook("exit 2");
    let outcome = decide_tool_call(&[&allowing, &refusing], &EMPTY, None).await;
    assert!(matches!(outcome.verdict, HookVerdict::Deny { .. }));
    assert_eq!(outcome.results.len(), 2);
}

#[tokio::test]
async fn lets_asking_beat_allowing_whichever_came_first() {
    let allowing = hook("echo '{\"permission\":\"allow\"}'");
    let asking = hook("echo '{\"permission\":\"ask\"}'");
    let allow_then_ask = decide_tool_call(&[&allowing, &asking], &EMPTY, None).await;
    assert!(matches!(allow_then_ask.verdict, HookVerdict::Ask { .. }));
    let ask_then_allow = decide_tool_call(&[&asking, &allowing], &EMPTY, None).await;
    assert!(matches!(ask_then_allow.verdict, HookVerdict::Ask { .. }));
}

#[tokio::test]
async fn ignores_hooks_declared_for_other_events() {
    let other = hook_with("exit 2", HookEvent::PostToolUse, 10_000.0, None);
    let outcome = decide_tool_call(&[&other], &EMPTY, None).await;
    assert_eq!(outcome.verdict, HookVerdict::Abstain);
    assert!(outcome.results.is_empty());
}

#[tokio::test]
async fn denies_on_a_timeout_rather_than_letting_the_call_through() {
    let slow = hook_with("sleep 30", HookEvent::PreToolUse, 200.0, None);
    let outcome = decide_tool_call(&[&slow], &EMPTY, None).await;
    let HookVerdict::Deny { reason } = &outcome.verdict else {
        panic!("expected a denial")
    };
    assert!(reason.contains("timed out"));
}

// ---------------------------------------------------------------------------
// refusal messages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prefers_what_the_hook_wrote() {
    let result = run_hook(&hook("echo because reasons >&2; exit 1"), &EMPTY, None).await;
    assert!(describe_refusal(&result).contains("because reasons"));
}

#[tokio::test]
async fn states_the_outcome_when_the_hook_wrote_nothing() {
    let result = run_hook(&hook("exit 7"), &EMPTY, None).await;
    assert!(describe_refusal(&result).contains('7'));
}

#[tokio::test]
async fn always_names_the_command() {
    let result = run_hook(&hook("exit 1"), &EMPTY, None).await;
    assert!(describe_refusal(&result).contains("exit 1"));
}

// ---------------------------------------------------------------------------
// the runtime
// ---------------------------------------------------------------------------

type Reports = Arc<Mutex<Vec<(String, HookReportLevel)>>>;

fn runtime(hooks: Vec<Hook>) -> (HookRuntime, Reports) {
    runtime_with_diagnostics(hooks, Vec::new())
}

fn runtime_with_diagnostics(
    hooks: Vec<Hook>,
    diagnostics: Vec<HookDiagnostic>,
) -> (HookRuntime, Reports) {
    let reports: Reports = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&reports);
    let runtime = HookRuntime::new(HookRuntimeOptions {
        hooks,
        diagnostics,
        context: Arc::new(|| HookSessionContext {
            session_id: "s-1".to_string(),
            transcript_path: Some("/tmp/s-1.jsonl".to_string()),
            cwd: "/tmp/project".to_string(),
        }),
        report: Some(Arc::new(move |message, level| {
            recorded
                .lock()
                .expect("reports")
                .push((message.to_string(), level));
        })),
        signal: None,
    });
    (runtime, reports)
}

/// A file a hook writes its payload to, so the payload can be inspected.
struct Capture {
    directory: TempDir,
}

impl Capture {
    fn new() -> Self {
        Self {
            directory: TempDir::new("hook-payload-"),
        }
    }

    fn path(&self) -> String {
        self.directory
            .path
            .join("payload.json")
            .to_string_lossy()
            .into_owned()
    }

    fn command(&self) -> String {
        format!("cat > {}", self.path())
    }

    fn read(&self) -> Option<Map<String, Value>> {
        let raw = std::fs::read_to_string(self.path()).ok()?;
        serde_json::from_str::<Value>(&raw)
            .ok()?
            .as_object()
            .cloned()
    }
}

fn post(command: &str) -> Hook {
    hook_with(command, HookEvent::PostToolUse, 10_000.0, None)
}

fn fields(entries: Value) -> Map<String, Value> {
    entries.as_object().cloned().unwrap_or_default()
}

#[tokio::test]
async fn runs_the_hooks_declared_for_an_event_and_nothing_else() {
    let capture = Capture::new();
    let (runtime, _reports) = runtime(vec![
        post(&capture.command()),
        hook_with("exit 1", HookEvent::SessionStart, 10_000.0, None),
    ]);
    runtime
        .emit(
            HookEvent::PostToolUse,
            fields(json!({ "tool_name": "write" })),
            Some("write"),
        )
        .await;
    assert_eq!(
        capture
            .read()
            .expect("payload")
            .get("hook_event_name")
            .and_then(Value::as_str),
        Some("PostToolUse")
    );
}

#[tokio::test]
async fn carries_the_session_identity_on_every_payload() {
    let capture = Capture::new();
    let (runtime, _reports) = runtime(vec![post(&capture.command())]);
    runtime.emit(HookEvent::PostToolUse, Map::new(), None).await;
    let payload = capture.read().expect("payload");
    assert_eq!(
        payload.get("session_id").and_then(Value::as_str),
        Some("s-1")
    );
    assert_eq!(
        payload.get("transcript_path").and_then(Value::as_str),
        Some("/tmp/s-1.jsonl")
    );
    assert_eq!(
        payload.get("cwd").and_then(Value::as_str),
        Some("/tmp/project")
    );
}

#[tokio::test]
async fn does_nothing_at_all_when_nothing_is_declared() {
    let (runtime, reports) = runtime(Vec::new());
    runtime.emit(HookEvent::Stop, Map::new(), None).await;
    assert!(reports.lock().expect("reports").is_empty());
    assert!(runtime.is_empty());
}

#[tokio::test]
async fn narrows_by_tool_name_so_a_matcher_restricts_what_runs() {
    let capture = Capture::new();
    let (runtime, _reports) = runtime(vec![hook_with(
        &capture.command(),
        HookEvent::PostToolUse,
        10_000.0,
        Some("edit"),
    )]);
    runtime
        .emit(
            HookEvent::PostToolUse,
            fields(json!({ "tool_name": "write" })),
            Some("write"),
        )
        .await;
    assert!(capture.read().is_none());
    runtime
        .emit(
            HookEvent::PostToolUse,
            fields(json!({ "tool_name": "edit" })),
            Some("edit"),
        )
        .await;
    assert_eq!(
        capture
            .read()
            .expect("payload")
            .get("tool_name")
            .and_then(Value::as_str),
        Some("edit")
    );
}

#[tokio::test]
async fn a_failing_hook_is_reported_with_what_it_wrote() {
    let (runtime, reports) = runtime(vec![post("echo broke >&2; exit 2")]);
    runtime.emit(HookEvent::PostToolUse, Map::new(), None).await;
    let reports = reports.lock().expect("reports");
    assert_eq!(reports.len(), 1);
    assert!(reports[0].0.contains("broke"));
    assert!(reports[0].0.contains("exited with 2"));
    assert_eq!(reports[0].1, HookReportLevel::Warning);
}

#[tokio::test]
async fn a_failing_hook_does_not_stop_the_ones_after_it() {
    let capture = Capture::new();
    let (runtime, reports) = runtime(vec![post("exit 1"), post(&capture.command())]);
    runtime.emit(HookEvent::PostToolUse, Map::new(), None).await;
    assert_eq!(
        capture
            .read()
            .expect("payload")
            .get("hook_event_name")
            .and_then(Value::as_str),
        Some("PostToolUse")
    );
    assert_eq!(reports.lock().expect("reports").len(), 1);
}

#[tokio::test]
async fn reports_a_timeout_as_such_rather_than_as_an_exit_status() {
    let (runtime, reports) = runtime(vec![hook_with(
        "sleep 30",
        HookEvent::PostToolUse,
        200.0,
        None,
    )]);
    runtime.emit(HookEvent::PostToolUse, Map::new(), None).await;
    assert!(reports.lock().expect("reports")[0].0.contains("timed out"));
}

#[tokio::test]
async fn what_a_hook_writes_comes_back_as_context() {
    let (runtime, _reports) = runtime(vec![hook_with(
        "echo 'branch: main'",
        HookEvent::UserPromptSubmit,
        10_000.0,
        None,
    )]);
    assert_eq!(
        runtime
            .emit(HookEvent::UserPromptSubmit, Map::new(), None)
            .await,
        vec!["branch: main".to_string()]
    );
}

#[tokio::test]
async fn collects_from_every_hook_that_had_something_to_say() {
    let (runtime, _reports) = runtime(vec![post("echo first"), post("true"), post("echo second")]);
    assert_eq!(
        runtime.emit(HookEvent::PostToolUse, Map::new(), None).await,
        vec!["first".to_string(), "second".to_string()]
    );
}

#[tokio::test]
async fn ignores_a_stated_decision_which_is_not_information_for_the_model() {
    let (runtime, _reports) = runtime(vec![post("echo '{\"permission\":\"allow\"}'")]);
    assert!(
        runtime
            .emit(HookEvent::PostToolUse, Map::new(), None)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn does_not_pass_on_what_a_failing_hook_wrote() {
    let (runtime, reports) = runtime(vec![post("echo broken-output; exit 1")]);
    assert!(
        runtime
            .emit(HookEvent::PostToolUse, Map::new(), None)
            .await
            .is_empty()
    );
    assert!(!reports.lock().expect("reports").is_empty());
}

#[tokio::test]
async fn returns_nothing_when_no_hook_is_declared() {
    let (runtime, _reports) = runtime(Vec::new());
    assert!(
        runtime
            .emit(HookEvent::UserPromptSubmit, Map::new(), None)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_broken_pre_tool_use_hook_is_reported() {
    let (runtime, reports) = runtime(vec![hook("this-command-does-not-exist-xyz")]);
    let outcome = runtime.decide("write", &Map::new()).await;
    assert_eq!(outcome.verdict, HookVerdict::Abstain);
    assert_eq!(reports.lock().expect("reports").len(), 1);
}

#[tokio::test]
async fn a_deliberate_refusal_is_not_reported_as_broken() {
    let (runtime, reports) = runtime(vec![hook("exit 2")]);
    let outcome = runtime.decide("write", &Map::new()).await;
    assert!(matches!(outcome.verdict, HookVerdict::Deny { .. }));
    assert!(reports.lock().expect("reports").is_empty());
}

#[test]
fn load_diagnostics_are_shown_once_the_user_can_see_them() {
    let (runtime, reports) = runtime_with_diagnostics(
        Vec::new(),
        vec![HookDiagnostic {
            source: "/tmp/hooks.json".to_string(),
            message: "unknown event".to_string(),
        }],
    );
    runtime.report_diagnostics();
    let reports = reports.lock().expect("reports");
    assert!(reports[0].0.contains("unknown event"));
    assert_eq!(reports[0].1, HookReportLevel::Warning);
}

#[tokio::test]
async fn deciding_passes_the_tool_name_and_arguments_to_the_hook() {
    let capture = Capture::new();
    let (runtime, _reports) = runtime(vec![hook(&capture.command())]);
    runtime
        .decide("write", &fields(json!({ "path": "a.ts" })))
        .await;
    let payload = capture.read().expect("payload");
    assert_eq!(
        payload.get("hook_event_name").and_then(Value::as_str),
        Some("PreToolUse")
    );
    assert_eq!(
        payload.get("tool_name").and_then(Value::as_str),
        Some("write")
    );
    assert_eq!(payload.get("tool_input"), Some(&json!({ "path": "a.ts" })));
}

#[tokio::test]
async fn deciding_abstains_without_running_anything_when_no_hook_matches() {
    let (runtime, _reports) = runtime(vec![hook_with(
        "exit 2",
        HookEvent::PreToolUse,
        10_000.0,
        Some("edit"),
    )]);
    let outcome = runtime.decide("write", &Map::new()).await;
    assert_eq!(outcome.verdict, HookVerdict::Abstain);
    assert!(outcome.results.is_empty());
}
