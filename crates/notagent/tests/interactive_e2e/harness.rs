//! The harness the G3 scenarios drive: the app against the virtual terminal.
//!
//! Two halves that already exist meet here. [`crate::app_runtime::HeadlessApp`]
//! is the whole app runtime of the G2 suites — real model runtime with the faux
//! provider registered natively, real services, a session out of
//! `create_agent_session_from_services`; only the provider is scripted. The
//! other half is `notagent_tui::test_terminal::VirtualTerminal` behind feature
//! `test-terminal` (interface request A-2) together with the pump seam of A-20:
//! `run_until(ui, pump, until)` drives rendering and input while a scenario
//! waits for something to appear on screen.
//!
//! The third piece is the entry point of the interactive mode, on main since
//! C's task 13: [`InteractiveE2e::start`] hands the terminal and its pump to
//! `create_interactive_mode` (the seam of A-23) and drives the mode's future on
//! this loop. A scenario whose wiring is not in C's current slice still carries
//! `#[ignore]` with the piece it waits for.
//!
//! [`InteractiveDriver::from_parts`] takes the same three pieces from anywhere,
//! so `harness_check` exercises every driver method today against a plain
//! `TuiMainScreen` — the harness itself is under test even while the scenarios
//! wait.

#![allow(dead_code)]

use std::future::Future;
use std::time::{Duration, Instant};

use notagent_ai::providers::faux::FauxCore;
use notagent_tui::terminal::TerminalPump;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{RenderLoop, run_until};

use notagent::modes::interactive::interactive_mode::{
    InteractiveModeOptions, InteractiveTerminal, create_interactive_mode,
};

use crate::app_runtime::HeadlessApp;

/// How long [`InteractiveDriver::wait_for`] keeps the loop running before it
/// gives up. Generous on purpose: a scenario that waits for a scripted answer
/// waits for a whole agent turn.
const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// The default terminal of a scenario. 80×24 is what `ProcessTerminal` falls
/// back to without a size (`packages/tui/src/terminal.ts`).
pub const DEFAULT_COLUMNS: usize = 80;
pub const DEFAULT_ROWS: usize = 24;

/// Return — submits the editor line.
pub const KEY_ENTER: &str = "\r";
/// Escape — closes an overlay.
pub const KEY_ESCAPE: &str = "\x1b";
pub const KEY_DOWN: &str = "\x1b[B";
pub const KEY_UP: &str = "\x1b[A";
/// Ctrl+C — interrupt, twice in a row exits.
pub const KEY_CTRL_C: &str = "\x03";

/// The marker in front of the selected row. Both list components draw it:
/// `SelectList` writes `"→ "` itself (`packages/tui/src/components/select-list.ts`,
/// ported in `select_list.rs:201`) and `SettingsList` takes it from the theme
/// (`get_settings_list_theme().cursor`).
pub const LIST_CURSOR: &str = "→ ";

/// Run a scenario body on a `LocalSet`.
///
/// Everything in the TUI is `!Send` (interface request A-2), so a scenario that
/// starts background work reaches for `spawn_local`, and that needs a
/// `LocalSet` in scope. `#[tokio::test(flavor = "current_thread")]` alone does
/// not provide one.
pub async fn run_local<F: Future>(body: F) -> F::Output {
    tokio::task::LocalSet::new().run_until(body).await
}

/// An app and a terminal, before the interactive mode joins them.
pub struct InteractiveE2e {
    app: HeadlessApp,
    terminal: VirtualTerminal,
}

impl InteractiveE2e {
    /// A scenario on the default 80×24 terminal.
    pub async fn new() -> Self {
        Self::with_size(DEFAULT_COLUMNS, DEFAULT_ROWS).await
    }

    pub async fn with_size(columns: usize, rows: usize) -> Self {
        InteractiveE2e {
            app: HeadlessApp::create().await,
            terminal: VirtualTerminal::new(columns, rows),
        }
    }

    /// The scripted provider. Arrange the answers before [`Self::start`].
    pub fn faux(&self) -> &FauxCore {
        self.app.faux()
    }

    pub fn app(&self) -> &HeadlessApp {
        &self.app
    }

    /// Another handle on the same terminal — `VirtualTerminal` is an `Rc`
    /// handle, so the scenario and the TUI share one emulator.
    pub fn terminal(&self) -> VirtualTerminal {
        self.terminal.clone()
    }

    /// A path inside the project directory of this run.
    pub fn path(&self, name: &str) -> String {
        self.app.path(name)
    }

    /// Hand the terminal to the interactive mode and return the driver.
    ///
    /// The seam of A-23, delivered with C's task 13: `create_interactive_mode`
    /// takes the terminal and its pump instead of building a `ProcessTerminal`
    /// (`InteractiveTerminal`, the port of the optional `terminal` of
    /// `createInteractiveTui`, `interactive-mode.ts:344-354`), and hands back
    /// the renderer, the pump and the mode's future — the three pieces
    /// [`InteractiveDriver`] needs.
    pub async fn start(self) -> InteractiveDriver {
        let InteractiveE2e { app, terminal } = self;
        let handle = create_interactive_mode(
            app.runtime(),
            InteractiveModeOptions {
                terminal: Some(InteractiveTerminal {
                    terminal: Box::new(terminal.clone()),
                    pump: Box::new(terminal.pump_handle()),
                }),
                ..InteractiveModeOptions::default()
            },
        );
        InteractiveDriver::from_parts(handle.renderer, handle.pump, terminal)
            .with_app(app)
            .with_exit(handle.run)
    }
}

/// Drives a running interactive screen: send keys, wait for output, resize.
///
/// The renderer is a trait object because the mode picks it
/// (`TuiMainScreen` for `tuiMode: "regular"`, `TuiAltScreen` for
/// `"fullscreen"` — `interactive-mode.ts:352-365`), and both implement
/// `RenderLoop`.
pub struct InteractiveDriver {
    ui: Box<dyn RenderLoop>,
    pump: Box<dyn TerminalPump>,
    terminal: VirtualTerminal,
    /// Kept alive for the lifetime of the driver: the app owns the temporary
    /// project directory the session works in.
    app: Option<HeadlessApp>,
    /// The running mode, spawned on the `LocalSet` of [`run_local`]: it has to
    /// make progress whenever the driver awaits anything, because the mode
    /// initialises, reacts to input and answers the session inside this future
    /// while the loop only renders and pumps.
    exit: Option<tokio::task::JoinHandle<i32>>,
}

impl InteractiveDriver {
    pub fn from_parts(
        ui: Box<dyn RenderLoop>,
        pump: Box<dyn TerminalPump>,
        terminal: VirtualTerminal,
    ) -> Self {
        InteractiveDriver {
            ui,
            pump,
            terminal,
            app: None,
            exit: None,
        }
    }

    #[must_use]
    pub fn with_app(mut self, app: HeadlessApp) -> Self {
        self.app = Some(app);
        self
    }

    /// Start the mode beside the loop.
    ///
    /// It runs as a local task rather than something the driver awaits itself:
    /// a scenario spends its time in [`Self::settle`], and the mode has to keep
    /// running there — it is what puts the header on screen, hands input to the
    /// components and drives a prompt. `LocalSet::run_until` polls the task
    /// whenever the scenario awaits, so both halves advance together.
    /// [`Self::wait_for_exit`] then waits for its exit code.
    #[must_use]
    pub fn with_exit(mut self, exit: impl Future<Output = i32> + 'static) -> Self {
        self.exit = Some(tokio::task::spawn_local(exit));
        self
    }

    /// The app runtime behind the screen — the session, the scripted provider
    /// and the project directory.
    pub fn app(&self) -> &HeadlessApp {
        self.app
            .as_ref()
            .expect("the driver was built without an app runtime")
    }

    pub fn faux(&self) -> &FauxCore {
        self.app().faux()
    }

    pub fn terminal(&self) -> VirtualTerminal {
        self.terminal.clone()
    }

    /// Render and pump until `until` resolves — the loop of A-20.
    pub async fn run_until<F: Future>(&mut self, until: F) -> F::Output {
        run_until(&mut *self.ui, &mut *self.pump, until).await
    }

    /// Keep the loop running until the mode returns, and give back its exit
    /// code — how a scenario ends that quits with `/quit` or Ctrl+C twice.
    pub async fn wait_for_exit(&mut self) -> i32 {
        let exit = self
            .exit
            .take()
            .expect("the driver was built without the mode's exit future");
        self.run_until(exit)
            .await
            .expect("the interactive mode panicked")
    }

    /// Ask for a frame — what a component does after it changed.
    pub fn request_render(&self) {
        self.ui.core().request_render();
    }

    /// A second handle on the shared TUI state, for work that runs beside the
    /// loop and has to ask for a frame when it is done (A-20: the mode keeps
    /// its own handle for exactly this reason).
    pub fn core_handle(&self) -> notagent_tui::tui::TuiCore {
        self.ui.core().clone()
    }

    /// Let the throttled render pipeline run out, so the screen shows
    /// everything that is due (the TS suites' `await terminal.waitForRender()`).
    pub async fn settle(&mut self) {
        let terminal = self.terminal.clone();
        self.run_until(async move { terminal.wait_for_render().await })
            .await;
    }

    /// Type at the terminal. The input reaches the focused component the same
    /// way a keystroke does, and the screen settles afterwards.
    pub async fn send_keys(&mut self, data: &str) {
        self.terminal.send_input(data);
        self.settle().await;
    }

    /// A line of text followed by Return.
    pub async fn submit(&mut self, text: &str) {
        self.send_keys(text).await;
        self.send_keys("\r").await;
    }

    /// Move the selection of an open list onto the row that holds `label` and
    /// confirm it with Return.
    ///
    /// Both lists wrap around at the end, so walking down reaches every row.
    pub async fn choose(&mut self, label: &str) {
        const MAX_STEPS: usize = 64;
        for _ in 0..MAX_STEPS {
            if self.selected_row().is_some_and(|row| row.contains(label)) {
                self.send_keys(KEY_ENTER).await;
                return;
            }
            self.send_keys(KEY_DOWN).await;
        }
        panic!(
            "no row containing {label:?} took the selection within {MAX_STEPS} steps.\n\
             --- screen ---\n{}",
            self.screen()
        );
    }

    /// The row the open list has selected, marker included.
    pub fn selected_row(&self) -> Option<String> {
        self.viewport()
            .into_iter()
            .find(|line| line.contains(LIST_CURSOR))
    }

    /// Resize the terminal, which fires the resize handler of the TUI.
    pub async fn resize(&mut self, columns: usize, rows: usize) {
        self.terminal.resize(columns, rows);
        self.settle().await;
    }

    /// The visible screen, one string per row.
    pub fn viewport(&self) -> Vec<String> {
        self.terminal.flush_and_get_viewport()
    }

    /// The visible screen as one string.
    pub fn screen(&self) -> String {
        self.viewport().join("\n")
    }

    /// The whole buffer including scrollback — where the transcript of the main
    /// screen renderer ends up once it grows past the viewport.
    pub fn scrollback(&self) -> String {
        self.terminal.get_scroll_buffer().join("\n")
    }

    /// The cursor as `(column, row)` in the viewport.
    pub fn cursor(&self) -> (usize, usize) {
        self.terminal.get_cursor_position()
    }

    /// Everything the TUI wrote, escape sequences included — for the cases that
    /// pin a control sequence rather than a glyph (alternate screen, title).
    pub fn writes(&self) -> String {
        self.terminal.get_writes()
    }

    /// Keep the loop running until `needle` is on screen (scrollback included).
    ///
    /// Returns the buffer it matched in. Panics with the screen dump on
    /// timeout, which is the failure mode that tells a scenario what actually
    /// happened.
    pub async fn wait_for(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            self.settle().await;
            let buffer = self.scrollback();
            if buffer.contains(needle) {
                return buffer;
            }
            if Instant::now() >= deadline {
                panic!(
                    "{:?} never appeared within {:?}.\n--- screen ---\n{}\n--- buffer ---\n{}",
                    needle,
                    WAIT_TIMEOUT,
                    self.screen(),
                    buffer
                );
            }
        }
    }

    /// Keep the loop running until `needle` is on screen, across the
    /// renderer's hard wraps.
    ///
    /// A long absolute path does not fit into 80 columns, and the main screen
    /// breaks it over two rows (`…/does-not-e` + `xist.txt`), so the literal
    /// needle is nowhere in the buffer although the user plainly reads it. This
    /// searches the rows joined back together; use it only where the wrap is
    /// the point, and [`Self::wait_for`] everywhere else.
    pub async fn wait_for_across_wraps(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            self.settle().await;
            let joined = self.joined_rows();
            if joined.contains(needle) {
                return joined;
            }
            if Instant::now() >= deadline {
                panic!(
                    "{:?} never appeared within {:?}, wraps included.\n--- screen ---\n{}",
                    needle,
                    WAIT_TIMEOUT,
                    self.screen()
                );
            }
        }
    }

    /// Every row of the buffer joined without separators, padding and the
    /// one-column indent of the transcript removed — the inverse of the
    /// renderer's hard wrap.
    pub fn joined_rows(&self) -> String {
        self.scrollback()
            .lines()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("")
    }

    /// Keep the loop running until `needle` is gone — a closed overlay, a
    /// finished spinner.
    pub async fn wait_until_gone(&mut self, needle: &str) {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            self.settle().await;
            if !self.scrollback().contains(needle) {
                return;
            }
            if Instant::now() >= deadline {
                panic!(
                    "{:?} was still on screen after {:?}.\n--- screen ---\n{}",
                    needle,
                    WAIT_TIMEOUT,
                    self.screen()
                );
            }
        }
    }

    /// Assert on what is on screen right now, without waiting.
    pub fn assert_shows(&self, needle: &str) {
        let buffer = self.scrollback();
        assert!(
            buffer.contains(needle),
            "{needle:?} is not on screen.\n--- buffer ---\n{buffer}"
        );
    }

    pub fn assert_hides(&self, needle: &str) {
        let buffer = self.scrollback();
        assert!(
            !buffer.contains(needle),
            "{needle:?} is on screen but should not be.\n--- buffer ---\n{buffer}"
        );
    }

    /// No rendered line is wider than the terminal — the property a resize has
    /// to restore, and the one a wrapped line breaks.
    pub fn assert_fits(&self, columns: usize) {
        for (row, line) in self.viewport().iter().enumerate() {
            let width = unicode_width::UnicodeWidthStr::width(line.as_str());
            assert!(
                width <= columns,
                "row {row} is {width} cells wide in a {columns}-column terminal: {line:?}"
            );
        }
    }
}
