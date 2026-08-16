//! Scenario 4 — operating a selector.
//!
//! `/model` opens the model selector (`slash-commands.ts:21`, handled in
//! `interactive-mode.ts:5025 showModelSelector`), the arrow keys move its
//! marker, Return picks the row and Escape closes the overlay without a
//! change. The component itself is ported and tested (`tests/model_selector.rs`);
//! what runs here is the path from a typed slash command to the overlay and
//! back.
//!
//! `"Model Name: "` is the marker for "the overlay is open": the selector
//! writes that line under its list for the highlighted row, and nothing else
//! on this screen does. The bare model name would not do — the footer shows it
//! too. Neither would the scope line: `"Scope: "` only exists when the
//! settings hold scoped models (`model-selector.ts:86-95` builds it in that
//! branch and the warning "Only showing models from configured providers." in
//! the other), and the app runtime of this suite has none.

use notagent::config::APP_NAME;

use super::harness::{InteractiveE2e, KEY_ESCAPE, run_local};

/// The detail line the model selector writes for the highlighted row
/// (`model_selector.rs:490`).
const SELECTOR_MARKER: &str = "Model Name: ";

#[tokio::test(flavor = "current_thread")]
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
        // A row carries the model *id*, not its name (`model-selector.ts:266-277`:
        // `${id} [${provider}]` plus the check mark on the current one); the name
        // only stands in the detail line under the list.
        driver.choose(&before.id).await;

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
