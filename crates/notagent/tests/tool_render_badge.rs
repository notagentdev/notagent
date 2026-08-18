//! Badge-style properties of the tool renderers (user decisions 2026-08-18):
//! results stack directly under the badge line without a blank row, diffs
//! close with a summary line, a written file previews as an added-lines diff,
//! and a failed skill says so.
//!
//! The standard style stays the TS original and is pinned by
//! `tool_render_oracle.rs`; these cases pin the badge deviations.

use notagent::core::tools::tool_definition::{
    ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
};
use notagent::core::tools::{ToolName, create_tool_definition};
use notagent::modes::interactive::theme::theme::{BlockStyle, init_theme, set_block_style, theme};
use notagent::utils::ansi::strip_ansi;
use serde_json::{Value, json};

/// Serializes the cases: theme, block style and capabilities are global.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn badge_setup() -> std::sync::MutexGuard<'static, ()> {
    let guard = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);
    guard
}

fn plain_result_lines(
    tool: &str,
    args: Value,
    content_text: &str,
    details: Option<Value>,
    is_error: bool,
) -> Vec<String> {
    let name = ToolName::parse(tool).expect("known tool");
    let definition = create_tool_definition(name, "/tmp", None);
    let mut context = ToolRenderContext::new("badge-call", args, "/tmp".to_string());
    context.is_error = is_error;
    let content = vec![notagent_ai::types::TextOrImageContent::Text(
        notagent_ai::types::TextContent {
            text: content_text.to_string(),
            ..Default::default()
        },
    )];
    let component = definition
        .render_result(
            ToolRenderResult {
                content: &content,
                details: details.as_ref(),
            },
            ToolRenderResultOptions {
                expanded: false,
                is_partial: false,
            },
            &theme(),
            &context,
        )
        .expect("renders a result");
    let lines = component.borrow_mut().render(80);
    lines
        .iter()
        .map(|line| strip_ansi(line).trim_end().to_string())
        .collect()
}

#[test]
fn a_text_result_stacks_directly_under_the_badge() {
    let _guard = badge_setup();
    let lines = plain_result_lines(
        "task_list",
        json!({"all": true}),
        "1 task(s)",
        Some(json!({"count": 1})),
        false,
    );
    assert_eq!(lines.first().map(String::as_str), Some("1 task(s)"));
}

#[test]
fn a_failed_skill_says_failed() {
    let _guard = badge_setup();
    let lines = plain_result_lines("skill", json!({"name": "x"}), "boom", None, true);
    assert_eq!(lines, ["failed"]);
}

#[test]
fn a_patch_result_starts_with_the_diff_and_closes_with_the_summary() {
    let _guard = badge_setup();
    let diff = " 1 fn main() {\n-2     old();\n+2     new();\n 3 }";
    let lines = plain_result_lines(
        "patch_minified",
        json!({"path": "/tmp/x.rs"}),
        "ok",
        Some(json!({"diff": diff})),
        false,
    );
    assert!(
        !lines[0].is_empty(),
        "no blank row above the diff: {lines:?}"
    );
    assert_eq!(
        lines.last().map(String::as_str),
        Some("Added 1 line, removed 1 line"),
        "{lines:?}"
    );
}

#[test]
fn a_written_file_previews_as_an_added_lines_diff() {
    let _guard = badge_setup();
    let definition = create_tool_definition(ToolName::parse("write").expect("write"), "/tmp", None);
    let context = ToolRenderContext::new(
        "badge-call",
        json!({"path": "/tmp/sample.py", "content": "a = 1\nb = 2"}),
        "/tmp".to_string(),
    );
    let component = definition
        .render_call(&context.args.clone(), &theme(), &context)
        .expect("renders the call");
    let lines: Vec<String> = component
        .borrow_mut()
        .render(80)
        .iter()
        .map(|line| strip_ansi(line).trim_end().to_string())
        .collect();
    // The badge shell hoists the WRITE label; the call text starts at the path.
    assert!(lines[0].ends_with("sample.py"), "{lines:?}");
    assert_eq!(lines[1], "+1 a = 1", "no blank row, diff look: {lines:?}");
    assert_eq!(lines[2], "+2 b = 2", "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("Added 2 lines"));
}
