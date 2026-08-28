//! Native behavior tests for the virtual terminal harness.

use notagent_tui::terminal::Terminal;
use notagent_tui::test_terminal::{TerminalEvent, VirtualTerminal};
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
