use notagent_tui::activity::RUNNING_DOT;
use notagent_tui::components::text::Text;
use notagent_tui::stdin_buffer::{StdinBuffer, StdinBufferOptions, StdinEvent};
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{TuiCore, TuiStopOptions, component_ref};
use notagent_tui::tui_alt_screen::{TuiAltScreen, TuiAltScreenOptions};
use notagent_tui::tui_main_screen::TuiMainScreen;

#[test]
fn only_visible_main_screen_markers_schedule_animation() {
    let terminal = VirtualTerminal::new(40, 5);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let core = screen.core().clone();
    core.set_activity_animation(true);
    core.add_child(component_ref(Text::new(
        format!("{RUNNING_DOT} working"),
        0,
        0,
    )));
    screen.render_now(false);
    assert!(core.activity_deadline().is_some());
    assert!(
        core.render_deadline().is_some(),
        "the renderer must wake even without a tool event"
    );
    assert!(
        !terminal.get_writes().contains("notagent:a"),
        "internal markers must never reach the terminal"
    );
    core.add_child(component_ref(Text::new("1\n2\n3\n4\n5\n6", 0, 0)));
    screen.render_now(false);
    assert!(
        core.activity_deadline().is_none(),
        "offscreen markers must not drive the clock"
    );
    screen.stop(TuiStopOptions::default());
}

#[test]
fn fullscreen_focus_and_completion_stop_the_animation_deadline() {
    let terminal = VirtualTerminal::new(40, 6);
    let mut screen = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let core = screen.core().clone();
    core.set_activity_animation(true);
    core.add_child(component_ref(Text::new(
        format!("{RUNNING_DOT} work"),
        0,
        0,
    )));
    screen.start();
    screen.render_now(false);
    assert!(core.activity_deadline().is_some());
    core.handle_terminal_input("\x1b[O");
    screen.render_now(false);
    assert!(core.activity_deadline().is_none());
    assert!(terminal.get_viewport().join("\n").contains("● work"));
    core.handle_terminal_input("\x1b[I");
    screen.render_now(false);
    assert!(core.activity_deadline().is_some());
    core.clear();
    core.add_child(component_ref(Text::new("● done", 0, 0)));
    screen.render_now(false);
    assert!(core.activity_deadline().is_none());
    screen.stop(TuiStopOptions::default());
    assert!(!terminal.get_writes().contains("notagent:a"));
}

#[test]
fn fragmented_focus_reports_are_consumed_but_paste_content_is_preserved() {
    let core = TuiCore::new(Box::new(VirtualTerminal::new(40, 5)));
    let mut buffer = StdinBuffer::new(StdinBufferOptions::default());
    assert!(buffer.process("\x1b[").is_empty());
    for event in buffer.process("O") {
        if let StdinEvent::Data(data) = event {
            core.handle_terminal_input(&data);
        }
    }
    assert!(!core.terminal_focused());
    let events = buffer.process("\x1b[200~\x1b[I\x1b[201~");
    assert_eq!(events, vec![StdinEvent::Paste("\x1b[I".to_owned())]);
    assert!(
        !core.terminal_focused(),
        "pasted escape text must not become a focus event"
    );
}
