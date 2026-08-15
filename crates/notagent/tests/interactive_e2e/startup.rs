//! Scenario 1 — startup.
//!
//! What the master plan asks for: the interactive mode comes up over the
//! virtual terminal and paints its screen. The evidence is the built-in header
//! (`interactive-mode.ts:925-977`: the logo `APP_NAME v<version>` plus the
//! compact hint line, shown unless `quietStartup` is set — it defaults to
//! `false`) and an editor that has the focus, which shows by the typed text
//! appearing on screen.

use notagent::config::APP_NAME;

use super::harness::{InteractiveE2e, KEY_CTRL_C, run_local};

#[tokio::test(flavor = "current_thread")]
#[ignore = "waits for C task 13: the interactive-mode entry point and its terminal seam (A-23)"]
async fn paints_the_header_and_takes_input_in_the_editor() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;

        // The header of a non-quiet startup.
        driver.wait_for(APP_NAME).await;
        driver.assert_shows("commands");
        driver.assert_shows("bash");

        // The editor has the focus: what is typed lands on screen and stays
        // there until it is submitted.
        driver.send_keys("hello from the harness").await;
        driver.assert_shows("hello from the harness");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "waits for C task 13: the interactive-mode entry point and its terminal seam (A-23)"]
async fn puts_the_terminal_into_raw_mode_and_hands_it_back_on_exit() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        // `Terminal::start` turns bracketed paste on (`terminal.ts`), which is
        // the visible half of taking the terminal over.
        assert!(
            driver.writes().contains("\x1b[?2004h"),
            "bracketed paste was never enabled: {:?}",
            driver.writes()
        );

        // Ctrl+C twice is the documented way out of an empty editor
        // (`interactive-mode.ts`, hint "to exit").
        driver.send_keys(KEY_CTRL_C).await;
        driver.send_keys(KEY_CTRL_C).await;

        assert_eq!(driver.wait_for_exit().await, 0);
        assert!(
            driver.writes().contains("\x1b[?2004l"),
            "the terminal was never handed back: {:?}",
            driver.writes()
        );
    })
    .await;
}
