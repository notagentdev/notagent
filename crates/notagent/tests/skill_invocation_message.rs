//! Tests for `components/skill-invocation-message.ts`, which has no TypeScript
//! suite: what the component renders is only asserted through the interactive
//! mode there. The cases pin both states of `updateDisplay`.

use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::agent_session::ParsedSkillBlock;
use notagent::core::keybindings::KeybindingsManager;
use notagent::modes::interactive::components::skill_invocation_message::SkillInvocationMessageComponent;
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::tui::Component;

/// The theme and the keybindings registry are process globals.
fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    init_theme(Some("dark"), false);
    set_keybindings(KeybindingsManager::default().to_tui());
    guard
}

fn skill_block() -> ParsedSkillBlock {
    ParsedSkillBlock {
        name: "commit".to_string(),
        location: "/skills/commit".to_string(),
        content: "Write the message first.".to_string(),
        user_message: None,
    }
}

#[test]
fn collapsed_shows_the_label_the_name_and_the_expand_hint() {
    let _guard = test_lock();

    let mut component = SkillInvocationMessageComponent::new(skill_block(), None);
    let rendered = strip_ansi(&component.render(60).join("\n"));

    assert!(rendered.contains("[skill] commit"), "{rendered}");
    assert!(rendered.contains("to expand"), "{rendered}");
    assert!(!rendered.contains("Write the message first."), "{rendered}");
}

#[test]
fn expanded_shows_the_name_as_a_heading_and_the_full_content() {
    let _guard = test_lock();

    let mut component = SkillInvocationMessageComponent::new(skill_block(), None);
    component.set_expanded(true);
    let rendered = strip_ansi(&component.render(60).join("\n"));

    assert!(rendered.contains("[skill]"), "{rendered}");
    assert!(rendered.contains("commit"), "{rendered}");
    assert!(rendered.contains("Write the message first."), "{rendered}");
    // The expand hint belongs to the collapsed line only.
    assert!(!rendered.contains("to expand"), "{rendered}");
}

#[test]
fn collapsing_again_rebuilds_the_single_line() {
    let _guard = test_lock();

    let mut component = SkillInvocationMessageComponent::new(skill_block(), None);
    component.set_expanded(true);
    component.set_expanded(false);
    let rendered = strip_ansi(&component.render(60).join("\n"));

    assert!(rendered.contains("[skill] commit"), "{rendered}");
    assert!(!rendered.contains("Write the message first."), "{rendered}");
}
