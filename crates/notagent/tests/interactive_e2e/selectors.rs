//! Scenario 4 — operating a selector.
//!
//! `/model` opens the model selector (`slash-commands.ts:21`, handled in
//! `interactive-mode.ts:5025 showModelSelector`), the arrow keys move its
//! marker, Return picks the row and Escape closes the overlay without a
//! change. The component itself is ported and tested (`tests/model_selector.rs`);
//! what runs here is the path from a typed slash command to the overlay and
//! back.
//!
//! `"Scope: "` is the marker for "the overlay is open": the model selector
//! draws that line above its list, and nothing else on the screen does. The
//! model name alone would not do — the footer shows it too.

use notagent::config::APP_NAME;

use super::harness::{InteractiveE2e, KEY_ESCAPE, run_local};

/// The scope line of the model selector (`model_selector.rs`, `"Scope: "`).
const SELECTOR_MARKER: &str = "Scope: ";

#[tokio::test(flavor = "current_thread")]
#[ignore = "waits for C task 13: the interactive-mode entry point and its terminal seam (A-23)"]
async fn slash_model_opens_the_selector_and_escape_closes_it() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        let model = driver
            .app()
            .session()
            .model()
            .expect("the session runs on the faux model");

        driver.submit("/model").await;

        // The selector lists the models of the registered provider, the marker
        // sits on a row.
        driver.wait_for(SELECTOR_MARKER).await;
        driver.assert_shows(&model.name);
        assert!(
            driver.selected_row().is_some(),
            "no row carries the selection marker.\n--- screen ---\n{}",
            driver.screen()
        );

        driver.send_keys(KEY_ESCAPE).await;

        // The overlay is gone and the session kept its model.
        driver.wait_until_gone(SELECTOR_MARKER).await;
        assert_eq!(
            driver.app().session().model().map(|model| model.id),
            Some(model.id)
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "waits for C task 13: the interactive-mode entry point and its terminal seam (A-23)"]
async fn picking_a_row_closes_the_selector_and_keeps_the_session_on_that_model() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;
        let before = driver
            .app()
            .session()
            .model()
            .expect("the session runs on the faux model");

        driver.submit("/model").await;
        driver.wait_for(SELECTOR_MARKER).await;
        driver.choose(&before.name).await;

        // The faux provider offers one model, so picking a row lands on the
        // model the session already runs on; what the case pins is that the
        // selector answers Return at all and hands the screen back.
        driver.wait_until_gone(SELECTOR_MARKER).await;
        assert_eq!(
            driver.app().session().model().map(|model| model.id),
            Some(before.id)
        );
    })
    .await;
}
