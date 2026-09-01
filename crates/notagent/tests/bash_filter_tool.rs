use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use notagent::core::tools::bash::{
    BashExecError, BashExecOptions, BashExecResult, BashOperations, BashToolOptions,
    create_bash_tool_definition,
};
use notagent::core::tools::tool_definition::ToolDefinition;
use notagent_agent::types::{AgentToolResult, BoxFuture};
use notagent_ai::types::TextOrImageContent;
use serde_json::{Value, json};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("notagent-bash-filter-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        Self { path }
    }

    fn as_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, contents: &str) {
        std::fs::write(self.path.join(name), contents).expect("write");
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
            TextOrImageContent::Image(_) => None,
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

fn with_filter(enabled: bool) -> Option<BashToolOptions> {
    Some(BashToolOptions {
        bash_filter: Some(Arc::new(move || enabled)),
        ..BashToolOptions::default()
    })
}

/// Records the command it was handed and answers with scripted output, so a
/// test can see the rewrite without running the real tool.
struct RecordingOperations {
    commands: Mutex<Vec<String>>,
    stdout: String,
    exit_code: Option<i32>,
}

impl RecordingOperations {
    fn new(stdout: &str) -> Arc<Self> {
        Arc::new(Self {
            commands: Mutex::new(Vec::new()),
            stdout: stdout.to_owned(),
            exit_code: Some(0),
        })
    }

    fn commands(&self) -> Vec<String> {
        self.commands.lock().expect("commands").clone()
    }
}

impl BashOperations for RecordingOperations {
    fn exec<'a>(
        &'a self,
        command: &'a str,
        _cwd: &'a str,
        options: BashExecOptions,
    ) -> BoxFuture<'a, Result<BashExecResult, BashExecError>> {
        Box::pin(async move {
            self.commands
                .lock()
                .expect("commands")
                .push(command.to_owned());
            if let Some(on_data) = &options.on_data {
                on_data(self.stdout.as_bytes());
            }
            Ok(BashExecResult {
                exit_code: self.exit_code,
            })
        })
    }
}

/// A cargo run of the size the filter exists for. Two tests would not pay for
/// the pointer to the raw output that replaces them, and the filter says so by
/// standing down — which is what `a_short_output_is_left_alone` pins.
fn cargo_test_output() -> String {
    let mut output = String::from("running 60 tests\n");
    for index in 1..=60 {
        output.push_str(&format!(
            "test module::sub::case_number_{index:02} ... ok\n"
        ));
    }
    output
        .push_str("test result: ok. 60 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n");
    output
}

// ---- the gate --------------------------------------------------------------

#[tokio::test]
async fn the_filter_is_off_unless_it_is_switched_on() {
    let directory = TempDir::new();
    // Off is the default, and an absent gate means the same thing.
    for options in [None, with_filter(false)] {
        let definition = create_bash_tool_definition(&directory.as_str(), options);
        let result = run(&definition, json!({ "command": "printf 'a\\nb\\n'" }))
            .await
            .expect("command runs");
        assert_eq!(text_output(&result), "a\nb\n");
    }
}

#[tokio::test]
async fn a_switched_on_filter_compacts_what_the_model_reads() {
    let directory = TempDir::new();
    let operations = RecordingOperations::new(&cargo_test_output());
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations.clone()),
            bash_filter: Some(Arc::new(|| true)),
            ..BashToolOptions::default()
        }),
    );

    let result = run(&definition, json!({ "command": "cargo test" }))
        .await
        .expect("command runs");
    let text = text_output(&result);

    assert!(
        text.starts_with("cargo test: 60 passed (1 suite)"),
        "{text}"
    );
    assert!(text.contains("Full output:"), "{text}");
    assert!(
        estimated_tokens(&text) < estimated_tokens(&cargo_test_output()),
        "the compacted form must not cost more than the raw one:\n{text}"
    );
}

#[tokio::test]
async fn a_typical_test_run_saves_at_least_half_the_estimated_tokens() {
    let directory = TempDir::new();
    let operations = RecordingOperations::new(&cargo_test_output());
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations),
            bash_filter: Some(Arc::new(|| true)),
            ..BashToolOptions::default()
        }),
    );

    let result = run(&definition, json!({ "command": "cargo test" }))
        .await
        .expect("command runs");
    let text = text_output(&result);
    let full_output_path = result
        .details
        .as_ref()
        .and_then(|details| details["fullOutputPath"].as_str())
        .expect("details name the full output");
    let measured_text = text.replace(full_output_path, "<full-output-path>");
    let compact_tokens = estimated_tokens(&measured_text);
    let raw_tokens = estimated_tokens(&cargo_test_output());

    assert_eq!(
        (raw_tokens, compact_tokens),
        (624, 24),
        "keep the README fixture measurement current"
    );
    assert!(
        compact_tokens * 2 <= raw_tokens,
        "a representative filtered test run must save at least half the estimated tokens: raw={raw_tokens}, compact={compact_tokens}\n{text}"
    );
}

#[tokio::test]
async fn the_raw_output_stays_reachable_after_compaction() {
    let directory = TempDir::new();
    let operations = RecordingOperations::new(&cargo_test_output());
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations.clone()),
            bash_filter: Some(Arc::new(|| true)),
            ..BashToolOptions::default()
        }),
    );

    let result = run(&definition, json!({ "command": "cargo test" }))
        .await
        .expect("command runs");
    let details = result.details.expect("details name the full output");
    let path = details["fullOutputPath"]
        .as_str()
        .expect("fullOutputPath")
        .to_owned();
    let raw = std::fs::read_to_string(&path).expect("the raw output is on disk");
    let _ = std::fs::remove_file(&path);

    assert_eq!(raw, cargo_test_output());
}

#[tokio::test]
async fn a_short_output_is_left_alone() {
    let directory = TempDir::new();
    let raw = "running 2 tests\ntest one ... ok\ntest two ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
    let operations = RecordingOperations::new(raw);
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations.clone()),
            bash_filter: Some(Arc::new(|| true)),
            ..BashToolOptions::default()
        }),
    );

    let result = run(&definition, json!({ "command": "cargo test" }))
        .await
        .expect("command runs");

    // The summary is shorter than the run, but not shorter than the run plus
    // the pointer to the raw output that would replace it.
    assert_eq!(text_output(&result), raw);
    assert_eq!(result.details, None);
}

// ---- the execution rewrite -------------------------------------------------

#[tokio::test]
async fn the_rewrite_reaches_the_process_and_the_original_reaches_the_reader() {
    let directory = TempDir::new();
    let operations = RecordingOperations::new("");
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations.clone()),
            bash_filter: Some(Arc::new(|| true)),
            ..BashToolOptions::default()
        }),
    );

    run(&definition, json!({ "command": "go test ./..." }))
        .await
        .expect("command runs");

    assert_eq!(
        operations.commands(),
        vec!["go test -json ./...".to_owned()]
    );
}

#[tokio::test]
async fn a_command_prefix_wraps_the_rewrite_rather_than_defeating_it() {
    let directory = TempDir::new();
    let operations = RecordingOperations::new("");
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations.clone()),
            command_prefix: Some("export CI=1".to_owned()),
            bash_filter: Some(Arc::new(|| true)),
            ..BashToolOptions::default()
        }),
    );

    run(&definition, json!({ "command": "go test ./..." }))
        .await
        .expect("command runs");

    // Prepared from the command the model wrote — a prefixed line is
    // multi-line, and the classifier refuses those.
    assert_eq!(
        operations.commands(),
        vec!["export CI=1\ngo test -json ./...".to_owned()]
    );
}

#[tokio::test]
async fn a_compound_command_is_never_rewritten() {
    let directory = TempDir::new();
    let operations = RecordingOperations::new("");
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations.clone()),
            bash_filter: Some(Arc::new(|| true)),
            ..BashToolOptions::default()
        }),
    );

    run(&definition, json!({ "command": "go test ./... | tail -5" }))
        .await
        .expect("command runs");

    assert_eq!(
        operations.commands(),
        vec!["go test ./... | tail -5".to_owned()]
    );
}

// ---- the execution override ------------------------------------------------

#[tokio::test]
async fn a_read_is_answered_from_the_filesystem_without_a_process() {
    let directory = TempDir::new();
    directory.write("sample.txt", "alpha\nbeta\ngamma\n");
    let definition = create_bash_tool_definition(&directory.as_str(), with_filter(true));

    let result = run(&definition, json!({ "command": "cat sample.txt" }))
        .await
        .expect("command runs");

    assert!(text_output(&result).contains("alpha"));
}

#[tokio::test]
async fn the_override_stands_down_for_operations_that_run_elsewhere() {
    let directory = TempDir::new();
    directory.write("sample.txt", "alpha\n");
    let operations = RecordingOperations::new("output from the other machine\n");
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations.clone()),
            bash_filter: Some(Arc::new(|| true)),
            ..BashToolOptions::default()
        }),
    );

    let result = run(&definition, json!({ "command": "cat sample.txt" }))
        .await
        .expect("command runs");

    // The adapter reads this machine; custom operations may not be on it.
    assert_eq!(operations.commands(), vec!["cat sample.txt".to_owned()]);
    assert!(
        text_output(&result).contains("output from the other machine"),
        "{}",
        text_output(&result)
    );
}

// ---- the guards ------------------------------------------------------------

#[tokio::test]
async fn output_the_filter_cannot_read_passes_through_untouched() {
    let directory = TempDir::new();
    let operations = RecordingOperations::new("this is not a cargo summary\n");
    let definition = create_bash_tool_definition(
        &directory.as_str(),
        Some(BashToolOptions {
            operations: Some(operations.clone()),
            bash_filter: Some(Arc::new(|| true)),
            ..BashToolOptions::default()
        }),
    );

    let result = run(&definition, json!({ "command": "cargo test" }))
        .await
        .expect("command runs");

    assert_eq!(text_output(&result), "this is not a cargo summary\n");
    assert_eq!(result.details, None);
}

#[tokio::test]
async fn a_failing_command_still_reports_its_exit_code() {
    let directory = TempDir::new();
    let definition = create_bash_tool_definition(&directory.as_str(), with_filter(true));

    let error = run(&definition, json!({ "command": "exit 3" }))
        .await
        .expect_err("non-zero exit");

    assert!(error.message.contains("code 3"), "{}", error.message);
}

/// `bytes / 4`, the estimate the filter's own never-worse guard uses.
fn estimated_tokens(value: &str) -> usize {
    value.len().div_ceil(4)
}
