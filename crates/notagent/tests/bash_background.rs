use std::sync::Arc;

use notagent::core::tasks::manager::{TaskManager, TaskManagerOptions};
use notagent::core::tasks::store::TaskStore;
use notagent::core::tools::bash::{
    BashExecError, BashExecOptions, BashExecResult, BashOperations, BashTaskManager,
    BashToolDefinition, BashToolOptions, BashToolSources, create_bash_tool_definition,
};
use notagent::core::tools::tool_definition::ToolDefinition;
use notagent_agent::types::{AgentToolResult, BoxFuture};
use notagent_ai::types::TextOrImageContent;
use serde_json::{Value, json};

/// Operations that emit a line and exit, without touching a real shell.
struct Scripted {
    output: Option<String>,
    exit_code: i32,
    hang: bool,
}

fn scripted(output: Option<&str>, exit_code: i32, hang: bool) -> Arc<Scripted> {
    Arc::new(Scripted {
        output: output.map(str::to_owned),
        exit_code,
        hang,
    })
}

impl BashOperations for Scripted {
    fn exec<'a>(
        &'a self,
        _command: &'a str,
        _cwd: &'a str,
        options: BashExecOptions,
    ) -> BoxFuture<'a, Result<BashExecResult, BashExecError>> {
        Box::pin(async move {
            if let Some(on_spawn) = &options.on_spawn {
                on_spawn(4242);
            }
            if let Some(output) = &self.output
                && let Some(on_data) = &options.on_data
            {
                on_data(output.as_bytes());
            }
            if self.hang {
                if let Some(signal) = &options.signal {
                    signal.cancelled().await;
                }
                return Err(BashExecError::Other("aborted".to_owned()));
            }
            Ok(BashExecResult {
                exit_code: Some(self.exit_code),
            })
        })
    }
}

fn harness(
    background: bool,
    operations: Arc<dyn BashOperations>,
) -> (tempfile::TempDir, BashToolDefinition, TaskManager) {
    let directory = tempfile::Builder::new()
        .prefix("bash-bg-")
        .tempdir()
        .expect("temp dir");
    let manager = TaskManager::new(
        Arc::new(TaskStore::new(directory.path())),
        TaskManagerOptions::default(),
    );
    let source_manager = manager.clone();
    let definition = create_bash_tool_definition(
        &std::env::current_dir().unwrap().to_string_lossy(),
        Some(BashToolOptions {
            operations: Some(operations),
            expose_session_environment: Some(false),
            sources: Some(BashToolSources {
                manager: Arc::new(move || {
                    Some(Arc::new(source_manager.clone()) as Arc<dyn BashTaskManager>)
                }),
                background_allowed: Arc::new(move || background),
                auto_background_on_timeout: None,
            }),
            ..BashToolOptions::default()
        }),
    );
    (directory, definition, manager)
}

async fn run(definition: &BashToolDefinition, input: Value) -> AgentToolResult {
    definition
        .execute("call-1", input, None, None, None)
        .await
        .expect("tool result")
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

// ── the gate ──────────────────────────────────────────────────────────

#[tokio::test]
async fn keeps_the_flag_out_of_the_schema_when_a_task_cannot_be_observed() {
    let (_directory, definition, _manager) = harness(false, scripted(None, 0, false));
    let properties = definition.parameters()["properties"]
        .as_object()
        .expect("properties");
    assert!(!properties.contains_key("run_in_background"));
    assert!(!properties.contains_key("disable_timeout"));
}

#[tokio::test]
async fn offers_it_once_a_task_can_be_observed() {
    let (_directory, definition, _manager) = harness(true, scripted(None, 0, false));
    assert!(
        definition.parameters()["properties"]
            .as_object()
            .expect("properties")
            .contains_key("run_in_background")
    );
}

#[tokio::test]
async fn describes_the_behaviour_that_is_actually_in_force() {
    let (_a, without, _m1) = harness(false, scripted(None, 0, false));
    let (_b, with, _m2) = harness(true, scripted(None, 0, false));
    assert!(without.description().contains("not available here"));
    assert!(with.description().contains("moved to the background"));
}

#[tokio::test]
async fn refuses_the_flag_rather_than_silently_running_in_the_foreground() {
    let (_directory, definition, _manager) = harness(false, scripted(None, 0, false));
    let error = definition
        .execute(
            "call-1",
            json!({ "command": "true", "run_in_background": true, "description": "x" }),
            None,
            None,
            None,
        )
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("not available here"),
        "{}",
        error.message
    );
}

// ── starting a command in the background ──────────────────────────────

#[tokio::test]
async fn returns_a_task_id_instead_of_the_output() {
    let (_directory, definition, _manager) = harness(true, scripted(Some("hello\n"), 0, true));
    let text = text_of(
        &run(
            &definition,
            json!({ "command": "serve", "run_in_background": true, "description": "the dev server" }),
        )
        .await,
    );
    assert!(matches_task_id(&text, "bash-"), "{text}");
    assert!(text.contains("status: running"), "{text}");
    assert!(!text.contains("hello"), "{text}");
}

#[tokio::test]
async fn requires_a_description_since_a_bare_id_tells_nobody_anything() {
    let (_directory, definition, _manager) = harness(true, scripted(None, 0, false));
    let error = definition
        .execute(
            "call-1",
            json!({ "command": "true", "run_in_background": true }),
            None,
            None,
            None,
        )
        .await
        .expect_err("refused");
    assert!(
        error.message.contains("`description` is required"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn registers_it_where_the_observation_tools_will_find_it() {
    let (_directory, definition, manager) = harness(true, scripted(None, 0, true));
    run(
        &definition,
        json!({ "command": "serve", "run_in_background": true, "description": "the dev server" }),
    )
    .await;
    let listed = manager.list(true, None);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].description(), "the dev server");
    assert!(listed[0].is_detached());
}

// ── running a command in the foreground ───────────────────────────────

#[tokio::test]
async fn returns_the_output_as_before() {
    let (_directory, definition, _manager) = harness(true, scripted(Some("done\n"), 0, false));
    let text = text_of(&run(&definition, json!({ "command": "echo done" })).await);
    assert!(text.contains("done"), "{text}");
}

#[tokio::test]
async fn reports_a_non_zero_exit_as_an_error() {
    let (_directory, definition, _manager) = harness(true, scripted(Some("nope\n"), 3, false));
    let error = definition
        .execute("call-1", json!({ "command": "false" }), None, None, None)
        .await
        .expect_err("failed");
    assert!(
        error.message.contains("exited with code 3"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn leaves_no_log_behind_for_a_small_command_nobody_detached() {
    let (_directory, definition, manager) = harness(true, scripted(Some("done\n"), 0, false));
    run(&definition, json!({ "command": "echo done" })).await;
    assert!(manager.list(false, None).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn moves_to_the_background_instead_of_failing_when_its_deadline_fires() {
    let (_directory, definition, _manager) = harness(true, scripted(Some("building\n"), 0, true));
    // A deadline short enough to fire during the test, with backgrounding on:
    // the call returns a task id rather than a timeout error.
    let text = text_of(&run(&definition, json!({ "command": "make", "timeout": 0.05 })).await);
    assert!(matches_task_id(&text, "bash-"), "{text}");
    assert!(text.contains("moved to the background"), "{text}");
}
