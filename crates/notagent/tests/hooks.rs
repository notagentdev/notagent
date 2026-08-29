#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use notagent::core::hooks::events::{HOOK_EVENTS, HookEvent};
use notagent::core::hooks::payload::HookSessionContext;
use notagent::core::hooks::runner::{
    HookVerdict, decide_hooks, describe_refusal, is_hook_fault, read_verdict, run_hook, run_hooks,
};
use notagent::core::hooks::runtime::{
    HookContextOutput, HookReportLevel, HookRuntime, HookRuntimeOptions,
};
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
        timeout_ms: 10_000,
        source: "test".to_string(),
    }
}

fn hook_with(command: &str, event: HookEvent, timeout_ms: u64, matcher: Option<&str>) -> Hook {
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

async fn wait_for_file(path: &std::path::Path) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "hook child did not publish its pid"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

fn process_is_alive(pid: i32) -> bool {
    // Signal zero only queries the process table and does not affect the child.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

async fn wait_for_process_exit(pid: i32) -> bool {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while process_is_alive(pid) {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    true
}

// ---------------------------------------------------------------------------
// event vocabulary
// ---------------------------------------------------------------------------

#[test]
fn carries_every_supported_event_once() {
    assert_eq!(
        HOOK_EVENTS.map(HookEvent::as_str),
        [
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionRequest",
            "PermissionResult",
            "UserPromptSubmit",
            "UserPromptQueued",
            "TurnStarted",
            "Stop",
            "StopFailure",
            "Interrupt",
            "SessionStart",
            "SessionEnd",
            "SubagentStart",
            "SubagentStop",
            "TaskStarted",
            "PreCompact",
            "PostCompact",
            "Notification",
        ]
    );
}

#[test]
fn pins_blocking_and_tool_scoped_capabilities_separately() {
    assert_eq!(
        HOOK_EVENTS
            .into_iter()
            .filter(|event| event.is_blocking())
            .map(HookEvent::as_str)
            .collect::<Vec<_>>(),
        vec!["PreToolUse", "UserPromptSubmit"]
    );
    assert_eq!(
        HOOK_EVENTS
            .into_iter()
            .filter(|event| event.is_tool_scoped())
            .map(HookEvent::as_str)
            .collect::<Vec<_>>(),
        vec![
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionRequest",
            "PermissionResult",
        ]
    );
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
fn rejects_an_excessive_timeout() {
    let directory = dir_with(json!([declaration(json!({ "timeout_ms": 999_999 }))]));
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert!(result.hooks.is_empty());
    assert!(messages(&result.diagnostics).contains(&MAX_HOOK_TIMEOUT_MS.to_string()));
}

#[test]
fn rejects_a_fractional_timeout() {
    let directory = dir_with(json!([declaration(json!({ "timeout_ms": 1.5 }))]));
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert!(result.hooks.is_empty());
    assert!(messages(&result.diagnostics).contains("invalid timeout_ms"));
}

#[test]
fn rejects_the_old_timeout_field_and_names_the_supported_field() {
    let directory = dir_with(json!([declaration(json!({ "timeout": 30 }))]));
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert!(result.hooks.is_empty());
    assert!(messages(&result.diagnostics).contains("timeout_ms"));
}

#[test]
fn rejects_an_unknown_field_without_losing_other_declarations() {
    let directory = dir_with(json!([
        declaration(json!({ "mystery": true })),
        declaration(json!({ "command": "second" })),
    ]));
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert_eq!(result.hooks.len(), 1);
    assert_eq!(result.hooks[0].command, "second");
    assert!(messages(&result.diagnostics).contains("mystery"));
}

#[test]
fn reports_duplicate_declarations_but_keeps_both() {
    let duplicate = declaration(json!({ "command": "same" }));
    let directory = dir_with(json!([duplicate.clone(), duplicate]));
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert_eq!(result.hooks.len(), 2);
    assert!(messages(&result.diagnostics).contains("duplicate"));
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
fn rejects_a_matcher_on_a_non_tool_event() {
    let directory = dir_with(json!([declaration(
        json!({ "event": "SessionStart", "matcher": "write" })
    )]));
    let result = load_hooks(std::slice::from_ref(&directory.path));
    assert!(result.hooks.is_empty());
    assert!(messages(&result.diagnostics).contains("only tool events carry a tool name"));
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
fn marks_both_preflight_events_as_able_to_refuse() {
    let directory = dir_with(json!([
        declaration(json!({})),
        declaration(json!({ "event": "UserPromptSubmit" })),
        declaration(json!({ "event": "PostToolUse" })),
    ]));
    let hooks = load_hooks(std::slice::from_ref(&directory.path)).hooks;
    assert_eq!(
        hooks.iter().map(is_blocking_hook).collect::<Vec<_>>(),
        vec![true, true, false]
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
async fn an_exited_hook_leaves_a_redirected_background_process_running() {
    let result = run_hook(&hook("sleep 30 >/dev/null 2>&1 & echo $!"), &EMPTY, None).await;
    assert!(
        result.ok,
        "the hook itself must exit successfully: {result:?}"
    );
    let pid = result.stdout.trim().parse::<i32>().expect("background pid");
    assert!(
        process_is_alive(pid),
        "a detached process with redirected streams must survive normal hook exit"
    );

    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    assert!(
        wait_for_process_exit(pid).await,
        "test background process {pid} did not stop during cleanup"
    );
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
        &hook_with("sleep 30", HookEvent::PreToolUse, 200, None),
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
async fn caps_multibyte_output_including_the_truncation_marker() {
    let command = format!("printf '%s' '{}'", "é".repeat(5_000));
    let result = run_hook(&hook(&command), &EMPTY, None).await;
    assert!(result.ok);
    assert_eq!(result.stdout.encode_utf16().count(), 4_000);
    assert!(result.stdout.ends_with('…'));
}

#[tokio::test]
async fn timeout_removes_the_hook_process_and_its_descendant() {
    let directory = TempDir::new("hook-tree-");
    let pid_file = directory.path.join("child.pid");
    let command = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
    let result = run_hook(
        &hook_with(&command, HookEvent::PreToolUse, 200, None),
        &EMPTY,
        None,
    )
    .await;
    let pid = std::fs::read_to_string(&pid_file)
        .expect("child pid")
        .trim()
        .parse::<i32>()
        .expect("numeric pid");
    assert!(result.timed_out);
    assert!(
        wait_for_process_exit(pid).await,
        "descendant {pid} survived the hook timeout"
    );
}

#[tokio::test]
async fn cancellation_removes_the_hook_process_and_its_descendant() {
    let directory = TempDir::new("hook-tree-");
    let pid_file = directory.path.join("child.pid");
    let command = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
    let token = CancellationToken::new();
    let declaration = hook_with(&command, HookEvent::PreToolUse, 30_000, None);
    let run = run_hook(&declaration, &EMPTY, Some(&token));
    let cancel = async {
        wait_for_file(&pid_file).await;
        token.cancel();
    };
    let (result, ()) = tokio::join!(run, cancel);
    let pid = std::fs::read_to_string(&pid_file)
        .expect("child pid")
        .trim()
        .parse::<i32>()
        .expect("numeric pid");
    assert!(result.cancelled);
    assert!(
        wait_for_process_exit(pid).await,
        "descendant {pid} survived hook cancellation"
    );
}

#[tokio::test]
async fn stops_early_when_the_run_is_aborted() {
    let token = CancellationToken::new();
    let hook = hook_with("sleep 30", HookEvent::PreToolUse, 30_000, None);
    let run = run_hook(&hook, &EMPTY, Some(&token));
    let abort = async {
        tokio::task::yield_now().await;
        token.cancel();
    };
    let (result, ()) = tokio::join!(run, abort);
    assert!(!result.ok);
    assert!(result.cancelled);
    assert!(!result.timed_out);
    assert_eq!(read_verdict(&result), HookVerdict::Abstain);
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
async fn denies_on_a_timeout_for_a_blocking_event() {
    let result = run_hook(
        &hook_with("sleep 30", HookEvent::PreToolUse, 200, None),
        &EMPTY,
        None,
    )
    .await;
    assert!(matches!(read_verdict(&result), HookVerdict::Deny { .. }));
    assert!(is_hook_fault(&result));
}

#[tokio::test]
async fn lets_a_broken_hook_still_state_a_decision_it_managed_to_print() {
    let verdict = verdict_of(
        "echo '{\"hook_output\":{\"decision\":\"deny\",\"reason\":\"policy\"}}'; exit 127",
    )
    .await;
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
        verdict_of("echo '{\"hook_output\":{\"decision\":\"allow\"}}'").await,
        HookVerdict::Allow { reason: None }
    );
    assert_eq!(
        verdict_of("echo '{\"hook_output\":{\"decision\":\"ask\",\"reason\":\"check first\"}}'",)
            .await,
        HookVerdict::Ask {
            reason: Some("check first".to_string())
        }
    );
}

#[tokio::test]
async fn rejects_legacy_top_level_decisions() {
    let result = run_hook(&hook("echo '{\"permission\":\"allow\"}'"), &EMPTY, None).await;
    assert!(matches!(read_verdict(&result), HookVerdict::Deny { .. }));
    assert!(is_hook_fault(&result));
}

#[tokio::test]
async fn does_not_search_plain_output_for_a_trailing_decision() {
    let result = run_hook(
        &hook("echo working; echo '{\"hook_output\":{\"decision\":\"deny\",\"reason\":\"nope\"}}'"),
        &EMPTY,
        None,
    )
    .await;
    assert_eq!(read_verdict(&result), HookVerdict::Abstain);
    assert!(!is_hook_fault(&result));
}

#[tokio::test]
async fn lets_a_stated_denial_override_a_zero_exit() {
    let verdict =
        verdict_of("echo '{\"hook_output\":{\"decision\":\"deny\",\"reason\":\"no\"}}'; exit 0")
            .await;
    assert!(matches!(verdict, HookVerdict::Deny { .. }));
}

#[tokio::test]
async fn rejects_json_shaped_output_that_does_not_match_the_envelope() {
    assert!(matches!(
        verdict_of("echo '{\"unrelated\":true}'").await,
        HookVerdict::Deny { .. }
    ));
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
    let outcome = decide_hooks(&[&first, &second], &EMPTY, None).await;
    assert_eq!(outcome.verdict, HookVerdict::Abstain);
}

#[tokio::test]
async fn denies_on_a_refusal_and_names_the_hook() {
    let refusing = hook("echo not allowed >&2; exit 2");
    let outcome = decide_hooks(&[&refusing], &EMPTY, None).await;
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
    let outcome = decide_hooks(&[&refusing, &never], &EMPTY, None).await;
    assert!(matches!(outcome.verdict, HookVerdict::Deny { .. }));
    assert_eq!(outcome.results.len(), 1);
}

#[tokio::test]
async fn keeps_going_past_an_allow_so_an_earlier_hook_cannot_suppress_a_later_denial() {
    let allowing = hook("echo '{\"hook_output\":{\"decision\":\"allow\"}}'");
    let refusing = hook("exit 2");
    let outcome = decide_hooks(&[&allowing, &refusing], &EMPTY, None).await;
    assert!(matches!(outcome.verdict, HookVerdict::Deny { .. }));
    assert_eq!(outcome.results.len(), 2);
}

#[tokio::test]
async fn lets_asking_beat_allowing_whichever_came_first() {
    let allowing = hook("echo '{\"hook_output\":{\"decision\":\"allow\"}}'");
    let asking = hook("echo '{\"hook_output\":{\"decision\":\"ask\"}}'");
    let allow_then_ask = decide_hooks(&[&allowing, &asking], &EMPTY, None).await;
    assert!(matches!(allow_then_ask.verdict, HookVerdict::Ask { .. }));
    let ask_then_allow = decide_hooks(&[&asking, &allowing], &EMPTY, None).await;
    assert!(matches!(ask_then_allow.verdict, HookVerdict::Ask { .. }));
}

#[tokio::test]
async fn ignores_hooks_declared_for_other_events() {
    let other = hook_with("exit 2", HookEvent::PostToolUse, 10_000, None);
    let outcome = decide_hooks(&[&other], &EMPTY, None).await;
    assert_eq!(outcome.verdict, HookVerdict::Abstain);
    assert!(outcome.results.is_empty());
}

#[tokio::test]
async fn denies_on_a_timeout_rather_than_letting_the_call_through() {
    let slow = hook_with("sleep 30", HookEvent::PreToolUse, 200, None);
    let outcome = decide_hooks(&[&slow], &EMPTY, None).await;
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
async fn a_plain_stdout_reason_survives_an_exit_code_refusal() {
    let result = run_hook(&hook("echo plain refusal; exit 2"), &EMPTY, None).await;
    assert!(describe_refusal(&result).contains("plain refusal"));
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
    hook_with(command, HookEvent::PostToolUse, 10_000, None)
}

fn fields(entries: Value) -> Map<String, Value> {
    entries.as_object().cloned().unwrap_or_default()
}

fn context_texts(outputs: Vec<HookContextOutput>) -> Vec<String> {
    outputs.into_iter().map(|output| output.text).collect()
}

async fn decide_tool(
    runtime: &HookRuntime,
    tool_name: &str,
    tool_input: Map<String, Value>,
) -> notagent::core::hooks::runner::HookVerdictOutcome {
    runtime
        .decide(
            HookEvent::PreToolUse,
            fields(json!({
                "tool_call_id": "call-1",
                "tool_name": tool_name,
                "tool_input": tool_input,
            })),
            Some(tool_name),
        )
        .await
}

#[tokio::test]
async fn runs_the_hooks_declared_for_an_event_and_nothing_else() {
    let capture = Capture::new();
    let (runtime, _reports) = runtime(vec![
        post(&capture.command()),
        hook_with("exit 1", HookEvent::SessionStart, 10_000, None),
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
        10_000,
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
        200,
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
        10_000,
        None,
    )]);
    assert_eq!(
        context_texts(
            runtime
                .emit(HookEvent::UserPromptSubmit, Map::new(), None)
                .await
        ),
        vec!["branch: main".to_string()]
    );
}

#[tokio::test]
async fn structured_context_is_returned_without_the_envelope() {
    let (runtime, _reports) = runtime(vec![post(
        "echo '{\"hook_output\":{\"context\":\"branch: main\"}}'",
    )]);
    let outputs = runtime.emit(HookEvent::PostToolUse, Map::new(), None).await;
    assert_eq!(context_texts(outputs), vec!["branch: main".to_string()]);
}

#[tokio::test]
async fn collects_from_every_hook_that_had_something_to_say() {
    let (runtime, _reports) = runtime(vec![post("echo first"), post("true"), post("echo second")]);
    assert_eq!(
        context_texts(runtime.emit(HookEvent::PostToolUse, Map::new(), None).await),
        vec!["first".to_string(), "second".to_string()]
    );
}

#[tokio::test]
async fn ignores_a_stated_decision_which_is_not_information_for_the_model() {
    let (runtime, reports) = runtime(vec![post(
        "echo '{\"hook_output\":{\"decision\":\"allow\"}}'",
    )]);
    assert!(
        runtime
            .emit(HookEvent::PostToolUse, Map::new(), None)
            .await
            .is_empty()
    );
    assert!(
        reports.lock().expect("reports")[0]
            .0
            .contains("decision ignored")
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
    let outcome = decide_tool(&runtime, "write", Map::new()).await;
    assert_eq!(outcome.verdict, HookVerdict::Abstain);
    assert_eq!(reports.lock().expect("reports").len(), 1);
}

#[tokio::test]
async fn a_deliberate_refusal_is_not_reported_as_broken() {
    let (runtime, reports) = runtime(vec![hook("exit 2")]);
    let outcome = decide_tool(&runtime, "write", Map::new()).await;
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
    decide_tool(&runtime, "write", fields(json!({ "path": "a.ts" }))).await;
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
    assert_eq!(
        payload.get("tool_call_id").and_then(Value::as_str),
        Some("call-1")
    );
}

#[tokio::test]
async fn deciding_abstains_without_running_anything_when_no_hook_matches() {
    let (runtime, _reports) = runtime(vec![hook_with(
        "exit 2",
        HookEvent::PreToolUse,
        10_000,
        Some("edit"),
    )]);
    let outcome = decide_tool(&runtime, "write", Map::new()).await;
    assert_eq!(outcome.verdict, HookVerdict::Abstain);
    assert!(outcome.results.is_empty());
}
