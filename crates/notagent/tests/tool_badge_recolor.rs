//! The badge must flip from the pending fill to success/error as soon
//! as the result arrives (mid-turn), not at the end of the turn.

use std::rc::Rc;

use notagent::modes::interactive::components::tool_execution::{
    ToolExecutionComponent, ToolExecutionOptions, ToolExecutionResult,
};
use notagent::modes::interactive::theme::theme::{
    BlockStyle, ThemeBg, init_theme, set_block_style, theme,
};
use notagent_tui::tui::Component;
use serde_json::json;

#[test]
fn the_badge_recolors_when_the_result_arrives() {
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    let mut component = ToolExecutionComponent::new(
        "bash",
        "call-1",
        json!({"command": "ls"}),
        ToolExecutionOptions::default(),
        None,
        Rc::new(|| {}),
        "/tmp",
    );
    component.mark_execution_started();

    // Extract just the background SGR sequence of each state fill.
    fn bg_sequence(sample: &str) -> String {
        let start = sample.find("\u{1b}[48;").expect("bg sequence");
        let end = sample[start..].find('m').expect("terminator") + start + 1;
        sample[start..end].to_string()
    }
    let pending_fill = bg_sequence(&theme().bg(ThemeBg::ToolPendingBg.badge_fill(), "x"));
    let success_fill = bg_sequence(&theme().bg(ThemeBg::ToolSuccessBg.badge_fill(), "x"));
    assert_ne!(pending_fill, success_fill, "theme fills must differ");

    let before = component.render(80).join("\n");
    assert!(
        before.contains(&pending_fill),
        "pending badge missing; lines: {before:?}"
    );

    component.update_result(
        ToolExecutionResult {
            content: vec![notagent_ai::types::TextOrImageContent::Text(
                notagent_ai::types::TextContent {
                    text: "ok".to_string(),
                    ..Default::default()
                },
            )],
            details: None,
            is_error: false,
        },
        false,
    );

    let after = component.render(80).join("\n");
    assert!(
        after.contains(&success_fill),
        "success badge missing after the result; lines: {after:?}"
    );
    assert!(
        !after.contains(&pending_fill),
        "badge still wears the pending fill; lines: {after:?}"
    );
}
