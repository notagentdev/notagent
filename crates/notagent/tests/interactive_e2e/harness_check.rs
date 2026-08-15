//! The harness under test.
//!
//! The scenarios wait for C task 13, so nothing else would exercise the driver
//! and it would rot before its first real run. These cases drive every method
//! of [`InteractiveDriver`] against a real `TuiMainScreen` over the virtual
//! terminal — the same loop, the same terminal, only the interactive mode is
//! replaced by stub components.

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, Focusable, component_ref};
use notagent_tui::tui_main_screen::TuiMainScreen;

use super::harness::{InteractiveDriver, KEY_DOWN, KEY_ENTER, LIST_CURSOR, run_local};

/// Lines behind a handle, so a case can change them after mounting (the same
/// trick `packages/tui/test/tui-render.test.ts` plays with a plain object).
#[derive(Clone, Default)]
struct Lines(Rc<RefCell<Vec<String>>>);

impl Lines {
    fn set<I: IntoIterator<Item = S>, S: Into<String>>(&self, lines: I) {
        *self.0.borrow_mut() = lines.into_iter().map(Into::into).collect();
    }
}

/// Renders the handle's lines and records what it was typed.
#[derive(Clone, Default)]
struct Stub {
    lines: Lines,
    input: Rc<RefCell<Vec<String>>>,
    focused: Rc<RefCell<bool>>,
}

impl Component for Stub {
    fn render(&mut self, _width: usize) -> Vec<String> {
        self.lines.0.borrow().clone()
    }

    fn handle_input(&mut self, data: &str) {
        self.input.borrow_mut().push(data.to_string());
    }

    fn invalidate(&mut self) {}

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for Stub {
    fn focused(&self) -> bool {
        *self.focused.borrow()
    }

    fn set_focused(&mut self, focused: bool) {
        *self.focused.borrow_mut() = focused;
    }
}

/// A list of rows that moves its marker on Down and remembers what Return
/// picked — the shape [`InteractiveDriver::choose`] walks.
#[derive(Clone)]
struct StubList {
    rows: Vec<String>,
    index: Rc<RefCell<usize>>,
    picked: Rc<RefCell<Option<String>>>,
}

impl StubList {
    fn new(rows: &[&str]) -> Self {
        StubList {
            rows: rows.iter().map(|row| (*row).to_string()).collect(),
            index: Rc::new(RefCell::new(0)),
            picked: Rc::new(RefCell::new(None)),
        }
    }
}

impl Component for StubList {
    fn render(&mut self, _width: usize) -> Vec<String> {
        let index = *self.index.borrow();
        self.rows
            .iter()
            .enumerate()
            .map(|(row, label)| {
                let marker = if row == index { LIST_CURSOR } else { "  " };
                format!("{marker}{label}")
            })
            .collect()
    }

    fn handle_input(&mut self, data: &str) {
        match data {
            KEY_DOWN => {
                let next = (*self.index.borrow() + 1) % self.rows.len();
                *self.index.borrow_mut() = next;
            }
            KEY_ENTER => {
                let index = *self.index.borrow();
                *self.picked.borrow_mut() = Some(self.rows[index].clone());
            }
            _ => {}
        }
    }

    fn invalidate(&mut self) {}
}

/// A driver over a started `TuiMainScreen`, with `component` mounted and
/// focused — everything the real scenarios get, minus the interactive mode.
fn driver_with(component: ComponentUnderTest, columns: usize, rows: usize) -> InteractiveDriver {
    let terminal = VirtualTerminal::new(columns, rows);
    let pump = terminal.pump_handle();
    let mut ui = TuiMainScreen::new(Box::new(terminal.clone()));
    ui.core().add_child(component.clone());
    ui.core().set_focus(Some(component));
    ui.start();
    ui.request_render(true);
    InteractiveDriver::from_parts(Box::new(ui), Box::new(pump), terminal)
}

type ComponentUnderTest = notagent_tui::tui::ComponentRef;

#[tokio::test(flavor = "current_thread")]
async fn renders_a_frame_and_reads_it_back_from_the_terminal() {
    run_local(async {
        let stub = Stub::default();
        stub.lines.set(["first line", "second line"]);
        let mut driver = driver_with(component_ref(stub.clone()), 40, 10);

        driver.settle().await;

        driver.assert_shows("first line");
        driver.assert_shows("second line");
        driver.assert_hides("third line");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn send_keys_reaches_the_focused_component() {
    run_local(async {
        let stub = Stub::default();
        stub.lines.set(["editor"]);
        let mut driver = driver_with(component_ref(stub.clone()), 40, 10);

        driver.settle().await;
        driver.submit("hello").await;

        assert_eq!(
            stub.input.borrow().as_slice(),
            ["hello".to_string(), "\r".to_string()]
        );
        assert!(*stub.focused.borrow(), "the mounted component has focus");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn wait_for_keeps_rendering_until_late_output_arrives() {
    run_local(async {
        let stub = Stub::default();
        stub.lines.set(["waiting"]);
        let mut driver = driver_with(component_ref(stub.clone()), 40, 10);
        driver.settle().await;
        driver.assert_hides("the answer");

        // What a scripted provider does: the answer lands while the loop runs.
        let lines = stub.lines.clone();
        let core = driver.core_handle();
        tokio::task::spawn_local(async move {
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            lines.set(["the answer"]);
            core.request_render();
        });

        driver.wait_for("the answer").await;
        driver.wait_until_gone("waiting").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn resize_reaches_the_tui_and_the_screen_fits_again() {
    run_local(async {
        let stub = Stub::default();
        stub.lines.set(["0123456789012345678901234567890123456789"]);
        let mut driver = driver_with(component_ref(stub.clone()), 40, 10);
        driver.settle().await;
        driver.assert_fits(40);

        driver.resize(20, 10).await;

        driver.assert_fits(20);
        assert_eq!(driver.terminal().get_viewport().len(), 10);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn choose_walks_the_list_to_the_row_and_confirms_it() {
    run_local(async {
        let list = StubList::new(&["dark", "light", "solarized"]);
        let mut driver = driver_with(component_ref(list.clone()), 40, 10);
        driver.settle().await;
        assert_eq!(
            driver.selected_row().as_deref(),
            Some("→ dark"),
            "the marker starts on the first row"
        );

        driver.choose("solarized").await;

        assert_eq!(list.picked.borrow().as_deref(), Some("solarized"));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn wait_for_exit_drives_the_loop_until_the_mode_returns() {
    run_local(async {
        let stub = Stub::default();
        stub.lines.set(["running"]);
        let driver = driver_with(component_ref(stub.clone()), 40, 10);
        let lines = stub.lines.clone();
        let core = driver.core_handle();
        // What a mode does on its way out: change the screen, ask for the
        // frame, return the exit code.
        let mut driver = driver.with_exit(async move {
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            lines.set(["stopped"]);
            core.request_render();
            0
        });

        assert_eq!(driver.wait_for_exit().await, 0);
        // The frame the mode's last change asked for went out before it left.
        driver.settle().await;
        driver.assert_shows("stopped");
    })
    .await;
}
