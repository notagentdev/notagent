use std::collections::HashMap;

use notagent_tui::keybindings::{
    KeybindingConflict, KeybindingsConfig, KeybindingsManager, TUI_KEYBINDINGS,
};

fn config(entries: &[(&str, &[&str])]) -> KeybindingsConfig {
    entries
        .iter()
        .map(|(id, keys)| {
            (
                (*id).to_string(),
                keys.iter().map(|key| (*key).to_string()).collect(),
            )
        })
        .collect()
}

#[test]
fn binds_ctrl_j_as_a_default_newline_alias() {
    let keybindings = KeybindingsManager::new(TUI_KEYBINDINGS, HashMap::new());

    assert_eq!(
        keybindings.get_keys("tui.input.newLine"),
        ["shift+enter", "ctrl+j"]
    );
    assert!(keybindings.matches("\n", "tui.input.newLine"));
    assert!(keybindings.matches("\x1b[106;5u", "tui.input.newLine"));
}

#[test]
fn binds_modified_and_unmodified_editor_viewport_navigation() {
    let keybindings = KeybindingsManager::new(TUI_KEYBINDINGS, HashMap::new());

    assert_eq!(
        keybindings.get_keys("tui.editor.cursorLineStart"),
        ["home", "ctrl+home", "ctrl+a"]
    );
    assert_eq!(
        keybindings.get_keys("tui.editor.cursorLineEnd"),
        ["end", "ctrl+end", "ctrl+e"]
    );
    assert_eq!(
        keybindings.get_keys("tui.editor.pageUp"),
        ["pageUp", "ctrl+pageUp"]
    );
    assert_eq!(
        keybindings.get_keys("tui.editor.pageDown"),
        ["pageDown", "ctrl+pageDown"]
    );
}

#[test]
fn leaves_dedicated_prompt_history_navigation_unbound_by_default() {
    let keybindings = KeybindingsManager::new(TUI_KEYBINDINGS, HashMap::new());

    assert!(
        keybindings
            .get_keys("tui.editor.historyPrevious")
            .is_empty()
    );
    assert!(keybindings.get_keys("tui.editor.historyNext").is_empty());
}

#[test]
fn binds_unmodified_terminal_viewport_shortcuts_to_alternate_screen_navigation() {
    let keybindings = KeybindingsManager::new(TUI_KEYBINDINGS, HashMap::new());

    assert_eq!(keybindings.get_keys("tui.altScreen.pageUp"), ["pageUp"]);
    assert_eq!(keybindings.get_keys("tui.altScreen.pageDown"), ["pageDown"]);
    assert!(keybindings.get_keys("tui.altScreen.halfPageUp").is_empty());
    assert!(
        keybindings
            .get_keys("tui.altScreen.halfPageDown")
            .is_empty()
    );
    assert!(keybindings.get_keys("tui.altScreen.lineUp").is_empty());
    assert!(keybindings.get_keys("tui.altScreen.lineDown").is_empty());
    assert_eq!(
        keybindings.get_keys("tui.altScreen.previousPrompt"),
        ["ctrl+shift+up"]
    );
    assert_eq!(
        keybindings.get_keys("tui.altScreen.nextPrompt"),
        ["ctrl+shift+down"]
    );
    assert_eq!(
        keybindings.get_keys("tui.altScreen.search"),
        ["ctrl+shift+f"]
    );
    assert_eq!(
        keybindings.get_keys("tui.altScreen.searchNext"),
        ["enter", "ctrl+g"]
    );
    assert_eq!(
        keybindings.get_keys("tui.altScreen.searchPrevious"),
        ["shift+enter", "ctrl+shift+g"]
    );
    assert_eq!(
        keybindings.get_keys("tui.altScreen.searchClose"),
        ["escape"]
    );
    assert_eq!(keybindings.get_keys("tui.altScreen.top"), ["home"]);
    assert_eq!(keybindings.get_keys("tui.altScreen.bottom"), ["end"]);
}

#[test]
fn does_not_evict_selector_confirm_when_input_submit_is_rebound() {
    let keybindings = KeybindingsManager::new(
        TUI_KEYBINDINGS,
        config(&[("tui.input.submit", &["enter", "ctrl+enter"])]),
    );

    assert_eq!(
        keybindings.get_keys("tui.input.submit"),
        ["enter", "ctrl+enter"]
    );
    assert_eq!(keybindings.get_keys("tui.select.confirm"), ["enter"]);
}

#[test]
fn does_not_evict_cursor_bindings_when_another_action_reuses_the_same_key() {
    let keybindings = KeybindingsManager::new(
        TUI_KEYBINDINGS,
        config(&[("tui.select.up", &["up", "ctrl+p"])]),
    );

    assert_eq!(keybindings.get_keys("tui.select.up"), ["up", "ctrl+p"]);
    assert_eq!(keybindings.get_keys("tui.editor.cursorUp"), ["up"]);
}

#[test]
fn still_reports_direct_user_binding_conflicts_without_evicting_defaults() {
    let keybindings = KeybindingsManager::new(
        TUI_KEYBINDINGS,
        config(&[
            ("tui.input.submit", &["ctrl+x"]),
            ("tui.select.confirm", &["ctrl+x"]),
        ]),
    );

    let mut conflicts = keybindings.get_conflicts();
    for conflict in &mut conflicts {
        conflict.keybindings.sort();
    }
    assert_eq!(
        conflicts,
        vec![KeybindingConflict {
            key: "ctrl+x".to_string(),
            keybindings: vec![
                "tui.input.submit".to_string(),
                "tui.select.confirm".to_string()
            ],
        }]
    );
    assert_eq!(
        keybindings.get_keys("tui.editor.cursorLeft"),
        ["left", "ctrl+b"]
    );
}
