use notagent_tui::activity::RUNNING_DOT;
use notagent_tui::components::text::Text;
use notagent_tui::stdin_buffer::{StdinBuffer, StdinBufferOptions, StdinEvent};
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{CURSOR_MARKER, Component, Line, RenderLoop};
use notagent_tui::tui::{TuiCore, TuiStopOptions, component_ref};
use notagent_tui::tui_alt_screen::{TuiAltScreen, TuiAltScreenOptions};
use notagent_tui::tui_main_screen::TuiMainScreen;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct CountedDocument {
    lines: Rc<RefCell<Vec<Line>>>,
    renders: Rc<Cell<usize>>,
}

impl Component for CountedDocument {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        self.renders.set(self.renders.get() + 1);
        self.lines.borrow().clone()
    }

    fn invalidate(&mut self) {}
}

struct AnimatedScreen {
    renderer: Box<dyn RenderLoop>,
    terminal: VirtualTerminal,
    lines: Rc<RefCell<Vec<Line>>>,
    renders: Rc<Cell<usize>>,
}

impl AnimatedScreen {
    fn new(fullscreen: bool, history: usize) -> Self {
        let terminal = VirtualTerminal::new(40, 6);
        let renderer: Box<dyn RenderLoop> = if fullscreen {
            let mut screen =
                TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
            screen.start();
            Box::new(screen)
        } else {
            Box::new(TuiMainScreen::new(Box::new(terminal.clone())))
        };
        let mut lines: Vec<Line> = (0..history)
            .map(|index| Line::from(format!("history {index}")))
            .collect();
        if let Some(first) = lines.first_mut() {
            *first = Line::from(format!("{RUNNING_DOT} offscreen"));
        }
        lines.extend([
            Line::from(format!("{RUNNING_DOT} work")),
            Line::from("unchanged body"),
            Line::from(format!("prompt{CURSOR_MARKER}")),
        ]);
        let lines = Rc::new(RefCell::new(lines));
        let renders = Rc::new(Cell::new(0));
        renderer.core().add_child(component_ref(CountedDocument {
            lines: lines.clone(),
            renders: renders.clone(),
        }));
        renderer.core().set_activity_animation(true);
        renderer.core().set_show_hardware_cursor(true);
        let mut result = Self {
            renderer,
            terminal,
            lines,
            renders,
        };
        result.renderer.render_pending_frame();
        result.terminal.clear_writes();
        result
    }

    fn queue_tick(&self) {
        let core = self.renderer.core();
        core.tick_activity(
            core.activity_deadline()
                .expect("a visible marker drives the clock"),
        );
    }
}

#[test]
fn blinking_work_does_not_render_the_transcript_again_or_touch_other_rows() {
    for fullscreen in [false, true] {
        for history in [0, 10_000] {
            let mut screen = AnimatedScreen::new(fullscreen, history);
            let renders = screen.renders.get();
            let cursor = screen.terminal.get_cursor_position();
            let before = screen.terminal.get_scroll_buffer();
            let scrollback_len = before.len().saturating_sub(6);
            for hidden in [true, false, true] {
                screen.queue_tick();
                screen.renderer.render_pending_frame();
                assert_eq!(
                    screen.renders.get(),
                    renders,
                    "blink frames must not traverse the document (fullscreen={fullscreen}, history={history})"
                );
                let viewport = screen.terminal.get_viewport().join("\n");
                assert_eq!(
                    viewport.contains("● work"),
                    !hidden,
                    "wrong blink phase: {viewport}"
                );
                assert!(
                    viewport.contains("work"),
                    "only the dot may blink: {viewport}"
                );
                assert_eq!(
                    screen.terminal.get_cursor_position(),
                    cursor,
                    "blinking must preserve the input cursor"
                );
                let writes = screen.terminal.get_writes();
                assert!(
                    writes.contains("work"),
                    "the visible marker row must be repainted: {writes:?}"
                );
                for forbidden in [
                    "history",
                    "offscreen",
                    "unchanged body",
                    "prompt",
                    "notagent:a",
                    "\x1b[2J",
                    "\x1b[3J",
                ] {
                    assert!(
                        !writes.contains(forbidden),
                        "blink must not replay unrelated content or clear scrollback: {writes:?}"
                    );
                }
                let after = screen.terminal.get_scroll_buffer();
                assert_eq!(
                    &after[..scrollback_len],
                    &before[..scrollback_len],
                    "blink must preserve terminal scrollback"
                );
                screen.terminal.clear_writes();
            }
        }
    }
}

#[test]
fn tool_completion_wins_over_a_queued_blink_in_either_request_order() {
    for fullscreen in [false, true] {
        for content_first in [false, true] {
            let mut screen = AnimatedScreen::new(fullscreen, 20);
            let renders = screen.renders.get();
            let work = screen.lines.borrow().len() - 3;
            screen.lines.borrow_mut()[work] = Line::from("● done");
            if content_first {
                screen.renderer.core().request_render();
            }
            screen.queue_tick();
            if !content_first {
                screen.renderer.core().request_render();
            }
            screen.renderer.render_pending_frame();
            assert!(
                screen.renders.get() > renders,
                "completion must render the new content"
            );
            let viewport = screen.terminal.get_viewport().join("\n");
            assert!(
                viewport.contains("● done"),
                "completion must remain visible: {viewport}"
            );
            assert!(
                !viewport.contains("work"),
                "cached activity must not resurrect a running tool: {viewport}"
            );
            assert!(
                screen.renderer.core().activity_deadline().is_none(),
                "offscreen work must not keep the completed viewport ticking"
            );
        }
    }
}

#[test]
fn resizing_and_focus_changes_do_not_reuse_stale_animation_rows() {
    for fullscreen in [false, true] {
        let mut screen = AnimatedScreen::new(fullscreen, 20);
        let renders = screen.renders.get();
        screen.queue_tick();
        screen.terminal.resize(35, 8);
        screen.renderer.render_pending_frame();
        assert!(
            screen.renders.get() > renders,
            "resize must recompute layout even when only a blink was pending"
        );
        screen.queue_tick();
        screen.renderer.core().handle_terminal_input("\x1b[O");
        screen.renderer.render_pending_frame();
        assert!(screen.renderer.core().activity_deadline().is_none());
        assert!(
            screen.terminal.get_viewport().join("\n").contains("● work"),
            "unfocused tools must have a static visible marker"
        );
        screen.renderer.core().handle_terminal_input("\x1b[I");
        screen.renderer.render_pending_frame();
        assert!(screen.renderer.core().activity_deadline().is_some());
    }
}

#[tokio::test]
async fn the_render_loop_can_animate_without_an_application_tick() {
    let mut screen = AnimatedScreen::new(false, 100);
    let renders = screen.renders.get();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        screen.renderer.core().wait_until_render_due(),
    )
    .await
    .expect("the shared clock must wake a render loop without application events");
    screen.renderer.render_pending_frame();
    assert_eq!(
        screen.renders.get(),
        renders,
        "a timer wake must use the animation path"
    );
    assert!(
        screen.terminal.get_writes().contains("work"),
        "a timer wake must paint the next phase"
    );
    assert!(
        screen.renderer.core().activity_deadline().is_some(),
        "painting must rearm the clock"
    );
}

#[test]
fn an_overlay_opened_during_a_pending_blink_is_painted_and_pauses_animation() {
    for fullscreen in [false, true] {
        let mut screen = AnimatedScreen::new(fullscreen, 20);
        screen.queue_tick();
        let overlay = screen
            .renderer
            .core()
            .show_overlay(component_ref(Text::new("choose an option", 0, 0)), None);
        screen.renderer.render_pending_frame();
        let viewport = screen.terminal.get_viewport().join("\n");
        assert!(
            viewport.contains("choose an option"),
            "a cached blink must not overwrite the overlay: {viewport}"
        );
        assert!(screen.renderer.core().activity_deadline().is_none());
        overlay.hide();
        screen.renderer.render_pending_frame();
        assert!(screen.renderer.core().activity_deadline().is_some());
    }
}

#[test]
#[ignore = "timing harness, run explicitly"]
fn animation_cost_with_short_and_long_transcripts() {
    for fullscreen in [false, true] {
        for history in [100, 10_000] {
            let mut screen = AnimatedScreen::new(fullscreen, history);
            let started = std::time::Instant::now();
            let frames = 100;
            for _ in 0..frames {
                screen.queue_tick();
                screen.renderer.render_pending_frame();
                screen.terminal.clear_writes();
            }
            let blink = started.elapsed() / frames;
            let started = std::time::Instant::now();
            for _ in 0..frames {
                screen.renderer.core().request_render();
                screen.renderer.render_pending_frame();
                screen.terminal.clear_writes();
            }
            let content = started.elapsed() / frames;
            println!(
                "fullscreen={fullscreen}, history={history}: blink={blink:?}, content={content:?}"
            );
        }
    }
}

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
