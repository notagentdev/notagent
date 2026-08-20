//! A line that changes only its colors (same visible text) between
//! frames must be repainted by the differential renderer.

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, Line, component_ref};
use notagent_tui::tui_main_screen::TuiMainScreen;

struct SharedLines(Rc<RefCell<Vec<Line>>>);

impl Component for SharedLines {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        self.0.borrow().clone()
    }
    fn invalidate(&mut self) {}
}

#[test]
fn a_color_only_change_is_repainted() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let lines: Rc<RefCell<Vec<Line>>> = Rc::new(RefCell::new(vec![
        Line::from("\x1b[41m BASH \x1b[0m ls -la"),
        Line::from("output line"),
    ]));
    tui.core()
        .add_child(component_ref(SharedLines(Rc::clone(&lines))));
    tui.start();
    tui.render_pending_frame();
    terminal.flush();

    // Same text, new colors: the badge flips from red to green.
    lines.borrow_mut()[0] = Line::from("\x1b[42m BASH \x1b[0m ls -la");
    terminal.clear_writes();
    tui.core().request_render();
    tui.render_pending_frame();
    terminal.flush();

    let writes = terminal.get_writes();
    assert!(
        writes.contains("\x1b[42m"),
        "the recolored line was not repainted; writes: {writes:?}"
    );
}
