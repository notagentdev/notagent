use std::rc::Rc;

use notagent_tui::components::editor::{Editor, EditorOptions, EditorTheme};
use notagent_tui::components::select_list::SelectListTheme;
use notagent_tui::keybindings::{
    KeybindingsConfig, KeybindingsManager, TUI_KEYBINDINGS, set_keybindings,
};
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::Component;
use notagent_tui::tui_main_screen::TuiMainScreen;

fn default_editor_theme() -> EditorTheme {
    EditorTheme {
        border_color: Rc::new(|text| format!("\x1b[2m{text}\x1b[22m")),
        select_list: Rc::new(|| SelectListTheme {
            selected_prefix: Rc::new(|text| format!("\x1b[34m{text}\x1b[39m")),
            selected_text: Rc::new(|text| format!("\x1b[1m{text}\x1b[22m")),
            description: Rc::new(|text| format!("\x1b[2m{text}\x1b[22m")),
            scroll_info: Rc::new(|text| format!("\x1b[2m{text}\x1b[22m")),
            no_match: Rc::new(|text| format!("\x1b[2m{text}\x1b[22m")),
        }),
    }
}

#[test]
fn browses_history_directly_without_first_moving_the_cursor() {
    let mut config = KeybindingsConfig::new();
    config.insert(
        "tui.editor.historyPrevious".to_string(),
        vec!["ctrl+p".to_string()],
    );
    config.insert(
        "tui.editor.historyNext".to_string(),
        vec!["ctrl+n".to_string()],
    );
    set_keybindings(KeybindingsManager::new(TUI_KEYBINDINGS, config));

    let tui = TuiMainScreen::new(Box::new(VirtualTerminal::new(80, 24)));
    let mut editor = Editor::new(
        tui.core().clone(),
        default_editor_theme(),
        EditorOptions::default(),
    );
    editor.add_to_history("older prompt");
    editor.add_to_history("newer\nmultiline prompt");
    editor.set_text("draft");
    editor.handle_input("\x1b[D");
    editor.handle_input("\x1b[D");

    editor.handle_input("\x10");
    assert_eq!(editor.get_text(), "newer\nmultiline prompt");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x10");
    assert_eq!(editor.get_text(), "older prompt");

    editor.handle_input("\x0e");
    assert_eq!(editor.get_text(), "newer\nmultiline prompt");
    assert_eq!(editor.get_cursor(), (1, 16));

    editor.handle_input("\x0e");
    assert_eq!(editor.get_text(), "draft");
    assert_eq!(editor.get_cursor(), (0, 3));

    set_keybindings(KeybindingsManager::new(
        TUI_KEYBINDINGS,
        KeybindingsConfig::new(),
    ));
}
