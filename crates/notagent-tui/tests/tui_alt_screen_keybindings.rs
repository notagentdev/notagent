//! Port of the custom-binding cases of
//! `packages/tui/test/tui-alt-screen.test.ts`.
//!
//! They live in their own test binary because they replace the global
//! keybindings manager, which must not race with the other cases.

use notagent_tui::components::text::Text;
use notagent_tui::keybindings::{
    KeybindingsConfig, KeybindingsManager, TUI_KEYBINDINGS, set_keybindings,
};
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{TuiStopOptions, component_ref};
use notagent_tui::tui_alt_screen::{TuiAltScreen, TuiAltScreenOptions};

fn numbered_text(count: usize) -> String {
    (1..=count)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn install_bindings(entries: &[(&str, &str)]) {
    let mut config = KeybindingsConfig::new();
    for (id, key) in entries {
        config.insert((*id).to_string(), vec![(*key).to_string()]);
    }
    set_keybindings(KeybindingsManager::new(TUI_KEYBINDINGS, config));
}

fn restore_bindings() {
    set_keybindings(KeybindingsManager::new(
        TUI_KEYBINDINGS,
        KeybindingsConfig::new(),
    ));
}

#[tokio::test]
async fn scrolls_the_transcript_with_custom_bindings() {
    // Both TS cases run in one function: the global manager is process state.
    let terminal = VirtualTerminal::new(20, 10);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    install_bindings(&[
        ("tui.altScreen.halfPageUp", "ctrl+u"),
        ("tui.altScreen.halfPageDown", "ctrl+d"),
    ]);
    tui.core()
        .add_child(component_ref(Text::new(numbered_text(30), 0, 0)));
    tui.start();
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 20);

    tui.handle_terminal_input("\x15");
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 15);

    tui.handle_terminal_input("\x04");
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 20);
    tui.stop(TuiStopOptions::default());

    let terminal = VirtualTerminal::new(20, 10);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    install_bindings(&[
        ("tui.altScreen.lineUp", "ctrl+y"),
        ("tui.altScreen.lineDown", "ctrl+e"),
    ]);
    tui.core()
        .add_child(component_ref(Text::new(numbered_text(30), 0, 0)));
    tui.start();
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 20);

    tui.handle_terminal_input("\x19");
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 19);

    tui.handle_terminal_input("\x05");
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 20);

    tui.stop(TuiStopOptions::default());
    restore_bindings();
}
