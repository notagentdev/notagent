//! Port of `packages/coding-agent/src/core/tools/write.ts` (tool half).
//!
//! `renderCall`/`renderResult` need the theme and TUI components and are wired
//! in task 13; the syntax-highlight cache they use belongs with them.

use std::path::Path;
use std::sync::Arc;

use notagent_agent::types::{
    AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::tools::file_mutation_queue::with_file_mutation_queue;
use crate::core::tools::path_utils::resolve_to_cwd;
use crate::core::tools::tool_definition::{SystemPromptContribution, ToolContext, ToolDefinition};

pub const WRITE_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Create or overwrite files",
        guidelines: &["Use write only for new files or complete rewrites."],
    };

const DESCRIPTION: &str = "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.";

fn write_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file to write (relative or absolute)" },
            "content": { "type": "string", "description": "Content to write to the file" },
        },
        "required": ["path", "content"],
    })
}

/// Pluggable operations, so writing can be delegated to a remote system.
pub trait WriteOperations: Send + Sync {
    fn write_file<'a>(
        &'a self,
        absolute_path: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn mkdir<'a>(&'a self, directory: &'a str) -> BoxFuture<'a, Result<(), String>>;
}

pub struct LocalWriteOperations;

impl WriteOperations for LocalWriteOperations {
    fn write_file<'a>(
        &'a self,
        absolute_path: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            tokio::fs::write(absolute_path, content)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn mkdir<'a>(&'a self, directory: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            tokio::fs::create_dir_all(directory)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

#[derive(Default)]
pub struct WriteToolOptions {
    pub operations: Option<Arc<dyn WriteOperations>>,
}

pub struct WriteToolDefinition {
    cwd: String,
    operations: Arc<dyn WriteOperations>,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_write_tool_definition(
    cwd: &str,
    options: Option<WriteToolOptions>,
) -> WriteToolDefinition {
    let options = options.unwrap_or_default();
    WriteToolDefinition {
        cwd: cwd.to_owned(),
        operations: options
            .operations
            .unwrap_or_else(|| Arc::new(LocalWriteOperations)),
        parameters: write_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

impl ToolDefinition for WriteToolDefinition {
    fn name(&self) -> &str {
        "write"
    }

    fn label(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(WRITE_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        WRITE_TOOL_SYSTEM_PROMPT_CONTRIBUTION
            .guidelines
            .iter()
            .map(|line| (*line).to_owned())
            .collect()
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        self.constrained_sampling.as_ref()
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let path = params
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let content = params
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let absolute_path = resolve_to_cwd(&path, &self.cwd);
            let directory = Path::new(&absolute_path)
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".to_owned());

            with_file_mutation_queue(&absolute_path, async {
                // Do not abort from a listener: that would release the mutation
                // queue while a filesystem call is still in flight. Checking the
                // signal after each await observes the same aborts and keeps the
                // queue locked until the current operation has settled.
                let throw_if_aborted = || -> Result<(), ToolExecutionError> {
                    match &signal {
                        Some(signal) if signal.is_cancelled() => {
                            Err(ToolExecutionError::new("Operation aborted"))
                        }
                        _ => Ok(()),
                    }
                };

                throw_if_aborted()?;
                self.operations
                    .mkdir(&directory)
                    .await
                    .map_err(ToolExecutionError::new)?;
                throw_if_aborted()?;
                self.operations
                    .write_file(&absolute_path, &content)
                    .await
                    .map_err(ToolExecutionError::new)?;
                throw_if_aborted()?;

                // `content.length` in JS counts UTF-16 units, not bytes.
                let length = content.encode_utf16().count();
                Ok(AgentToolResult {
                    content: vec![TextOrImageContent::Text(TextContent::new(format!(
                        "Successfully wrote {length} bytes to {path}"
                    )))],
                    details: None,
                    usage: None,
                    added_tool_names: None,
                    terminate: None,
                })
            })
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let directory = tempfile::Builder::new()
                .prefix("notagent-write-tool-")
                .tempdir()
                .expect("temp dir");
            let path = directory.path().to_path_buf();
            let _ = directory.keep();
            Self { path }
        }

        fn cwd(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn text_of(result: &AgentToolResult) -> String {
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

    #[tokio::test]
    async fn writes_a_file_and_creates_parent_directories() {
        let directory = TempDir::new();
        let tool = create_write_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute(
                "call-1",
                json!({ "path": "nested/dir/file.txt", "content": "hello" }),
                None,
                None,
                None,
            )
            .await
            .expect("write");
        assert_eq!(
            text_of(&result),
            "Successfully wrote 5 bytes to nested/dir/file.txt"
        );
        let written =
            std::fs::read_to_string(directory.path.join("nested/dir/file.txt")).expect("read");
        assert_eq!(written, "hello");
    }

    #[tokio::test]
    async fn overwrites_an_existing_file() {
        let directory = TempDir::new();
        let file = directory.path.join("file.txt");
        std::fs::write(&file, "old").expect("write");
        let tool = create_write_tool_definition(&directory.cwd(), None);
        tool.execute(
            "call-1",
            json!({ "path": "file.txt", "content": "new" }),
            None,
            None,
            None,
        )
        .await
        .expect("write");
        assert_eq!(std::fs::read_to_string(&file).expect("read"), "new");
    }

    #[tokio::test]
    async fn counts_utf16_units_like_the_javascript_string_length() {
        let directory = TempDir::new();
        let tool = create_write_tool_definition(&directory.cwd(), None);
        // "🎉" is one char, two UTF-16 units and four bytes.
        let result = tool
            .execute(
                "call-1",
                json!({ "path": "emoji.txt", "content": "🎉" }),
                None,
                None,
                None,
            )
            .await
            .expect("write");
        assert_eq!(text_of(&result), "Successfully wrote 2 bytes to emoji.txt");
    }

    #[tokio::test]
    async fn refuses_to_run_when_the_signal_is_already_aborted() {
        let directory = TempDir::new();
        let tool = create_write_tool_definition(&directory.cwd(), None);
        let signal = CancellationToken::new();
        signal.cancel();
        let error = tool
            .execute(
                "call-1",
                json!({ "path": "file.txt", "content": "x" }),
                Some(signal),
                None,
                None,
            )
            .await
            .expect_err("aborted");
        assert_eq!(error.message, "Operation aborted");
        assert!(!directory.path.join("file.txt").exists());
    }

    #[test]
    fn advertises_its_schema_and_prompt_contribution() {
        let tool = create_write_tool_definition("/tmp", None);
        assert_eq!(tool.name(), "write");
        assert_eq!(tool.label(), "write");
        assert_eq!(tool.description(), DESCRIPTION);
        assert_eq!(tool.prompt_snippet(), Some("Create or overwrite files"));
        assert_eq!(
            tool.prompt_guidelines(),
            vec!["Use write only for new files or complete rewrites.".to_owned()]
        );
        assert_eq!(tool.parameters()["required"], json!(["path", "content"]));
    }
}
