//! Port of the component half of
//! `packages/coding-agent/test/tool-execution-component.test.ts` (537 LOC).
//!
//! The remaining cases of that file drive `createBashToolDefinition` and belong
//! to the bash tool itself (`tests/bash_tool.rs`); ported here are the five that
//! exercise `ToolExecutionComponent`.
//!
//! Class-1 deviation in the harness: TS builds tool definitions as object
//! literals with optional `renderCall`/`renderResult`; here they are small
//! structs implementing `ToolDefinition`.

use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::tools::ToolDef;
use notagent::core::tools::tool_definition::{
    RenderShell, ToolDefinition, ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
};
use notagent::modes::interactive::components::tool_execution::{
    ToolExecutionComponent, ToolExecutionOptions, ToolExecutionResult,
};
use notagent::modes::interactive::theme::theme::{BlockStyle, Theme, init_theme, set_block_style};
use notagent::utils::ansi::strip_ansi;
use notagent_agent::types::{AgentToolResult, ToolExecutionError};
use notagent_ai::types::{TextContent, TextOrImageContent};
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, ComponentRef, component_ref};
use serde_json::{Value, json};

/// The theme and the block style are process-global; the TS-parity cases pin
/// the standard layout, the badge cases set Badge themselves.
fn guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_block_style(BlockStyle::Standard);
    guard
}

/// `createBaseToolDefinition(name)` plus the optional renderers of each case.
struct StubTool {
    name: String,
    parameters: Value,
    render_shell: RenderShell,
    call_text: Option<&'static str>,
    result_text: Option<&'static str>,
}

impl StubTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            parameters: json!({}),
            render_shell: RenderShell::Default,
            call_text: None,
            result_text: None,
        }
    }
}

impl ToolDefinition for StubTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "custom tool"
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn render_shell(&self) -> RenderShell {
        self.render_shell
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _params: Value,
        _signal: Option<tokio_util::sync::CancellationToken>,
        _on_update: Option<notagent_agent::types::AgentToolUpdateCallback>,
        _context: Option<notagent::core::tools::tool_definition::ToolContext>,
    ) -> futures::future::BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent {
                    text: "ok".to_string(),
                    ..TextContent::default()
                })],
                ..AgentToolResult::default()
            })
        })
    }

    fn render_call(
        &self,
        _args: &Value,
        _theme: &Theme,
        _context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        self.call_text
            .map(|text| component_ref(Text::new(text, 0, 0)) as ComponentRef)
    }

    fn render_result(
        &self,
        _result: ToolRenderResult<'_>,
        _options: ToolRenderResultOptions,
        _theme: &Theme,
        _context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        self.result_text
            .map(|text| component_ref(Text::new(text, 0, 0)) as ComponentRef)
    }
}

fn text_result(text: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        content: vec![TextOrImageContent::Text(TextContent {
            text: text.to_string(),
            ..TextContent::default()
        })],
        details: Some(json!({})),
        is_error: false,
    }
}

fn no_render() -> Rc<dyn Fn()> {
    Rc::new(|| {})
}

fn cwd() -> String {
    std::env::current_dir()
        .expect("cwd")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn stacks_custom_call_and_result_renderers_like_the_old_implementation() {
    let _guard = guard();
    init_theme(Some("dark"), false);

    let tool: ToolDef = std::sync::Arc::new(StubTool {
        call_text: Some("custom call"),
        result_text: Some("custom result"),
        ..StubTool::new("custom_tool")
    });
    let mut component = ToolExecutionComponent::new(
        "custom_tool",
        "tool-1",
        json!({}),
        ToolExecutionOptions::default(),
        Some(tool),
        no_render(),
        cwd(),
    );
    assert!(strip_ansi(&component.render(120).join("\n")).contains("custom call"));

    component.update_result(text_result("done"), false);

    let rendered = strip_ansi(&component.render(120).join("\n"));
    assert!(rendered.contains("custom call"), "{rendered}");
    assert!(rendered.contains("custom result"), "{rendered}");
}

#[test]
fn self_rendered_empty_tool_rows_take_no_layout_space() {
    let _guard = guard();
    init_theme(Some("dark"), false);

    let tool: ToolDef = std::sync::Arc::new(StubTool {
        render_shell: RenderShell::SelfManaged,
        call_text: Some(""),
        result_text: Some(""),
        ..StubTool::new("custom_tool")
    });
    let mut component = ToolExecutionComponent::new(
        "custom_tool",
        "tool-empty-self-render",
        json!({}),
        ToolExecutionOptions::default(),
        Some(tool),
        no_render(),
        cwd(),
    );
    assert_eq!(component.render(120), Vec::<String>::new());

    component.update_result(
        ToolExecutionResult {
            content: Vec::new(),
            details: Some(json!({})),
            is_error: false,
        },
        false,
    );

    assert_eq!(component.render(120), Vec::<String>::new());
}

#[test]
fn uses_built_in_rendering_for_built_in_overrides_without_custom_renderers() {
    let _guard = guard();
    init_theme(Some("dark"), false);

    let tool: ToolDef = std::sync::Arc::new(StubTool::new("edit"));
    let mut component = ToolExecutionComponent::new(
        "edit",
        "tool-2",
        json!({ "path": "README.md", "oldText": "before", "newText": "after" }),
        ToolExecutionOptions::default(),
        Some(tool),
        no_render(),
        cwd(),
    );
    component.update_result(
        ToolExecutionResult {
            content: Vec::new(),
            details: Some(json!({ "diff": "+1 after", "firstChangedLine": 1 })),
            is_error: false,
        },
        false,
    );

    let rendered = strip_ansi(&component.render(120).join("\n"));
    assert!(rendered.contains("edit"), "{rendered}");
    assert!(rendered.contains("README.md"), "{rendered}");
    assert!(!rendered.contains(":1"), "{rendered}");
}

#[test]
fn preserves_legacy_file_path_rendering_compatibility_for_built_in_tools() {
    let _guard = guard();
    init_theme(Some("dark"), false);

    let mut component = ToolExecutionComponent::new(
        "read",
        "tool-3",
        json!({ "file_path": "README.md" }),
        ToolExecutionOptions::default(),
        None,
        no_render(),
        cwd(),
    );

    let rendered = strip_ansi(&component.render(120).join("\n"));
    assert!(rendered.contains("read"), "{rendered}");
    assert!(rendered.contains("README.md"), "{rendered}");
}

#[test]
fn falls_back_to_the_plain_header_and_output_without_any_definition() {
    let _guard = guard();
    init_theme(Some("dark"), false);

    let mut component = ToolExecutionComponent::new(
        "not_a_tool",
        "tool-4",
        json!({ "some": "argument" }),
        ToolExecutionOptions::default(),
        None,
        no_render(),
        cwd(),
    );
    component.update_result(text_result("the output"), false);

    let rendered = strip_ansi(&component.render(120).join("\n"));
    assert!(rendered.contains("not_a_tool"), "{rendered}");
    assert!(rendered.contains("\"some\": \"argument\""), "{rendered}");
    assert!(rendered.contains("the output"), "{rendered}");
}

// ---------------------------------------------------------------------------
// Badge block style (takeover of the reference's `tool_execution.rs` cases)
// ---------------------------------------------------------------------------

#[test]
fn badge_style_leads_with_a_state_badge_and_drops_the_padding_rows() {
    let _guard = guard();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    let tool: ToolDef = std::sync::Arc::new(StubTool {
        call_text: Some("custom call"),
        ..StubTool::new("custom_tool")
    });
    let mut component = ToolExecutionComponent::new(
        "custom_tool",
        "tool-badge",
        json!({}),
        ToolExecutionOptions::default(),
        Some(tool),
        no_render(),
        cwd(),
    );
    component.update_result(text_result(""), false);

    let stripped: Vec<String> = component
        .render(60)
        .iter()
        .map(|line| strip_ansi(line).trim_end().to_string())
        .collect();

    // Spacer, then badge and call sharing one row (underscores read as
    // spaces) — no surface, no padding rows.
    let expected = vec!["".to_string(), "  CUSTOM TOOL  (custom call)".to_string()];
    assert_eq!(stripped, expected);
}

#[test]
fn badge_style_badge_carries_the_three_states() {
    use notagent::modes::interactive::theme::theme::{ThemeBg, badge, theme};

    let _guard = guard();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    let build = |id: &str| {
        let tool: ToolDef = std::sync::Arc::new(StubTool::new("custom_tool"));
        ToolExecutionComponent::new(
            "custom_tool",
            id,
            json!({}),
            ToolExecutionOptions::default(),
            Some(tool),
            no_render(),
            cwd(),
        )
    };
    let mut pending = build("tool-pending");
    let mut succeeded = build("tool-ok");
    succeeded.update_result(text_result(""), false);
    let mut failed = build("tool-err");
    failed.update_result(
        ToolExecutionResult {
            content: Vec::new(),
            details: Some(json!({})),
            is_error: true,
        },
        false,
    );

    let theme_instance = theme();
    let actual = [
        pending.render(60).join("\n").contains(&badge(
            &theme_instance,
            ThemeBg::ToolPendingBg,
            "custom_tool",
        )),
        succeeded.render(60).join("\n").contains(&badge(
            &theme_instance,
            ThemeBg::ToolSuccessBg,
            "custom_tool",
        )),
        failed.render(60).join("\n").contains(&badge(
            &theme_instance,
            ThemeBg::ToolErrorBg,
            "custom_tool",
        )),
    ];
    assert_eq!(actual, [true, true, true]);
}

#[test]
fn badge_style_fallback_shares_the_badge_row_with_the_first_output_line() {
    let _guard = guard();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    // No renderer at all — the plain-text fallback.
    let mut component = ToolExecutionComponent::new(
        "goal",
        "tool-goal-badge",
        json!({}),
        ToolExecutionOptions::default(),
        None,
        no_render(),
        cwd(),
    );
    component.update_result(text_result("done"), false);

    let stripped: Vec<String> = component
        .render(60)
        .iter()
        .map(|line| strip_ansi(line).trim_end().to_string())
        .collect();

    // Spacer, then the badge (with its pill padding) sharing its row with
    // the output — no padding rows, no second name line, no argument JSON
    // while collapsed.
    let expected = vec!["".to_string(), "  GOAL  done".to_string()];
    assert_eq!(stripped, expected);
}

#[test]
fn badge_style_expanded_result_closes_with_the_collapse_info_line() {
    let _guard = guard();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    let tool: ToolDef = std::sync::Arc::new(StubTool {
        call_text: Some("custom call"),
        result_text: Some("full output"),
        ..StubTool::new("custom_tool")
    });
    let mut component = ToolExecutionComponent::new(
        "custom_tool",
        "tool-collapse-hint",
        json!({}),
        ToolExecutionOptions::default(),
        Some(tool),
        no_render(),
        cwd(),
    );
    component.update_result(text_result("hidden detail"), false);

    let collapsed = strip_ansi(&component.render(60).join("\n"));
    assert!(!collapsed.contains("to collapse)"), "{collapsed}");

    component.set_expanded(true);
    let expanded: Vec<String> = component
        .render(60)
        .iter()
        .map(|line| strip_ansi(line).trim_end().to_string())
        .collect();
    assert!(
        expanded
            .last()
            .is_some_and(|line| line.contains("to collapse)")),
        "{expanded:?}"
    );
}

#[test]
fn switching_the_style_restyles_an_already_rendered_row() {
    let _guard = guard();
    init_theme(Some("dark"), false);

    let tool: ToolDef = std::sync::Arc::new(StubTool {
        call_text: Some("custom call"),
        ..StubTool::new("custom_tool")
    });
    let mut component = ToolExecutionComponent::new(
        "custom_tool",
        "tool-style-switch",
        json!({}),
        ToolExecutionOptions::default(),
        Some(tool),
        no_render(),
        cwd(),
    );
    component.update_result(text_result(""), false);
    let standard = strip_ansi(&component.render(60).join("\n"));
    assert!(!standard.contains("CUSTOM TOOL"), "{standard}");

    set_block_style(BlockStyle::Badge);
    let badge = strip_ansi(&component.render(60).join("\n"));
    assert!(badge.contains("CUSTOM TOOL"), "{badge}");
    set_block_style(BlockStyle::Standard);
}
