//! Port of `packages/coding-agent/test/patch-minified-tool.test.ts`, plus the
//! `read_minified` cases its own file states as behaviour but never exercises
//! (the TS suite has no read_minified test file).

use std::path::PathBuf;

use notagent::core::tools::patch_minified::{
    create_multi_patch_minified_tool_definition, create_patch_minified_tool_definition,
};
use notagent::core::tools::read_minified::create_read_minified_tool_definition;
use notagent::core::tools::tool_definition::ToolDefinition;
use notagent_agent::types::AgentToolResult;
use notagent_ai::types::TextOrImageContent;
use serde_json::{Value, json};

struct Workspace {
    path: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("notagent-test-minified-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        Self { path }
    }

    fn dir(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, content: &str) -> String {
        let path = self.path.join(name);
        std::fs::write(&path, content).expect("writes");
        path.to_string_lossy().into_owned()
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path.join(name)).expect("reads")
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

async fn run(
    tool: &dyn ToolDefinition,
    input: Value,
) -> Result<AgentToolResult, notagent_agent::types::ToolExecutionError> {
    tool.execute("t", input, None, None, None).await
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

fn warnings(result: &AgentToolResult) -> Vec<String> {
    result.details.as_ref().expect("details")["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .map(|warning| warning.as_str().unwrap_or_default().to_owned())
        .collect()
}

const RUST_FIXTURE: &str = "fn main() {\n    // keep me\n    let x = 1;\n    let y = 2;\n}\n";

// ---------------------------------------------------------------------------
// patch_minified
// ---------------------------------------------------------------------------

#[tokio::test]
async fn splices_a_single_edit_and_leaves_the_rest_identical() {
    let workspace = Workspace::new();
    let path = workspace.write("main.rs", RUST_FIXTURE);
    let tool = create_patch_minified_tool_definition(&workspace.dir(), None);

    let result = run(
        &tool,
        json!({ "path": path, "old_string": "let x = 1;", "new_string": "let x = 42;" }),
    )
    .await
    .expect("applies");

    assert_eq!(
        workspace.read("main.rs"),
        "fn main() {\n    // keep me\n    let x = 42;\n    let y = 2;\n}\n"
    );
    assert_eq!(warnings(&result), Vec::<String>::new());
}

#[tokio::test]
async fn surfaces_warnings_in_the_text_output() {
    let workspace = Workspace::new();
    let path = workspace.write(
        "main.rs",
        "fn main() {\n    let x = 1;\n    // hidden\n    let y = 2;\n}\n",
    );
    let tool = create_patch_minified_tool_definition(&workspace.dir(), None);

    let result = run(
        &tool,
        json!({
            "path": path,
            "old_string": "let x = 1;\n let y = 2;",
            "new_string": "let x = 1;\n let y = 3;",
        }),
    )
    .await
    .expect("applies");

    assert!(text_output(&result).contains("1 comment(s)"));
    assert_eq!(warnings(&result).len(), 1);
}

#[tokio::test]
async fn rejects_an_ambiguous_match_without_replace_all_and_writes_nothing() {
    let workspace = Workspace::new();
    let source = "fn main() {\n    foo(1);\n    foo(1);\n}\n";
    let path = workspace.write("main.rs", source);
    let tool = create_patch_minified_tool_definition(&workspace.dir(), None);

    let error = run(
        &tool,
        json!({ "path": path, "old_string": "foo(1)", "new_string": "foo(2)" }),
    )
    .await
    .expect_err("ambiguous");
    assert!(
        error.message.contains("Multiple matches"),
        "{}",
        error.message
    );
    assert_eq!(workspace.read("main.rs"), source);
}

#[tokio::test]
async fn replaces_every_occurrence_with_replace_all() {
    let workspace = Workspace::new();
    let path = workspace.write(
        "main.rs",
        "fn main() {\n    foo(1);\n    bar();\n    foo(1);\n}\n",
    );
    let tool = create_patch_minified_tool_definition(&workspace.dir(), None);

    run(
        &tool,
        json!({
            "path": path,
            "old_string": "foo(1)",
            "new_string": "foo(2)",
            "replace_all": true,
        }),
    )
    .await
    .expect("applies");

    assert_eq!(
        workspace.read("main.rs"),
        "fn main() {\n    foo(2);\n    bar();\n    foo(2);\n}\n"
    );
}

#[tokio::test]
async fn falls_back_to_raw_exact_replacement_for_unsupported_languages() {
    let workspace = Workspace::new();
    let path = workspace.write("notes.txt", "alpha\nbeta\ngamma\n");
    let tool = create_patch_minified_tool_definition(&workspace.dir(), None);

    run(
        &tool,
        json!({ "path": path, "old_string": "beta", "new_string": "delta" }),
    )
    .await
    .expect("applies");

    assert_eq!(workspace.read("notes.txt"), "alpha\ndelta\ngamma\n");
}

#[tokio::test]
async fn reports_a_raw_fallback_ambiguity_instead_of_silently_replacing() {
    let workspace = Workspace::new();
    let source = "alpha\nalpha\n";
    let path = workspace.write("notes.txt", source);
    let tool = create_patch_minified_tool_definition(&workspace.dir(), None);

    let error = run(
        &tool,
        json!({ "path": path, "old_string": "alpha", "new_string": "beta" }),
    )
    .await
    .expect_err("ambiguous");
    assert!(
        error.message.contains("Multiple matches"),
        "{}",
        error.message
    );
    assert_eq!(workspace.read("notes.txt"), source);
}

#[tokio::test]
async fn preserves_a_bom() {
    let workspace = Workspace::new();
    let path = workspace.write("main.rs", "\u{feff}fn main() {\n    let x = 1;\n}\n");
    let tool = create_patch_minified_tool_definition(&workspace.dir(), None);

    run(
        &tool,
        json!({ "path": path, "old_string": "let x = 1;", "new_string": "let x = 2;" }),
    )
    .await
    .expect("applies");

    assert_eq!(
        workspace.read("main.rs"),
        "\u{feff}fn main() {\n    let x = 2;\n}\n"
    );
}

// ---------------------------------------------------------------------------
// multi_patch_minified
// ---------------------------------------------------------------------------

#[tokio::test]
async fn applies_edits_sequentially_against_the_previous_result() {
    let workspace = Workspace::new();
    let path = workspace.write("main.rs", RUST_FIXTURE);
    let tool = create_multi_patch_minified_tool_definition(&workspace.dir(), None);

    run(
        &tool,
        json!({
            "path": path,
            "edits": [
                { "old_string": "let x = 1;", "new_string": "let x = 10;" },
                // Only matches if the first edit was already applied.
                { "old_string": "let x = 10;", "new_string": "let x = 11;" },
                { "old_string": "let y = 2;", "new_string": "let y = 20;" },
            ],
        }),
    )
    .await
    .expect("applies");

    assert_eq!(
        workspace.read("main.rs"),
        "fn main() {\n    // keep me\n    let x = 11;\n    let y = 20;\n}\n"
    );
}

#[tokio::test]
async fn is_atomic_a_failing_edit_leaves_the_file_untouched() {
    let workspace = Workspace::new();
    let path = workspace.write("main.rs", RUST_FIXTURE);
    let tool = create_multi_patch_minified_tool_definition(&workspace.dir(), None);

    let error = run(
        &tool,
        json!({
            "path": path,
            "edits": [
                { "old_string": "let x = 1;", "new_string": "let x = 10;" },
                { "old_string": "does_not_exist", "new_string": "nope" },
            ],
        }),
    )
    .await
    .expect_err("second edit fails");
    assert!(
        error.message.contains("Could not find match"),
        "{}",
        error.message
    );
    assert_eq!(workspace.read("main.rs"), RUST_FIXTURE);
}

#[tokio::test]
async fn rejects_an_empty_edit_list() {
    let workspace = Workspace::new();
    let path = workspace.write("main.rs", RUST_FIXTURE);
    let tool = create_multi_patch_minified_tool_definition(&workspace.dir(), None);

    let error = run(&tool, json!({ "path": path, "edits": [] }))
        .await
        .expect_err("no edits");
    assert!(
        error.message.contains("at least one replacement"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn collects_warnings_from_every_edit() {
    let workspace = Workspace::new();
    let path = workspace.write(
        "main.py",
        "def f(a):\n    # old\n    if a:\n        return 1\n    return 2\n",
    );
    let tool = create_multi_patch_minified_tool_definition(&workspace.dir(), None);

    let result = run(
        &tool,
        json!({
            "path": path,
            "edits": [
                { "old_string": "# old", "new_string": "# updated" },
                { "old_string": "if a:\n  return 1", "new_string": "if a:\n  return 10" },
            ],
        }),
    )
    .await
    .expect("applies");

    assert_eq!(
        workspace.read("main.py"),
        "def f(a):\n    # updated\n    if a:\n        return 10\n    return 2\n"
    );
    assert!(
        warnings(&result)
            .iter()
            .any(|warning| warning.contains("keep_comments enabled"))
    );
}

// ---------------------------------------------------------------------------
// read_minified
// ---------------------------------------------------------------------------

#[tokio::test]
async fn returns_the_compact_view_of_a_source_file() {
    let workspace = Workspace::new();
    let path = workspace.write(
        "main.rs",
        "/// Doc comment.\nfn main() {\n    // gone\n    let x = 1;\n}\n",
    );
    let tool = create_read_minified_tool_definition(&workspace.dir(), None);

    let result = run(&tool, json!({ "path": path })).await.expect("reads");

    assert_eq!(text_output(&result), "fn main() {\n let x = 1;\n}\n");
    assert_eq!(
        result.details.as_ref().expect("details")["minified"],
        json!(true)
    );
}

#[tokio::test]
async fn keeps_comments_when_asked() {
    let workspace = Workspace::new();
    let path = workspace.write(
        "main.rs",
        "fn main() {\n    // keep me\n    let x = 1;\n}\n",
    );
    let tool = create_read_minified_tool_definition(&workspace.dir(), None);

    let result = run(&tool, json!({ "path": path, "keep_comments": true }))
        .await
        .expect("reads");

    assert!(text_output(&result).contains("// keep me"));
}

#[tokio::test]
async fn applies_the_offset_and_limit_range_before_minification() {
    let workspace = Workspace::new();
    let path = workspace.write(
        "main.rs",
        "fn a() {\n    let x = 1;\n}\nfn b() {\n    let y = 2;\n}\n",
    );
    let tool = create_read_minified_tool_definition(&workspace.dir(), None);

    let result = run(&tool, json!({ "path": path, "offset": 4, "limit": 3 }))
        .await
        .expect("reads");

    assert_eq!(text_output(&result), "fn b() {\n let y = 2;\n}");
}

#[tokio::test]
async fn rejects_an_offset_beyond_the_end_of_the_file() {
    let workspace = Workspace::new();
    let path = workspace.write("main.rs", "fn a() {}\n");
    let tool = create_read_minified_tool_definition(&workspace.dir(), None);

    let error = run(&tool, json!({ "path": path, "offset": 99 }))
        .await
        .expect_err("beyond the end");
    assert!(
        error.message.contains("Offset 99 is beyond end of file"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn falls_back_to_raw_content_for_an_unsupported_language() {
    let workspace = Workspace::new();
    let path = workspace.write("notes.txt", "alpha\n\nbeta\n");
    let tool = create_read_minified_tool_definition(&workspace.dir(), None);

    let result = run(&tool, json!({ "path": path })).await.expect("reads");

    let text = text_output(&result);
    assert!(text.starts_with("alpha\n\nbeta\n"), "{text}");
    assert!(text.contains("[Language not supported for minification; raw content shown.]"));
    assert_eq!(
        result.details.as_ref().expect("details")["minified"],
        json!(false)
    );
}

#[tokio::test]
async fn rejects_visual_content() {
    let workspace = Workspace::new();
    // A one-pixel PNG: enough of a header for the mime sniffer.
    let png: Vec<u8> = {
        use std::io::Write;
        let mut bytes = vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00,
        ];
        bytes.write_all(&[0x1f, 0x15, 0xc4, 0x89]).expect("writes");
        bytes
    };
    let path = workspace.path.join("shot.png");
    std::fs::write(&path, &png).expect("writes");
    let tool = create_read_minified_tool_definition(&workspace.dir(), None);

    let error = run(&tool, json!({ "path": path.to_string_lossy() }))
        .await
        .expect_err("visual content");
    assert!(
        error.message.contains("is visual content (image/PDF)"),
        "{}",
        error.message
    );
}
