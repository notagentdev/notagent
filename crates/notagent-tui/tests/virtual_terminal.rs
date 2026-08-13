//! Abnahme des Test-Harness (WS-A-Plan Task 1).
//!
//! `tests/fixtures/virtual-terminal-oracle.json` enthält das von
//! `@xterm/headless` 5.5.0 gerenderte Bild für genau die Sequenzen, die
//! `TuiMainScreen` und `TuiAltScreen` emittieren. Der Rust-Harness muss es
//! reproduzieren: Viewport, Scrollback, Cursorposition, Resize, CSI/OSC/APC
//! inklusive Synchronized-Output-Passthrough.

use notagent_tui::terminal::Terminal;
use notagent_tui::test_terminal::{TerminalEvent, VirtualTerminal};
use serde_json::Value;

/// Szenarien, die der Emulator bewusst anders behandelt (siehe PARITY.md).
const DOCUMENTED_DIVERGENCES: &[(&str, &str)] = &[
    (
        "autowrap-off",
        "vt100 kennt DECAWM (CSI ?7l) nicht; nicht beobachtbar, weil der \
         Alt-Screen vor jeder Zeile absolut positioniert und der Main-Screen \
         Zeilen ≤ Terminalbreite garantiert",
    ),
    (
        "emoji",
        "vt100 rechnet Emoji mit Zellbreite 2 (wie graphemeWidth der TUI), \
         @xterm/headless mit Unicode-V6-Tabellen 1",
    ),
];

fn oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/virtual-terminal-oracle.json"))
        .expect("fixture is valid JSON")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect()
}

#[test]
fn reproduces_xterm_js_rendering_for_renderer_sequences() {
    let data = oracle();
    let mut checked = 0;
    for (name, case) in data.as_object().expect("object") {
        if DOCUMENTED_DIVERGENCES.iter().any(|(n, _)| n == name) {
            continue;
        }
        let cols = case["cols"].as_u64().expect("cols") as usize;
        let rows = case["rows"].as_u64().expect("rows") as usize;

        let terminal = if name == "resize" {
            let terminal = VirtualTerminal::new(10, 3);
            terminal.clone().write("hello\r\nworld");
            terminal.resize(cols, rows);
            terminal
        } else {
            let terminal = VirtualTerminal::new(cols, rows);
            for chunk in case["writes"].as_array().expect("writes") {
                terminal.clone().write(chunk.as_str().expect("chunk"));
            }
            terminal
        };

        assert_eq!(
            terminal.get_viewport(),
            strings(&case["viewport"]),
            "Viewport weicht ab: {name}"
        );
        assert_eq!(
            terminal.get_scroll_buffer(),
            strings(&case["scrollback"]),
            "Scrollback weicht ab: {name}"
        );
        assert_eq!(
            terminal.get_cursor_position(),
            (
                case["cursor"]["x"].as_u64().expect("x") as usize,
                case["cursor"]["y"].as_u64().expect("y") as usize
            ),
            "Cursorposition weicht ab: {name}"
        );
        checked += 1;
    }
    assert_eq!(checked, 21, "unerwartete Zahl geprüfter Szenarien");
}

#[test]
fn records_start_write_and_stop_events() {
    let mut terminal = VirtualTerminal::new(10, 3);
    terminal.start(Box::new(|_| {}), Box::new(|| {}));
    terminal.write("hi");
    terminal.stop();

    assert_eq!(
        terminal.events(),
        vec![
            TerminalEvent::Start,
            TerminalEvent::Write("\x1b[?2004h".to_string()),
            TerminalEvent::Write("hi".to_string()),
            TerminalEvent::Stop,
            TerminalEvent::Write("\x1b[?2004l".to_string()),
        ]
    );
    assert_eq!(terminal.get_writes(), "\x1b[?2004hhi\x1b[?2004l");
    terminal.clear_writes();
    assert_eq!(terminal.get_writes(), "");
}

#[test]
fn forwards_input_and_resize_to_the_handlers() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let received: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let resizes = Rc::new(RefCell::new(0));

    let mut terminal = VirtualTerminal::new(10, 3);
    {
        let received = Rc::clone(&received);
        let resizes = Rc::clone(&resizes);
        terminal.start(
            Box::new(move |data| received.borrow_mut().push(data.to_string())),
            Box::new(move || *resizes.borrow_mut() += 1),
        );
    }

    terminal.send_input("\x1b[A");
    terminal.resize(20, 5);

    assert_eq!(received.borrow().as_slice(), ["\x1b[A".to_string()]);
    assert_eq!(*resizes.borrow(), 1);
    assert_eq!(terminal.columns(), 20);
    assert_eq!(terminal.rows(), 5);
}

#[test]
fn terminal_helpers_emit_the_expected_sequences() {
    let mut terminal = VirtualTerminal::new(10, 3);
    terminal.move_by(2);
    terminal.move_by(-3);
    terminal.move_by(0);
    terminal.hide_cursor();
    terminal.show_cursor();
    terminal.clear_line();
    terminal.clear_from_cursor();
    terminal.clear_screen();
    terminal.set_title("t");

    assert_eq!(
        terminal.get_writes(),
        "\x1b[2B\x1b[3A\x1b[?25l\x1b[?25h\x1b[K\x1b[J\x1b[2J\x1b[H\x1b]0;t\x07"
    );
    assert!(terminal.kitty_protocol_active());
}

#[test]
fn clear_and_reset_drop_the_screen_contents() {
    let terminal = VirtualTerminal::new(8, 2);
    terminal.clone().write("abc\r\ndef");
    assert_eq!(terminal.get_viewport(), vec!["abc", "def"]);

    terminal.clear();
    assert_eq!(terminal.get_viewport(), vec!["", ""]);

    terminal.clone().write("x");
    terminal.reset();
    assert_eq!(terminal.get_viewport(), vec!["", ""]);
}

#[tokio::test]
async fn wait_for_render_settles_the_throttled_pipeline() {
    let terminal = VirtualTerminal::new(8, 2);
    terminal.clone().write("ab");
    terminal.wait_for_render().await;
    assert_eq!(terminal.flush_and_get_viewport(), vec!["ab", ""]);
}
