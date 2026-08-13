//! Port of `packages/tui/test/tui-alt-screen.test.ts` (1267 LOC).
//!
//! This stage covers the renderer core and wheel routing; the selection,
//! scrollbar drag and transcript search cases follow with the next stages.

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::scroll_view::{ScrollView, ScrollViewOptions};
use notagent_tui::components::stack::{StackEntryOptions, StackOptions};
use notagent_tui::components::text::Text;
use notagent_tui::components::v_stack::VStack;
use notagent_tui::layout_node::StackBasis;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{ComponentRef, TuiStopOptions, component_ref};
use notagent_tui::tui_alt_screen::{TuiAltScreen, TuiAltScreenOptions};

fn numbered_text(count: usize) -> String {
    (1..=count)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n")
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
