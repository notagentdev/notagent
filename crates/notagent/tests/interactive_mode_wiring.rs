//! The wiring of the interactive mode, slice by slice.
//!
//! Evidence for the parts of plan task 13 that are on main. It is deliberately
//! narrow: workstream A owns the G3 end-to-end scenarios
//! (`tests/g3_interactive_e2e.rs`), which drive the same entry point through the
//! virtual terminal with the full app runtime. What is checked here is that the
//! seam of interface request A-23 does what it promises — the mode takes a
//! terminal and its pump from outside, hands back a renderer the caller drives,
//! and resolves with an exit code — and that the paths of the current slice
//! reach the screen.
//!
//! Everything below the mode is the production path: the app runtime of the
//! headless G2 suites with only the provider scripted.

mod app_runtime;

use std::time::Duration;

use app_runtime::{HeadlessApp, reply};
use notagent::modes::interactive::interactive_mode::{
    InteractiveModeHandle, InteractiveModeOptions, InteractiveTerminal, create_interactive_mode,
};
use notagent_tui::terminal::TerminalPump;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{RenderLoop, run_until};

const COLUMNS: usize = 80;
const ROWS: usize = 24;

/// Enter — submits the editor line.
const KEY_ENTER: &str = "\r";
/// Ctrl+C — clears the editor, twice in a row exits.
const KEY_CTRL_C: &str = "\x03";
/// Ctrl+D — exits on an empty editor.
const KEY_CTRL_D: &str = "\x04";

/// A running mode together with the pieces the caller drives.
struct Driver {
    renderer: Box<dyn RenderLoop>,
    pump: Box<dyn TerminalPump>,
    terminal: VirtualTerminal,
    run: Option<std::pin::Pin<Box<dyn std::future::Future<Output = i32>>>>,
    exit_code: Option<i32>,
}

impl Driver {
    async fn start(app: &HeadlessApp, terminal: VirtualTerminal) -> Self {
        let InteractiveModeHandle {
            renderer,
            pump,
            run,
        } = create_interactive_mode(
            app.runtime(),
            InteractiveModeOptions {
                terminal: Some(InteractiveTerminal {
                    terminal: Box::new(terminal.clone()),
                    pump: Box::new(terminal.pump_handle()),
                }),
                ..InteractiveModeOptions::default()
            },
        );
        Driver {
            renderer,
            pump,
            terminal,
            run: Some(run),
            exit_code: None,
        }
    }

    /// Run the loop for `millis`, or until the mode exits.
    async fn settle_for(&mut self, millis: u64) {
        if self.exit_code.is_some() {
            return;
        }
        let run = self.run.as_mut().expect("mode still running");
        let outcome = run_until(self.renderer.as_mut(), self.pump.as_mut(), async {
            tokio::select! {
                exit_code = run => Some(exit_code),
                () = tokio::time::sleep(Duration::from_millis(millis)) => None,
            }
        })
        .await;
        if let Some(exit_code) = outcome {
            self.exit_code = Some(exit_code);
            self.run = None;
        }
    }

    /// Run the loop until `needle` shows up on screen, or fail after ten seconds.
    async fn wait_for(&mut self, needle: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if self.screen().contains(needle) {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!("{needle:?} never appeared; screen was:\n{}", self.screen());
            }
            if self.exit_code.is_some() {
                panic!("the mode exited before {needle:?} appeared");
            }
            self.settle_for(25).await;
        }
    }

    async fn send_keys(&mut self, data: &str) {
        self.terminal.send_input(data);
        self.settle_for(30).await;
    }

    async fn submit(&mut self, text: &str) {
        self.send_keys(text).await;
        self.send_keys(KEY_ENTER).await;
    }

    /// Everything the terminal has shown: scrollback plus viewport, because the
    /// transcript scrolls out of an 80×24 viewport quickly.
    fn screen(&self) -> String {
        let mut lines = self.terminal.get_scroll_buffer();
        lines.extend(self.terminal.get_viewport());
        lines.join("\n")
    }

    /// Drive until the mode resolves.
    async fn wait_for_exit(&mut self) -> i32 {
        if let Some(exit_code) = self.exit_code {
            return exit_code;
        }
        let run = self.run.take().expect("mode still running");
        let exit_code = run_until(self.renderer.as_mut(), self.pump.as_mut(), run).await;
        self.exit_code = Some(exit_code);
        exit_code
    }
}

async fn local<F: std::future::Future>(body: F) -> F::Output {
    tokio::task::LocalSet::new().run_until(body).await
}

#[tokio::test(flavor = "current_thread")]
async fn the_startup_screen_shows_the_header_and_the_editor() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;

        driver.wait_for("notagent").await;
        let screen = driver.screen();
        assert!(
            screen.contains("/ commands") && screen.contains("! bash"),
            "the compact startup help is part of the header: {screen}"
        );
        // The editor is focused, so the mode drew its border.
        assert!(
            screen.contains('─'),
            "the editor frame is on screen: {screen}"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_submitted_prompt_reaches_the_provider_and_the_answer_reaches_the_transcript() {
    local(async {
        let app = HeadlessApp::create().await;
        app.faux()
            .set_responses(vec![reply("Answer from the faux provider")]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("Hello there").await;

        driver.wait_for("Answer from the faux provider").await;
        let screen = driver.screen();
        assert!(
            screen.contains("Hello there"),
            "the user message is in the transcript: {screen}"
        );
        assert_eq!(
            app.session().get_last_assistant_text().as_deref(),
            Some("Answer from the faux provider")
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn two_prompts_run_one_after_the_other() {
    local(async {
        let app = HeadlessApp::create().await;
        app.faux()
            .set_responses(vec![reply("First answer"), reply("Second answer")]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("First question").await;
        driver.wait_for("First answer").await;
        driver.submit("Second question").await;
        driver.wait_for("Second answer").await;

        assert_eq!(
            app.session().get_last_assistant_text().as_deref(),
            Some("Second answer")
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn ctrl_c_clears_the_editor_and_a_second_one_exits() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.send_keys("some text").await;
        driver.wait_for("some text").await;
        driver.send_keys(KEY_CTRL_C).await;
        assert!(
            !driver
                .terminal
                .get_viewport()
                .join("\n")
                .contains("some text"),
            "the first Ctrl+C clears the editor"
        );

        driver.send_keys(KEY_CTRL_C).await;
        assert_eq!(driver.wait_for_exit().await, 0);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn ctrl_d_exits_on_an_empty_editor() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.send_keys(KEY_CTRL_D).await;
        assert_eq!(driver.wait_for_exit().await, 0);
    })
    .await;
}
