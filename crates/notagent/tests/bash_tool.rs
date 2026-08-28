use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use notagent::core::bash_executor::{BashExecutorOptions, execute_bash_with_operations};
use notagent::core::tools::bash::{
    BashExecError, BashExecOptions, BashExecResult, BashOperations, BashTaskManager,
    BashToolOptions, BashToolSources, ForegroundRelease, ManagedTaskSnapshot, RegisterTaskOptions,
    ShellTaskSpec, create_bash_tool_definition, create_local_bash_operations, testing,
};
use notagent::core::tools::tool_definition::ToolDefinition;
use notagent::utils::shell::{CommandTransport, ShellConfig};
use notagent_agent::types::{AgentToolResult, BoxFuture};
use notagent_ai::types::TextOrImageContent;
use serde_json::{Value, json};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("notagent-test-bash-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        Self { path }
    }

    fn as_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn text_output(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            TextOrImageContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn run(
    definition: &dyn ToolDefinition,
    input: Value,
) -> Result<AgentToolResult, notagent_agent::types::ToolExecutionError> {
    definition
        .execute("test-call", input, None, None, None)
        .await
}

/// `BashOperations` literal.
struct ScriptedOperations {
    #[allow(clippy::type_complexity)]
    script: Box<dyn Fn(&BashExecOptions) -> Result<BashExecResult, BashExecError> + Send + Sync>,
}

impl ScriptedOperations {
    fn new(
        script: impl Fn(&BashExecOptions) -> Result<BashExecResult, BashExecError>
        + Send
        + Sync
        + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            script: Box::new(script),
        })
    }
}

impl BashOperations for ScriptedOperations {
    fn exec<'a>(
        &'a self,
        _command: &'a str,
        _cwd: &'a str,
        options: BashExecOptions,
    ) -> BoxFuture<'a, Result<BashExecResult, BashExecError>> {
        Box::pin(async move { (self.script)(&options) })
    }
}

fn emit(options: &BashExecOptions, text: &str) {
    if let Some(on_data) = &options.on_data {
        on_data(text.as_bytes());
    }
}

#[tokio::test]
async fn executes_simple_commands() {
    let definition =
        create_bash_tool_definition(&std::env::current_dir().unwrap().to_string_lossy(), None);
    let result = run(&definition, json!({ "command": "echo 'test output'" }))
        .await
        .expect("command runs");
    assert!(text_output(&result).contains("test output"));
    assert_eq!(result.details, None);
}

#[tokio::test]
async fn reports_command_errors() {
    let definition =
        create_bash_tool_definition(&std::env::current_dir().unwrap().to_string_lossy(), None);
    let error = run(&definition, json!({ "command": "exit 1" }))
        .await
        .expect_err("non-zero exit");
    assert!(error.message.contains("code 1"), "{}", error.message);
}

#[tokio::test]
async fn respects_the_timeout() {
    let definition =
        create_bash_tool_definition(&std::env::current_dir().unwrap().to_string_lossy(), None);
    let error = run(&definition, json!({ "command": "sleep 5", "timeout": 1 }))
        .await
        .expect_err("times out");
    assert!(
        error.message.to_lowercase().contains("timed out"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn includes_the_full_output_path_in_truncated_timeout_and_abort_errors() {
    let directory = TempDir::new();
    for (failure, expected) in [
        (
            BashExecError::Timeout(5.0),
            "Command timed out after 5 seconds",
        ),
        (BashExecError::Aborted, "Command aborted"),
    ] {
        let operations = ScriptedOperations::new(move |options| {
            for index in 1..=3000 {
                emit(options, &format!("{index}\n"));
            }
            Err(failure.clone())
        });
        let definition = create_bash_tool_definition(
            &directory.as_str(),
            Some(BashToolOptions {
                operations: Some(operations),
                ..BashToolOptions::default()
            }),
        );

        let error = run(&definition, json!({ "command": "chatty-fail" }))
            .await
            .expect_err("fails");
        let message = error.message;
        assert!(message.contains(expected), "{message}");
        let showing = regex_capture(&message).expect("a full output footer");
        assert!(!showing.contains("undefined"), "{message}");
        let full_output = std::fs::read_to_string(&showing).expect("the full output file");
        assert!(full_output.contains("1\n2\n3"), "{full_output:.40}");
        assert!(full_output.contains("2998\n2999\n3000"));
    }
}

fn regex_capture(message: &str) -> Option<String> {
    assert!(
        message.contains("Full output: "),
        "no full output footer in {message}"
    );
    let start = message.rfind("Full output: ")? + "Full output: ".len();
    let rest = &message[start..];
    let end = rest.find([']', '\n']).unwrap_or(rest.len());
    Some(rest[..end].to_owned())
}

#[tokio::test]
async fn fails_when_the_working_directory_does_not_exist() {
    let definition =
        create_bash_tool_definition("/this/directory/definitely/does/not/exist/12345", None);
    let error = run(&definition, json!({ "command": "echo test" }))
        .await
        .expect_err("no working directory");
    assert!(
        error.message.contains("Working directory does not exist"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn reports_process_spawn_errors() {
    let directory = TempDir::new();
    // An executable whose interpreter is missing: `execve` fails with ENOENT,
    // shell path that does not exist.
    let shell = directory.path.join("broken-shell");
    std::fs::write(&shell, "#!/nonexistent-interpreter-xyz123\n").expect("write shell");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755))
            .expect("make it executable");
    }
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            shell_path: Some(shell.to_string_lossy().into_owned()),
            ..BashToolOptions::default()
        }),
    );

    let error = run(&definition, json!({ "command": "echo test" }))
        .await
        .expect_err("spawn fails");
    assert!(error.message.contains("ENOENT"), "{}", error.message);
}

#[tokio::test]
async fn passes_the_shell_path_through_to_shell_resolution() {
    let directory = TempDir::new();
    let operations = ScriptedOperations::new(|_| Ok(BashExecResult { exit_code: Some(0) }));
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            shell_path: Some("/custom/bash".to_owned()),
            operations: Some(operations),
            ..BashToolOptions::default()
        }),
    );
    // Custom operations replace shell resolution entirely.
    run(&definition, json!({ "command": "echo test" }))
        .await
        .expect("the scripted operations run");

    let local = create_local_bash_operations(Some("/custom/bash".to_owned()));
    let error = local
        .exec("echo test", &directory.as_str(), BashExecOptions::default())
        .await
        .expect_err("no such shell");
    assert_eq!(
        error,
        BashExecError::Other("Custom shell path not found: /custom/bash".to_owned())
    );
}

#[tokio::test]
async fn sends_commands_over_stdin_when_shell_resolution_requires_it() {
    let directory = TempDir::new();
    // `cat` writes back exactly what it is given, so the transport is visible.
    let operations = testing::local_bash_operations_with_shell_config(ShellConfig {
        shell: "cat".to_owned(),
        args: Vec::new(),
        command_transport: Some(CommandTransport::Stdin),
    });
    let chunks: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&chunks);
    let command = "name='World'; echo \"Hello, ${name}!\"; count=3; for i in $(seq 1 ${count}); do echo \"Iteration ${i} of ${count}\"; done";

    let result = operations
        .exec(
            command,
            &directory.as_str(),
            BashExecOptions {
                on_data: Some(Arc::new(move |data: &[u8]| {
                    sink.lock().expect("chunks").extend_from_slice(data);
                })),
                ..BashExecOptions::default()
            },
        )
        .await
        .expect("cat runs");

    assert_eq!(result.exit_code, Some(0));
    assert_eq!(
        String::from_utf8(chunks.lock().expect("chunks").clone()).expect("utf-8"),
        command
    );
}

#[tokio::test]
async fn prepends_the_command_prefix_when_configured() {
    let directory = TempDir::new();
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            command_prefix: Some("export TEST_VAR=hello".to_owned()),
            ..BashToolOptions::default()
        }),
    );
    let result = run(&definition, json!({ "command": "echo $TEST_VAR" }))
        .await
        .expect("runs");
    assert_eq!(text_output(&result).trim(), "hello");
}

#[tokio::test]
async fn includes_output_from_both_prefix_and_command() {
    let directory = TempDir::new();
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            command_prefix: Some("echo prefix-output".to_owned()),
            ..BashToolOptions::default()
        }),
    );
    let result = run(&definition, json!({ "command": "echo command-output" }))
        .await
        .expect("runs");
    assert_eq!(text_output(&result).trim(), "prefix-output\ncommand-output");
}

#[tokio::test]
async fn works_without_a_command_prefix() {
    let directory = TempDir::new();
    let definition =
        create_bash_tool_definition(&directory.as_str(), Some(BashToolOptions::default()));
    let result = run(&definition, json!({ "command": "echo no-prefix" }))
        .await
        .expect("runs");
    assert_eq!(text_output(&result).trim(), "no-prefix");
}

#[tokio::test]
async fn coalesces_streaming_updates_for_chatty_output() {
    let directory = TempDir::new();
    let operations = ScriptedOperations::new(|options| {
        for index in 0..5000 {
            emit(options, &format!("line {index}\n"));
        }
        Ok(BashExecResult { exit_code: Some(0) })
    });
    let updates = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&updates);
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations),
            ..BashToolOptions::default()
        }),
    );

    let result = definition
        .execute(
            "test-call-chatty-updates",
            json!({ "command": "chatty" }),
            None,
            Some(Arc::new(move |_update| {
                counter.fetch_add(1, Ordering::SeqCst);
            })),
            None,
        )
        .await
        .expect("runs");

    assert!(
        updates.load(Ordering::SeqCst) < 25,
        "{} updates",
        updates.load(Ordering::SeqCst)
    );
    assert!(text_output(&result).contains("line 4999"));
}

#[tokio::test]
async fn does_not_count_a_trailing_newline_as_an_extra_truncated_line() {
    let directory = TempDir::new();
    let operations = ScriptedOperations::new(|options| {
        for index in 1..=4000 {
            emit(options, &format!("line-{index:04}\n"));
        }
        Ok(BashExecResult { exit_code: Some(0) })
    });
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations),
            ..BashToolOptions::default()
        }),
    );

    let result = run(&definition, json!({ "command": "many-lines" }))
        .await
        .expect("runs");
    let output = text_output(&result);
    let truncation = &result.details.as_ref().expect("details")["truncation"];
    assert_eq!(truncation["totalLines"], json!(4000));
    assert_eq!(truncation["outputLines"], json!(2000));
    assert!(output.contains("line-2001"));
    assert!(output.contains("line-4000"));
    assert!(
        output.contains("[Showing lines 2001-4000 of 4000. Full output: "),
        "{output:.200}"
    );
    assert!(!output.contains("4001"));
}

#[tokio::test]
async fn decodes_utf8_characters_split_across_output_chunks() {
    let directory = TempDir::new();
    let operations = ScriptedOperations::new(|options| {
        let euro = "€\n".as_bytes();
        if let Some(on_data) = &options.on_data {
            on_data(&euro[..1]);
            on_data(&euro[1..]);
        }
        Ok(BashExecResult { exit_code: Some(0) })
    });
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations),
            ..BashToolOptions::default()
        }),
    );

    let result = run(&definition, json!({ "command": "split-utf8" }))
        .await
        .expect("runs");
    assert_eq!(text_output(&result).trim(), "€");
}

#[tokio::test]
async fn exposes_local_bash_operations_for_reuse() {
    let directory = TempDir::new();
    let operations = create_local_bash_operations(None);
    let chunks: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&chunks);
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    env.insert(
        "TEST_LOCAL_BASH_OPS".to_owned(),
        "from-local-ops".to_owned(),
    );

    let result = operations
        .exec(
            "echo $TEST_LOCAL_BASH_OPS",
            &directory.as_str(),
            BashExecOptions {
                on_data: Some(Arc::new(move |data: &[u8]| {
                    sink.lock().expect("chunks").extend_from_slice(data);
                })),
                env: Some(env),
                ..BashExecOptions::default()
            },
        )
        .await
        .expect("runs");

    assert_eq!(result.exit_code, Some(0));
    assert_eq!(
        String::from_utf8(chunks.lock().expect("chunks").clone())
            .expect("utf-8")
            .trim(),
        "from-local-ops"
    );
}

#[tokio::test]
async fn preserves_executor_sanitization_with_local_operations() {
    let operations = create_local_bash_operations(None);
    let result = execute_bash_with_operations(
        "printf '\\033[31mred\\033[0m\\r\\n'",
        &std::env::current_dir().unwrap().to_string_lossy(),
        &operations,
        None,
    )
    .await
    .expect("runs");

    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.output, "red\n");
}

#[tokio::test]
async fn persists_full_output_when_truncation_happens_by_line_count_only() {
    let directory = TempDir::new();
    let definition = create_bash_tool_definition(&directory.as_str(), None);
    let result = run(&definition, json!({ "command": "seq 3000" }))
        .await
        .expect("runs");
    let output = text_output(&result);
    let details = result.details.as_ref().expect("details");
    assert_eq!(details["truncation"]["truncated"], json!(true));
    assert_eq!(details["truncation"]["truncatedBy"], json!("lines"));
    let full_output_path = details["fullOutputPath"].as_str().expect("a path");
    assert!(
        output.contains("[Showing lines 1001-3000 of 3000. Full output: "),
        "{output:.200}"
    );
    let full_output = std::fs::read_to_string(full_output_path).expect("the full output file");
    assert!(full_output.contains("1\n2\n3"));
    assert!(full_output.contains("2998\n2999\n3000"));
}

#[tokio::test]
async fn executor_persists_full_output_when_truncation_happens_by_line_count_only() {
    let operations = create_local_bash_operations(None);
    let result = execute_bash_with_operations(
        "seq 3000",
        &std::env::current_dir().unwrap().to_string_lossy(),
        &operations,
        Some(BashExecutorOptions::default()),
    )
    .await
    .expect("runs");

    assert!(result.truncated);
    let path = result.full_output_path.expect("a path");
    let full_output = std::fs::read_to_string(&path).expect("the full output file");
    assert!(full_output.contains("1\n2\n3"));
    assert!(full_output.contains("2998\n2999\n3000"));
}

/// covers the local escalation (terminate the process group, kill it after the
/// grace window) against a real process.
#[tokio::test]
async fn stops_a_running_command_when_the_signal_fires() {
    let directory = TempDir::new();
    let definition = create_bash_tool_definition(&directory.as_str(), None);
    let signal = tokio_util::sync::CancellationToken::new();
    let trigger = signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        trigger.cancel();
    });

    let started = std::time::Instant::now();
    let error = definition
        .execute(
            "test-call-abort",
            json!({ "command": "echo working; sleep 30" }),
            Some(signal),
            None,
            None,
        )
        .await
        .expect_err("aborted");

    assert!(
        error.message.contains("Command aborted"),
        "{}",
        error.message
    );
    assert!(error.message.contains("working"), "{}", error.message);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the command was not stopped promptly"
    );
}

// ---------------------------------------------------------------------------
//
// The gate is the part that matters. A model that can start a detached command
// but cannot list, read or stop one has been handed a way to lose work, so the
// flag is absent from the schema rather than present and refused.
// ---------------------------------------------------------------------------

/// Records what the shell tool registers. Running the task is the manager's
/// job and is covered with the real manager in plan task 10.
struct Registration {
    description: String,
    detached: bool,
    timeout_ms: Option<u64>,
    auto_background_on_timeout: bool,
}

#[derive(Default)]
struct RecordingManager {
    registered: Mutex<Vec<Registration>>,
}

impl BashTaskManager for RecordingManager {
    fn register_shell_task(
        &self,
        task: ShellTaskSpec,
        options: RegisterTaskOptions,
    ) -> Result<String, String> {
        self.registered
            .lock()
            .expect("registered")
            .push(Registration {
                description: task.description.clone(),
                detached: options.detached,
                timeout_ms: options.timeout_ms,
                auto_background_on_timeout: options.auto_background_on_timeout,
            });
        Ok("bash-1a2b3c4d".to_owned())
    }

    fn wait_for_foreground_release<'a>(
        &'a self,
        _task_id: &'a str,
    ) -> BoxFuture<'a, Option<ForegroundRelease>> {
        Box::pin(async { Some(ForegroundRelease::TimeoutDetached) })
    }

    fn get_task(&self, _task_id: &str) -> Option<ManagedTaskSnapshot> {
        None
    }
}

fn harness(
    background: bool,
) -> (
    notagent::core::tools::bash::BashToolDefinition,
    Arc<RecordingManager>,
) {
    let manager = Arc::new(RecordingManager::default());
    let manager_for_source: Arc<dyn BashTaskManager> =
        Arc::clone(&manager) as Arc<dyn BashTaskManager>;
    let definition = create_bash_tool_definition(
        &std::env::current_dir().unwrap().to_string_lossy(),
        Some(BashToolOptions {
            operations: Some(ScriptedOperations::new(|_| {
                Ok(BashExecResult { exit_code: Some(0) })
            })),
            expose_session_environment: Some(false),
            sources: Some(BashToolSources {
                manager: Arc::new(move || Some(Arc::clone(&manager_for_source))),
                background_allowed: Arc::new(move || background),
                auto_background_on_timeout: None,
            }),
            ..BashToolOptions::default()
        }),
    );
    (definition, manager)
}

#[test]
fn keeps_the_flag_out_of_the_schema_when_a_task_cannot_be_observed() {
    let (definition, _) = harness(false);
    let properties = definition.parameters()["properties"]
        .as_object()
        .expect("properties");
    assert!(!properties.contains_key("run_in_background"));
    assert!(!properties.contains_key("disable_timeout"));
}

#[test]
fn offers_the_flag_once_a_task_can_be_observed() {
    let (definition, _) = harness(true);
    let properties = definition.parameters()["properties"]
        .as_object()
        .expect("properties");
    assert!(properties.contains_key("run_in_background"));
}

#[test]
fn describes_the_behaviour_that_is_actually_in_force() {
    assert!(
        harness(false)
            .0
            .description()
            .contains("not available here")
    );
    assert!(
        harness(true)
            .0
            .description()
            .contains("moved to the background")
    );
}

#[tokio::test]
async fn refuses_the_flag_rather_than_silently_running_in_the_foreground() {
    let (definition, _) = harness(false);
    let error = run(
        &definition,
        json!({ "command": "true", "run_in_background": true, "description": "x" }),
    )
    .await
    .expect_err("refused");
    assert!(
        error.message.contains("not available here"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn returns_a_task_id_instead_of_the_output_when_started_in_the_background() {
    let (definition, manager) = harness(true);
    let result = run(
        &definition,
        json!({ "command": "serve", "run_in_background": true, "description": "the dev server" }),
    )
    .await
    .expect("registers");
    let text = text_output(&result);
    assert!(text.contains("task_id: bash-1a2b3c4d"), "{text}");
    assert!(text.contains("status: running"), "{text}");
    assert!(text.contains("description: the dev server"), "{text}");
    assert!(text.contains("Started in the background."), "{text}");

    let registered = manager.registered.lock().expect("registered");
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0].description, "the dev server");
    assert!(registered[0].detached, "registered as detached");
    // The background default of ten minutes, in milliseconds.
    assert_eq!(registered[0].timeout_ms, Some(600_000));
}

#[tokio::test]
async fn requires_a_description_since_a_bare_id_tells_nobody_anything() {
    let (definition, _) = harness(true);
    let error = run(
        &definition,
        json!({ "command": "true", "run_in_background": true }),
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
async fn runs_a_task_without_a_deadline_when_the_timeout_is_disabled() {
    let (definition, manager) = harness(true);
    run(
        &definition,
        json!({
            "command": "serve",
            "run_in_background": true,
            "description": "the dev server",
            "disable_timeout": true,
        }),
    )
    .await
    .expect("registers");
    assert_eq!(
        manager.registered.lock().expect("registered")[0].timeout_ms,
        None
    );
}

#[tokio::test]
async fn moves_to_the_background_instead_of_failing_when_its_deadline_fires() {
    let (definition, manager) = harness(true);
    let result = run(&definition, json!({ "command": "make", "timeout": 0.05 }))
        .await
        .expect("released");
    let text = text_output(&result);
    assert!(text.contains("task_id: bash-1a2b3c4d"), "{text}");
    assert!(text.contains("moved to the background"), "{text}");

    let registered = manager.registered.lock().expect("registered");
    assert_eq!(registered[0].description, "bash: make");
    assert!(!registered[0].detached, "registered in the foreground");
    assert_eq!(registered[0].timeout_ms, Some(50));
    assert!(
        registered[0].auto_background_on_timeout,
        "auto-backgrounding stays on by default"
    );
}
