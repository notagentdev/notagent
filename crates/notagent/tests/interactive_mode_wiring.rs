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

#[path = "support/llama_server.rs"]
mod llama_server;

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
/// Escape — closes an overlay or a selector.
const KEY_ESCAPE: &str = "\x1b";

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
        Self::start_with(
            app,
            terminal,
            notagent::core::settings_manager::TuiMode::Regular,
        )
        .await
    }

    async fn start_with(
        app: &HeadlessApp,
        terminal: VirtualTerminal,
        tui_mode: notagent::core::settings_manager::TuiMode,
    ) -> Self {
        Self::start_with_options(
            app,
            terminal,
            InteractiveModeOptions {
                tui_mode: Some(tui_mode),
                ..InteractiveModeOptions::default()
            },
        )
        .await
    }

    /// Start with everything but the terminal chosen by the caller.
    async fn start_with_options(
        app: &HeadlessApp,
        terminal: VirtualTerminal,
        options: InteractiveModeOptions,
    ) -> Self {
        let InteractiveModeHandle {
            renderer,
            pump,
            run,
            ..
        } = create_interactive_mode(
            app.runtime(),
            InteractiveModeOptions {
                terminal: Some(InteractiveTerminal {
                    terminal: Box::new(terminal.clone()),
                    pump: Box::new(terminal.pump_handle()),
                }),
                ..options
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

    /// Run the loop until `needle` is gone from the viewport.
    async fn wait_until_absent(&mut self, needle: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if !self.terminal.get_viewport().join("\n").contains(needle) {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!(
                    "{needle:?} never disappeared; screen was:\n{}",
                    self.screen()
                );
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

// ---------------------------------------------------------------------------
// Slice 2 — the slash command dispatch
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn session_and_hotkeys_and_changelog_print_their_blocks() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("/session").await;
        driver.wait_for("Session Info").await;

        driver.submit("/hotkeys").await;
        driver.wait_for("Keyboard Shortcuts").await;

        driver.submit("/changelog").await;
        driver.wait_for("What's New").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn name_sets_the_session_name_and_reports_it() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("/name release-notes").await;
        driver.wait_for("Session name set: release-notes").await;
        assert_eq!(
            app.session()
                .with_session_manager(|manager| manager.get_session_name()),
            Some("release-notes".to_owned())
        );

        driver.submit("/name").await;
        driver.wait_for("Session name: release-notes").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn copy_without_an_answer_reports_that_there_is_nothing_to_copy() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("/copy").await;
        driver.wait_for("No agent messages to copy yet.").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn export_writes_the_session_to_the_given_jsonl_path() {
    local(async {
        let app = HeadlessApp::create().await;
        app.faux().set_responses(vec![reply("Exported answer")]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;
        driver.submit("A question").await;
        driver.wait_for("Exported answer").await;

        let target = app.path("session-export.jsonl");
        driver.submit(&format!("/export {target}")).await;
        driver.wait_for("Session exported to:").await;

        let exported = std::fs::read_to_string(&target).expect("exported file");
        assert!(
            exported.contains("Exported answer"),
            "the export carries the transcript: {exported}"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn quit_ends_the_mode() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("/quit").await;
        assert_eq!(driver.wait_for_exit().await, 0);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_unknown_slash_word_is_no_command_and_reaches_the_model() {
    local(async {
        let app = HeadlessApp::create().await;
        app.faux().set_responses(vec![reply("Answer to the slash")]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("/not-a-command").await;
        driver.wait_for("Answer to the slash").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn new_starts_a_fresh_session() {
    local(async {
        let app = HeadlessApp::create().await;
        app.faux()
            .set_responses(vec![reply("First session answer")]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;
        driver.submit("A question").await;
        driver.wait_for("First session answer").await;
        let first_session_id = app.session().session_id();

        driver.submit("/new").await;
        driver.wait_for("New session started").await;
        assert_ne!(app.runtime().session().session_id(), first_session_id);
    })
    .await;
}

// ---------------------------------------------------------------------------
// Slice 3 — bash mode and the queues
// ---------------------------------------------------------------------------

/// Alt+Enter — `app.message.followUp`.
const KEY_ALT_ENTER: &str = "\x1b\r";

#[tokio::test(flavor = "current_thread")]
async fn a_bash_command_runs_and_its_output_lands_in_the_transcript() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("!echo hello-from-bash").await;

        driver.wait_for("hello-from-bash").await;
        assert!(
            app.session().messages().iter().any(|message| matches!(
                message,
                notagent_agent::types::AgentMessage::BashExecution(bash)
                    if bash.command == "echo hello-from-bash" && bash.exclude_from_context != Some(true)
            )),
            "the run is recorded in the session: {:#?}",
            app.session().messages()
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_double_bang_keeps_the_output_out_of_the_context() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("!!echo excluded-output").await;

        driver.wait_for("excluded-output").await;
        assert!(
            app.session().messages().iter().any(|message| matches!(
                message,
                notagent_agent::types::AgentMessage::BashExecution(bash)
                    if bash.command == "echo excluded-output" && bash.exclude_from_context == Some(true)
            )),
            "the run is recorded as excluded: {:#?}",
            app.session().messages()
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn alt_enter_queues_a_follow_up_while_the_agent_runs() {
    local(async {
        // A provider slow enough that the second message arrives mid-run.
        let app = HeadlessApp::create_slow(20.0).await;
        app.faux().set_responses(vec![
            reply("A long first answer that streams token by token for a while"),
            reply("The queued follow-up answer"),
        ]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("First question").await;
        driver.wait_for("Working").await;

        driver.send_keys("a follow-up question").await;
        driver.send_keys(KEY_ALT_ENTER).await;
        driver.wait_for("Follow-up: a follow-up question").await;

        driver.wait_for("The queued follow-up answer").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_dequeue_action_puts_the_queued_messages_back_into_the_editor() {
    local(async {
        let app = HeadlessApp::create_slow(20.0).await;
        app.faux().set_responses(vec![
            reply("A long first answer that streams token by token for a while"),
            reply("Second answer"),
        ]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("First question").await;
        driver.wait_for("Working").await;
        driver.send_keys("queued text").await;
        driver.send_keys(KEY_ALT_ENTER).await;
        driver.wait_for("Follow-up: queued text").await;

        // `app.message.dequeue` is Alt+Up by default.
        driver.send_keys("\x1b[1;3A").await;
        driver.wait_for("Restored 1 queued message to editor").await;
        driver.wait_until_absent("Follow-up: queued text").await;
    })
    .await;
}

// ---------------------------------------------------------------------------
// Slice 4 — the selectors
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn the_model_selector_opens_and_escape_closes_it() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("/model").await;
        // The faux provider is the only configured one, so its model is the
        // single row of the list.
        driver.wait_for("faux-1").await;

        driver.send_keys(KEY_ESCAPE).await;
        driver.wait_until_absent("Model Name").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_model_command_with_an_exact_reference_switches_the_model() {
    local(async {
        let app = HeadlessApp::create().await;
        let model = app.session().model().expect("a model is configured");
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver
            .submit(&format!("/model {}/{}", model.provider, model.id))
            .await;

        driver.wait_for(&format!("Model: {}", model.id)).await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_settings_selector_opens_and_escape_closes_it() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("/settings").await;
        driver.wait_for("Auto-compact").await;

        driver.send_keys(KEY_ESCAPE).await;
        driver.wait_until_absent("Auto-compact").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn fork_opens_the_message_selector_and_forks_to_a_new_session() {
    local(async {
        let app = HeadlessApp::create().await;
        app.faux()
            .set_responses(vec![reply("An answer to fork at")]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;
        driver.submit("A question to fork from").await;
        driver.wait_for("An answer to fork at").await;
        let first_session_id = app.session().session_id();

        driver.submit("/fork").await;
        driver.wait_for("Fork from Message").await;
        driver.send_keys(KEY_ENTER).await;

        driver.wait_for("Forked to new session").await;
        assert_ne!(app.runtime().session().session_id(), first_session_id);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn trust_opens_the_trust_selector() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("/trust").await;
        driver.wait_for("Project trust").await;

        driver.send_keys(KEY_ESCAPE).await;
        driver.wait_until_absent("Project trust").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn resume_opens_the_session_selector_and_lists_this_session() {
    local(async {
        let app = HeadlessApp::create().await;
        app.faux().set_responses(vec![reply("A first answer")]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;
        driver.submit("A question").await;
        driver.wait_for("A first answer").await;

        driver.submit("/resume").await;
        // The loads run in the loop; the header appears once the list loaded.
        driver.wait_for("Resume Session").await;

        driver.send_keys(KEY_ESCAPE).await;
        driver.wait_until_absent("Resume Session").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn tree_opens_the_navigation_selector() {
    local(async {
        let app = HeadlessApp::create().await;
        app.faux()
            .set_responses(vec![reply("An answer in the tree")]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;
        driver.submit("A question").await;
        driver.wait_for("An answer in the tree").await;

        driver.submit("/tree").await;
        driver.wait_for("Session Tree").await;

        driver.send_keys(KEY_ESCAPE).await;
        driver.wait_until_absent("Session Tree").await;
    })
    .await;
}

// ---------------------------------------------------------------------------
// Slice 5 (part) — the key actions
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn ctrl_o_toggles_the_tool_output_expansion() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        // `app.tools.expand` is Ctrl+O; it also expands the startup header.
        driver.send_keys("\x0f").await;
        driver.wait_for("Tool output: expanded").await;
        driver.send_keys("\x0f").await;
        driver.wait_for("Tool output: collapsed").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_double_escape_on_an_empty_editor_opens_the_configured_view() {
    local(async {
        let app = HeadlessApp::create().await;
        app.faux()
            .set_responses(vec![reply("An answer to fork at")]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;
        driver.submit("A question").await;
        driver.wait_for("An answer to fork at").await;

        // The default action is the session tree.
        driver.send_keys(KEY_ESCAPE).await;
        driver.send_keys(KEY_ESCAPE).await;
        driver.wait_for("Session Tree").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_mode_can_start_on_the_alternate_screen() {
    local(async {
        let app = HeadlessApp::create().await;
        app.faux()
            .set_responses(vec![reply("An answer on the alt screen")]);
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start_with(
            &app,
            terminal,
            notagent::core::settings_manager::TuiMode::Fullscreen,
        )
        .await;

        driver.wait_for("notagent").await;
        driver.submit("A question").await;
        driver.wait_for("An answer on the alt screen").await;
        // The dock keeps the editor and the footer at the bottom.
        let viewport = driver.terminal.get_viewport().join("\n");
        assert!(
            viewport.contains('─'),
            "the editor frame is on the alternate screen: {viewport}"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn scoped_models_opens_the_model_scope_selector() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("/scoped-models").await;
        // The footer also carries the model name, so the assertion uses a
        // marker only the selector draws.
        driver.wait_for("Model Configuration").await;

        driver.send_keys(KEY_ESCAPE).await;
        driver.wait_until_absent("Model Configuration").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn tasks_opens_the_task_browser() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("/tasks").await;
        driver.wait_for("Tasks").await;

        driver.send_keys(KEY_ESCAPE).await;
        driver.wait_until_absent("Tasks").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn login_opens_the_sign_in_method_selector() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.submit("/login").await;
        driver.wait_for("How do you want to sign in?").await;

        driver.send_keys(KEY_ESCAPE).await;
        driver
            .wait_until_absent("How do you want to sign in?")
            .await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn logout_lists_the_stored_credentials() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        // The app runtime stores an api key for the faux provider.
        driver.submit("/logout").await;
        driver.wait_for("Select provider to logout").await;

        driver.send_keys(KEY_ESCAPE).await;
        driver.wait_until_absent("Select provider to logout").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_llama_command_runs_and_asks_for_credentials_when_there_are_none() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        // Without a credential the manager does not open; it says how to get one.
        driver.send_keys("/llama").await;
        driver.send_keys(KEY_ENTER).await;
        driver
            .wait_for("Configure llama.cpp with /login llama.cpp")
            .await;

        // The editor is back and usable.
        driver.submit("/quit").await;
        assert_eq!(driver.wait_for_exit().await, 0);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_services_register_the_native_llama_provider() {
    local(async {
        let app = HeadlessApp::create().await;
        let services = app.runtime().services();
        assert!(
            services
                .model_runtime
                .get_registered_provider_ids()
                .contains(&"llama.cpp".to_owned()),
            "`registerNativeProvider` of the built-in llama extension happens natively"
        );
        assert!(
            services
                .model_runtime
                .get_provider("llama.cpp")
                .is_some_and(|provider| provider.is_dynamic()),
            "the registered provider is the llama.cpp one, catalog and all"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_llama_command_opens_the_manager_and_gives_the_editor_back() {
    local(async {
        // A router with one unloaded model; the manager only lists it here.
        let server = llama_server::TestHttpServer::start(|request| {
            let path = request
                .path
                .split('?')
                .next()
                .unwrap_or_default()
                .to_owned();
            match path.as_str() {
                "/models/sse" => llama_server::Reply::Sse,
                "/models" => llama_server::Reply::json(serde_json::json!({
                    "data": [{ "id": "router-model", "status": { "value": "unloaded" } }]
                })),
                _ => llama_server::Reply::status(404),
            }
        })
        .await;

        let app = HeadlessApp::create_with_llama(&server.base_url).await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        driver.send_keys("/llama").await;
        driver.send_keys(KEY_ENTER).await;
        driver.wait_for("llama.cpp models").await;
        let screen = driver.screen();
        assert!(
            screen.contains("router-model") && screen.contains("Download model…"),
            "the manager took the editor's place: {screen}"
        );

        // Escape closes the manager; the editor comes back and takes input.
        driver.send_keys(KEY_ESCAPE).await;
        driver.wait_until_absent("Download model…").await;

        driver.submit("/quit").await;
        assert_eq!(driver.wait_for_exit().await, 0);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_reload_that_finds_project_resources_writes_the_implicit_trust_decision() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        // The session started in a folder without trust-requiring resources, so
        // it was trusted implicitly; `main.ts` hands that cwd to the mode.
        let mut driver = Driver::start_with_options(
            &app,
            terminal,
            InteractiveModeOptions {
                auto_trust_on_reload_cwd: Some(app.cwd()),
                ..InteractiveModeOptions::default()
            },
        )
        .await;
        driver.wait_for("notagent").await;

        // The reload finds a project settings file that was not there before.
        let config = std::path::Path::new(&app.cwd()).join(".notagent");
        std::fs::create_dir_all(&config).expect("config dir");
        std::fs::write(config.join("settings.json"), "{}").expect("settings");

        driver.submit("/reload").await;
        driver.wait_for("saved project trust").await;

        let trust =
            std::fs::read_to_string(std::path::Path::new(&app.agent_dir()).join("trust.json"))
                .expect("the trust file was written");
        assert!(
            trust.contains(&app.cwd()),
            "the implicit decision names the project: {trust}"
        );

        // The implicit decision is written once: with the cwd forgotten, a
        // second reload does not write it again.
        let trust_file = std::path::Path::new(&app.agent_dir()).join("trust.json");
        std::fs::remove_file(&trust_file).expect("removes");
        driver.submit("/reload").await;
        driver.settle_for(300).await;
        assert!(
            !trust_file.exists(),
            "the decision is not written a second time"
        );

        driver.submit("/quit").await;
        assert_eq!(driver.wait_for_exit().await, 0);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn importing_a_session_whose_cwd_is_gone_offers_the_current_one() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let mut driver = Driver::start(&app, terminal).await;
        driver.wait_for("notagent").await;

        // The fixture records `/Users/badlogic/workspaces/pi-mono` as its cwd,
        // which does not exist here — the case the dialog is for.
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/before-compaction.jsonl");
        driver
            .submit(&format!("/import {}", fixture.display()))
            .await;
        driver.wait_for("Import session").await;
        driver.send_keys(KEY_ENTER).await;

        driver.wait_for("Session cwd not found").await;
        let screen = driver.screen();
        assert!(
            screen.contains("cwd from session file does not exist")
                && screen.contains("/Users/badlogic/workspaces/pi-mono"),
            "the dialog names both directories: {screen}"
        );

        // "Yes" continues in the current cwd.
        driver.send_keys(KEY_ENTER).await;
        driver.wait_for("Session imported from:").await;

        driver.submit("/quit").await;
        assert_eq!(driver.wait_for_exit().await, 0);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_shutdown_signal_of_the_binary_ends_the_session_with_zero() {
    local(async {
        let app = HeadlessApp::create().await;
        let terminal = VirtualTerminal::new(COLUMNS, ROWS);
        let shutdown = tokio_util::sync::CancellationToken::new();
        let mut driver = Driver::start_with_options(
            &app,
            terminal,
            InteractiveModeOptions {
                shutdown_signal: Some(shutdown.clone()),
                ..InteractiveModeOptions::default()
            },
        )
        .await;
        driver.wait_for("notagent").await;

        // What the SIGTERM/SIGHUP task of `main_app` does.
        shutdown.cancel();
        assert_eq!(driver.wait_for_exit().await, 0);
    })
    .await;
}
