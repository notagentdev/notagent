//! Port of `packages/tui/test/tui-cell-size-input.test.ts` (82 LOC).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent_tui::terminal_image::{
    CellDimensions, get_cell_dimensions, reset_capabilities_cache, set_cell_dimensions,
};
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, TuiStopOptions, component_ref};
use notagent_tui::tui_main_screen::TuiMainScreen;

fn guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Default)]
struct InputRecorder {
    inputs: Rc<RefCell<Vec<String>>>,
}

impl Component for InputRecorder {
    fn render(&mut self, _width: usize) -> Vec<String> {
        vec![String::new()]
    }

    fn handle_input(&mut self, data: &str) {
        self.inputs.borrow_mut().push(data.to_string());
    }

    fn invalidate(&mut self) {}
}

/// Pretend to run inside a terminal with image support (TS: `withImageTerminal`).
fn with_image_terminal(body: impl FnOnce()) {
    let previous: Vec<(&str, Option<String>)> = ["TERM_PROGRAM", "TERM", "GHOSTTY_RESOURCES_DIR"]
        .iter()
        .map(|name| (*name, std::env::var(name).ok()))
        .collect();
    // SAFETY: tests in this binary are serialized through `guard()`.
    unsafe {
        std::env::set_var("TERM_PROGRAM", "ghostty");
        std::env::remove_var("TERM");
        std::env::remove_var("GHOSTTY_RESOURCES_DIR");
    }
    reset_capabilities_cache();

    body();

    for (name, value) in previous {
        // SAFETY: see above.
        unsafe {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
    reset_capabilities_cache();
}

#[test]
fn forwards_bare_escape_even_when_a_cell_size_query_was_sent_at_startup() {
    let _guard = guard();
    with_image_terminal(|| {
        let terminal = VirtualTerminal::new(80, 24);
        let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
        let recorder = InputRecorder::default();
        let inputs = Rc::clone(&recorder.inputs);
        tui.core().set_focus(Some(component_ref(recorder)));
        tui.start();

        terminal.send_input("\x1b");

        assert_eq!(inputs.borrow().as_slice(), ["\x1b".to_string()]);
        tui.stop(TuiStopOptions::default());
    });
}

#[test]
fn consumes_cell_size_responses_and_still_forwards_later_user_input() {
    let _guard = guard();
    with_image_terminal(|| {
        set_cell_dimensions(CellDimensions {
            width_px: 9,
            height_px: 18,
        });

        let terminal = VirtualTerminal::new(80, 24);
        let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
        let recorder = InputRecorder::default();
        let inputs = Rc::clone(&recorder.inputs);
        tui.core().set_focus(Some(component_ref(recorder)));
        tui.start();

        terminal.send_input("\x1b[6;20;10t");
        assert!(inputs.borrow().is_empty());
        assert_eq!(
            get_cell_dimensions(),
            CellDimensions {
                width_px: 10,
                height_px: 20
            }
        );

        terminal.send_input("q");
        assert_eq!(inputs.borrow().as_slice(), ["q".to_string()]);
        tui.stop(TuiStopOptions::default());
    });
}
