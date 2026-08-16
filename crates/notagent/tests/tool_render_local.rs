//! The two render paths the TypeScript oracle cannot pin.
//!
//! `tests/tool_render_oracle.rs` compares every other case against the real
//! TypeScript renderers. Two paths are checked here instead, because their
//! output depends on something the two implementations cannot share: the
//! wording of a filesystem error (Node's `Error code: ENOENT` against Rust's
//! message, deviation class 1) and the clock of a command that is still
//! running.

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use notagent::core::tools::bash::BashRenderState;
use notagent::core::tools::tool_definition::{
    ToolRenderContext, ToolRenderResult, ToolRenderResultOptions, tool_render_state,
};
use notagent::core::tools::{ToolName, create_tool_definition};
use notagent::modes::interactive::theme::theme::{BlockStyle, init_theme, set_block_style, theme};
use notagent::utils::ansi::strip_ansi;
use notagent_ai::types::{TextContent, TextOrImageContent};
use notagent_tui::{TerminalCapabilities, reset_capabilities_cache, set_capabilities};
use serde_json::json;

fn global_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn prepare() {
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: true,
        hyperlinks: false,
    });
    init_theme(Some("dark"), false);
    // These pins are the TS-parity look: the standard style, not the badge
    // default (v0.1.9).
    set_block_style(BlockStyle::Standard);
}

fn rendered(component: &notagent_tui::tui::ComponentRef, width: usize) -> String {
    component
        .borrow_mut()
        .render(width)
        .iter()
        .map(|line| strip_ansi(line).trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_edit_preview_reports_a_file_it_cannot_read() {
    let _guard = global_lock();
    prepare();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let definition = create_tool_definition(ToolName::Edit, "/oracle-cwd", None);
    let args = json!({
        "path": "/oracle-cwd/missing-file.ts",
        "edits": [{ "oldText": "a", "newText": "b" }],
    });
    let mut context = ToolRenderContext::new("call-1", args.clone(), "/oracle-cwd");
    context.args_complete = true;

    let theme = theme();
    definition
        .render_call(&args, &theme, &context)
        .expect("the call renders");
    // The preview is pending, so the render loop is asked to run it.
    assert!(definition.render_deadline(&context).is_some());
    let pump = definition.pump_render(&context).expect("work to do");
    runtime.block_on(pump);
    assert!(
        definition.render_deadline(&context).is_none(),
        "nothing is left pending once the preview landed"
    );

    let call = definition
        .render_call(&args, &theme, &context)
        .expect("the call renders");
    let text = rendered(&call, 100);
    assert!(
        text.contains("Could not edit file: /oracle-cwd/missing-file.ts."),
        "the preview shows why the file cannot be edited: {text}"
    );
    reset_capabilities_cache();
}

#[test]
fn the_bash_result_counts_up_while_the_command_runs() {
    let _guard = global_lock();
    prepare();

    let definition = create_tool_definition(ToolName::Bash, "/oracle-cwd", None);
    let args = json!({ "command": "sleep 5" });
    let mut context = ToolRenderContext::new("call-1", args.clone(), "/oracle-cwd");
    context.execution_started = true;
    context.is_partial = true;

    let theme = theme();
    definition
        .render_call(&args, &theme, &context)
        .expect("the call renders");
    {
        // Pretend the command has been running for two and a half seconds.
        let mut state = tool_render_state::<BashRenderState>(&context.state);
        assert!(state.started_at.is_some(), "the clock starts with the run");
        state.started_at = Some(Instant::now() - Duration::from_millis(2500));
    }

    let content = vec![TextOrImageContent::Text(TextContent::new("working"))];
    let result = definition
        .render_result(
            ToolRenderResult {
                content: &content,
                details: None,
            },
            ToolRenderResultOptions {
                expanded: false,
                is_partial: true,
            },
            &theme,
            &context,
        )
        .expect("the result renders");
    let text = rendered(&result, 100);
    assert!(
        text.contains("Elapsed 2.5s"),
        "a running command shows the elapsed time: {text}"
    );

    // The tick the TS version schedules with setInterval is a deadline here.
    let deadline = definition
        .render_deadline(&context)
        .expect("the elapsed line ticks");
    assert!(deadline > Instant::now());

    let result = definition
        .render_result(
            ToolRenderResult {
                content: &content,
                details: None,
            },
            ToolRenderResultOptions {
                expanded: false,
                is_partial: false,
            },
            &theme,
            &context,
        )
        .expect("the result renders");
    let text = rendered(&result, 100);
    assert!(
        text.contains("Took 2.5s"),
        "a finished command shows how long it took: {text}"
    );
    assert!(
        definition.render_deadline(&context).is_none(),
        "and stops ticking"
    );
    reset_capabilities_cache();
}
