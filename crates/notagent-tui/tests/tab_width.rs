use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{
    Component, Line, OverlayOptions, SizeValue, TuiStopOptions, component_ref,
};
use notagent_tui::tui_main_screen::TuiMainScreen;
use notagent_tui::{extract_segments, normalize_terminal_output, slice_with_width, visible_width};

struct FullViewportContent;

impl Component for FullViewportContent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        ["base 0", "base 1", "base 2"]
            .iter()
            .map(|line| Line::from(format!("{line:<width$}")))
            .collect()
    }

    fn invalidate(&mut self) {}
}

struct TabStatusOverlay;

impl Component for TabStatusOverlay {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        vec![Line::from("\tX")]
    }

    fn invalidate(&mut self) {}
}

#[tokio::test]
async fn keeps_tab_containing_overlays_on_one_physical_terminal_row() {
    let terminal = VirtualTerminal::new(16, 3);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(FullViewportContent));
    tui.core().show_overlay(
        component_ref(TabStatusOverlay),
        Some(OverlayOptions {
            width: Some(SizeValue::Cells(4)),
            row: Some(SizeValue::Cells(1)),
            col: Some(SizeValue::Cells(4)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    tui.wait_for_render().await;

    assert_eq!(
        terminal.get_viewport(),
        ["base 0          ", "base   X        ", "base 2          "]
    );
    assert!(!terminal.get_writes().contains('\t'));

    tui.stop(TuiStopOptions::default());
}

#[test]
fn keeps_slice_helper_widths_consistent_with_visible_width() {
    let text = "out 192M\t.notagent/skill-tests/results-ha";
    let slice = slice_with_width(text, 0, 10, true);

    assert_eq!(slice.text, "out 192M");
    assert_eq!(slice.width, 8);
    assert_eq!(visible_width(&slice.text), slice.width);
}

#[test]
fn keeps_overlay_segment_widths_consistent_with_visible_width() {
    let text = "out 192M\t.notagent/skill-tests/results-ha";
    let segments = extract_segments(text, 10, 13, 10, true);

    assert_eq!(segments.before, "out 192M");
    assert_eq!(segments.before_width, 8);
    assert_eq!(visible_width(&segments.before), segments.before_width);

    let tab_fits = extract_segments(text, 11, 13, 10, true);
    assert_eq!(tab_fits.before, "out 192M\t");
    assert_eq!(tab_fits.before_width, 11);
    assert_eq!(visible_width(&tab_fits.before), tab_fits.before_width);
}

#[test]
fn keeps_tabs_inside_terminal_control_sequences_byte_identical() {
    let control_sequences = [
        "\x1b]8;;https://example.test/a\tb\x07",
        "\x1b]0;window\ttitle\x1b\\",
        "\x1b_payload\tdata\x1b\\",
    ];

    for control_sequence in control_sequences {
        assert_eq!(
            normalize_terminal_output(&format!("{control_sequence}label\ttext")),
            format!("{control_sequence}label   text")
        );
    }
}
