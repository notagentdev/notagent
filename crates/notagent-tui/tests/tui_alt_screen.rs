//! Port of `packages/tui/test/tui-alt-screen.test.ts` (1267 LOC).
//!
//! Covers the renderer core, wheel routing, mouse selection, the scrollbar
//! drag and the transcript search. `RecordingTerminal` of the TS suite is
//! `VirtualTerminal`, which records its events itself.

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::h_stack::HStack;
use notagent_tui::components::scroll_view::{
    ScrollView, ScrollViewOptions, ScrollViewScrollbar, ScrollViewState,
};
use notagent_tui::components::stack::{StackEntryOptions, StackOptions};
use notagent_tui::components::text::Text;
use notagent_tui::components::v_stack::VStack;
use notagent_tui::layout_node::StackBasis;
use notagent_tui::terminal_image::hyperlink;
use notagent_tui::test_terminal::TerminalEvent;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, ComponentRef, Focusable, TuiStopOptions, component_ref};
use notagent_tui::tui_alt_screen::{TuiAltScreen, TuiAltScreenOptions};

fn numbered_text(count: usize) -> String {
    (1..=count)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Number of recorded write events.
fn write_count(terminal: &VirtualTerminal) -> usize {
    terminal
        .events()
        .iter()
        .filter(|event| matches!(event, TerminalEvent::Write(_)))
        .count()
}

/// Number of recorded OSC 52 clipboard writes.
fn clipboard_write_count(terminal: &VirtualTerminal) -> usize {
    terminal
        .events()
        .iter()
        .filter(|event| match event {
            TerminalEvent::Write(data) => data.contains("\x1b]52;c;"),
            _ => false,
        })
        .count()
}

/// Concatenated writes recorded after `event_index`.
fn writes_since(terminal: &VirtualTerminal, event_index: usize) -> String {
    terminal
        .events()
        .iter()
        .skip(event_index)
        .filter_map(|event| match event {
            TerminalEvent::Write(data) => Some(data.as_str()),
            _ => None,
        })
        .collect()
}

/// `const editor = { focused, render, handleInput }` of the TS suite.
#[derive(Clone, Default)]
struct EditorProbe {
    focused: Rc<RefCell<bool>>,
    inputs: Rc<RefCell<Vec<String>>>,
}

struct Editor {
    probe: EditorProbe,
}

impl Component for Editor {
    fn render(&mut self, _width: usize) -> Vec<String> {
        vec!["editor".to_string()]
    }

    fn handle_input(&mut self, data: &str) {
        self.probe.inputs.borrow_mut().push(data.to_string());
    }

    fn invalidate(&mut self) {}

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for Editor {
    fn focused(&self) -> bool {
        *self.probe.focused.borrow()
    }

    fn set_focused(&mut self, focused: bool) {
        *self.probe.focused.borrow_mut() = focused;
    }
}

fn editor() -> (EditorProbe, ComponentRef) {
    let probe = EditorProbe::default();
    let component = component_ref(Editor {
        probe: probe.clone(),
    });
    (probe, component)
}

fn trimmed(terminal: &VirtualTerminal) -> Vec<String> {
    terminal
        .get_viewport()
        .iter()
        .map(|line| line.trim_end().to_string())
        .collect()
}

#[tokio::test]
async fn renders_a_terminal_height_viewport_and_preserves_manual_scroll_position() {
    let terminal = VirtualTerminal::new(20, 4);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let text = Rc::new(RefCell::new(Text::new(numbered_text(10), 0, 0)));
    tui.core().add_child(text.clone() as ComponentRef);
    tui.start();
    tui.wait_for_render().await;

    assert_eq!(
        trimmed(&terminal),
        ["line 7", "line 8", "line 9", "line 10"]
    );
    assert!(tui.is_following_output());

    tui.handle_terminal_input("\x1b[<64;1;1M");
    tui.wait_for_render().await;
    assert_eq!(trimmed(&terminal), ["line 6", "line 7", "line 8", "line 9"]);
    assert_eq!(tui.viewport_top(), 5);
    assert!(!tui.is_following_output());

    text.borrow_mut().set_text(numbered_text(12));
    tui.request_render(false);
    tui.wait_for_render().await;
    assert_eq!(trimmed(&terminal), ["line 6", "line 7", "line 8", "line 9"]);

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn keeps_an_explicit_dock_fixed_while_the_transcript_scrolls() {
    let terminal = VirtualTerminal::new(20, 6);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());

    let transcript_text = Rc::new(RefCell::new(Text::new(numbered_text(8), 0, 0)));
    let transcript = ScrollView::new(
        transcript_text.clone() as ComponentRef,
        ScrollViewOptions {
            follow_end: true,
            primary: true,
            ..ScrollViewOptions::default()
        },
    );
    let transcript_state = transcript.state();

    let mut dock = VStack::new(StackOptions::default());
    dock.add_child(component_ref(Text::new("editor", 0, 0)));
    dock.add_child(component_ref(Text::new("footer", 0, 0)));

    let mut root = VStack::new(StackOptions::default());
    root.add_child_with(
        component_ref(transcript),
        StackEntryOptions {
            basis: Some(StackBasis::Size(0)),
            grow: Some(1),
            min_size: Some(1),
            ..StackEntryOptions::default()
        },
    );
    root.add_child_with(
        component_ref(dock),
        StackEntryOptions {
            basis: Some(StackBasis::Auto),
            min_size: Some(1),
            ..StackEntryOptions::default()
        },
    );
    tui.set_layout_root(Some(component_ref(root)));
    tui.start();
    tui.wait_for_render().await;

    assert_eq!(
        trimmed(&terminal),
        ["line 5", "line 6", "line 7", "line 8", "editor", "footer"]
    );

    // A wheel event over the dock falls back to the primary transcript view.
    tui.handle_terminal_input("\x1b[<64;1;6M");
    tui.wait_for_render().await;
    assert_eq!(
        trimmed(&terminal),
        ["line 4", "line 5", "line 6", "line 7", "editor", "footer"]
    );
    assert!(!transcript_state.borrow().is_following_end());

    transcript_text.borrow_mut().set_text(numbered_text(10));
    tui.request_render(false);
    tui.wait_for_render().await;
    assert_eq!(
        trimmed(&terminal),
        ["line 4", "line 5", "line 6", "line 7", "editor", "footer"]
    );

    tui.scroll_to_bottom();
    tui.wait_for_render().await;
    assert_eq!(
        trimmed(&terminal),
        ["line 7", "line 8", "line 9", "line 10", "editor", "footer"]
    );

    tui.stop(TuiStopOptions::default());
}

// Transcript search corpus and match mapping.

use notagent_tui::alt_screen_search::{
    find_alt_screen_search_matches, get_alt_screen_search_match_key,
};

#[test]
fn finds_matches_with_row_and_column_mapping() {
    let lines = vec![
        "hello world".to_string(),
        "second line".to_string(),
        "  spaced  text  ".to_string(),
    ];

    let matches = find_alt_screen_search_matches(&lines, "world");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].segments.len(), 1);
    assert_eq!(matches[0].segments[0].row, 0);
    assert_eq!(matches[0].segments[0].start_col, 6);
    assert_eq!(matches[0].segments[0].end_col, 11);
}

#[test]
fn matches_are_case_insensitive_and_span_rows() {
    let lines = vec!["hello".to_string(), "world".to_string()];
    // Rows are joined with a separating space in the corpus.
    let matches = find_alt_screen_search_matches(&lines, "HELLO WORLD");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].segments.len(), 2);
    assert_eq!(matches[0].segments[0].row, 0);
    assert_eq!(matches[0].segments[1].row, 1);
}

#[test]
fn whitespace_runs_in_the_query_are_normalized() {
    let lines = vec!["spaced    text".to_string()];
    let matches = find_alt_screen_search_matches(&lines, "  spaced   text  ");
    assert_eq!(matches.len(), 1);
}

#[test]
fn an_empty_query_matches_nothing() {
    let lines = vec!["content".to_string()];
    assert!(find_alt_screen_search_matches(&lines, "   ").is_empty());
}

#[test]
fn match_keys_identify_position_and_extent() {
    let lines = vec!["alpha beta".to_string()];
    let matches = find_alt_screen_search_matches(&lines, "beta");
    assert_eq!(get_alt_screen_search_match_key(&matches[0]), "0:6:0:10");
}

#[test]
fn ansi_sequences_do_not_shift_match_columns() {
    let lines = vec!["\x1b[31mred\x1b[0m target".to_string()];
    let matches = find_alt_screen_search_matches(&lines, "target");
    assert_eq!(matches[0].segments[0].start_col, 4);
    assert_eq!(matches[0].segments[0].end_col, 10);
}

// Mouse text selection.

#[tokio::test]
async fn snaps_mouse_selection_to_wide_grapheme_boundaries() {
    let terminal = VirtualTerminal::new(20, 2);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core()
        .add_child(component_ref(Text::new("A界🙂éZ", 0, 0)));
    tui.start();
    tui.wait_for_render().await;

    let expected = format!("\x1b]52;c;{}\x07", base64("界🙂"));

    // Press inside 界, drag onto 🙂, release: both wide graphemes are selected.
    tui.handle_terminal_input("\x1b[<0;3;1M");
    tui.handle_terminal_input("\x1b[<32;4;1M");
    tui.handle_terminal_input("\x1b[<0;4;1m");
    tui.wait_for_render().await;
    assert_eq!(
        terminal.get_writes().matches(expected.as_str()).count(),
        1,
        "selection snapped to the grapheme boundaries"
    );

    // The same selection in the opposite direction.
    tui.handle_terminal_input("\x1b[<0;5;1M");
    tui.handle_terminal_input("\x1b[<32;2;1M");
    tui.handle_terminal_input("\x1b[<0;2;1m");
    tui.wait_for_render().await;
    assert_eq!(terminal.get_writes().matches(expected.as_str()).count(), 2);

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn double_click_selects_a_word_and_triple_click_the_line() {
    let terminal = VirtualTerminal::new(30, 2);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core()
        .add_child(component_ref(Text::new("alpha beta gamma", 0, 0)));
    tui.start();
    tui.wait_for_render().await;

    // Two presses on the same word select it.
    tui.handle_terminal_input("\x1b[<0;8;1M");
    tui.handle_terminal_input("\x1b[<0;8;1m");
    tui.handle_terminal_input("\x1b[<0;8;1M");
    tui.handle_terminal_input("\x1b[<0;8;1m");
    tui.wait_for_render().await;
    assert!(
        terminal
            .get_writes()
            .contains(&format!("\x1b]52;c;{}\x07", base64("beta"))),
        "double click copies the word"
    );

    // A third press extends the selection to the whole line.
    tui.handle_terminal_input("\x1b[<0;8;1M");
    tui.handle_terminal_input("\x1b[<0;8;1m");
    tui.wait_for_render().await;
    assert!(
        terminal
            .get_writes()
            .contains(&format!("\x1b]52;c;{}\x07", base64("alpha beta gamma"))),
        "triple click copies the line"
    );

    tui.stop(TuiStopOptions::default());
}

/// Base64 of the expected clipboard payload.
fn base64(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

#[tokio::test]
async fn opens_an_osc8_hyperlink_on_click_but_not_on_drag() {
    let terminal = VirtualTerminal::new(20, 3);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let url = "https://example.com/path?q=1";
    let bel_url = "https://example.com/bel";
    let emoji_url = "https://example.com/emoji";
    tui.core().add_child(component_ref(Text::new(
        format!(
            "{}\n\x1b]8;;{bel_url}\x07link\x1b]8;;\x07\n{}",
            hyperlink("link", url),
            hyperlink("🙂", emoji_url)
        ),
        0,
        0,
    )));
    tui.start();
    tui.wait_for_render().await;

    let mut opened_urls: Vec<String> = Vec::new();
    for (x, y, expected) in [(2, 1, url), (2, 2, bel_url), (2, 3, emoji_url)] {
        tui.handle_terminal_input(&format!("\x1b[<0;{x};{y}M"));
        tui.handle_terminal_input(&format!("\x1b[<0;{x};{y}m"));
        tui.wait_for_render().await;
        opened_urls.extend(tui.take_clicked_url());
        assert_eq!(opened_urls.last().map(String::as_str), Some(expected));
    }
    assert_eq!(opened_urls, vec![url, bel_url, emoji_url]);

    // A drag does not activate the link under the press.
    tui.handle_terminal_input("\x1b[<0;2;1M");
    tui.handle_terminal_input("\x1b[<32;4;1M");
    tui.handle_terminal_input("\x1b[<0;4;1m");
    tui.wait_for_render().await;
    assert_eq!(tui.take_clicked_url(), None);

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn selects_visible_text_with_the_mouse_and_copies_it_with_osc52() {
    let terminal = VirtualTerminal::new(20, 4);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core().add_child(component_ref(Text::new(
        "\x1b[1mal\x1b[0mpha\nbeta\ngamma\ndelta",
        0,
        0,
    )));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1b[<0;1;1M");
    tui.handle_terminal_input("\x1b[<32;4;2M");
    tui.handle_terminal_input("\x1b[<0;4;2m");
    tui.wait_for_render().await;

    let writes = terminal.get_writes();
    assert!(
        writes.contains(&format!("\x1b]52;c;{}\x07", base64("alpha\nbeta"))),
        "clipboard writes: {:?}",
        writes
    );
    assert!(writes.contains("\x1b[7m"));
    assert!(
        writes.contains("al\x1b[0m\x1b[7mpha"),
        "selection inverse must be reapplied after a reset inside the selection"
    );
    assert!(
        terminal
            .get_viewport()
            .iter()
            .any(|line| line.contains("Copied!"))
    );

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn does_not_append_whitespace_to_double_click_word_highlighting() {
    let terminal = VirtualTerminal::new(20, 1);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core()
        .add_child(component_ref(Text::new("foo  bar", 0, 0)));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1b[<0;1;1M");
    tui.handle_terminal_input("\x1b[<0;1;1m");
    tui.handle_terminal_input("\x1b[<0;3;1M");
    tui.wait_for_render().await;

    assert!(terminal.get_writes().contains("foo\x1b[27m"));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn highlights_a_complete_whitespace_segment_during_a_word_drag() {
    let terminal = VirtualTerminal::new(20, 1);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core()
        .add_child(component_ref(Text::new("foo  bar", 0, 0)));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1b[<0;1;1M");
    tui.handle_terminal_input("\x1b[<0;1;1m");
    tui.handle_terminal_input("\x1b[<0;2;1M");
    tui.handle_terminal_input("\x1b[<32;4;1M");
    tui.wait_for_render().await;

    assert!(terminal.get_writes().contains("foo  \x1b[27m"));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn selects_words_on_double_click_extends_word_drags_and_lines_on_triple_click() {
    let terminal = VirtualTerminal::new(20, 2);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core().add_child(component_ref(Text::new(
        "zero alpha beta\ngamma delta",
        0,
        0,
    )));
    tui.start();
    tui.wait_for_render().await;

    // The second click lands on a different character in alpha.
    tui.handle_terminal_input("\x1b[<0;6;1M");
    tui.handle_terminal_input("\x1b[<0;6;1m");
    tui.handle_terminal_input("\x1b[<0;10;1M");
    tui.handle_terminal_input("\x1b[<0;10;1m");
    tui.wait_for_render().await;
    assert!(
        terminal
            .get_writes()
            .contains(&format!("\x1b]52;c;{}\x07", base64("alpha")))
    );

    // A double-click drag includes each word touched, rather than partial words.
    tui.handle_terminal_input("\x1b[<0;12;1M");
    tui.handle_terminal_input("\x1b[<0;12;1m");
    tui.handle_terminal_input("\x1b[<0;14;1M");
    tui.handle_terminal_input("\x1b[<32;3;2M");
    tui.handle_terminal_input("\x1b[<0;3;2m");
    tui.wait_for_render().await;
    assert!(
        terminal
            .get_writes()
            .contains(&format!("\x1b]52;c;{}\x07", base64("beta\ngamma")))
    );

    tui.handle_terminal_input("\x1b[<0;7;2M");
    tui.handle_terminal_input("\x1b[<0;7;2m");
    tui.handle_terminal_input("\x1b[<0;9;2M");
    tui.handle_terminal_input("\x1b[<0;9;2m");
    tui.handle_terminal_input("\x1b[<0;11;2M");
    tui.handle_terminal_input("\x1b[<0;11;2m");
    tui.wait_for_render().await;
    assert!(
        terminal
            .get_writes()
            .contains(&format!("\x1b]52;c;{}\x07", base64("gamma delta")))
    );

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn does_not_repaint_idle_or_zero_width_selections_on_focus_loss() {
    let terminal = VirtualTerminal::new(20, 4);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core()
        .add_child(component_ref(Text::new("alpha\nbeta\ngamma\ndelta", 0, 0)));
    tui.start();
    tui.wait_for_render().await;

    let idle_write_count = write_count(&terminal);
    tui.handle_terminal_input("\x1b[O");
    tui.handle_terminal_input("\x1b[I");
    tui.wait_for_render().await;
    assert_eq!(write_count(&terminal), idle_write_count);

    // A completed click leaves a zero-width anchor; later orphaned drag and
    // release events must not extend it.
    tui.handle_terminal_input("\x1b[<0;1;1M");
    tui.handle_terminal_input("\x1b[<0;1;1m");
    tui.handle_terminal_input("\x1b[<32;4;2M");
    tui.handle_terminal_input("\x1b[<0;4;2m");
    tui.wait_for_render().await;
    assert_eq!(clipboard_write_count(&terminal), 0);

    // Losing focus after a press without a drag cancels the press without repainting.
    tui.handle_terminal_input("\x1b[<0;1;3M");
    tui.wait_for_render().await;
    let pressed_write_count = write_count(&terminal);
    tui.handle_terminal_input("\x1b[O");
    tui.handle_terminal_input("\x1b[I");
    tui.wait_for_render().await;
    assert_eq!(write_count(&terminal), pressed_write_count);
    tui.handle_terminal_input("\x1b[<32;4;2M");
    tui.handle_terminal_input("\x1b[<0;4;2m");
    tui.wait_for_render().await;
    assert_eq!(clipboard_write_count(&terminal), 0);
    assert!(terminal.get_writes().contains("\x1b[?1004h"));

    tui.stop(TuiStopOptions::default());
    assert!(terminal.get_writes().contains("\x1b[?1004l"));
}

#[tokio::test]
async fn clears_an_active_visible_selection_on_focus_loss_and_ignores_orphan_events() {
    let terminal = VirtualTerminal::new(20, 4);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core()
        .add_child(component_ref(Text::new("alpha\nbeta\ngamma\ndelta", 0, 0)));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1b[<0;1;1M");
    tui.handle_terminal_input("\x1b[<32;4;2M");
    tui.wait_for_render().await;
    let focus_loss_event_count = terminal.events().len();
    tui.handle_terminal_input("\x1b[O");
    tui.handle_terminal_input("\x1b[I");
    tui.wait_for_render().await;
    let focus_loss_writes = writes_since(&terminal, focus_loss_event_count);
    assert!(focus_loss_writes.contains("alpha"));
    assert!(focus_loss_writes.contains("beta"));
    assert!(!focus_loss_writes.contains("\x1b[7m"));

    tui.handle_terminal_input("\x1b[<32;4;2M");
    tui.handle_terminal_input("\x1b[<0;4;2m");
    tui.wait_for_render().await;
    assert_eq!(clipboard_write_count(&terminal), 0);
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn retains_a_completed_visible_selection_across_focus_changes() {
    let terminal = VirtualTerminal::new(20, 4);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core()
        .add_child(component_ref(Text::new("alpha\nbeta\ngamma\ndelta", 0, 0)));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1b[<0;1;1M");
    tui.handle_terminal_input("\x1b[<32;4;2M");
    tui.handle_terminal_input("\x1b[<0;4;2m");
    tui.wait_for_render().await;
    let completed_write_count = write_count(&terminal);
    tui.handle_terminal_input("\x1b[O");
    tui.handle_terminal_input("\x1b[I");
    tui.wait_for_render().await;
    assert_eq!(write_count(&terminal), completed_write_count);

    let redraw_event_count = terminal.events().len();
    tui.render_now(true);
    let redraw_writes = writes_since(&terminal, redraw_event_count);
    assert!(redraw_writes.contains("alpha"));
    assert!(redraw_writes.contains("beta"));
    assert!(redraw_writes.contains("\x1b[7m"));
    tui.stop(TuiStopOptions::default());
}

/// Wait `ms` and then apply an elapsed scrollbar hide deadline.
///
/// Deviation class 1: the TS `ScrollView` hides the transient scrollbar from a
/// `setTimeout`; here the deadline is reported and fired by the driver.
async fn settle_scrollbar(state: &Rc<RefCell<ScrollViewState>>, ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    let due = state
        .borrow()
        .scrollbar_hide_deadline()
        .is_some_and(|deadline| deadline <= std::time::Instant::now());
    if due {
        state.borrow_mut().fire_scrollbar_hide();
    }
}

#[tokio::test]
async fn drags_a_visible_scrollbar_thumb_and_keeps_it_visible_until_release() {
    let terminal = VirtualTerminal::new(10, 5);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let scroll_view = ScrollView::new(
        component_ref(Text::new(numbered_text(20), 0, 0)),
        ScrollViewOptions {
            primary: true,
            scrollbar: ScrollViewScrollbar::Auto,
            scrollbar_hide_delay_ms: 50,
            ..ScrollViewOptions::default()
        },
    );
    let state = scroll_view.state();
    tui.set_layout_root(Some(component_ref(scroll_view)));
    tui.start();
    tui.wait_for_render().await;
    assert!(!state.borrow().is_scrollbar_visible());

    tui.handle_terminal_input("\x1b[<65;10;1M");
    tui.wait_for_render().await;
    assert_eq!(state.borrow().scroll_top(), 1);
    assert!(state.borrow().is_scrollbar_visible());

    // Pressing the thumb keeps the scrollbar visible past the hide delay.
    tui.handle_terminal_input("\x1b[<0;10;1M");
    tui.wait_for_render().await;
    settle_scrollbar(&state, 70).await;
    assert!(state.borrow().is_scrollbar_visible());

    tui.handle_terminal_input("\x1b[<32;10;4M");
    tui.wait_for_render().await;
    assert_eq!(state.borrow().scroll_top(), 15);
    assert_eq!(
        trimmed(&terminal),
        ["line 16", "line 17", "line 18", "line 19", "line 20"]
    );

    tui.handle_terminal_input("\x1b[<0;10;4m");
    tui.wait_for_render().await;
    assert!(state.borrow().is_scrollbar_visible());
    settle_scrollbar(&state, 70).await;
    assert!(state.borrow().is_scrollbar_visible());
    tui.handle_terminal_input("\x1b[<35;9;4M");
    settle_scrollbar(&state, 70).await;
    assert!(!state.borrow().is_scrollbar_visible());

    tui.handle_terminal_input("\x1b[<64;10;5M");
    tui.wait_for_render().await;
    assert_eq!(state.borrow().scroll_top(), 14);
    settle_scrollbar(&state, 70).await;
    assert!(state.borrow().is_scrollbar_visible());
    tui.handle_terminal_input("\x1b[<35;9;5M");
    settle_scrollbar(&state, 70).await;
    assert!(!state.borrow().is_scrollbar_visible());

    assert_eq!(clipboard_write_count(&terminal), 0);
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn keeps_the_scrollbar_column_selectable_while_the_thumb_is_hidden() {
    let terminal = VirtualTerminal::new(10, 2);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let scroll_view = ScrollView::new(
        component_ref(Text::new("123456789A\nabcdefghij\nmore\nlines", 0, 0)),
        ScrollViewOptions {
            scrollbar: ScrollViewScrollbar::Auto,
            ..ScrollViewOptions::default()
        },
    );
    let state = scroll_view.state();
    tui.set_layout_root(Some(component_ref(scroll_view)));
    tui.start();
    tui.wait_for_render().await;
    assert!(!state.borrow().is_scrollbar_visible());

    tui.handle_terminal_input("\x1b[<0;10;1M");
    tui.handle_terminal_input("\x1b[<32;10;2M");
    tui.handle_terminal_input("\x1b[<0;10;2m");
    tui.wait_for_render().await;

    assert!(
        terminal
            .get_writes()
            .contains(&format!("\x1b]52;c;{}\x07", base64("A\nabcdefghij"))),
        "writes: {:?}",
        terminal.get_writes()
    );
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn supports_keyboard_viewport_navigation_with_four_rows_of_page_overlap() {
    let terminal = VirtualTerminal::new(20, 8);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core()
        .add_child(component_ref(Text::new(numbered_text(12), 0, 0)));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1b[57421u");
    tui.handle_terminal_input("\x1b[57421;1:3u");
    tui.wait_for_render().await;
    assert_eq!(
        trimmed(&terminal),
        (1..=8).map(|i| format!("line {i}")).collect::<Vec<_>>()
    );

    tui.handle_terminal_input("\x1b[57422u");
    tui.handle_terminal_input("\x1b[57422;1:3u");
    tui.wait_for_render().await;
    assert_eq!(
        trimmed(&terminal),
        (5..=12).map(|i| format!("line {i}")).collect::<Vec<_>>()
    );

    tui.handle_terminal_input("\x1bOH");
    tui.wait_for_render().await;
    assert_eq!(
        trimmed(&terminal),
        (1..=8).map(|i| format!("line {i}")).collect::<Vec<_>>()
    );

    tui.handle_terminal_input("\x1bOF");
    tui.wait_for_render().await;
    assert_eq!(
        trimmed(&terminal),
        (5..=12).map(|i| format!("line {i}")).collect::<Vec<_>>()
    );

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn jumps_between_osc133_semantic_prompt_markers() {
    let terminal = VirtualTerminal::new(20, 3);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let text = (1..=4)
        .flat_map(|message| {
            vec![
                format!("\x1b]133;A\x07message {message}"),
                "detail".to_string(),
            ]
        })
        .collect::<Vec<_>>()
        .join("\n");
    tui.core().add_child(component_ref(Text::new(text, 0, 0)));
    tui.start();
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 5);

    tui.handle_terminal_input("\x1b[57419;6u");
    tui.handle_terminal_input("\x1b[57419;6:3u");
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 4);
    assert_eq!(trimmed(&terminal)[0], "message 3");

    tui.handle_terminal_input("\x1b[1;6A");
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 2);
    assert_eq!(trimmed(&terminal)[0], "message 2");

    tui.handle_terminal_input("\x1b[57420;6u");
    tui.handle_terminal_input("\x1b[57420;6:3u");
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 4);
    assert_eq!(trimmed(&terminal)[0], "message 3");

    tui.handle_terminal_input("\x1b[1;6B");
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 5);
    assert_eq!(trimmed(&terminal)[1], "message 4");
    assert!(tui.is_following_output());

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn routes_ctrl_modified_viewport_navigation_to_the_focused_component() {
    let terminal = VirtualTerminal::new(20, 6);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let transcript = ScrollView::new(
        component_ref(Text::new(numbered_text(12), 0, 0)),
        ScrollViewOptions {
            follow_end: true,
            primary: true,
            ..ScrollViewOptions::default()
        },
    );
    let transcript_state = transcript.state();
    let (probe, editor_component) = editor();

    let mut root = VStack::new(StackOptions::default());
    root.add_child_with(
        component_ref(transcript),
        StackEntryOptions {
            basis: Some(StackBasis::Size(0)),
            grow: Some(1),
            min_size: Some(1),
            ..StackEntryOptions::default()
        },
    );
    root.add_child_with(
        editor_component.clone(),
        StackEntryOptions {
            basis: Some(StackBasis::Size(1)),
            shrink: Some(0),
            ..StackEntryOptions::default()
        },
    );
    tui.set_layout_root(Some(component_ref(root)));
    tui.core().set_focus(Some(editor_component));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1bOH");
    tui.wait_for_render().await;
    assert_eq!(transcript_state.borrow().scroll_top(), 0);
    assert!(probe.inputs.borrow().is_empty());

    let modified_inputs = [
        "\x1b[1;5H",
        "\x1b[1;5F",
        "\x1b[5;5~",
        "\x1b[6;5~",
        "\x1b[57423;5u",
    ];
    for input in modified_inputs {
        tui.handle_terminal_input(input);
    }
    tui.handle_terminal_input("\x1b[57423;5:3u");
    tui.wait_for_render().await;
    assert_eq!(transcript_state.borrow().scroll_top(), 0);
    assert_eq!(*probe.inputs.borrow(), modified_inputs);

    tui.handle_terminal_input("\x1b[6~");
    tui.wait_for_render().await;
    assert_eq!(transcript_state.borrow().scroll_top(), 1);
    assert_eq!(*probe.inputs.borrow(), modified_inputs);

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn uses_configured_styles_for_current_and_non_current_search_matches() {
    let terminal = VirtualTerminal::new(60, 4);
    let mut tui = TuiAltScreen::new(
        Box::new(terminal.clone()),
        TuiAltScreenOptions {
            search_match_style: Rc::new(|text| format!("\x1b[41m{text}\x1b[49m")),
            search_current_match_style: Rc::new(|text| format!("\x1b[42m{text}\x1b[49m")),
            ..TuiAltScreenOptions::default()
        },
    );
    tui.core().add_child(component_ref(Text::new(
        "needle first\nmiddle\nneedle second\nend",
        0,
        0,
    )));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1b[102;6u");
    tui.handle_terminal_input("needle");
    tui.wait_for_render().await;

    let writes = terminal.get_writes();
    assert!(writes.contains("\x1b[42mneedle\x1b[49m"));
    assert!(writes.contains("\x1b[41mneedle\x1b[49m"));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn searches_the_transcript_and_restores_editor_focus_on_close() {
    let terminal = VirtualTerminal::new(60, 8);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let transcript_text = (1..=12)
        .map(|index| match index {
            5 => "line 5 needle one".to_string(),
            10 => "line 10 needle two".to_string(),
            _ => format!("line {index}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let transcript = ScrollView::new(
        component_ref(Text::new(transcript_text, 0, 0)),
        ScrollViewOptions {
            follow_end: true,
            primary: true,
            ..ScrollViewOptions::default()
        },
    );
    let transcript_state = transcript.state();
    let (probe, editor_component) = editor();

    let mut root = VStack::new(StackOptions::default());
    root.add_child_with(
        component_ref(transcript),
        StackEntryOptions {
            basis: Some(StackBasis::Size(0)),
            grow: Some(1),
            min_size: Some(1),
            ..StackEntryOptions::default()
        },
    );
    root.add_child_with(
        editor_component.clone(),
        StackEntryOptions {
            basis: Some(StackBasis::Size(1)),
            shrink: Some(0),
            ..StackEntryOptions::default()
        },
    );
    tui.set_layout_root(Some(component_ref(root)));
    tui.core().set_focus(Some(editor_component));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1b[102;6u");
    tui.handle_terminal_input("needle");
    tui.wait_for_render().await;
    assert!(!transcript_state.borrow().is_following_end());
    assert!(
        terminal
            .get_viewport()
            .iter()
            .any(|line| line.contains("Find transcript") && line.contains("2/2"))
    );
    assert!(
        terminal
            .get_viewport()
            .iter()
            .any(|line| line.contains("line 10 needle two"))
    );
    assert!(probe.inputs.borrow().is_empty());
    assert!(terminal.get_writes().contains("\x1b[1;7mneedle\x1b[22;27m"));

    for _ in 0..6 {
        tui.handle_terminal_input("\x1b[<64;1;4M");
    }
    tui.wait_for_render().await;
    assert_eq!(transcript_state.borrow().scroll_top(), 0);
    assert!(
        terminal
            .get_viewport()
            .iter()
            .any(|line| line.contains("> needle"))
    );

    tui.handle_terminal_input("\x07");
    tui.wait_for_render().await;
    assert!(
        terminal
            .get_viewport()
            .iter()
            .any(|line| line.contains("Find transcript") && line.contains("1/2"))
    );
    assert!(
        terminal
            .get_viewport()
            .iter()
            .any(|line| line.contains("line 5 needle one"))
    );

    tui.handle_terminal_input("\x1b[103;6u");
    tui.wait_for_render().await;
    assert!(
        terminal
            .get_viewport()
            .iter()
            .any(|line| line.contains("Find transcript") && line.contains("2/2"))
    );
    assert!(
        terminal
            .get_viewport()
            .iter()
            .any(|line| line.contains("line 10 needle two"))
    );

    tui.handle_terminal_input("\x1b");
    tui.handle_terminal_input("x");
    tui.wait_for_render().await;
    assert!(
        !terminal
            .get_viewport()
            .iter()
            .any(|line| line.contains("Find transcript"))
    );
    assert_eq!(*probe.inputs.borrow(), ["x"]);

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn routes_wheel_input_to_the_scroll_view_under_the_pointer() {
    let terminal = VirtualTerminal::new(20, 4);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let left = ScrollView::new(
        component_ref(Text::new("a1\na2\na3\na4\na5\na6\na7", 0, 0)),
        ScrollViewOptions {
            follow_end: true,
            primary: true,
            ..ScrollViewOptions::default()
        },
    );
    let right = ScrollView::new(
        component_ref(Text::new("b1\nb2\nb3\nb4\nb5\nb6\nb7", 0, 0)),
        ScrollViewOptions {
            follow_end: true,
            ..ScrollViewOptions::default()
        },
    );
    let left_state = left.state();
    let right_state = right.state();
    let mut root = HStack::new(StackOptions::default());
    root.add_child_with(
        component_ref(left),
        StackEntryOptions {
            basis: Some(StackBasis::Size(10)),
            shrink: Some(0),
            ..StackEntryOptions::default()
        },
    );
    root.add_child_with(
        component_ref(right),
        StackEntryOptions {
            basis: Some(StackBasis::Size(10)),
            shrink: Some(0),
            ..StackEntryOptions::default()
        },
    );
    tui.set_layout_root(Some(component_ref(root)));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1b[<64;15;1M");
    tui.wait_for_render().await;
    assert_eq!(left_state.borrow().scroll_top(), 3);
    assert_eq!(right_state.borrow().scroll_top(), 2);
    assert_eq!(
        trimmed(&terminal),
        [
            "a4        b3",
            "a5        b4",
            "a6        b5",
            "a7        b6"
        ]
    );
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn chains_unused_wheel_delta_to_an_outer_scroll_view() {
    let terminal = VirtualTerminal::new(20, 4);
    let mut tui = TuiAltScreen::new(
        Box::new(terminal.clone()),
        TuiAltScreenOptions {
            wheel_scroll_lines: 3,
            ..TuiAltScreenOptions::default()
        },
    );
    let inner = ScrollView::new(
        component_ref(Text::new("i1\ni2\ni3\ni4\ni5\ni6", 0, 0)),
        ScrollViewOptions::default(),
    );
    let inner_state = inner.state();
    let mut stack = VStack::new(StackOptions::default());
    stack.add_child_with(
        component_ref(inner),
        StackEntryOptions {
            basis: Some(StackBasis::Size(2)),
            ..StackEntryOptions::default()
        },
    );
    stack.add_child(component_ref(Text::new(
        "tail1\ntail2\ntail3\ntail4\ntail5",
        0,
        0,
    )));
    let outer = ScrollView::new(
        component_ref(stack),
        ScrollViewOptions {
            primary: true,
            ..ScrollViewOptions::default()
        },
    );
    let outer_state = outer.state();
    tui.set_layout_root(Some(component_ref(outer)));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1b[<65;1;1M");
    tui.wait_for_render().await;
    assert_eq!(inner_state.borrow().scroll_top(), 3);
    assert_eq!(outer_state.borrow().scroll_top(), 0);

    tui.handle_terminal_input("\x1b[<65;1;1M");
    tui.wait_for_render().await;
    assert_eq!(inner_state.borrow().scroll_top(), 4);
    assert_eq!(outer_state.borrow().scroll_top(), 2);
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn ignores_horizontal_trackpad_wheel_events() {
    let terminal = VirtualTerminal::new(20, 4);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core()
        .add_child(component_ref(Text::new(numbered_text(8), 0, 0)));
    tui.start();
    tui.wait_for_render().await;

    tui.handle_terminal_input("\x1b[<66;1;1M");
    tui.handle_terminal_input("\x1b[<67;1;1M");
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 4);
    assert_eq!(trimmed(&terminal), ["line 5", "line 6", "line 7", "line 8"]);

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn auto_scrolls_and_extends_a_drag_selection_held_at_the_viewport_edge() {
    let terminal = VirtualTerminal::new(20, 4);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core()
        .add_child(component_ref(Text::new(numbered_text(10), 0, 0)));
    tui.start();
    tui.wait_for_render().await;
    assert_eq!(tui.viewport_top(), 6);

    tui.handle_terminal_input("\x1b[<0;1;3M");
    tui.handle_terminal_input("\x1b[<32;1;1M");
    tokio::time::sleep(std::time::Duration::from_millis(130)).await;
    while tui
        .selection_auto_scroll_deadline()
        .is_some_and(|deadline| deadline <= std::time::Instant::now())
    {
        tui.auto_scroll_selection();
    }
    tui.wait_for_render().await;

    let selection_top = tui.viewport_top();
    assert!(
        selection_top < 6,
        "expected auto-scroll above row 6, got {selection_top}"
    );
    tui.handle_terminal_input("\x1b[<0;1;1m");
    tui.wait_for_render().await;

    let mut selected_lines: Vec<String> = (selection_top..8)
        .map(|index| format!("line {}", index + 1))
        .collect();
    selected_lines.push("l".to_string());
    assert!(
        terminal.get_writes().contains(&format!(
            "\x1b]52;c;{}\x07",
            base64(&selected_lines.join("\n"))
        )),
        "writes: {:?}",
        terminal.get_writes()
    );
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn stacks_flash_messages_and_collapses_them_as_they_expire() {
    let terminal = VirtualTerminal::new(20, 4);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core()
        .add_child(component_ref(Text::new("one\ntwo\nthree\nfour", 0, 0)));
    tui.start();
    tui.wait_for_render().await;

    tui.flash("First", Some(80));
    tui.flash("Second", Some(500));
    tui.wait_for_render().await;
    // The rows keep their written trailing space, so they are not trimmed here.
    let viewport = terminal.get_viewport();
    assert!(viewport[0].ends_with(" First "), "row: {:?}", viewport[0]);
    assert!(viewport[1].ends_with(" Second "), "row: {:?}", viewport[1]);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    tui.request_render(false);
    tui.wait_for_render().await;
    let viewport = terminal.get_viewport();
    assert!(viewport[0].ends_with(" Second "), "row: {:?}", viewport[0]);
    assert!(!viewport.iter().any(|line| line.contains("First")));

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn restores_keyboard_state_before_leaving_alt_mode_and_prints_the_full_document() {
    let terminal = VirtualTerminal::new(20, 3);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core().add_child(component_ref(Text::new(
        "first\nsecond\nthird\nfourth\nfifth\nsixth",
        0,
        0,
    )));
    tui.start();
    tui.wait_for_render().await;
    tui.stop(TuiStopOptions::default());

    let events = terminal.events();
    let position = |predicate: &dyn Fn(&TerminalEvent) -> bool| events.iter().position(predicate);
    let start_index = position(&|event| matches!(event, TerminalEvent::Start)).expect("start");
    let stop_index = position(&|event| matches!(event, TerminalEvent::Stop)).expect("stop");
    let write_index = |needle: &str| {
        events.iter().position(|event| match event {
            TerminalEvent::Write(data) => data.contains(needle),
            _ => false,
        })
    };
    let alt_screen_enter_index = write_index("\x1b[?1049h").expect("alt screen enter");
    let mouse_disable_index = write_index("\x1b[?1006l").expect("mouse disable");
    let main_screen_restore_index = write_index("\x1b[?1049l").expect("main screen restore");
    assert!(alt_screen_enter_index < start_index);
    assert!(mouse_disable_index < stop_index);
    assert!(main_screen_restore_index > stop_index);

    let TerminalEvent::Write(restore) = &events[main_screen_restore_index] else {
        panic!("restore event must be a write");
    };
    for word in ["first", "second", "third", "fourth", "fifth", "sixth"] {
        assert!(restore.contains(word), "missing {word} in {restore:?}");
    }
    assert!(restore.find("first") < restore.find("sixth"));
}
