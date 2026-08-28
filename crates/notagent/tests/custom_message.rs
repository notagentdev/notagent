use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::custom_message::CustomMessageComponent;
use notagent::modes::interactive::theme::theme::{BlockStyle, init_theme, set_block_style};
use notagent::utils::ansi::strip_ansi;
use notagent_agent::CustomMessage;
use notagent_ai::types::UserContent;
use notagent_tui::tui::Component;

fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // tests pin the standard layout, the badge tests set Badge themselves.
    set_block_style(BlockStyle::Standard);
    guard
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

    // `outputPad` only ever reached the dropped custom renderer.
    component.set_output_pad(0);
    let unpadded: Vec<String> = component.render(40).iter().map(|l| strip_ansi(l)).collect();
    assert_eq!(unpadded, padded);
}

#[test]
fn badge_style_leads_with_a_type_badge_and_sheds_the_wash() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    let mut component = CustomMessageComponent::new(message(), None, Some(1));
    let raw = component.render(40).join("\n");
    let rendered = strip_ansi(&raw);

    assert!(rendered.contains("TEST"), "{rendered}");
    assert!(!rendered.contains("[test]"), "{rendered}");
    assert!(rendered.contains("custom"), "{rendered}");
}
