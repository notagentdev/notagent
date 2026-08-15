//! Pins the `renderCall`/`renderResult` halves of the built-in tools against
//! the TypeScript originals.
//!
//! `tools/gen-tool-render-oracle.mjs` renders every case with the real tool
//! definitions of the TypeScript repo (dark theme, truecolor, no hyperlinks, no
//! images) and writes `tests/fixtures/tool-render-oracle.json`. It is run with
//! `FORCE_COLOR=1` for the same reason as the render-diff oracle: the port
//! always styles, because it replaces chalk with direct ANSI sequences.

use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::tools::tool_definition::{
    ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
};
use notagent::core::tools::{ToolName, create_tool_definition};
use notagent::modes::interactive::theme::theme::{init_theme, theme};
use notagent_ai::types::{ImageContent, TextContent, TextOrImageContent};
use notagent_tui::{TerminalCapabilities, reset_capabilities_cache, set_capabilities};
use serde_json::Value;

fn global_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OracleCase {
    tool: String,
    args: Value,
    cwd: String,
    width: usize,
    expanded: bool,
    is_partial: bool,
    is_error: bool,
    show_images: bool,
    args_complete: bool,
    execution_started: bool,
    result: Option<OracleResult>,
    call_lines: Vec<String>,
    result_lines: Option<Vec<String>>,
}

#[derive(serde::Deserialize)]
struct OracleResult {
    content: Vec<Value>,
    details: Option<Value>,
}

/// `{HOME}` in a fixture string is this machine's home directory, so that the
/// generator and the test agree on the paths `shortenPath` abbreviates.
fn expand(value: &Value) -> Value {
    let home = dirs::home_dir()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();
    match value {
        Value::String(text) => Value::String(text.replace("{HOME}", &home)),
        Value::Array(entries) => Value::Array(entries.iter().map(expand).collect()),
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, entry)| (key.clone(), expand(entry)))
                .collect(),
        ),
        value => value.clone(),
    }
}

fn content_block(value: &Value) -> TextOrImageContent {
    match value.get("type").and_then(Value::as_str) {
        Some("image") => TextOrImageContent::Image(ImageContent {
            data: value
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            mime_type: value
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        _ => TextOrImageContent::Text(TextContent::new(
            value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )),
    }
}

#[test]
fn matches_the_typescript_renderers_for_every_oracle_case() {
    let _guard = global_lock();
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: true,
        hyperlinks: false,
    });
    init_theme(Some("dark"), false);

    let oracle: Vec<OracleCase> =
        serde_json::from_str(include_str!("fixtures/tool-render-oracle.json"))
            .expect("oracle parses");
    assert!(!oracle.is_empty());

    let theme = theme();
    let mut failures = Vec::new();
    for (index, case) in oracle.iter().enumerate() {
        let name = ToolName::parse(&case.tool).expect("known tool name");
        let definition = create_tool_definition(name, &case.cwd, None);
        let args = expand(&case.args);

        let mut context = ToolRenderContext::new("oracle-call", args.clone(), case.cwd.clone());
        context.execution_started = case.execution_started;
        context.args_complete = case.args_complete;
        context.is_partial = case.is_partial;
        context.expanded = case.expanded;
        context.show_images = case.show_images;
        context.is_error = case.is_error;

        let call = definition
            .render_call(&args, &theme, &context)
            .expect("built-in tools render their call");
        let call_lines = call.borrow_mut().render(case.width);
        if call_lines != case.call_lines {
            failures.push(format!(
                "case {index} ({}) call\n  expected {:?}\n  actual   {:?}",
                case.tool, case.call_lines, call_lines
            ));
        }

        if let Some(result) = &case.result {
            let content: Vec<TextOrImageContent> = result
                .content
                .iter()
                .map(|block| content_block(&expand(block)))
                .collect();
            let details = result.details.as_ref().map(expand);
            let rendered = definition
                .render_result(
                    ToolRenderResult {
                        content: &content,
                        details: details.as_ref(),
                    },
                    ToolRenderResultOptions {
                        expanded: case.expanded,
                        is_partial: case.is_partial,
                    },
                    &theme,
                    &context,
                )
                .expect("built-in tools render their result");
            let result_lines = rendered.borrow_mut().render(case.width);
            let expected = case.result_lines.clone().unwrap_or_default();
            if result_lines != expected {
                failures.push(format!(
                    "case {index} ({}) result\n  expected {expected:?}\n  actual   {result_lines:?}",
                    case.tool
                ));
            }
        }
    }

    reset_capabilities_cache();
    assert!(
        failures.is_empty(),
        "{} of {} oracle cases differ:\n{}",
        failures.len(),
        oracle.len(),
        failures.join("\n")
    );
}
