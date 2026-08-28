//! Puts a file back the way it was before the last change.
//! Every mutating tool copies a file into the snapshot store before touching it
//! (`core/snapshots.rs`). This restores the newest copy and consumes it, so
//! calling it again steps back another change — the history is walked one step
//! at a time rather than jumped through.
//! Nothing here asks for permission, and that is deliberate. The only state
//! undo can produce is one the workspace already had, on a path this agent
//! already changed; asking to take a change back would be asking about the
//! remedy rather than the damage. The one exception is a credentials file,
//! which the chain stops ahead of this — a snapshot of a secret is still a
//! secret.

use std::path::Path;
use std::sync::Arc;

use notagent_agent::types::{
    AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_tui::tui::ComponentRef;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::snapshots::SnapshotStore;
use crate::core::tools::file_lease::{LeaseCoordinator, LeaseGate};
use crate::core::tools::path_utils::resolve_to_cwd;
use crate::core::tools::render_utils::call_title;
use crate::core::tools::tool_definition::{
    SystemPromptContribution, ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult,
    ToolRenderResultOptions, render_text_call, wrap_tool_definition,
};
use crate::modes::interactive::theme::theme::{Theme, ThemeColor};

pub const UNDO_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Take back the last change to a file",
        guidelines: &[
            "Use undo to revert a change you made to a file, rather than rewriting the old content from memory — the snapshot is exact and your recollection is not.",
        ],
    };

const DESCRIPTION: &str = concat!(
    "Reverts the most recent change this agent made to one file, restoring the content it had beforehand.\n",
    "\n",
    "Each call steps back exactly one change. Calling it again on the same path steps back the one before that, and so on until there is nothing left to undo — which the tool says rather than failing.\n",
    "\n",
    "Only changes made through write, patch and the minified patch tools can be undone; those are the ones that leave a snapshot. A file deleted with `rm` through bash is not recoverable, because nothing recorded it. Prefer this over reconstructing previous content by hand: the snapshot is exact.",
);

/// How long an undo expects to hold its lease.
/// The same as a write, which is what this is — the reference takes no lease
/// here at all (`fs_undo.rs`), but our subagents can write the same file
/// concurrently, and restoring underneath one of them would lose its work
/// without either side noticing.
const UNDO_LEASE_DURATION_MS: u64 = 15_000;

fn undo_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Path of the file to revert. Must be the same path that was changed.",
            },
        },
        "required": ["path"],
    })
}

#[derive(Clone, Default)]
pub struct UndoToolOptions {
    /// Where snapshots are kept. Absent means undo has nothing to work with and
    /// says so, rather than pretending to have restored something.
    pub snapshots: Option<SnapshotStore>,
    /// Whether atomic file leases are enabled; absent means disabled.
    pub leases: Option<LeaseGate>,
}

pub struct UndoToolDefinition {
    cwd: String,
    snapshots: Option<SnapshotStore>,
    leases: LeaseCoordinator,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_undo_tool_definition(
    cwd: &str,
    options: Option<UndoToolOptions>,
) -> UndoToolDefinition {
    let options = options.unwrap_or_default();
    UndoToolDefinition {
        cwd: cwd.to_owned(),
        snapshots: options.snapshots,
        leases: LeaseCoordinator::new(cwd, options.leases),
        parameters: undo_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

pub fn create_undo_tool(
    cwd: &str,
    options: Option<UndoToolOptions>,
) -> Arc<dyn notagent_agent::types::AgentTool> {
    wrap_tool_definition(Arc::new(create_undo_tool_definition(cwd, options)), None)
}

/// What one call did, which is what the result reports.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// There was no snapshot: nothing this agent did to this file can be taken
    /// back.
    NothingToUndo,
    /// The file was gone and the snapshot brought it back.
    Recreated { lines: usize },
    /// The file was there and its content was rolled back.
    Restored { lines: usize },
}

impl UndoToolDefinition {
    fn describe(&self, path: &str, outcome: &Outcome, more_remain: bool) -> String {
        let step = if more_remain {
            " There is an earlier change that can be undone as well."
        } else {
            ""
        };
        match outcome {
            Outcome::NothingToUndo => format!(
                "Nothing to undo for {path}. No change to this file was recorded, so there is no earlier content to restore."
            ),
            Outcome::Recreated { lines } => format!(
                "Restored {path} ({lines} lines). The file had been removed; its recorded content is back.{step}"
            ),
            Outcome::Restored { lines } => {
                format!("Reverted the last change to {path} ({lines} lines).{step}")
            }
        }
    }
}

impl ToolDefinition for UndoToolDefinition {
    fn name(&self) -> &str {
        "undo"
    }

    fn label(&self) -> &str {
        "undo"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(UNDO_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        UNDO_TOOL_SYSTEM_PROMPT_CONTRIBUTION
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

    /// The path and nothing else, as the reference renders it
    /// (`tool_renderers.rs:2342`). What changed is visible in the file itself,
    /// and a diff here would repeat the original call's diff backwards.
    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Some(render_text_call(
            context,
            &format!(
                "{}{}",
                call_title(theme, "undo"),
                theme.fg(ThemeColor::Accent, &path)
            ),
        ))
    }

    fn render_result(
        &self,
        _result: ToolRenderResult<'_>,
        _options: ToolRenderResultOptions,
        _theme: &Theme,
        _context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        None
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        _signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let path = params
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .ok_or_else(|| ToolExecutionError::new("undo requires a path."))?;
            let absolute_path = resolve_to_cwd(path, &self.cwd);

            let Some(store) = self.snapshots.as_ref() else {
                return Err(ToolExecutionError::new(
                    "Undo is not available in this session: no snapshots are being kept.",
                ));
            };

            let target = Path::new(&absolute_path);
            let existed_before = tokio::fs::metadata(target).await.is_ok();

            // The same lease a write takes, so a subagent cannot be halfway
            // through writing this file while it is restored underneath it.
            let lease = self
                .leases
                .reserve(target, self.name(), UNDO_LEASE_DURATION_MS)
                .await
                .map_err(|error| ToolExecutionError::new(error.to_string()))?;

            let restored = store.restore(target).await;

            if let Some(lease) = &lease {
                // Released whatever the restore did: a lease held past its call
                // blocks every later write to the file.
                let committed = self.leases.prepare_commit(lease.clone()).await;
                if let Ok(committed) = committed {
                    let _ = self.leases.release(&committed).await;
                }
            }

            let restored = restored.map_err(ToolExecutionError::new)?;

            let (outcome, more_remain) = match restored {
                None => (Outcome::NothingToUndo, false),
                Some(restored) => {
                    let lines = String::from_utf8_lossy(&restored.bytes).lines().count();
                    let outcome = if existed_before {
                        Outcome::Restored { lines }
                    } else {
                        Outcome::Recreated { lines }
                    };
                    (outcome, restored.more_remain)
                }
            };

            let display_path = path.to_owned();
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(self.describe(
                    &display_path,
                    &outcome,
                    more_remain,
                )))],
                details: Some(json!({
                    "path": absolute_path,
                    "status": match outcome {
                        Outcome::NothingToUndo => "no_changes",
                        Outcome::Recreated { .. } => "created",
                        Outcome::Restored { .. } => "restored",
                    },
                    "moreRemain": more_remain,
                })),
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(cwd: &Path, store: Option<SnapshotStore>) -> UndoToolDefinition {
        create_undo_tool_definition(
            &cwd.display().to_string(),
            Some(UndoToolOptions {
                snapshots: store,
                leases: None,
            }),
        )
    }

    async fn run(tool: &UndoToolDefinition, path: &Path) -> Result<String, String> {
        let result = tool
            .execute(
                "call-1",
                json!({ "path": path.display().to_string() }),
                None,
                None,
                None,
            )
            .await
            .map_err(|error| error.message)?;
        Ok(result
            .content
            .iter()
            .filter_map(|part| match part {
                TextOrImageContent::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<String>>()
            .join("\n"))
    }

    #[tokio::test]
    async fn says_so_when_there_is_nothing_to_undo() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = SnapshotStore::new(directory.path().join("snapshots"));
        let file = directory.path().join("note.txt");
        tokio::fs::write(&file, "only ever this")
            .await
            .expect("write");

        let message = run(&tool(directory.path(), Some(store)), &file)
            .await
            .expect("ran");
        assert!(message.contains("Nothing to undo"), "{message}");
        assert_eq!(
            tokio::fs::read_to_string(&file).await.expect("read"),
            "only ever this",
            "the file was left alone"
        );
    }

    #[tokio::test]
    async fn reverts_the_last_change() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = SnapshotStore::new(directory.path().join("snapshots"));
        let file = directory.path().join("note.txt");

        tokio::fs::write(&file, "first\nsecond")
            .await
            .expect("write");
        store.capture(&file).await.expect("captured");
        tokio::fs::write(&file, "replaced").await.expect("write");

        let message = run(&tool(directory.path(), Some(store)), &file)
            .await
            .expect("ran");
        assert!(message.contains("Reverted the last change"), "{message}");
        assert!(message.contains("2 lines"), "{message}");
        assert_eq!(
            tokio::fs::read_to_string(&file).await.expect("read"),
            "first\nsecond"
        );
    }

    #[tokio::test]
    async fn brings_back_a_file_that_was_deleted() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = SnapshotStore::new(directory.path().join("snapshots"));
        let file = directory.path().join("note.txt");

        tokio::fs::write(&file, "worth keeping")
            .await
            .expect("write");
        store.capture(&file).await.expect("captured");
        tokio::fs::remove_file(&file).await.expect("removed");

        let message = run(&tool(directory.path(), Some(store)), &file)
            .await
            .expect("ran");
        assert!(message.contains("had been removed"), "{message}");
        assert_eq!(
            tokio::fs::read_to_string(&file).await.expect("read"),
            "worth keeping"
        );
    }

    /// A model that has more to take back should be told, or it will assume one
    /// call was the whole undo.
    #[tokio::test]
    async fn reports_that_an_earlier_change_remains() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = SnapshotStore::new(directory.path().join("snapshots"));
        let file = directory.path().join("note.txt");

        tokio::fs::write(&file, "one").await.expect("write");
        store.capture(&file).await.expect("captured");
        tokio::fs::write(&file, "two").await.expect("write");
        store.capture(&file).await.expect("captured");
        tokio::fs::write(&file, "three").await.expect("write");

        let tool = tool(directory.path(), Some(store));
        let first = run(&tool, &file).await.expect("ran");
        assert!(first.contains("earlier change"), "{first}");
        let second = run(&tool, &file).await.expect("ran");
        assert!(!second.contains("earlier change"), "{second}");
        assert_eq!(tokio::fs::read_to_string(&file).await.expect("read"), "one");
    }

    #[tokio::test]
    async fn refuses_when_the_session_keeps_no_snapshots() {
        let directory = tempfile::tempdir().expect("temp dir");
        let file = directory.path().join("note.txt");
        let error = run(&tool(directory.path(), None), &file)
            .await
            .expect_err("refused");
        assert!(error.contains("not available"), "{error}");
    }

    #[tokio::test]
    async fn refuses_an_empty_path() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = SnapshotStore::new(directory.path().join("snapshots"));
        let error = create_undo_tool_definition(
            &directory.path().display().to_string(),
            Some(UndoToolOptions {
                snapshots: Some(store),
                leases: None,
            }),
        )
        .execute("call-1", json!({ "path": "  " }), None, None, None)
        .await
        .expect_err("refused");
        assert!(
            error.message.contains("requires a path"),
            "{}",
            error.message
        );
    }
}
