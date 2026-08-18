//! Port of `packages/tui/test/tui-shrink.test.ts` (45 LOC).

use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, Line, TuiStopOptions, component_ref, shared_lines};
use notagent_tui::tui_main_screen::TuiMainScreen;

struct Lines(Vec<String>);

impl Component for Lines {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        shared_lines(self.0.clone())
    }
    fn invalidate(&mut self) {}
}

#[tokio::test]
async fn clears_all_rendered_lines_when_content_shrinks_to_zero() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(Lines(vec![
        "first".to_string(),
        "second".to_string(),
        "third".to_string(),
    ])));
    tui.start();
    tui.wait_for_render().await;

    for expected in ["first", "second", "third"] {
        assert!(
            terminal.get_viewport().iter().any(|l| l.contains(expected)),
            "{expected} should be rendered"
        );
    }

    tui.core().clear();
    tui.request_render(false);
    tui.wait_for_render().await;

    let viewport = terminal.get_viewport();
    for expected in ["first", "second", "third"] {
        assert!(
            !viewport.iter().any(|l| l.contains(expected)),
            "{expected} line should be cleared"
        );
    }

    tui.stop(TuiStopOptions::default());
}
