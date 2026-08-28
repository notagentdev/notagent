use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::markdown_transform::{
    MarkdownMessageType, MarkdownTransformContext, MarkdownTransformer,
};
use notagent::modes::interactive::components::user_message::UserMessageComponent;
use notagent::modes::interactive::theme::theme::{BlockStyle, init_theme, set_block_style};
use notagent::utils::ansi::strip_ansi;
use notagent_tui::tui::Component;

/// cases pin the standard layout, the badge cases set Badge themselves.
fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    set_block_style(BlockStyle::Standard);
    guard
}

const OSC133_ZONE_START: &str = "\x1b]133;A\x07";
const OSC133_ZONE_END: &str = "\x1b]133;B\x07";
const OSC133_ZONE_FINAL: &str = "\x1b]133;C\x07";
const BG_RESET: &str = "\x1b[49m";

#[test]
fn keeps_user_message_height_stable_while_moving_closing_osc_markers_off_line_end() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = UserMessageComponent::new("hello", None, None, Vec::new());
    let lines = component.render(20);

    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains(OSC133_ZONE_START), "{:?}", lines[0]);
    assert!(lines[0].ends_with(BG_RESET), "{:?}", lines[0]);
    assert!(!lines[0].contains(OSC133_ZONE_END), "{:?}", lines[0]);
    assert!(lines[1].contains("hello"), "{:?}", lines[1]);
    assert!(
        lines[2].starts_with(&format!("{OSC133_ZONE_END}{OSC133_ZONE_FINAL}")),
        "{:?}",
        lines[2]
    );
    assert!(lines[2].ends_with(BG_RESET), "{:?}", lines[2]);
}

#[test]
fn chains_markdown_transformers_with_user_message_context() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let calls: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let contexts: Rc<RefCell<Vec<MarkdownTransformContext>>> = Rc::new(RefCell::new(Vec::new()));
    let formula_calls = Rc::clone(&calls);
    let formula_contexts = Rc::clone(&contexts);
    let suffix_calls = Rc::clone(&calls);
    let transformers: Vec<MarkdownTransformer> = vec![
        Rc::new(move |markdown: &str, context: &MarkdownTransformContext| {
            formula_calls.borrow_mut().push("formula".to_string());
            formula_contexts.borrow_mut().push(context.clone());
            markdown.replace("$x^2$", "x²")
        }),
        Rc::new(move |markdown: &str, _context: &MarkdownTransformContext| {
            suffix_calls.borrow_mut().push("suffix".to_string());
            format!("{markdown} Done.")
        }),
    ];

    let mut component =
        UserMessageComponent::new("The input is $x^2$.", None, Some(1), transformers);

    assert!(
        strip_ansi(&component.render(80).join("\n")).contains("The input is x². Done."),
        "{:?}",
        strip_ansi(&component.render(80).join("\n"))
    );
    assert_eq!(*calls.borrow(), ["formula", "suffix"]);
    assert_eq!(contexts.borrow().len(), 1);
    assert_eq!(
        contexts.borrow()[0],
        MarkdownTransformContext {
            message_type: MarkdownMessageType::User,
            is_streaming: false,
            available_width: 78,
        }
    );
}

#[test]
fn reapplies_markdown_transformers_when_invalidated() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let suffix: Rc<RefCell<String>> = Rc::new(RefCell::new("before".to_string()));
    let transformer_suffix = Rc::clone(&suffix);
    let transformers: Vec<MarkdownTransformer> = vec![Rc::new(
        move |markdown: &str, _context: &MarkdownTransformContext| {
            format!("{markdown} {}", transformer_suffix.borrow())
        },
    )];
    let mut component = UserMessageComponent::new("Message", None, Some(1), transformers);

    assert!(
        strip_ansi(&component.render(80).join("\n")).contains("Message before"),
        "{:?}",
        strip_ansi(&component.render(80).join("\n"))
    );

    *suffix.borrow_mut() = "after".to_string();
    component.invalidate();

    assert!(
        strip_ansi(&component.render(80).join("\n")).contains("Message after"),
        "{:?}",
        strip_ansi(&component.render(80).join("\n"))
    );
}

#[test]
fn badge_style_is_the_washed_text_without_padding_rows() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    let mut component = UserMessageComponent::new("hello", None, None, Vec::new());
    let lines = component.render(20);

    // Exactly the message line — no box padding rows — carrying the wash
    // and the whole OSC zone in order: start, end, final, then the text.
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with(&format!(
            "{OSC133_ZONE_START}{OSC133_ZONE_END}{OSC133_ZONE_FINAL}"
        )),
        "{:?}",
        lines[0]
    );
    assert!(lines[0].contains("hello"), "{:?}", lines[0]);
    assert!(lines[0].ends_with(BG_RESET), "{:?}", lines[0]);
}

#[test]
fn switching_the_style_rebuilds_the_message_layout() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = UserMessageComponent::new("hello", None, None, Vec::new());
    let standard_rows = component.render(20).len();

    set_block_style(BlockStyle::Badge);
    let badge_rows = component.render(20).len();

    assert_eq!(standard_rows, 3);
    assert_eq!(badge_rows, 1);
    set_block_style(BlockStyle::Standard);
}
