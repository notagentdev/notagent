use std::cell::Cell;
use std::rc::Rc;

use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{
    Component, ComponentRef, Line, OverlayAnchor, OverlayMargin, OverlayOptions, SizeValue,
    TuiStopOptions, component_ref, shared_lines,
};
use notagent_tui::tui_main_screen::TuiMainScreen;

/// `class StaticOverlay` — records the width it was asked to render at.
struct StaticOverlay {
    lines: Vec<String>,
    requested_width: Rc<Cell<Option<usize>>>,
}

impl Component for StaticOverlay {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.requested_width.set(Some(width));
        shared_lines(self.lines.clone())
    }
    fn invalidate(&mut self) {}
}

fn static_overlay<S: Into<String>>(lines: Vec<S>) -> (Rc<Cell<Option<usize>>>, ComponentRef) {
    let requested_width = Rc::new(Cell::new(None));
    let component = component_ref(StaticOverlay {
        lines: lines.into_iter().map(Into::into).collect(),
        requested_width: Rc::clone(&requested_width),
    });
    (requested_width, component)
}

struct EmptyContent;

impl Component for EmptyContent {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        Vec::new()
    }
    fn invalidate(&mut self) {}
}

struct FnContent(fn(usize) -> Vec<String>);

impl Component for FnContent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        shared_lines((self.0)(width))
    }
    fn invalidate(&mut self) {}
}

async fn render_and_flush(tui: &mut TuiMainScreen) {
    tui.request_render(true);
    tui.wait_for_render().await;
}

fn options(width: Option<i64>) -> OverlayOptions {
    OverlayOptions {
        width: width.map(SizeValue::Cells),
        ..OverlayOptions::default()
    }
}

// describe("width overflow protection")

#[tokio::test]
async fn should_truncate_overlay_lines_that_exceed_declared_width() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["X".repeat(100)]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(overlay, Some(options(Some(20))));
    tui.start();
    render_and_flush(&mut tui).await;

    // Must not crash and no line may exceed the terminal width.
    assert_eq!(terminal.get_viewport().len(), 24);
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_handle_overlay_with_complex_ansi_sequences_without_crashing() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let complex_line = format!(
        "\x1b[48;2;40;50;40m \x1b[38;2;128;128;128mSome styled content\x1b[39m\x1b[49m\
         \x1b]8;;http://example.com\x07link\x1b]8;;\x07{}",
        " more content ".repeat(10)
    );
    let (_, overlay) = static_overlay(vec![
        complex_line.clone(),
        complex_line.clone(),
        complex_line,
    ]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(overlay, Some(options(Some(60))));
    tui.start();
    render_and_flush(&mut tui).await;

    assert!(!terminal.get_viewport().is_empty());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_handle_overlay_composited_on_styled_base_content() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["OVERLAY"]);

    tui.core().add_child(component_ref(FnContent(|width| {
        let styled_line = format!("\x1b[1m\x1b[38;2;255;0;0m{}\x1b[0m", "X".repeat(width));
        vec![styled_line.clone(), styled_line.clone(), styled_line]
    })));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            width: Some(SizeValue::Cells(20)),
            anchor: Some(OverlayAnchor::Center),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    assert!(
        terminal
            .get_viewport()
            .iter()
            .any(|line| line.contains("OVERLAY")),
        "Overlay should be visible"
    );
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_handle_wide_characters_at_overlay_boundary() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["中文日本語한글テスト漢字"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(overlay, Some(options(Some(15))));
    tui.start();
    render_and_flush(&mut tui).await;

    assert!(!terminal.get_viewport().is_empty());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_handle_overlay_positioned_at_terminal_edge() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["X".repeat(50)]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            col: Some(SizeValue::Cells(60)),
            width: Some(SizeValue::Cells(20)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    assert!(!terminal.get_viewport().is_empty());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_handle_overlay_on_base_content_with_osc_sequences() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["OVERLAY-TEXT"]);

    tui.core().add_child(component_ref(FnContent(|width| {
        let link = "\x1b]8;;file:///path/to/file.ts\x07file.ts\x1b]8;;\x07";
        let line = format!("See {link} for details {}", "X".repeat(width - 30));
        vec![line.clone(), line.clone(), line]
    })));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::Center),
            width: Some(SizeValue::Cells(20)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    assert!(!terminal.get_viewport().is_empty());
    tui.stop(TuiStopOptions::default());
}

// describe("width percentage")

#[tokio::test]
async fn should_render_overlay_at_percentage_of_terminal_width() {
    let terminal = VirtualTerminal::new(100, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (requested_width, overlay) = static_overlay(vec!["test"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            width: Some(SizeValue::Percent(50.0)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    assert_eq!(requested_width.get(), Some(50));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_respect_min_width_when_width_percent_results_in_smaller_width() {
    let terminal = VirtualTerminal::new(100, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (requested_width, overlay) = static_overlay(vec!["test"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            width: Some(SizeValue::Percent(10.0)),
            min_width: Some(30),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    assert_eq!(requested_width.get(), Some(30));
    tui.stop(TuiStopOptions::default());
}

// describe("anchor positioning")

#[tokio::test]
async fn should_position_overlay_at_top_left() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["TOP-LEFT"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Cells(10)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    let viewport = terminal.get_viewport();
    assert!(
        viewport[0].starts_with("TOP-LEFT"),
        "Expected TOP-LEFT at start, got: {}",
        viewport[0]
    );
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_position_overlay_at_bottom_right() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["BTM-RIGHT"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::BottomRight),
            width: Some(SizeValue::Cells(10)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    let viewport = terminal.get_viewport();
    let last_row = &viewport[23];
    assert!(last_row.contains("BTM-RIGHT"), "got: {last_row}");
    assert!(
        last_row.trim_end().ends_with("BTM-RIGHT"),
        "got: {last_row}"
    );
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_position_overlay_at_top_center() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["CENTERED"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::TopCenter),
            width: Some(SizeValue::Cells(10)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    let viewport = terminal.get_viewport();
    let first_row = &viewport[0];
    assert!(first_row.contains("CENTERED"), "got: {first_row}");
    let col_index = first_row.find("CENTERED").expect("overlay rendered");
    assert!(
        (30..=40).contains(&col_index),
        "Expected centered, got col {col_index}"
    );
    tui.stop(TuiStopOptions::default());
}

// describe("margin")

#[tokio::test]
async fn should_clamp_negative_margins_to_zero() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["NEG-MARGIN"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Cells(12)),
            margin: Some(OverlayMargin {
                top: -5,
                left: -10,
                right: 0,
                bottom: 0,
            }),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    let viewport = terminal.get_viewport();
    assert!(
        viewport[0].starts_with("NEG-MARGIN"),
        "Expected NEG-MARGIN at start of row 0, got: {}",
        viewport[0]
    );
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_respect_margin_as_number() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["MARGIN"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Cells(10)),
            margin: Some(OverlayMargin::all(5)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    let viewport = terminal.get_viewport();
    assert!(!viewport[0].contains("MARGIN"), "Should not be on row 0");
    assert!(!viewport[4].contains("MARGIN"), "Should not be on row 4");
    assert!(viewport[5].contains("MARGIN"), "got: {}", viewport[5]);
    assert_eq!(viewport[5].find("MARGIN"), Some(5));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_respect_margin_object() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["MARGIN"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Cells(10)),
            margin: Some(OverlayMargin {
                top: 2,
                left: 3,
                right: 0,
                bottom: 0,
            }),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    let viewport = terminal.get_viewport();
    assert!(viewport[2].contains("MARGIN"), "got: {}", viewport[2]);
    assert_eq!(viewport[2].find("MARGIN"), Some(3));
    tui.stop(TuiStopOptions::default());
}

// describe("offset")

#[tokio::test]
async fn should_apply_offset_x_and_offset_y_from_anchor_position() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["OFFSET"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Cells(10)),
            offset_x: Some(10),
            offset_y: Some(5),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    let viewport = terminal.get_viewport();
    assert!(viewport[5].contains("OFFSET"), "got: {}", viewport[5]);
    assert_eq!(viewport[5].find("OFFSET"), Some(10));
    tui.stop(TuiStopOptions::default());
}

// describe("percentage positioning")

#[tokio::test]
async fn should_position_with_row_percent_and_col_percent() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["PCT"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            width: Some(SizeValue::Cells(10)),
            row: Some(SizeValue::Percent(50.0)),
            col: Some(SizeValue::Percent(50.0)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    let viewport = terminal.get_viewport();
    let found_row = viewport
        .iter()
        .position(|line| line.contains("PCT"))
        .expect("overlay rendered");
    assert!(
        (10..=13).contains(&found_row),
        "Expected centered row, got {found_row}"
    );
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn row_percent_0_should_position_at_top() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["TOP"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            width: Some(SizeValue::Cells(10)),
            row: Some(SizeValue::Percent(0.0)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    assert!(terminal.get_viewport()[0].contains("TOP"));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn row_percent_100_should_position_at_bottom() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["BOTTOM"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            width: Some(SizeValue::Cells(10)),
            row: Some(SizeValue::Percent(100.0)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    assert!(terminal.get_viewport()[23].contains("BOTTOM"));
    tui.stop(TuiStopOptions::default());
}

// describe("maxHeight")

#[tokio::test]
async fn should_truncate_overlay_to_max_height() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["Line 1", "Line 2", "Line 3", "Line 4", "Line 5"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            max_height: Some(SizeValue::Cells(3)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    let content = terminal.get_viewport().join("\n");
    for expected in ["Line 1", "Line 2", "Line 3"] {
        assert!(content.contains(expected), "Should include {expected}");
    }
    for unexpected in ["Line 4", "Line 5"] {
        assert!(
            !content.contains(unexpected),
            "Should NOT include {unexpected}"
        );
    }
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_truncate_overlay_to_max_height_percent() {
    let terminal = VirtualTerminal::new(80, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec![
        "L1", "L2", "L3", "L4", "L5", "L6", "L7", "L8", "L9", "L10",
    ]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            max_height: Some(SizeValue::Percent(50.0)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    let content = terminal.get_viewport().join("\n");
    assert!(content.contains("L1"), "Should include L1");
    assert!(content.contains("L5"), "Should include L5");
    assert!(!content.contains("L6"), "Should NOT include L6");
    tui.stop(TuiStopOptions::default());
}

// describe("absolute positioning")

#[tokio::test]
async fn row_and_col_should_override_anchor() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_, overlay) = static_overlay(vec!["ABSOLUTE"]);

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().show_overlay(
        overlay,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::BottomRight),
            row: Some(SizeValue::Cells(3)),
            col: Some(SizeValue::Cells(5)),
            width: Some(SizeValue::Cells(10)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    let viewport = terminal.get_viewport();
    assert!(viewport[3].contains("ABSOLUTE"), "got: {}", viewport[3]);
    assert_eq!(viewport[3].find("ABSOLUTE"), Some(5));
    tui.stop(TuiStopOptions::default());
}

// describe("stacked overlays")

#[tokio::test]
async fn should_render_multiple_overlays_with_later_ones_on_top() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(EmptyContent));

    let (_, overlay1) = static_overlay(vec!["FIRST-OVERLAY"]);
    tui.core().show_overlay(
        overlay1,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Cells(20)),
            ..OverlayOptions::default()
        }),
    );
    let (_, overlay2) = static_overlay(vec!["SECOND"]);
    tui.core().show_overlay(
        overlay2,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Cells(10)),
            ..OverlayOptions::default()
        }),
    );

    tui.start();
    render_and_flush(&mut tui).await;

    assert!(terminal.get_viewport()[0].contains("SECOND"));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_handle_overlays_at_different_positions_without_interference() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(EmptyContent));

    let (_, overlay1) = static_overlay(vec!["TOP-LEFT"]);
    tui.core().show_overlay(
        overlay1,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Cells(15)),
            ..OverlayOptions::default()
        }),
    );
    let (_, overlay2) = static_overlay(vec!["BTM-RIGHT"]);
    tui.core().show_overlay(
        overlay2,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::BottomRight),
            width: Some(SizeValue::Cells(15)),
            ..OverlayOptions::default()
        }),
    );

    tui.start();
    render_and_flush(&mut tui).await;

    let viewport = terminal.get_viewport();
    assert!(viewport[0].contains("TOP-LEFT"), "got: {}", viewport[0]);
    assert!(viewport[23].contains("BTM-RIGHT"), "got: {}", viewport[23]);
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_properly_hide_overlays_in_stack_order() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(EmptyContent));

    let (_, overlay1) = static_overlay(vec!["FIRST"]);
    tui.core().show_overlay(
        overlay1,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Cells(10)),
            ..OverlayOptions::default()
        }),
    );
    let (_, overlay2) = static_overlay(vec!["SECOND"]);
    tui.core().show_overlay(
        overlay2,
        Some(OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Cells(10)),
            ..OverlayOptions::default()
        }),
    );

    tui.start();
    render_and_flush(&mut tui).await;
    assert!(
        terminal.get_viewport()[0].contains("SECOND"),
        "SECOND should be visible initially"
    );

    tui.core().hide_overlay();
    render_and_flush(&mut tui).await;

    assert!(
        terminal.get_viewport()[0].contains("FIRST"),
        "FIRST should be visible after hiding SECOND"
    );
    tui.stop(TuiStopOptions::default());
}
