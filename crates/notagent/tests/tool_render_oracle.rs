//! Pins the `renderCall`/`renderResult` halves of the built-in tools against
//! the TypeScript originals.
//!
//! `tools/gen-tool-render-oracle.mjs` renders every case with the real tool
//! definitions of the TypeScript repo (dark theme, truecolor, no hyperlinks, no
//! images) and writes `tests/fixtures/tool-render-oracle.json`. It is run with
//! `FORCE_COLOR=1` for the same reason as the render-diff oracle: the port
//! always styles, because it replaces chalk with direct ANSI sequences.

use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::tools::bash::BashRenderState;
use notagent::core::tools::tool_definition::{
    ToolRenderContext, ToolRenderResult, ToolRenderResultOptions, tool_render_state,
};
use notagent::core::tools::{ToolName, create_tool_definition};
use notagent::modes::interactive::theme::theme::{BlockStyle, init_theme, set_block_style, theme};
use notagent::utils::ansi::strip_ansi;
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
    /// A file the case needs on disk, for the renderers that read one.
    file: Option<OracleFile>,
    /// Seeds the `bash` clock so the elapsed line is deterministic.
    state_elapsed: Option<OracleElapsed>,
    /// Runs the pending render work (the `edit` preview) and draws again.
    pump_preview: bool,
    /// `bytes` compares the rendered lines verbatim; `plain` strips the ANSI
    /// first, for the cases that run through the syntax highlighter (master
    /// plan, class 3: highlight.js is substituted by tree-sitter, so the token
    /// colours differ by construction while the layout must not).
    compare: String,
    steps: Vec<OracleStep>,
    cwd: String,
    width: usize,
    expanded: bool,
    is_partial: bool,
    is_error: bool,
    show_images: bool,
    execution_started: bool,
    result: Option<OracleResult>,
    call_line_steps: Vec<Vec<String>>,
    result_lines: Option<Vec<String>>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OracleStep {
    args: Value,
    args_complete: bool,
}

#[derive(serde::Deserialize)]
struct OracleFile {
    path: String,
    content: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OracleElapsed {
    started_at: u64,
    ended_at: u64,
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

/// The lines as the case compares them.
fn comparable(lines: &[String], compare: &str) -> Vec<String> {
    match compare {
        "plain" => lines
            .iter()
            .map(|line| strip_ansi(line).trim_end().to_string())
            .collect(),
        _ => lines.to_vec(),
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
    // These pins are the TS-parity look: the standard style, not the badge
    // default (v0.1.9).
    set_block_style(BlockStyle::Standard);
    // Set after the theme is loaded, like the generator does: the compact `read`
    // header resolves the packaged docs against it.
    // SAFETY: single-threaded test, guarded by the module lock above.
    unsafe { std::env::set_var("NOTAGENT_PACKAGE_DIR", "/oracle-package") };

    let oracle: Vec<OracleCase> =
        serde_json::from_str(include_str!("fixtures/tool-render-oracle.json"))
            .expect("oracle parses");
    assert!(!oracle.is_empty());

    // Every built-in tool renders both halves, so every one of them is pinned.
    for name in notagent::core::tools::ALL_TOOL_NAMES {
        assert!(
            oracle.iter().any(|case| case.tool == name.as_str()),
            "{} has no oracle case",
            name.as_str()
        );
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let theme = theme();
    let mut failures = Vec::new();
    for (index, case) in oracle.iter().enumerate() {
        let name = ToolName::parse(&case.tool).expect("known tool name");
        let definition = create_tool_definition(name, &case.cwd, None);

        // One context per case, so the steps share their render state exactly as
        // the rows of `tool-execution.ts` do.
        if let Some(file) = &case.file {
            let path = std::path::Path::new(&file.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("fixture directory");
            }
            std::fs::write(path, &file.content).expect("fixture file");
        }

        let mut context = ToolRenderContext::new("oracle-call", Value::Null, case.cwd.clone());
        context.execution_started = case.execution_started;
        context.is_partial = case.is_partial;
        context.expanded = case.expanded;
        context.show_images = case.show_images;
        context.is_error = case.is_error;

        if let Some(elapsed) = &case.state_elapsed {
            let now = std::time::Instant::now();
            let mut state = tool_render_state::<BashRenderState>(&context.state);
            state.started_at =
                Some(now - std::time::Duration::from_millis(elapsed.ended_at - elapsed.started_at));
            state.ended_at = Some(now);
        }

        for (step_index, step) in case.steps.iter().enumerate() {
            let args = expand(&step.args);
            context.args = args.clone();
            context.args_complete = step.args_complete;
            let call = definition
                .render_call(&args, &theme, &context)
                .expect("built-in tools render their call");
            let call_lines = comparable(&call.borrow_mut().render(case.width), &case.compare);
            let expected = comparable(
                case.call_line_steps
                    .get(step_index)
                    .map_or(&[][..], Vec::as_slice),
                &case.compare,
            );
            if call_lines != expected {
                failures.push(format!(
                    "case {index} ({}) call step {step_index}\n  expected {expected:?}\n  actual   {call_lines:?}",
                    case.tool
                ));
            }
        }

        if case.pump_preview {
            // The render loop awaits the work a renderer started; here that is
            // the `edit` preview.
            if let Some(pump) = definition.pump_render(&context) {
                runtime.block_on(pump);
            }
            let step = case.steps.last().expect("a case has steps");
            let args = expand(&step.args);
            context.args = args.clone();
            context.args_complete = step.args_complete;
            let call = definition
                .render_call(&args, &theme, &context)
                .expect("built-in tools render their call");
            let call_lines = comparable(&call.borrow_mut().render(case.width), &case.compare);
            let expected = comparable(
                case.call_line_steps.last().map_or(&[][..], Vec::as_slice),
                &case.compare,
            );
            if call_lines != expected {
                failures.push(format!(
                    "case {index} ({}) call after pump\n  expected {expected:?}\n  actual   {call_lines:?}",
                    case.tool
                ));
            }
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
            let result_lines = comparable(&rendered.borrow_mut().render(case.width), &case.compare);
            let expected = comparable(case.result_lines.as_deref().unwrap_or(&[]), &case.compare);
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
