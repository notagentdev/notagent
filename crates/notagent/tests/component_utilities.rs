//! Behaviour of the batch-1 interactive components that the TypeScript suites do
//! not cover. Expectations are taken from
//! `packages/coding-agent/src/modes/interactive/components/*.ts`.

use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::bordered_loader::BorderedLoader;
use notagent::modes::interactive::components::countdown_timer::CountdownTimer;
use notagent::modes::interactive::components::dynamic_border::DynamicBorder;
use notagent::modes::interactive::components::keybinding_hints::{
    KeyTextFormatOptions, format_key_text, key_display_text, key_hint, key_text, raw_key_hint,
};
use notagent::modes::interactive::components::markdown_transform::{
    MarkdownMessageType, MarkdownTransformContext, MarkdownTransformer, create_markdown_transform,
};
use notagent::modes::interactive::components::visual_truncate::truncate_to_visual_lines;
use notagent::modes::interactive::theme::theme::{
    ThemeColor, get_theme_by_name, init_theme, theme,
};
use notagent_tui::tui::{Component, Line};

fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

// --- visual-truncate -----------------------------------------------------------

#[test]
fn returns_nothing_for_empty_text() {
    let result = truncate_to_visual_lines("", 5, 20, 0);
    assert!(result.visual_lines.is_empty());
    assert_eq!(result.skipped_count, 0);
}

#[test]
fn keeps_every_line_below_the_limit() {
    // `Text` pads every line to the render width, as in TypeScript.
    let result = truncate_to_visual_lines("a\nb\nc", 5, 20, 0);
    assert_eq!(
        result.visual_lines,
        vec![
            Line::from("a".to_string() + &" ".repeat(19)),
            Line::from("b".to_string() + &" ".repeat(19)),
            Line::from("c".to_string() + &" ".repeat(19))
        ]
    );
    assert_eq!(result.skipped_count, 0);
}

#[test]
fn keeps_the_last_lines_and_counts_the_skipped_ones() {
    let result = truncate_to_visual_lines("a\nb\nc\nd", 2, 20, 0);
    assert_eq!(
        result.visual_lines,
        vec![
            Line::from("c".to_string() + &" ".repeat(19)),
            Line::from("d".to_string() + &" ".repeat(19))
        ]
    );
    assert_eq!(result.skipped_count, 2);
}

#[test]
fn counts_wrapped_lines_as_visual_lines() {
    // One source line wraps into three visual lines at width 10; the limit of
    // two keeps the last two.
    let result = truncate_to_visual_lines("aaaa bbbb cccc dddd eeee", 2, 10, 0);
    assert_eq!(
        result.visual_lines,
        vec![Line::from("cccc dddd "), Line::from("eeee      ")]
    );
    assert_eq!(result.skipped_count, 1);
}

#[test]
fn applies_the_horizontal_padding() {
    let padded = truncate_to_visual_lines("ab", 5, 10, 1);
    assert_eq!(padded.visual_lines, vec![Line::from(" ab       ")]);
    let unpadded = truncate_to_visual_lines("ab", 5, 10, 0);
    assert_eq!(unpadded.visual_lines, vec![Line::from("ab        ")]);
}

#[test]
fn keeps_every_line_for_a_zero_limit() {
    // `slice(-0)` is `slice(0)` in JavaScript (bug-compat).
    let result = truncate_to_visual_lines("a\nb", 0, 20, 0);
    assert_eq!(
        result.visual_lines,
        vec![
            Line::from("a".to_string() + &" ".repeat(19)),
            Line::from("b".to_string() + &" ".repeat(19))
        ]
    );
    assert_eq!(result.skipped_count, 2);
}

// --- dynamic-border ------------------------------------------------------------

#[test]
fn draws_a_rule_across_the_width() {
    let mut border = DynamicBorder::new(Some(Rc::new(|text: &str| format!("<{text}>"))));
    assert_eq!(border.render(4), vec![Line::from("<────>")]);
    // Never narrower than one cell.
    assert_eq!(border.render(0), vec![Line::from("<─>")]);
}

#[test]
fn uses_the_muted_border_color_by_default() {
    // Deviation from the TS original (user decision 2026-08-17, v0.1.11):
    // the rules of a dialog are furniture, so they take the same muted grey
    // the input frame draws itself in instead of the accent blue.
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut border = DynamicBorder::new(None);
    assert_eq!(
        border.render(3),
        vec![Line::from(theme().fg(ThemeColor::BorderMuted, "───"))]
    );
}

// --- keybinding-hints ----------------------------------------------------------

#[test]
fn formats_key_combinations_and_alternatives() {
    let plain = KeyTextFormatOptions::default();
    assert_eq!(format_key_text("ctrl+c", plain), "ctrl+c");
    assert_eq!(format_key_text("escape/ctrl+c", plain), "escape/ctrl+c");
    assert_eq!(
        format_key_text("ctrl+shift+p", KeyTextFormatOptions { capitalize: true }),
        "Ctrl+Shift+P"
    );
    assert_eq!(format_key_text("", plain), "");
}

#[cfg(target_os = "macos")]
#[test]
fn renames_alt_to_option_on_macos() {
    let plain = KeyTextFormatOptions::default();
    assert_eq!(format_key_text("alt+enter", plain), "option+enter");
    assert_eq!(format_key_text("Alt+enter", plain), "option+enter");
    assert_eq!(
        format_key_text("alt+enter", KeyTextFormatOptions { capitalize: true }),
        "Option+Enter"
    );
}

#[test]
fn resolves_keys_through_the_global_keybindings() {
    assert_eq!(key_text("tui.select.cancel"), "escape/ctrl+c");
    assert_eq!(key_display_text("tui.select.cancel"), "Escape/Ctrl+C");
    // An unregistered keybinding has no keys.
    assert_eq!(key_text("app.interrupt"), "");
}

#[test]
fn themes_the_hint_text() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let theme = theme();
    assert_eq!(
        key_hint("tui.select.cancel", "cancel"),
        theme.fg(ThemeColor::Dim, "escape/ctrl+c") + &theme.fg(ThemeColor::Muted, " cancel")
    );
    assert_eq!(
        raw_key_hint("ctrl+g", "editor"),
        theme.fg(ThemeColor::Dim, "ctrl+g") + &theme.fg(ThemeColor::Muted, " editor")
    );
}

// --- countdown-timer -----------------------------------------------------------

#[test]
fn rounds_the_initial_seconds_up() {
    assert_eq!(CountdownTimer::new(2400).remaining_seconds(), 3);
    assert_eq!(CountdownTimer::new(2000).remaining_seconds(), 2);
    assert_eq!(CountdownTimer::new(1).remaining_seconds(), 1);
    assert_eq!(CountdownTimer::new(0).remaining_seconds(), 0);
}

#[test]
fn ticks_once_per_second_and_expires_at_zero() {
    let mut timer = CountdownTimer::new(1000);
    assert_eq!(timer.remaining_seconds(), 1);
    assert!(timer.tick().is_none(), "no tick before the deadline");

    std::thread::sleep(std::time::Duration::from_millis(1050));
    let tick = timer.tick().expect("tick is due");
    assert_eq!(tick.remaining_seconds, 0);
    assert!(tick.expired);
    // Expiry disposes the timer, so nothing fires afterwards.
    assert!(timer.deadline().is_none());
    assert!(timer.tick().is_none());
}

#[test]
fn stops_ticking_after_dispose() {
    let mut timer = CountdownTimer::new(5000);
    timer.dispose();
    std::thread::sleep(std::time::Duration::from_millis(1050));
    assert!(timer.tick().is_none());
}

// --- markdown-transform --------------------------------------------------------

#[test]
fn chains_the_transformers_in_order() {
    let upper: MarkdownTransformer = Rc::new(|markdown: &str, _| markdown.to_uppercase());
    let suffix: MarkdownTransformer = Rc::new(|markdown: &str, _| format!("{markdown}!"));
    let transform =
        create_markdown_transform(MarkdownMessageType::Assistant, false, vec![upper, suffix]);
    assert_eq!(transform("hi", 40), "HI!");
}

#[test]
fn hands_the_context_to_every_transformer() {
    let seen: Rc<std::cell::RefCell<Vec<MarkdownTransformContext>>> =
        Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    let recorder: MarkdownTransformer = Rc::new(move |markdown: &str, context| {
        sink.borrow_mut().push(context.clone());
        markdown.to_string()
    });
    let transform =
        create_markdown_transform(MarkdownMessageType::AssistantThinking, true, vec![recorder]);
    transform("x", 42);

    assert_eq!(
        seen.borrow().as_slice(),
        [MarkdownTransformContext {
            message_type: MarkdownMessageType::AssistantThinking,
            is_streaming: true,
            available_width: 42,
        }]
    );
}

#[test]
fn returns_the_markdown_unchanged_without_transformers() {
    let transform = create_markdown_transform(MarkdownMessageType::User, false, Vec::new());
    assert_eq!(transform("unchanged", 40), "unchanged");
}

// --- bordered-loader -----------------------------------------------------------

#[test]
fn frames_the_loader_and_shows_the_cancel_hint() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let theme_instance = get_theme_by_name("dark").expect("dark theme");

    let mut loader = BorderedLoader::new(&theme_instance, "Working", None);
    let lines = loader.render(40);
    // border, loader (blank + spinner line), spacer, hint, spacer, border
    assert_eq!(lines.len(), 7);
    assert!(lines[0].contains("────"), "{:?}", lines[0]);
    assert_eq!(lines[1].as_ref(), "");
    assert!(lines[2].contains("Working"), "{:?}", lines[2]);
    assert_eq!(lines[3].as_ref(), "");
    assert!(lines[4].contains("escape/ctrl+c"), "{:?}", lines[4]);
    assert!(lines[4].contains("cancel"), "{:?}", lines[4]);
    assert_eq!(lines[5].as_ref(), "");
    assert!(lines[6].contains("────"), "{:?}", lines[6]);

    assert!(!loader.signal().is_cancelled());
    loader.handle_input("\x1b");
    assert!(loader.signal().is_cancelled());
}

#[test]
fn omits_the_cancel_hint_when_not_cancellable() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let theme_instance = get_theme_by_name("dark").expect("dark theme");

    let mut loader = BorderedLoader::new(&theme_instance, "Working", Some(false));
    let lines = loader.render(40);
    // border, loader (blank + spinner line), spacer, border
    assert_eq!(lines.len(), 5);
    assert!(!lines.iter().any(|line| line.contains("cancel")));
    loader.handle_input("\x1b");
    assert!(!loader.signal().is_cancelled());
}
