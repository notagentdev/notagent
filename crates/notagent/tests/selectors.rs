//! Behaviour of the batch-3 selectors that the TypeScript suites do not cover.
//! Expectations are taken from
//! `packages/coding-agent/src/modes/interactive/components/*-selector.ts`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::list_selector::{
    ListSelectorComponent, ListSelectorOptions,
};
use notagent::modes::interactive::components::show_images_selector::ShowImagesSelectorComponent;
use notagent::modes::interactive::components::theme_selector::ThemeSelectorComponent;
use notagent::modes::interactive::components::thinking_selector::ThinkingSelectorComponent;
use notagent::modes::interactive::components::user_message_selector::{
    UserMessageItem, UserMessageSelectorComponent,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_agent::ThinkingLevel;
use notagent_tui::tui::Component;

fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

const ESCAPE: &str = "\x1b";
const ENTER: &str = "\r";
const ARROW_DOWN: &str = "\x1b[B";
const ARROW_UP: &str = "\x1b[A";

// --- list selector (extension-selector.ts) ------------------------------------

#[test]
fn moves_the_cursor_and_confirms_the_selected_option() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let selected: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let cancelled = Rc::new(RefCell::new(0usize));

    let sink = Rc::clone(&selected);
    let cancel_sink = Rc::clone(&cancelled);
    let mut selector = ListSelectorComponent::new(
        "Pick one",
        vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
        Box::new(move |option| sink.borrow_mut().push(option)),
        Box::new(move || *cancel_sink.borrow_mut() += 1),
        None,
    );

    let rendered = strip_ansi(&selector.render(40).join("\n"));
    assert!(rendered.contains("Pick one"), "{rendered}");
    assert!(rendered.contains("→ alpha"), "{rendered}");
    assert!(rendered.contains("  beta"), "{rendered}");

    selector.handle_input(ARROW_DOWN);
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("→ beta"));
    // `j`/`k` move as well.
    selector.handle_input("j");
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("→ gamma"));
    // The cursor clamps at both ends.
    selector.handle_input("j");
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("→ gamma"));
    selector.handle_input(ARROW_UP);
    selector.handle_input("k");
    selector.handle_input("k");
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("→ alpha"));

    selector.handle_input(ENTER);
    assert_eq!(*selected.borrow(), vec!["alpha"]);

    selector.handle_input(ESCAPE);
    assert_eq!(*cancelled.borrow(), 1);
}

#[test]
fn counts_the_timeout_down_in_the_title_and_cancels_at_zero() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let cancelled = Rc::new(RefCell::new(0usize));
    let cancel_sink = Rc::clone(&cancelled);

    let mut selector = ListSelectorComponent::new(
        "Confirm",
        vec!["yes".to_string()],
        Box::new(|_| {}),
        Box::new(move || *cancel_sink.borrow_mut() += 1),
        Some(ListSelectorOptions {
            timeout: Some(1000),
            on_toggle_tools_expanded: None,
        }),
    );

    assert!(strip_ansi(&selector.render(40).join("\n")).contains("Confirm (1s)"));
    assert!(selector.countdown_deadline().is_some());

    std::thread::sleep(std::time::Duration::from_millis(1050));
    assert!(selector.tick_countdown());
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("Confirm (0s)"));
    assert_eq!(*cancelled.borrow(), 1);
    assert!(selector.countdown_deadline().is_none());
}

// --- thinking selector ---------------------------------------------------------

#[test]
fn preselects_the_current_thinking_level_and_reports_the_choice() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let chosen: Rc<RefCell<Vec<ThinkingLevel>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&chosen);

    let mut selector = ThinkingSelectorComponent::new(
        ThinkingLevel::Medium,
        vec![
            ThinkingLevel::Off,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ],
        Box::new(move |level| sink.borrow_mut().push(level)),
        Box::new(|| {}),
    );

    let rendered = strip_ansi(&selector.render(60).join("\n"));
    assert!(
        rendered.contains("Moderate reasoning (~8k tokens)"),
        "{rendered}"
    );
    assert_eq!(
        selector
            .get_select_list()
            .borrow()
            .get_selected_item()
            .map(|item| item.value),
        Some("medium".to_string())
    );

    selector.handle_input(ENTER);
    assert_eq!(*chosen.borrow(), vec![ThinkingLevel::Medium]);
}

// --- theme selector -------------------------------------------------------------

#[test]
fn marks_the_current_theme_and_previews_on_selection_change() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let previews: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let selected: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let preview_sink = Rc::clone(&previews);
    let select_sink = Rc::clone(&selected);

    let mut selector = ThemeSelectorComponent::new(
        "dark",
        Box::new(move |name| select_sink.borrow_mut().push(name)),
        Box::new(|| {}),
        Box::new(move |name| preview_sink.borrow_mut().push(name)),
    );

    let rendered = strip_ansi(&selector.render(60).join("\n"));
    assert!(rendered.contains("dark"), "{rendered}");
    assert!(rendered.contains("(current)"), "{rendered}");
    // The built-in themes sort as dark, light.
    assert_eq!(
        selector
            .get_select_list()
            .borrow()
            .get_selected_item()
            .map(|item| item.value),
        Some("dark".to_string())
    );

    selector.handle_input(ARROW_DOWN);
    assert_eq!(*previews.borrow(), vec!["light"]);
    selector.handle_input(ENTER);
    assert_eq!(*selected.borrow(), vec!["light"]);
}

// --- show images selector --------------------------------------------------------

#[test]
fn maps_the_show_images_answer_back_to_a_boolean() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let answers: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&answers);

    let mut selector = ShowImagesSelectorComponent::new(
        false,
        Box::new(move |show| sink.borrow_mut().push(show)),
        Box::new(|| {}),
    );
    // `false` preselects "No".
    assert_eq!(
        selector
            .get_select_list()
            .borrow()
            .get_selected_item()
            .map(|item| item.value),
        Some("no".to_string())
    );
    selector.handle_input(ENTER);
    assert_eq!(*answers.borrow(), vec![false]);

    selector.handle_input(ARROW_UP);
    selector.handle_input(ENTER);
    assert_eq!(*answers.borrow(), vec![false, true]);
}

// --- user message selector --------------------------------------------------------

fn message(id: &str, text: &str) -> UserMessageItem {
    UserMessageItem {
        id: id.to_string(),
        text: text.to_string(),
        timestamp: None,
    }
}

#[test]
fn starts_at_the_newest_message_and_wraps_around() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let selected: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&selected);

    let mut selector = UserMessageSelectorComponent::new(
        vec![
            message("a", "first"),
            message("b", "second"),
            message("c", "third"),
        ],
        Box::new(move |id| sink.borrow_mut().push(id.to_string())),
        Box::new(|| {}),
        None,
    );

    let rendered = strip_ansi(&selector.render(40).join("\n"));
    assert!(rendered.contains("Fork from Message"), "{rendered}");
    assert!(rendered.contains("› third"), "{rendered}");
    assert!(rendered.contains("Message 3 of 3"), "{rendered}");

    // Down at the end wraps to the top.
    selector.handle_input(ARROW_DOWN);
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("› first"));
    // Up at the top wraps to the end.
    selector.handle_input(ARROW_UP);
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("› third"));

    selector.handle_input(ENTER);
    assert_eq!(*selected.borrow(), vec!["c"]);
}

#[test]
fn starts_at_the_requested_message_and_normalises_newlines() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut selector = UserMessageSelectorComponent::new(
        vec![message("a", "one\ntwo"), message("b", "other")],
        Box::new(|_| {}),
        Box::new(|| {}),
        Some("a"),
    );

    let rendered = strip_ansi(&selector.render(40).join("\n"));
    assert!(rendered.contains("› one two"), "{rendered}");
    assert!(!selector.is_empty());
}

#[test]
fn reports_an_empty_message_list() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut selector =
        UserMessageSelectorComponent::new(Vec::new(), Box::new(|_| {}), Box::new(|| {}), None);
    assert!(selector.is_empty());
    assert!(
        strip_ansi(&selector.render(40).join("\n")).contains("No user messages found"),
        "{:?}",
        selector.render(40)
    );
}
