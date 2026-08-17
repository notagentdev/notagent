//! `/leases` (port addition, v0.1.19) — the command form of the reference's
//! Atomic leases setting.
//!
//! The store and the two-phase protocol are covered in
//! `core/tools/file_lease.rs`, and the three mutating tools are covered in
//! their own suites. What runs here is the path a user takes: the typed
//! command reaches the handler, the handler answers on screen and the choice
//! lands in the settings the tools read their gate from.
//!
//! `wait_for` searches the accumulated scrollback, so each notice is awaited
//! only on its first appearance; the later steps read the setting instead.

use notagent::config::APP_NAME;

use super::harness::{InteractiveE2e, run_local};

#[tokio::test(flavor = "current_thread")]
async fn the_leases_command_switches_the_gate_and_persists_it() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        let enabled = |driver: &super::harness::InteractiveDriver| {
            driver
                .app()
                .session()
                .settings_manager()
                .get_atomic_leases_enabled()
        };

        assert!(!enabled(&driver), "atomic leases are off by default");

        driver.submit("/leases on").await;
        driver.wait_for("Atomic file leases: enabled").await;
        assert!(enabled(&driver));

        driver.submit("/leases off").await;
        driver.wait_for("Atomic file leases: disabled").await;
        assert!(!enabled(&driver));

        // No argument toggles, as the reference's own switch does.
        driver.submit("/leases").await;
        driver.settle().await;
        assert!(enabled(&driver), "an argument-less /leases toggles");

        // An unusable argument is reported and changes nothing.
        driver.submit("/leases sometimes").await;
        driver
            .wait_for("Unknown /leases argument: sometimes (use on|off)")
            .await;
        assert!(enabled(&driver));
    })
    .await;
}
