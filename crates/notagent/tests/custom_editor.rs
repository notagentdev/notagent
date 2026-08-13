//! Port of `packages/coding-agent/test/custom-editor-history-keybindings.test.ts` (34 LOC).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::keybindings::KeybindingsManager;
use notagent::modes::interactive::components::custom_editor::CustomEditor;
use notagent_tui::components::editor::EditorTheme;
use notagent_tui::components::select_list::SelectListTheme;
use notagent_tui::editor_component::EditorComponent;
use notagent_tui::keybindings::{KeybindingsConfig, set_keybindings};
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::Component;
use notagent_tui::tui_main_screen::TuiMainScreen;

/// The keybindings registry the editor resolves its history keys against is a
/// process global, so the cases run one at a time.
fn keybindings_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// `packages/tui/test/test-themes.ts`
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
fn gives_an_explicit_history_binding_precedence_over_model_cycling() {
    let _guard = keybindings_lock();
    let mut config = KeybindingsConfig::new();
    config.insert(
        "tui.editor.historyPrevious".to_string(),
        vec!["ctrl+p".to_string()],
    );
    config.insert(
        "tui.editor.historyNext".to_string(),
        vec!["ctrl+n".to_string()],
    );
    let keybindings = Rc::new(RefCell::new(KeybindingsManager::new(config, None)));
    set_keybindings(keybindings.borrow().to_tui());

    let tui = TuiMainScreen::new(Box::new(VirtualTerminal::new(80, 24)));
    let mut editor = CustomEditor::new(
        tui.core().clone(),
        default_editor_theme(),
        Rc::clone(&keybindings),
        None,
    );
    let model_cycles = Rc::new(RefCell::new(0usize));
    let sink = Rc::clone(&model_cycles);
    editor.on_action(
        "app.model.cycleForward",
        Box::new(move || {
            *sink.borrow_mut() += 1;
            true
        }),
    );
    editor.add_to_history("previous prompt");
    editor.set_text("draft");

    editor.handle_input("\x10"); // Ctrl+P
    assert_eq!(editor.get_text(), "previous prompt");
    assert_eq!(*model_cycles.borrow(), 0);

    editor.handle_input("\x0e"); // Ctrl+N
    assert_eq!(editor.get_text(), "draft");

    set_keybindings(KeybindingsManager::default().to_tui());
}

#[test]
fn dispatches_app_actions_and_lets_a_declining_handler_fall_through() {
    let _guard = keybindings_lock();
    let keybindings = Rc::new(RefCell::new(KeybindingsManager::default()));
    set_keybindings(keybindings.borrow().to_tui());

    let tui = TuiMainScreen::new(Box::new(VirtualTerminal::new(80, 24)));
    let mut editor = CustomEditor::new(
        tui.core().clone(),
        default_editor_theme(),
        Rc::clone(&keybindings),
        None,
    );

    // `app.tools.expand` is ctrl+o and claims the key.
    let expands = Rc::new(RefCell::new(0usize));
    let sink = Rc::clone(&expands);
    editor.on_action(
        "app.tools.expand",
        Box::new(move || {
            *sink.borrow_mut() += 1;
            true
        }),
    );
    editor.handle_input("\x0f");
    assert_eq!(*expands.borrow(), 1);
    assert_eq!(editor.get_text(), "");

    // A handler returning `false` declines the key, which then reaches the editor.
    let declines = Rc::new(RefCell::new(0usize));
    let sink = Rc::clone(&declines);
    editor.on_action(
        "app.editor.external",
        Box::new(move || {
            *sink.borrow_mut() += 1;
            false
        }),
    );
    editor.handle_input("\x07"); // Ctrl+G
    assert_eq!(*declines.borrow(), 1);

    // Escape prefers the dynamic handler over the registered action.
    let escapes = Rc::new(RefCell::new(0usize));
    let sink = Rc::clone(&escapes);
    editor.on_action(
        "app.interrupt",
        Box::new(move || {
            *sink.borrow_mut() += 1;
            true
        }),
    );
    let dynamic_escapes = Rc::new(RefCell::new(0usize));
    let sink = Rc::clone(&dynamic_escapes);
    editor.on_escape = Some(Box::new(move || *sink.borrow_mut() += 1));
    editor.handle_input("\x1b");
    assert_eq!(*dynamic_escapes.borrow(), 1);
    assert_eq!(*escapes.borrow(), 0);

    editor.on_escape = None;
    editor.handle_input("\x1b");
    assert_eq!(*escapes.borrow(), 1);
}

#[test]
fn exit_only_fires_on_an_empty_editor() {
    let _guard = keybindings_lock();
    let keybindings = Rc::new(RefCell::new(KeybindingsManager::default()));
    set_keybindings(keybindings.borrow().to_tui());

    let tui = TuiMainScreen::new(Box::new(VirtualTerminal::new(80, 24)));
    let mut editor = CustomEditor::new(
        tui.core().clone(),
        default_editor_theme(),
        Rc::clone(&keybindings),
        None,
    );
    let exits = Rc::new(RefCell::new(0usize));
    let sink = Rc::clone(&exits);
    editor.on_ctrl_d = Some(Box::new(move || *sink.borrow_mut() += 1));

    editor.handle_input("\x04"); // Ctrl+D on an empty editor
    assert_eq!(*exits.borrow(), 1);

    // With text the key falls through to delete-char-forward.
    editor.set_text("ab");
    editor.handle_input("\x1b[H"); // home
    editor.handle_input("\x04");
    assert_eq!(*exits.borrow(), 1);
    assert_eq!(editor.get_text(), "b");
}
