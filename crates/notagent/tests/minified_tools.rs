use std::path::PathBuf;

use notagent::core::tools::patch_minified::{
    create_multi_patch_minified_tool_definition, create_patch_minified_tool_definition,
};
use notagent::core::tools::read::create_read_tool_definition;
use notagent::core::tools::tool_definition::ToolDefinition;
use notagent_agent::types::AgentToolResult;
use notagent_ai::types::TextOrImageContent;
use notagent_ai::utils::estimate::estimate_text_tokens;
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
// read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn returns_the_compact_view_of_a_source_file() {
    let workspace = Workspace::new();
    let source = "/// Doc comment.\nfn main() {\n    // gone\n    let x = 1;\n}\n";
    let path = workspace.write("main.rs", source);
    let tool = create_read_tool_definition(&workspace.dir(), None);

    let result = run(&tool, json!({ "path": path })).await.expect("reads");
    let compact = text_output(&result);

    assert_eq!(compact, "fn main() {\n let x = 1;\n}\n");
    assert_eq!(
        result.details.as_ref().expect("details")["minified"],
        json!(true)
    );
}

#[tokio::test]
async fn a_commented_source_file_saves_at_least_half_the_estimated_tokens() {
    let workspace = Workspace::new();
    let source = "/// Doc comment.\nfn main() {\n    // gone\n    let x = 1;\n}\n";
    let path = workspace.write("main.rs", source);
    let tool = create_read_tool_definition(&workspace.dir(), None);

    let result = run(&tool, json!({ "path": path })).await.expect("reads");
    let compact = text_output(&result);
    let raw_tokens = estimate_text_tokens(source);
    let compact_tokens = estimate_text_tokens(&compact);

    assert_eq!(
        (raw_tokens, compact_tokens),
        (15, 7),
        "keep the README fixture measurement current"
    );
    assert!(
        compact_tokens * 2 <= raw_tokens,
        "a representative minified read must save at least half the estimated tokens: raw={raw_tokens}, compact={compact_tokens}"
    );
}

#[tokio::test]
async fn keeps_comments_when_asked() {
    let workspace = Workspace::new();
    let path = workspace.write(
        "main.rs",
        "fn main() {\n    // keep me\n    let x = 1;\n}\n",
    );
    let tool = create_read_tool_definition(&workspace.dir(), None);

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
    let tool = create_read_tool_definition(&workspace.dir(), None);

    let result = run(&tool, json!({ "path": path, "offset": 4, "limit": 3 }))
        .await
        .expect("reads");

    assert_eq!(
        text_output(&result),
        "fn b() {\n let y = 2;\n}\n\n[1 more lines in file. Use offset=7 to continue.]"
    );
}

#[tokio::test]
async fn rejects_an_offset_beyond_the_end_of_the_file() {
    let workspace = Workspace::new();
    let path = workspace.write("main.rs", "fn a() {}\n");
    let tool = create_read_tool_definition(&workspace.dir(), None);

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
    let tool = create_read_tool_definition(&workspace.dir(), None);

    let result = run(&tool, json!({ "path": path })).await.expect("reads");

    let text = text_output(&result);
    assert!(text.starts_with("alpha\n\nbeta\n"), "{text}");
    assert_eq!(text, "alpha\n\nbeta\n");
    assert_eq!(
        result.details.as_ref().expect("details")["minified"],
        json!(false)
    );
}

#[tokio::test]
async fn rejects_pdfs_instead_of_returning_binary_text() {
    let workspace = Workspace::new();
    let path = workspace.write("document.pdf", "%PDF-1.7\n");
    let tool = create_read_tool_definition(&workspace.dir(), None);
    let error = run(&tool, json!({ "path": path }))
        .await
        .expect_err("unsupported PDF");
    assert!(error.message.contains("PDF reading is not supported"));
}

#[tokio::test]
async fn original_view_requires_a_reason_and_preserves_exact_source() {
    let workspace = Workspace::new();
    let source = "/// Documentation.\nfn main() {\n    // keep me\n}\n";
    let path = workspace.write("main.rs", source);
    let tool = create_read_tool_definition(&workspace.dir(), None);
    for reason in [json!(null), json!(""), json!("  ")] {
        let error = run(
            &tool,
            json!({"path": path, "view": "original", "reason": reason}),
        )
        .await
        .expect_err("original requires a reason");
        assert!(error.message.contains("keep_comments=true"));
    }
    let result = run(
        &tool,
        json!({"path": path, "view": "original", "reason": "Check exact source line references"}),
    )
    .await
    .expect("original source");
    assert_eq!(text_output(&result), source);
    assert_eq!(result.details.as_ref().unwrap()["minified"], json!(false));
    assert_eq!(result.details.as_ref().unwrap()["view"], json!("original"));
}

#[test]
fn tool_presets_offer_one_reader_with_explicit_comment_instructions() {
    use notagent::core::tools::{
        create_all_tool_definitions, create_coding_tool_definitions,
        create_read_only_tool_definitions,
    };
    let presets = [
        create_all_tool_definitions("/tmp", None)
            .into_values()
            .collect::<Vec<_>>(),
        create_coding_tool_definitions("/tmp", None),
        create_read_only_tool_definitions("/tmp", None),
    ];
    for tools in presets {
        assert_eq!(tools.iter().filter(|tool| tool.name() == "read").count(), 1);
        assert!(tools.iter().all(|tool| tool.name() != "read_minified"));
        let read = tools.iter().find(|tool| tool.name() == "read").unwrap();
        assert!(read.description().contains("must set keep_comments=true"));
        assert!(
            read.prompt_guidelines()
                .join(" ")
                .contains("must set keep_comments=true")
        );
    }
}

#[tokio::test]
async fn comment_only_source_requires_the_comment_switch_to_be_visible() {
    let workspace = Workspace::new();
    let path = workspace.write("notes.rs", "// important instruction\n");
    let tool = create_read_tool_definition(&workspace.dir(), None);
    let compact = run(&tool, json!({"path": path})).await.unwrap();
    assert!(!text_output(&compact).contains("important instruction"));
    let commented = run(&tool, json!({"path": path, "keep_comments": true}))
        .await
        .unwrap();
    assert!(text_output(&commented).contains("important instruction"));
}
