use notagent::config::APP_NAME;

use super::harness::{InteractiveE2e, run_local};

#[tokio::test(flavor = "current_thread")]
async fn the_bash_filter_command_switches_the_gate_and_persists_it() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        let enabled = |driver: &super::harness::InteractiveDriver| {
            driver
                .app()
                .session()
                .settings_manager()
                .get_bash_filter_enabled()
        };

        assert!(!enabled(&driver), "the bash filter is off by default");

        driver.submit("/bash-filter on").await;
        driver.wait_for("Bash filter: enabled").await;
        assert!(enabled(&driver));

        driver.submit("/bash-filter off").await;
        driver.wait_for("Bash filter: disabled").await;
        assert!(!enabled(&driver));

        // No argument toggles, as the reference's own switch does.
        driver.submit("/bash-filter").await;
        driver.settle().await;
        assert!(enabled(&driver), "an argument-less /bash-filter toggles");

        // An unusable argument is reported and changes nothing.
        driver.submit("/bash-filter sometimes").await;
        driver
            .wait_for("Unknown /bash-filter argument: sometimes (use on|off)")
            .await;
        assert!(enabled(&driver));
    })
    .await;
}
