//! Port of `packages/tui/test/tui-overlay-style-leak.test.ts` (81 LOC).

use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, OverlayOptions, SizeValue, TuiStopOptions, component_ref};
use notagent_tui::tui_main_screen::TuiMainScreen;

struct StaticLines(Vec<String>);

impl Component for StaticLines {
    fn render(&mut self, _width: usize) -> Vec<String> {
        self.0.clone()
    }
    fn invalidate(&mut self) {}
}

struct StaticOverlay(String);

impl Component for StaticOverlay {
    fn render(&mut self, _width: usize) -> Vec<String> {
        vec![self.0.clone()]
    }
    fn invalidate(&mut self) {}
}

async fn render_and_flush(tui: &mut TuiMainScreen) {
    tui.request_render(true);
    tui.wait_for_render().await;
}

#[tokio::test]
async fn should_not_leak_styles_when_a_trailing_reset_sits_beyond_the_last_visible_column() {
    let width = 20;
    let base_line = format!("\x1b[3m{}\x1b[23m", "X".repeat(width));

    let terminal = VirtualTerminal::new(width, 6);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(StaticLines(vec![
        base_line,
        "INPUT".to_string(),
    ])));
    tui.start();
    render_and_flush(&mut tui).await;

    assert!(!terminal.get_cell_italic(1, 0));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn should_not_leak_styles_when_overlay_slicing_drops_trailing_sgr_resets() {
    let width = 20;
    let base_line = format!("\x1b[3m{}\x1b[23m", "X".repeat(width));

    let terminal = VirtualTerminal::new(width, 6);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(StaticLines(vec![
        base_line,
        "INPUT".to_string(),
    ])));
    tui.core().show_overlay(
        component_ref(StaticOverlay("OVR".to_string())),
        Some(OverlayOptions {
            row: Some(SizeValue::Cells(0)),
            col: Some(SizeValue::Cells(5)),
            width: Some(SizeValue::Cells(3)),
            ..OverlayOptions::default()
        }),
    );
    tui.start();
    render_and_flush(&mut tui).await;

    assert!(!terminal.get_cell_italic(1, 0));
    tui.stop(TuiStopOptions::default());
}
