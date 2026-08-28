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
