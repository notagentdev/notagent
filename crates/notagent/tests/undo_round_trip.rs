//! Taking back a change, through the tools that make it.
//! The unit tests either side of this cover the store and the `undo` tool on
//! their own. What is pinned here is that they meet: a real `write` or `patch`
//! leaves a snapshot behind, and a real `undo` finds it. That connection is one
//! call in each mutating tool and is exactly the thing a refactor drops without
//! any test noticing.

use std::path::Path;

use notagent::core::snapshots::SnapshotStore;
use notagent::core::tools::edit::{EditToolOptions, create_edit_tool_definition};
use notagent::core::tools::tool_definition::ToolDefinition;
use notagent::core::tools::undo::{UndoToolOptions, create_undo_tool_definition};
use notagent::core::tools::write::{WriteToolOptions, create_write_tool_definition};
use notagent_agent::types::{AgentToolResult, ToolExecutionError};
use notagent_ai::types::TextOrImageContent;
use serde_json::{Value, json};

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

async fn run(
    tool: &dyn ToolDefinition,
    args: Value,
) -> Result<AgentToolResult, ToolExecutionError> {
    tool.execute("call-1", args, None, None, None).await
}

struct Harness {
    _directory: tempfile::TempDir,
    cwd: String,
    store: SnapshotStore,
}

impl Harness {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("undo-round-trip-")
            .tempdir()
            .expect("temp dir");
        let cwd = directory.path().display().to_string();
        let store = SnapshotStore::new(directory.path().join("snapshots"));
        Self {
            _directory: directory,
            cwd,
            store,
        }
    }

    fn write_tool(&self) -> impl ToolDefinition + use<> {
        create_write_tool_definition(
            &self.cwd,
            Some(WriteToolOptions {
                snapshots: Some(self.store.clone()),
                ..WriteToolOptions::default()
            }),
        )
    }

    fn edit_tool(&self) -> impl ToolDefinition + use<> {
        create_edit_tool_definition(
            &self.cwd,
            Some(EditToolOptions {
                snapshots: Some(self.store.clone()),
                ..EditToolOptions::default()
            }),
        )
    }

    fn undo_tool(&self) -> impl ToolDefinition + use<> {
        create_undo_tool_definition(
            &self.cwd,
            Some(UndoToolOptions {
                snapshots: Some(self.store.clone()),
                leases: None,
            }),
        )
    }

    fn path(&self, name: &str) -> String {
        Path::new(&self.cwd).join(name).display().to_string()
    }
}

/// The whole point, in four calls.
#[tokio::test]
async fn a_write_can_be_taken_back() {
    let harness = Harness::new();
    let path = harness.path("note.txt");

    run(
        &harness.write_tool(),
        json!({ "path": &path, "content": "first" }),
    )
    .await
    .expect("wrote");
    run(
        &harness.write_tool(),
        json!({ "path": &path, "content": "second" }),
    )
    .await
    .expect("wrote");

    let undone = run(&harness.undo_tool(), json!({ "path": &path }))
        .await
        .expect("undone");
    assert!(
        text_of(&undone).contains("Reverted"),
        "{}",
        text_of(&undone)
    );
    assert_eq!(
        tokio::fs::read_to_string(&path).await.expect("read"),
        "first"
    );
}

/// Creating a file leaves nothing to undo. A snapshot of the emptiness before
/// it would make `undo` truncate the file rather than report that there is no
/// earlier version.
#[tokio::test]
async fn creating_a_file_leaves_nothing_to_take_back() {
    let harness = Harness::new();
    let path = harness.path("fresh.txt");

    run(
        &harness.write_tool(),
        json!({ "path": &path, "content": "brand new" }),
    )
    .await
    .expect("wrote");

    let undone = run(&harness.undo_tool(), json!({ "path": &path }))
        .await
        .expect("ran");
    assert!(
        text_of(&undone).contains("Nothing to undo"),
        "{}",
        text_of(&undone)
    );
    assert_eq!(
        tokio::fs::read_to_string(&path).await.expect("read"),
        "brand new",
        "the file was left alone"
    );
}

#[tokio::test]
async fn a_patch_can_be_taken_back() {
    let harness = Harness::new();
    let path = harness.path("code.rs");

    run(
        &harness.write_tool(),
        json!({ "path": &path, "content": "let value = 1;\n" }),
    )
    .await
    .expect("wrote");
    run(
        &harness.edit_tool(),
        json!({
            "path": &path,
            "old_string": "let value = 1;", "new_string": "let value = 2;",
        }),
    )
    .await
    .expect("patched");
    assert_eq!(
        tokio::fs::read_to_string(&path).await.expect("read"),
        "let value = 2;\n"
    );

    run(&harness.undo_tool(), json!({ "path": &path }))
        .await
        .expect("undone");
    assert_eq!(
        tokio::fs::read_to_string(&path).await.expect("read"),
        "let value = 1;\n"
    );
}

/// Each call steps back one change, and the tool says while there is another.
#[tokio::test]
async fn walks_back_through_several_changes() {
    let harness = Harness::new();
    let path = harness.path("note.txt");

    for content in ["one", "two", "three"] {
        run(
            &harness.write_tool(),
            json!({ "path": &path, "content": content }),
        )
        .await
        .expect("wrote");
    }

    let first = run(&harness.undo_tool(), json!({ "path": &path }))
        .await
        .expect("undone");
    assert_eq!(tokio::fs::read_to_string(&path).await.expect("read"), "two");
    assert!(
        text_of(&first).contains("earlier change"),
        "{}",
        text_of(&first)
    );

    let second = run(&harness.undo_tool(), json!({ "path": &path }))
        .await
        .expect("undone");
    assert_eq!(tokio::fs::read_to_string(&path).await.expect("read"), "one");
    assert!(
        !text_of(&second).contains("earlier change"),
        "{}",
        text_of(&second)
    );

    let third = run(&harness.undo_tool(), json!({ "path": &path }))
        .await
        .expect("ran");
    assert!(
        text_of(&third).contains("Nothing to undo"),
        "{}",
        text_of(&third)
    );
}

/// Snapshots belong to the store, not to the workspace. One inside the
/// repository would be a file the agent reads, searches and commits.
#[tokio::test]
async fn keeps_snapshots_out_of_the_workspace() {
    let harness = Harness::new();
    let path = harness.path("note.txt");

    run(
        &harness.write_tool(),
        json!({ "path": &path, "content": "first" }),
    )
    .await
    .expect("wrote");
    run(
        &harness.write_tool(),
        json!({ "path": &path, "content": "second" }),
    )
    .await
    .expect("wrote");

    let mut found = Vec::new();
    let mut stack = vec![Path::new(&harness.cwd).to_path_buf()];
    while let Some(directory) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&directory).await.expect("read dir");
        while let Some(entry) = entries.next_entry().await.expect("entry") {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                stack.push(entry_path);
            } else if entry_path.extension().is_some_and(|ext| ext == "snap") {
                found.push(entry_path);
            }
        }
    }
    let inside_workspace: Vec<_> = found
        .iter()
        .filter(|path| !path.starts_with(Path::new(&harness.cwd).join("snapshots")))
        .collect();
    assert!(
        inside_workspace.is_empty(),
        "snapshots landed in the workspace: {inside_workspace:?}"
    );
    assert!(!found.is_empty(), "the write left no snapshot at all");
}
