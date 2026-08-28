use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::keybindings::KeybindingsManager;
use notagent::modes::interactive::components::text_input_dialog::{
    TextInputDialogComponent, TextInputDialogOptions,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, Focusable};
use notagent_tui::tui_main_screen::TuiMainScreen;

/// Both the theme and the keybindings registry the dialog resolves against are
/// process globals, so the cases run one at a time.
fn registry_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

const ESCAPE: &str = "\x1b";
const ENTER: &str = "\r";
const CTRL_G: &str = "\x07";

struct Fixture {
    dialog: TextInputDialogComponent,
    submitted: Rc<RefCell<Vec<String>>>,
    cancelled: Rc<RefCell<usize>>,
}

fn fixture(options: TextInputDialogOptions) -> Fixture {
    init_theme(Some("dark"), false);
    let keybindings = Rc::new(RefCell::new(KeybindingsManager::default()));
    set_keybindings(keybindings.borrow().to_tui());

    let submitted: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let cancelled = Rc::new(RefCell::new(0usize));

    let submit_sink = Rc::clone(&submitted);
    let cancel_sink = Rc::clone(&cancelled);
    let tui = TuiMainScreen::new(Box::new(VirtualTerminal::new(80, 24)));
    let dialog = TextInputDialogComponent::new(
        tui.core().clone(),
        keybindings,
        "Custom summarization instructions",
        Box::new(move |value| submit_sink.borrow_mut().push(value)),
        Box::new(move || *cancel_sink.borrow_mut() += 1),
        Some(options),
    );

    Fixture {
        dialog,
        submitted,
        cancelled,
    }
}

/// The runner the tree wiring supplies, recording what it was asked to edit.
fn recording_runner(
    log: &Rc<RefCell<Vec<(String, String)>>>,
    reply: Option<&'static str>,
) -> notagent::modes::interactive::components::text_input_dialog::ExternalEditorRunner {
    let log = Rc::clone(log);
    Box::new(move |command, content| {
        log.borrow_mut()
            .push((command.to_string(), content.to_string()));
        reply.map(str::to_string)
    })
}

#[test]
fn renders_the_title_the_hint_and_both_borders() {
    let _guard = registry_lock();
    let mut fixture = fixture(TextInputDialogOptions::default());

    let lines: Vec<String> = fixture
        .dialog
        .render(60)
        .iter()
        .map(|line| strip_ansi(line))
        .collect();
    let screen = lines.join("\n");

    assert!(
        screen.contains("Custom summarization instructions"),
        "the title is missing:\n{screen}"
    );
    // `keyHint("tui.select.confirm", "submit")` and the three that follow it.
    for hint in ["submit", "newline", "cancel", "external editor"] {
        assert!(
            screen.contains(hint),
            "the hint {hint:?} is missing:\n{screen}"
        );
    }
    // `new DynamicBorder()` above and below; the editor between them draws its
    // own pair, which is why four rows are rules.
    let is_border =
        |line: &String| !line.trim().is_empty() && line.trim().chars().all(|char| char == '─');
    assert!(
        lines.first().is_some_and(is_border) && lines.last().is_some_and(is_border),
        "the dialog is not bordered:\n{screen}"
    );
    assert_eq!(
        lines.iter().filter(|line| is_border(line)).count(),
        4,
        "expected the dialog's two borders and the editor's own:\n{screen}"
    );
}

#[test]
fn a_prefill_lands_in_the_editor() {
    let _guard = registry_lock();
    let fixture = fixture(TextInputDialogOptions {
        prefill: Some("summarize the refactor".to_string()),
        ..TextInputDialogOptions::default()
    });

    assert_eq!(fixture.dialog.text(), "summarize the refactor");
}

#[test]
fn enter_submits_the_text_and_escape_cancels() {
    let _guard = registry_lock();
    let mut fixture = fixture(TextInputDialogOptions::default());

    fixture.dialog.handle_input("keep the api notes");
    assert_eq!(fixture.dialog.text(), "keep the api notes");
    assert!(fixture.submitted.borrow().is_empty());

    fixture.dialog.handle_input(ENTER);
    assert_eq!(*fixture.submitted.borrow(), vec!["keep the api notes"]);
    assert_eq!(*fixture.cancelled.borrow(), 0);

    fixture.dialog.handle_input(ESCAPE);
    assert_eq!(*fixture.cancelled.borrow(), 1);
    // Escape never reaches the editor.
    assert_eq!(fixture.submitted.borrow().len(), 1);
}

#[test]
fn the_external_editor_gets_the_text_and_its_result_replaces_it() {
    let _guard = registry_lock();
    let external: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let mut fixture = fixture(TextInputDialogOptions {
        external_editor_command: Some("my-editor".to_string()),
        on_external_editor: Some(recording_runner(&external, Some("edited elsewhere"))),
        ..TextInputDialogOptions::default()
    });
    fixture.dialog.handle_input("draft");

    fixture.dialog.handle_input(CTRL_G);

    assert_eq!(
        *external.borrow(),
        vec![("my-editor".to_string(), "draft".to_string())]
    );
    assert_eq!(fixture.dialog.text(), "edited elsewhere");
    // Ctrl+G is not typed into the editor, and it is not a submit.
    assert!(fixture.submitted.borrow().is_empty());
}

#[test]
fn an_abandoned_external_editor_leaves_the_text_alone() {
    let _guard = registry_lock();
    let external: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let mut fixture = fixture(TextInputDialogOptions {
        external_editor_command: Some("my-editor".to_string()),
        on_external_editor: Some(recording_runner(&external, None)),
        ..TextInputDialogOptions::default()
    });
    fixture.dialog.handle_input("draft");

    fixture.dialog.handle_input(CTRL_G);

    assert_eq!(external.borrow().len(), 1);
    assert_eq!(fixture.dialog.text(), "draft");
}

#[test]
fn without_a_runner_the_external_editor_key_does_nothing() {
    let _guard = registry_lock();
    let mut fixture = fixture(TextInputDialogOptions::default());
    fixture.dialog.handle_input("draft");

    fixture.dialog.handle_input(CTRL_G);

    assert_eq!(fixture.dialog.text(), "draft");
    assert!(fixture.submitted.borrow().is_empty());
}

#[test]
fn the_command_falls_back_to_the_environment_and_then_to_nano() {
    let _guard = registry_lock();
    let external: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
    // SAFETY: the case holds the registry lock, so no other test reads the
    // environment while it is changed.
    unsafe {
        std::env::set_var("VISUAL", "visual-editor");
        std::env::remove_var("EDITOR");
    }
    let mut visual = fixture(TextInputDialogOptions {
        on_external_editor: Some(recording_runner(&external, None)),
        ..TextInputDialogOptions::default()
    });

    visual.dialog.handle_input(CTRL_G);
    assert_eq!(external.borrow()[0].0, "visual-editor");

    unsafe {
        std::env::remove_var("VISUAL");
        std::env::set_var("EDITOR", "env-editor");
    }
    let mut editor = fixture(TextInputDialogOptions {
        on_external_editor: Some(recording_runner(&external, None)),
        ..TextInputDialogOptions::default()
    });
    editor.dialog.handle_input(CTRL_G);
    assert_eq!(external.borrow()[1].0, "env-editor");

    unsafe {
        std::env::remove_var("EDITOR");
    }
    let mut fallback = fixture(TextInputDialogOptions {
        on_external_editor: Some(recording_runner(&external, None)),
        ..TextInputDialogOptions::default()
    });
    fallback.dialog.handle_input(CTRL_G);
    assert_eq!(
        external.borrow()[2].0,
        if cfg!(windows) { "notepad" } else { "nano" }
    );
}

#[test]
fn focus_reaches_the_editor() {
    let _guard = registry_lock();
    let mut fixture = fixture(TextInputDialogOptions::default());

    assert!(!fixture.dialog.focused());
    fixture.dialog.set_focused(true);
    assert!(fixture.dialog.focused());

    // The editor draws its cursor marker only while it is focused.
    let focused = fixture.dialog.render(60).join("\n");
    fixture.dialog.set_focused(false);
    let blurred = fixture.dialog.render(60).join("\n");
    assert_ne!(
        focused, blurred,
        "the editor did not follow the dialog's focus"
    );
}
