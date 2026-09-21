mod app_runtime;
#[path = "support/llama_server.rs"]
mod llama_server;

use app_runtime::{HeadlessApp, reply, tool_call_reply};
use notagent::core::settings_manager::{BlockStyle, TuiMode};
use notagent::modes::interactive::interactive_mode::{
    InteractiveModeHandle, InteractiveModeOptions, InteractiveTerminal, create_interactive_mode,
};
use notagent_tui::terminal::TerminalPump;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{RenderLoop, run_until};
use std::time::Duration;

struct ReleaseCommand(String);
impl Drop for ReleaseCommand {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "release");
    }
}

struct Driver {
    renderer: Box<dyn RenderLoop>,
    pump: Box<dyn TerminalPump>,
    terminal: VirtualTerminal,
    run: std::pin::Pin<Box<dyn std::future::Future<Output = i32>>>,
}
impl Driver {
    fn new(app: &HeadlessApp, mode: TuiMode) -> Self {
        let terminal = VirtualTerminal::new(100, 40);
        let InteractiveModeHandle {
            renderer,
            pump,
            run,
            ..
        } = create_interactive_mode(
            app.runtime(),
            InteractiveModeOptions {
                tui_mode: Some(mode),
                terminal: Some(InteractiveTerminal {
                    terminal: Box::new(terminal.clone()),
                    pump: Box::new(terminal.pump_handle()),
                }),
                ..InteractiveModeOptions::default()
            },
        );
        Self {
            renderer,
            pump,
            terminal,
            run,
        }
    }
    async fn step(&mut self) {
        run_until(self.renderer.as_mut(), self.pump.as_mut(), async {
            tokio::select! {
                code = &mut self.run => panic!("mode exited unexpectedly: {code}"),
                () = tokio::time::sleep(Duration::from_millis(25)) => {},
            }
        })
        .await;
    }
    fn screen(&self) -> String {
        self.terminal.get_viewport().join("\n")
    }
    async fn wait(&mut self, text: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !self.screen().contains(text) {
            assert!(
                std::time::Instant::now() < deadline,
                "missing {text}: {}",
                self.screen()
            );
            self.step().await;
        }
    }
    async fn keys(&mut self, text: &str) {
        self.terminal.send_input(text);
        self.step().await;
    }
    async fn submit(&mut self, text: &str) {
        self.keys(text).await;
        self.keys("\r").await;
    }
    async fn change_style(&mut self) {
        self.submit("/settings").await;
        self.wait("Auto-compact").await;
        self.keys("Block style").await;
        self.wait("Block style").await;
        self.keys("\r").await;
        self.keys("\x1b").await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_settings_preserve_output_and_new_retains_dot_in_both_renderers() {
    tokio::task::LocalSet::new().run_until(async {
        for mode in [TuiMode::Regular, TuiMode::Fullscreen] {
            let app = HeadlessApp::create().await;
            app.session().settings_manager().set_block_style(BlockStyle::Badge).unwrap();
            let release = app.path("release-command");
            let _release_on_failure = ReleaseCommand(release.clone());
            app.faux().set_responses(vec![
                reply("DOT_BEFORE"),
                tool_call_reply("bash", "live-dot", serde_json::json!({"command": format!("sleep 0.2; printf 'DOT_%s\n' BEFORE; while [ ! -f '{}' ]; do sleep 0.1; done; printf 'DOT_%s\n' AFTER", release.replace('\'', "'\"'\"'"))})),
                reply("COMMAND_FINISHED"),
            ]);
            let mut driver = Driver::new(&app, mode);
            driver.wait("notagent").await;
            driver.submit("First answer").await;
            driver.wait("DOT_BEFORE").await;
            driver.submit("Run the command").await;
            driver.wait("Bash").await;
            driver.change_style().await;
            assert_eq!(app.session().settings_manager().get_block_style(), BlockStyle::Dot);
            driver.wait("DOT_BEFORE").await;
            assert!(!driver.screen().contains("COMMAND_FINISHED"));
            driver.change_style().await;
            assert_eq!(app.session().settings_manager().get_block_style(), BlockStyle::Badge);
            driver.wait("DOT_BEFORE").await;
            driver.change_style().await;
            assert_eq!(app.session().settings_manager().get_block_style(), BlockStyle::Dot);
            std::fs::write(&release, "release").unwrap();
            driver.wait("COMMAND_FINISHED").await;
            assert!(driver.screen().contains("DOT_AFTER"), "final output must survive restyling: {}", driver.screen());
            driver.submit("/new").await;
            driver.wait("New session started").await;
            assert_eq!(app.session().settings_manager().get_block_style(), BlockStyle::Dot);
            assert!(app.session().messages().is_empty());
            assert!(!driver.terminal.get_writes().contains("notagent:a"));
            driver.terminal.send_input("\x04");
            let code = run_until(driver.renderer.as_mut(), driver.pump.as_mut(), &mut driver.run).await;
            assert_eq!(code, 0);
        }
    }).await;
}
