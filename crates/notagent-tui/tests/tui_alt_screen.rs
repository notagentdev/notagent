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
