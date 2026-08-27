//! What the side-question panel puts on screen.
//!
//! The frame is what carries the feature's claim: the exchange is visibly not
//! part of the transcript. So the box, the title and the key hint are asserted
//! rather than left to look right.

use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::side_question_panel::{
    SideQuestionPanel, SideQuestionPanelOptions,
};
use notagent::modes::interactive::theme::theme::{get_markdown_theme, init_theme};
use notagent_tui::tui::Component;

/// The global theme is a process global.
fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// The panel's lines with their colour sequences removed.
fn visible(lines: &[notagent_tui::tui::Line]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            let mut out = String::new();
            let mut characters = line.chars();
            while let Some(character) = characters.next() {
                if character != '\x1b' {
                    out.push(character);
                    continue;
                }
                for escaped in characters.by_ref() {
                    if escaped == 'm' {
                        break;
                    }
                }
            }
            out
        })
        .collect()
}

fn panel(rows: usize, scroll_keys: bool) -> SideQuestionPanel {
    SideQuestionPanel::new(SideQuestionPanelOptions {
        markdown_theme: get_markdown_theme(),
        can_use_scroll_keys: Rc::new(move || scroll_keys),
        terminal_rows: Rc::new(move || rows),
    })
}

#[test]
fn an_empty_panel_invites_a_question() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(40, true);
    let lines = visible(&panel.render(60));

    assert!(lines[0].starts_with('╭'), "{:?}", lines[0]);
    assert!(lines[0].contains("BTW"), "{:?}", lines[0]);
    assert!(
        lines[0].contains("Esc close"),
        "the way out is named: {:?}",
        lines[0]
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Ready for a side question")),
        "{lines:?}"
    );
    assert!(
        lines.last().is_some_and(|line| line.starts_with('╰')),
        "the box closes itself: {:?}",
        lines.last()
    );
    for line in &lines[1..lines.len() - 1] {
        assert!(line.starts_with('│') && line.ends_with('│'), "{line:?}");
    }
}

#[test]
fn a_question_and_its_answer_stand_in_the_box() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(40, true);
    assert!(panel.submit("what does this crate do?"));
    assert_eq!(
        panel.take_pending_prompt().as_deref(),
        Some("what does this crate do?"),
        "the caller is handed the question to send"
    );

    let lines = visible(&panel.render(70));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Q: what does this crate do?")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Waiting for answer")),
        "an unanswered question says so: {lines:?}"
    );

    panel.set_answer("It renders the terminal UI.");
    let lines = visible(&panel.render(70));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("It renders the terminal UI.")),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("Waiting for answer")),
        "and stops saying so once it is answered: {lines:?}"
    );
}

#[test]
fn reasoning_stands_in_only_until_there_is_an_answer() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(40, true);
    panel.submit("why?");
    panel.take_pending_prompt();
    panel.set_thinking("first thought\nsecond thought\nthird thought");

    let lines = visible(&panel.render(70));
    // Only the tail: what it is thinking now says more than where it started.
    assert!(
        lines.iter().any(|line| line.contains("third thought")),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("first thought")),
        "the head is dropped: {lines:?}"
    );

    panel.set_answer("Because of the cache.");
    let lines = visible(&panel.render(70));
    assert!(
        !lines.iter().any(|line| line.contains("thought")),
        "the reasoning gives way to the answer: {lines:?}"
    );
}

#[test]
fn a_failure_is_shown_rather_than_swallowed() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(40, true);
    panel.submit("anything?");
    panel.take_pending_prompt();
    panel.mark_failed("the provider gave up");

    let lines = visible(&panel.render(70));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("the provider gave up")),
        "{lines:?}"
    );
    assert!(!panel.is_running());
}

/// A child that could not be started at all has no question to attach the
/// reason to, so the reason gets an entry of its own.
#[test]
fn a_failure_with_nothing_in_flight_still_reaches_the_screen() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(40, true);
    panel.mark_failed("no model is selected");

    let lines = visible(&panel.render(70));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("no model is selected")),
        "{lines:?}"
    );
}

#[test]
fn a_second_question_is_refused_while_the_first_is_unanswered() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(40, true);
    assert!(panel.submit("first"));
    panel.take_pending_prompt();
    assert!(panel.is_running());
    assert!(!panel.submit("second"), "one question at a time");

    panel.mark_done(None);
    assert!(!panel.is_running());
    assert!(panel.submit("second"), "and the next one goes through");
}

#[test]
fn an_empty_question_is_not_a_question() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(40, true);
    assert!(!panel.submit("   "));
    assert!(panel.is_empty());
}

/// The answer stands in when the model finished without producing text.
#[test]
fn a_summary_stands_in_for_an_empty_answer() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(40, true);
    panel.submit("why?");
    panel.take_pending_prompt();
    panel.mark_done(Some("nothing came back"));

    let lines = visible(&panel.render(70));
    assert!(
        lines.iter().any(|line| line.contains("nothing came back")),
        "{lines:?}"
    );
}

#[test]
fn the_panel_takes_at_most_a_third_of_the_terminal() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(30, true);
    panel.submit("long one");
    panel.take_pending_prompt();
    panel.set_answer(&"a line of answer\n".repeat(40));

    let lines = panel.render(70);
    // A third of thirty rows, less the top border, plus the two frame lines.
    assert!(lines.len() <= 11, "{} lines is too tall", lines.len());
}

#[test]
fn a_taller_body_can_be_scrolled_and_says_so() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(30, true);
    panel.submit("long one");
    panel.take_pending_prompt();
    // Numbered, so a shifted view is distinguishable from an unshifted one.
    let answer: String = (0..40).map(|index| format!("line {index}\n\n")).collect();
    panel.set_answer(&answer);
    let lines = visible(&panel.render(70));
    assert!(
        lines[0].contains("↑↓ scroll"),
        "the hint appears only when there is something to scroll: {:?}",
        lines[0]
    );

    assert!(panel.scroll(true), "there is something to scroll");
    let scrolled = visible(&panel.render(70));
    assert_ne!(lines[1], scrolled[1], "the body moved");
}

/// The arrows belong to the caret while the user is typing.
#[test]
fn the_scroll_hint_stays_away_while_the_keys_are_not_the_panels() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(30, false);
    panel.submit("long one");
    panel.take_pending_prompt();
    panel.set_answer(&"a line of answer\n".repeat(40));

    let lines = visible(&panel.render(70));
    assert!(!lines[0].contains("scroll"), "{:?}", lines[0]);
    assert!(lines[0].contains("Esc close"), "{:?}", lines[0]);
}

#[test]
fn a_short_body_has_nothing_to_scroll() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(40, true);
    panel.submit("short one");
    panel.take_pending_prompt();
    panel.set_answer("done");
    panel.render(70);

    assert!(!panel.scroll(true), "nothing to scroll");
}

/// Once the panel has grown it stays that size, so the editor below it does
/// not jump up and down while an answer streams in line by line.
#[test]
fn the_panel_does_not_shrink_back_as_the_answer_arrives() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(60, true);
    panel.submit("growing");
    panel.take_pending_prompt();
    panel.set_answer("one\ntwo\nthree\nfour\nfive");
    let tall = panel.render(70).len();

    panel.set_answer("one");
    let after = panel.render(70).len();
    assert_eq!(after, tall, "the frame kept its height");
}

#[test]
fn a_transient_notice_goes_away_once_something_happens() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut panel = panel(40, true);
    panel.submit("first");
    panel.take_pending_prompt();
    panel.add_transient_notice("Wait for the answer before asking again.");
    let lines = visible(&panel.render(70));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Wait for the answer")),
        "{lines:?}"
    );

    panel.mark_done(None);
    let lines = visible(&panel.render(70));
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("Wait for the answer")),
        "the notice belonged to the moment: {lines:?}"
    );
}
