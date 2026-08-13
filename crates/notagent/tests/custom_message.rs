//! Port of `packages/coding-agent/test/custom-message.test.ts` (44 LOC).
//!
//! The TypeScript case drives the component through a custom `MessageRenderer`,
//! which only extensions can supply and which the port drops
//! (`plans/facts/extension-boundary.md`). What remains observable is that the
//! output padding reaches the default rendering and that changing it rebuilds
//! the component, which is what this test asserts.

use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::custom_message::CustomMessageComponent;
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_agent::CustomMessage;
use notagent_ai::types::UserContent;
use notagent_tui::tui::Component;

fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn message() -> CustomMessage {
    CustomMessage {
        custom_type: "test".to_string(),
        content: UserContent::Text("custom".to_string()),
        display: true,
        details: None,
        timestamp: 0,
    }
}

#[test]
fn renders_the_custom_type_label_and_the_message_content() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = CustomMessageComponent::new(message(), None, Some(1));
    let rendered = strip_ansi(&component.render(40).join("\n"));

    assert!(rendered.contains("[test]"), "{rendered}");
    assert!(rendered.contains("custom"), "{rendered}");
}

#[test]
fn keeps_the_default_rendering_when_the_output_padding_changes() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = CustomMessageComponent::new(message(), None, Some(1));
    let padded: Vec<String> = component.render(40).iter().map(|l| strip_ansi(l)).collect();
    assert!(
        padded.iter().any(|line| line.starts_with(" custom")),
        "{padded:?}"
    );

    // The default rendering hard-codes the box padding to 1 in TypeScript too;
    // `outputPad` only ever reached the dropped custom renderer.
    component.set_output_pad(0);
    let unpadded: Vec<String> = component.render(40).iter().map(|l| strip_ansi(l)).collect();
    assert_eq!(unpadded, padded);
}
