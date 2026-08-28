use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, Line, TuiStopOptions, component_ref, shared_lines};
use notagent_tui::tui_main_screen::TuiMainScreen;

struct SimpleContent(Vec<String>);

impl Component for SimpleContent {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        shared_lines(self.0.clone())
    }
    fn invalidate(&mut self) {}
}

struct SimpleOverlay;

impl Component for SimpleOverlay {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        vec![
            Line::from("OVERLAY_TOP"),
            Line::from("OVERLAY_MID"),
            Line::from("OVERLAY_BOT"),
        ]
    }
    fn invalidate(&mut self) {}
}

#[tokio::test]
async fn should_render_overlay_when_content_is_shorter_than_terminal_height() {
    // The terminal has 24 rows but the content is only three lines.
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(SimpleContent(vec![
        "Line 1".to_string(),
        "Line 2".to_string(),
        "Line 3".to_string(),
    ])));

    // Centered overlay — around row 10 in a 24-row terminal.
    tui.core().show_overlay(component_ref(SimpleOverlay), None);

    tui.start();
    tui.wait_for_render().await;

    let viewport = terminal.get_viewport();
    assert!(
        viewport.iter().any(|line| line.contains("OVERLAY")),
        "Overlay should be visible when content is shorter than terminal: {viewport:?}"
    );

    tui.stop(TuiStopOptions::default());
}
