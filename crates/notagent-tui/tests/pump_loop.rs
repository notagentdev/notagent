//! The pump seam of the render loop (interface request C-14).
//!
//! There is no TS counterpart to port: Node's event loop delivers stdin,
//! SIGWINCH and the buffer timeouts by itself, so `createStartupTui` only hands
//! the terminal to `TuiMainScreen` and calls `start()`
//! (`packages/coding-agent/src/cli/startup-ui.ts:74-90`). These cases pin the
//! properties that substitute for it: input reaches the handler even when the
//! handler writes back into the terminal, a parked loop wakes on a render
//! request, and a cancelled `pump()` loses nothing.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use notagent_tui::terminal::{ProcessTerminal, PumpResult, Terminal, TerminalPump};
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, Line, TuiCore, component_ref, run_until, shared_lines};
use notagent_tui::tui_main_screen::TuiMainScreen;

struct TextComponent(Rc<RefCell<Vec<String>>>);

impl Component for TextComponent {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        shared_lines(self.0.borrow().clone())
    }

    fn invalidate(&mut self) {}
}

/// The case C-14 reports: after `TuiCore::new` the terminal lives inside the
/// core, and the input handler writes back into it. The pump must therefore
/// hold no borrow while it dispatches.
#[tokio::test(flavor = "current_thread")]
async fn dispatches_input_while_the_handler_writes_to_the_terminal() {
    let writes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&writes);
    let (terminal, mut pump) = ProcessTerminal::with_writer(Box::new(move |data| {
        sink.borrow_mut().push(data.to_string())
    }))
    .into_shared();
    terminal.begin_keyboard_protocol_negotiation();

    // The handler re-enters the terminal, like an overlay hiding the cursor.
    let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let seen_handler = Rc::clone(&seen);
    let mut echo = terminal.clone_handle();
    terminal.set_input_handler(Box::new(move |data| {
        seen_handler.borrow_mut().push(data.to_string());
        echo.write("\x1b[?25l");
        assert!(echo.columns() > 0);
    }));

    assert_eq!(pump.feed_stdin("a"), PumpResult::Input);

    assert_eq!(seen.borrow().as_slice(), ["a"]);
    assert!(writes.borrow().contains(&"\x1b[?25l".to_string()));
}

/// A cancelled `pump()` — every `tokio::select!` round drops one — must return
/// the stdin channel, otherwise the next call reports EOF.
#[tokio::test(flavor = "current_thread")]
async fn a_cancelled_pump_keeps_the_stdin_channel() {
    let (terminal, mut pump) = ProcessTerminal::with_writer(Box::new(|_| {})).into_shared();
    terminal.begin_keyboard_protocol_negotiation();
    let stdin = terminal.attach_test_stdin();

    let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let seen_handler = Rc::clone(&seen);
    terminal.set_input_handler(Box::new(move |data| {
        seen_handler.borrow_mut().push(data.to_string());
    }));

    // Cancel a waiting pump.
    tokio::select! {
        _ = pump.pump() => panic!("nothing was sent"),
        () = tokio::time::sleep(std::time::Duration::from_millis(5)) => {}
    }

    stdin.send(b"b".to_vec()).expect("channel is open");
    assert_eq!(pump.pump().await, PumpResult::Input);
    assert_eq!(seen.borrow().as_slice(), ["b"]);
}

/// Without a stdin channel there is nothing to wait for.
#[tokio::test(flavor = "current_thread")]
async fn reports_eof_without_a_stdin_channel() {
    let (_terminal, mut pump) = ProcessTerminal::with_writer(Box::new(|_| {})).into_shared();
    assert_eq!(pump.pump().await, PumpResult::Eof);
}

/// `wait_until_render_due` replaces `scheduleRender()`'s `setTimeout`: it parks
/// while nothing is requested and returns once a frame is due.
#[tokio::test(flavor = "current_thread")]
async fn wait_until_render_due_parks_until_a_frame_is_requested() {
    // The TUI is single-threaded (`!Send`), so its tasks need a `LocalSet`.
    tokio::task::LocalSet::new()
        .run_until(async {
            let core = TuiCore::new(Box::new(VirtualTerminal::new(40, 10)));

            tokio::select! {
                () = core.wait_until_render_due() => panic!("nothing was requested"),
                () = tokio::time::sleep(std::time::Duration::from_millis(5)) => {}
            }

            let waiter = core.clone();
            let woken = Rc::new(Cell::new(false));
            let flag = Rc::clone(&woken);
            let task = tokio::task::spawn_local(async move {
                waiter.wait_until_render_due().await;
                flag.set(true);
            });
            tokio::task::yield_now().await;
            assert!(!woken.get());

            core.request_render();
            task.await.expect("waiter finished");
            assert!(woken.get());
        })
        .await;
}

/// A request that arrives while the waiter is parked must wake it — the
/// `Notify` has to be registered before the deadline is read (see C-15).
#[tokio::test(flavor = "current_thread")]
async fn a_request_during_the_wait_wakes_the_loop() {
    // The TUI is single-threaded (`!Send`), so its tasks need a `LocalSet`.
    tokio::task::LocalSet::new()
        .run_until(async {
            let core = TuiCore::new(Box::new(VirtualTerminal::new(40, 10)));
            let requester = core.clone();
            tokio::task::spawn_local(async move {
                tokio::task::yield_now().await;
                requester.request_render();
            });

            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                core.wait_until_render_due(),
            )
            .await
            .expect("the request woke the loop");
        })
        .await;
}

/// Input outranks painting.
///
/// A component that asks for another frame from inside its own render — which
/// is what an animated status row does — leaves a frame due on every pass. A
/// loop that painted before it read would then never get to stdin, and a
/// keypress would wait for a lull that a streaming response never has. This is
/// the case behind Escape arriving long after it was pressed.
#[tokio::test(flavor = "current_thread")]
async fn input_is_read_even_while_a_frame_is_always_due() {
    struct AlwaysRepainting {
        core: TuiCore,
        rendered: Rc<Cell<usize>>,
    }

    impl Component for AlwaysRepainting {
        fn render(&mut self, _width: usize) -> Vec<Line> {
            self.rendered.set(self.rendered.get() + 1);
            // The next frame is due before this one is on screen.
            self.core.request_render();
            shared_lines(vec!["working".to_string()])
        }

        fn invalidate(&mut self) {}
    }

    tokio::task::LocalSet::new()
        .run_until(async {
            let terminal = VirtualTerminal::new(20, 4);
            let mut pump = terminal.pump_handle();
            let mut ui = TuiMainScreen::new(Box::new(terminal.clone()));

            let rendered = Rc::new(Cell::new(0));
            ui.core().add_child(component_ref(AlwaysRepainting {
                core: ui.core().clone(),
                rendered: Rc::clone(&rendered),
            }));

            let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
            let done_tx = Rc::new(RefCell::new(Some(done_tx)));
            ui.core().add_input_listener(Box::new(move |data: &str| {
                if data == "\x1b"
                    && let Some(sender) = done_tx.borrow_mut().take()
                {
                    let _ = sender.send(());
                }
                Default::default()
            }));
            ui.start();

            let sender = terminal.clone();
            tokio::task::spawn_local(async move {
                // Long enough for the loop to have painted many frames first.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                sender.send_input("\x1b");
            });

            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                run_until(&mut ui, &mut pump, done_rx),
            )
            .await
            .expect("the key was read while frames kept coming")
            .expect("the listener saw it");

            assert!(
                rendered.get() > 1,
                "the loop really was painting throughout: {} frames",
                rendered.get()
            );
        })
        .await;
}

/// The loop of `startStartupTui` plus an awaited dialog: input arrives, the
/// frame is rendered without the test driving it, and the result comes back.
///
/// Like the TS original the loop returns as soon as the dialog resolves — the
/// promise continuation runs before the pending `setTimeout` of
/// `scheduleRender()` — so the checked frame is the one before the last key.
#[tokio::test(flavor = "current_thread")]
async fn run_until_renders_and_returns_the_dialog_result() {
    // The TUI is single-threaded (`!Send`), so its tasks need a `LocalSet`.
    tokio::task::LocalSet::new()
        .run_until(async {
            let terminal = VirtualTerminal::new(20, 4);
            let mut pump = terminal.pump_handle();
            let lines = Rc::new(RefCell::new(vec!["before".to_string()]));
            let mut ui = TuiMainScreen::new(Box::new(terminal.clone()));

            let (done_tx, done_rx) = tokio::sync::oneshot::channel::<String>();
            let done_tx = Rc::new(RefCell::new(Some(done_tx)));
            let component_lines = Rc::clone(&lines);
            ui.core()
                .add_child(component_ref(TextComponent(Rc::clone(&lines))));
            // Like a component: change the content, then ask for a frame — the
            // TS core does not schedule one on input either
            // (`packages/tui/src/tui.ts:820-897`).
            let listener_core = ui.core().clone();
            ui.core().add_input_listener(Box::new(move |data: &str| {
                if data == "\r" {
                    if let Some(sender) = done_tx.borrow_mut().take() {
                        let _ = sender.send(component_lines.borrow()[0].clone());
                    }
                } else {
                    component_lines.borrow_mut()[0] = format!("got {data}");
                    listener_core.request_render();
                }
                Default::default()
            }));
            ui.start();

            let sender = terminal.clone();
            tokio::task::spawn_local(async move {
                tokio::task::yield_now().await;
                sender.send_input("x");
                // Long enough for the loop to pass the 16 ms render throttle.
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                sender.send_input("\r");
            });

            let result = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                run_until(&mut ui, &mut pump, done_rx),
            )
            .await
            .expect("the loop returned")
            .expect("the dialog resolved");

            assert_eq!(result, "got x");
            assert!(
                terminal.get_viewport()[0].starts_with("got x"),
                "viewport: {:?}",
                terminal.get_viewport()
            );
        })
        .await;
}
