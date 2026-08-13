//! Port of the parser part of `packages/tui/test/terminal-colors.test.ts` (252 LOC).
//!
use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::terminal_colors::{
    RgbColor, TerminalColorScheme, parse_osc11_background_color, parse_terminal_color_scheme_report,
};
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, TuiInputListenerResult, TuiStopOptions, component_ref};
use notagent_tui::tui_main_screen::TuiMainScreen;

#[test]
fn parses_16_bit_osc_11_rgb_responses() {
    assert_eq!(
        parse_osc11_background_color("\x1b]11;rgb:0000/8000/ffff\x07"),
        Some(RgbColor {
            r: 0,
            g: 128,
            b: 255
        })
    );
}

#[test]
fn parses_osc_11_hex_responses() {
    assert_eq!(
        parse_osc11_background_color("\x1b]11;#ffffff\x1b\\"),
        Some(RgbColor {
            r: 255,
            g: 255,
            b: 255
        })
    );
    assert_eq!(
        parse_osc11_background_color("\x1b]11;#000000\x07"),
        Some(RgbColor { r: 0, g: 0, b: 0 })
    );
}

#[test]
fn rejects_non_strict_osc_11_responses() {
    assert_eq!(parse_osc11_background_color("x\x1b]11;#ffffff\x07"), None);
    assert_eq!(parse_osc11_background_color("\x1b]10;#ffffff\x07"), None);
    assert_eq!(parse_osc11_background_color("\x1b]11;#ffffff\x07x"), None);
}

#[test]
fn parses_color_scheme_reports() {
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;1n"),
        Some(TerminalColorScheme::Dark)
    );
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;2n"),
        Some(TerminalColorScheme::Light)
    );
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;2n\x1b[?997;1n\x1b[?997;1n"),
        Some(TerminalColorScheme::Dark)
    );
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;1n\x1b[?997;2n\x1b[?997;2n"),
        Some(TerminalColorScheme::Light)
    );
    assert_eq!(parse_terminal_color_scheme_report("\x1b[?997;3n"), None);
    assert_eq!(parse_terminal_color_scheme_report("\x1b[?996n"), None);
    assert_eq!(parse_terminal_color_scheme_report("x\x1b[?997;1n"), None);
}

/// `class InputRecorder` of the TS suite.
#[derive(Clone, Default)]
struct InputRecorder {
    inputs: Rc<RefCell<Vec<String>>>,
}

impl Component for InputRecorder {
    fn render(&mut self, _width: usize) -> Vec<String> {
        Vec::new()
    }

    fn handle_input(&mut self, data: &str) {
        self.inputs.borrow_mut().push(data.to_string());
    }

    fn invalidate(&mut self) {}
}

struct QueryHarness {
    terminal: VirtualTerminal,
    tui: TuiMainScreen,
    component_inputs: Rc<RefCell<Vec<String>>>,
    listener_inputs: Rc<RefCell<Vec<String>>>,
}

fn query_harness(with_component: bool) -> QueryHarness {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let recorder = InputRecorder::default();
    let component_inputs = Rc::clone(&recorder.inputs);
    let listener_inputs: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    if with_component {
        let component = component_ref(recorder);
        tui.core().add_child(component.clone());
        tui.core().set_focus(Some(component));
        let sink = Rc::clone(&listener_inputs);
        tui.core().add_input_listener(Box::new(move |data| {
            sink.borrow_mut().push(data.to_string());
            None::<TuiInputListenerResult>
        }));
    }
    tui.start();

    QueryHarness {
        terminal,
        tui,
        component_inputs,
        listener_inputs,
    }
}

#[tokio::test]
async fn writes_osc_11_query_and_resolves_with_the_parsed_rgb_reply() {
    let mut harness = query_harness(false);
    let core = harness.tui.core().clone();
    let terminal = harness.terminal.clone();

    let query = core.query_terminal_background_color(std::time::Duration::from_secs(1));
    tokio::pin!(query);
    // Poll once so the query is registered and the OSC 11 sequence is written.
    tokio::select! {
        _ = &mut query => unreachable!("query cannot settle before the reply"),
        () = tokio::time::sleep(std::time::Duration::from_millis(1)) => {}
    }
    assert!(terminal.get_writes().contains("\x1b]11;?\x07"));

    terminal.send_input("\x1b]11;#ffffff\x07");
    assert_eq!(
        query.await,
        Some(RgbColor {
            r: 255,
            g: 255,
            b: 255
        })
    );

    harness.tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn consumes_osc_11_replies_before_listeners_and_component_dispatch() {
    let mut harness = query_harness(true);
    let core = harness.tui.core().clone();
    let terminal = harness.terminal.clone();

    let query = core.query_terminal_background_color(std::time::Duration::from_secs(1));
    tokio::pin!(query);
    tokio::select! {
        _ = &mut query => unreachable!("query cannot settle before the reply"),
        () = tokio::time::sleep(std::time::Duration::from_millis(1)) => {}
    }

    terminal.send_input("\x1b]11;#000000\x07");

    assert_eq!(query.await, Some(RgbColor { r: 0, g: 0, b: 0 }));
    assert!(harness.listener_inputs.borrow().is_empty());
    assert!(harness.component_inputs.borrow().is_empty());

    harness.tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn consumes_unparseable_strict_osc_11_replies_and_resolves_none() {
    let mut harness = query_harness(true);
    let core = harness.tui.core().clone();
    let terminal = harness.terminal.clone();

    let query = core.query_terminal_background_color(std::time::Duration::from_secs(1));
    tokio::pin!(query);
    tokio::select! {
        _ = &mut query => unreachable!("query cannot settle before the reply"),
        () = tokio::time::sleep(std::time::Duration::from_millis(1)) => {}
    }

    terminal.send_input("\x1b]11;not-a-color\x07");

    assert_eq!(query.await, None);
    assert!(harness.listener_inputs.borrow().is_empty());
    assert!(harness.component_inputs.borrow().is_empty());

    harness.tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn dispatches_non_matching_input_normally_while_waiting_for_an_osc_11_reply() {
    let mut harness = query_harness(true);
    let core = harness.tui.core().clone();
    let terminal = harness.terminal.clone();

    let query = core.query_terminal_background_color(std::time::Duration::from_secs(1));
    tokio::pin!(query);
    tokio::select! {
        _ = &mut query => unreachable!("query cannot settle before the reply"),
        () = tokio::time::sleep(std::time::Duration::from_millis(1)) => {}
    }

    terminal.send_input("x");
    assert_eq!(
        harness.listener_inputs.borrow().as_slice(),
        ["x".to_string()]
    );
    assert_eq!(
        harness.component_inputs.borrow().as_slice(),
        ["x".to_string()]
    );

    terminal.send_input("\x1b]11;#ffffff\x07");
    assert_eq!(
        query.await,
        Some(RgbColor {
            r: 255,
            g: 255,
            b: 255
        })
    );

    harness.tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn keeps_consuming_a_late_osc_11_reply_after_timeout() {
    let mut harness = query_harness(true);
    let core = harness.tui.core().clone();
    let terminal = harness.terminal.clone();

    assert_eq!(
        core.query_terminal_background_color(std::time::Duration::from_millis(1))
            .await,
        None
    );

    terminal.send_input("\x1b]11;#ffffff\x07");

    assert!(harness.listener_inputs.borrow().is_empty());
    assert!(harness.component_inputs.borrow().is_empty());

    harness.tui.stop(TuiStopOptions::default());
}
