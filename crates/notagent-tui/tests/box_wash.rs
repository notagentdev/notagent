//! A washed box over a child that finishes its lines.
//!
//! Markdown bakes the segment reset into its cached lines (step 8 of the
//! line-sharing plan). A `BoxComponent` with a background function embeds
//! child lines into its own styling; an embedded full SGR reset would cut the
//! wash off before the box's right padding column. The box therefore strips
//! the child's line terminator before painting — this pins that property,
//! which no byte oracle covers (the TS original never has resets inside
//! component output). The reference `../notagent-main-rust` washes straight
//! over the reset and shows exactly this artifact; we deliberately do not.

use std::rc::Rc;

use notagent_tui::components::box_component::BoxComponent;
use notagent_tui::components::markdown::{Markdown, MarkdownTheme, StyleFn};
use notagent_tui::tui::{Component, component_ref};

const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

fn plain() -> StyleFn {
    Rc::new(|text: &str| text.to_string())
}

fn unstyled_theme() -> MarkdownTheme {
    MarkdownTheme {
        heading: plain(),
        link: plain(),
        link_url: plain(),
        code: plain(),
        code_block: plain(),
        code_block_border: plain(),
        quote: plain(),
        quote_border: plain(),
        hr: plain(),
        list_bullet: plain(),
        bold: plain(),
        italic: plain(),
        strikethrough: plain(),
        underline: plain(),
        highlight_code: None,
        code_block_indent: None,
    }
}

#[test]
fn a_washed_box_keeps_its_background_to_the_right_edge() {
    let mut boxed = BoxComponent::new(1, 0, Some(Rc::new(|text: &str| format!("<BG>{text}</BG>"))));
    boxed.add_child(component_ref(Markdown::new(
        "hello",
        0,
        0,
        unstyled_theme(),
        None,
        None,
    )));

    let lines = boxed.render(12);
    assert!(!lines.is_empty());
    for line in &lines {
        assert!(
            !line.contains(SEGMENT_RESET),
            "a reset inside the wash cuts the background off: {:?}",
            line.as_ref()
        );
    }
    assert_eq!(lines[0].as_ref(), "<BG> hello      </BG>");
}
