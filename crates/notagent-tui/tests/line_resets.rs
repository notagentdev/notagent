//! The segment reset on rendered lines.
//!
//! Until v0.1.22 `apply_line_resets` appended the reset unconditionally, so
//! nothing could go wrong and nothing was asserted. Once components prepend it
//! while filling their caches the responsibility moves, and then this assertion
//! is the boundary: a line without the reset bleeds colour into the next one.
//!
//! The pass sits at three call sites here — the main screen
//! (`tui_main_screen.rs:435`) and the alternate screen twice
//! (`tui_alt_screen.rs:1865`, `:2005`) — where `../notagent-main-rust` has one.
//! All three call the same method, so one assertion over the method covers them;
//! the sites are named so a fourth cannot appear unnoticed.

use notagent_tui::Text;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, Container, Line, TuiCore, component_ref};

const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

fn core() -> TuiCore {
    TuiCore::new(Box::new(VirtualTerminal::new(80, 24)))
}

/// A tree the way a transcript builds one: coloured text, a tab, and a Thai
/// character that normalization has to decompose.
fn rendered_tree() -> Vec<Line> {
    let mut root = Container::new();
    root.add_child(component_ref(Text::new("an ordinary line", 0, 0)));
    root.add_child(component_ref(Text::new("\x1b[31mred\x1b[39m", 0, 0)));
    root.add_child(component_ref(Text::new("with\ta tab", 0, 0)));
    root.add_child(component_ref(Text::new(
        "Thai \u{0e33} in the middle",
        0,
        0,
    )));
    root.render(80)
}

#[test]
fn every_rendered_line_carries_the_reset() {
    let core = core();
    let mut lines = rendered_tree();

    core.apply_line_resets(&mut lines);

    assert!(!lines.is_empty(), "the tree renders lines");
    for line in &lines {
        assert!(
            line.ends_with(SEGMENT_RESET),
            "line without a reset: {line:?}"
        );
    }
}

#[test]
fn a_second_pass_changes_nothing() {
    // The property prepending in the cache rests on: a line that already carries
    // the reset is not rebuilt.
    let core = core();
    let mut lines = rendered_tree();

    core.apply_line_resets(&mut lines);
    let after_first = lines.clone();
    core.apply_line_resets(&mut lines);

    assert_eq!(lines, after_first);
}

#[test]
fn a_prepared_line_is_left_alone() {
    // Exactly what step 8 of the plan produces.
    let core = core();
    let prepared = format!("already finished{SEGMENT_RESET}");
    let mut lines = vec![Line::from(prepared.clone())];

    core.apply_line_resets(&mut lines);

    assert_eq!(lines[0].as_ref(), prepared);
}

#[test]
fn image_lines_are_untouched() {
    // A reset inside a graphics sequence destroys the output.
    let core = core();
    let image = "\x1b_Ga=T,f=100;AAAA\x1b\\";
    let mut lines = vec![Line::from(image)];

    core.apply_line_resets(&mut lines);

    assert_eq!(
        lines[0].as_ref(),
        image,
        "an image line must not get a reset"
    );
}

#[test]
fn normalization_still_applies() {
    // The reset is not the only thing the pass does, and `../notagent-main-rust`
    // lost exactly this half.
    let core = core();
    let mut lines = vec![Line::from("with\ta tab"), Line::from("Thai \u{0e33}")];

    core.apply_line_resets(&mut lines);

    assert!(lines[0].starts_with("with   a tab"), "{:?}", lines[0]);
    assert!(
        lines[1].contains('\u{0e4d}') && !lines[1].contains('\u{0e33}'),
        "{:?}",
        lines[1]
    );
}
