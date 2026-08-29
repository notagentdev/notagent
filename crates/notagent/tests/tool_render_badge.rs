use std::rc::Rc;

use notagent::core::tools::tool_definition::{
    ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
};
use notagent::core::tools::{ToolName, create_tool_definition};
use notagent::modes::interactive::components::tool_execution::{
    ToolExecutionComponent, ToolExecutionOptions,
};
use notagent::modes::interactive::theme::theme::{
    BlockStyle, ThemeColor, init_theme, set_block_style, theme,
};
use notagent::utils::ansi::strip_ansi;
use notagent_tui::tui::Component;
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

fn plain_call_block_lines(tool: &str, args: Value, execution_started: bool) -> Vec<String> {
    let mut component = ToolExecutionComponent::new(
        tool,
        "badge-call",
        args,
        ToolExecutionOptions::default(),
        None,
        Rc::new(|| {}),
        "/tmp",
    );
    if execution_started {
        component.mark_execution_started();
    }
    component
        .render(120)
        .iter()
        .map(|line| strip_ansi(line).trim_end().to_string())
        .collect()
}

fn first_content_line(lines: &[String]) -> &str {
    lines
        .iter()
        .find(|line| !line.is_empty())
        .map(String::as_str)
        .expect("the badge block renders content")
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
        Some(json!({
            "diff": diff,
            "warnings": ["Normalized replacement text into minified form before applying indentation expansion."]
        })),
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
    assert!(
        lines.iter().all(|line| !line.contains("Normalized")),
        "tool warnings belong to the agent result, not below the visible diff: {lines:?}"
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

#[test]
fn mutation_badges_put_diff_counts_after_the_file_name() {
    let _guard = badge_setup();
    let write_lines = plain_call_block_lines(
        "write",
        json!({"path": "/tmp/sample.py", "content": "a = 1\nb = 2"}),
        false,
    );
    let write_header = first_content_line(&write_lines);
    assert!(
        write_header.ends_with("(/tmp/sample.py) +2"),
        "the write count must follow the filename: {write_lines:?}"
    );

    let patch_lines = plain_call_block_lines(
        "patch",
        json!({
            "path": "/tmp/sample.py",
            "old_string": "a = 1",
            "new_string": "a = 2"
        }),
        false,
    );
    let patch_header = first_content_line(&patch_lines);
    assert!(
        patch_header.ends_with("(/tmp/sample.py) +1 -1"),
        "both patch counts must follow the filename: {patch_lines:?}"
    );
}

#[test]
fn mutation_file_names_use_the_ordinary_badge_text_color() {
    let _guard = badge_setup();
    let definition = create_tool_definition(ToolName::parse("write").expect("write"), "/tmp", None);
    let context = ToolRenderContext::new(
        "badge-call",
        json!({"path": "/tmp/sample.py", "content": "a = 1"}),
        "/tmp".to_string(),
    );
    let component = definition
        .render_call(&context.args.clone(), &theme(), &context)
        .expect("renders the call");
    let rendered = component.borrow_mut().render(80).join("\n");
    let ordinary_path = theme().fg(ThemeColor::ToolOutput, "/tmp/sample.py");
    let accented_path = theme().fg(ThemeColor::Accent, "/tmp/sample.py");
    assert!(
        rendered.contains(&ordinary_path),
        "the mutation filename must use the ordinary output color"
    );
    assert!(
        !rendered.contains(&accented_path),
        "the mutation filename must not use the accent color"
    );
}

#[test]
fn a_running_foreground_bash_puts_compact_timing_after_the_command() {
    let _guard = badge_setup();
    let lines = plain_call_block_lines(
        "bash",
        json!({"command": "printf ok", "timeout": 600}),
        true,
    );
    let header = first_content_line(&lines);
    assert!(
        header.ends_with("($ printf ok) [0s / 10m]"),
        "the compact timing must follow the complete command: {lines:?}"
    );
    assert!(
        !header.contains("[Running:"),
        "the badge timing must not repeat labels already conveyed by its position: {lines:?}"
    );
}
