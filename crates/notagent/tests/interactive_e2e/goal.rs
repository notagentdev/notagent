//! `/goal` (port addition, v0.1.21) — goal mode from the user's side.
//!
//! The state machine and the two tools are covered by `core/goal` and
//! `tests/goal_tools.rs`. What runs here is the path a user takes: the typed
//! command reaches the handler, the handler answers on screen, and the session
//! holds what it said it would.
//!
//! `wait_for` searches the accumulated scrollback, so each notice is awaited
//! only on its first appearance; the later steps read the session instead.

use notagent::config::APP_NAME;
use notagent::core::goal::ThreadGoalStatus;

use super::harness::{InteractiveDriver, InteractiveE2e, run_local};

fn goal_status(driver: &InteractiveDriver) -> Option<ThreadGoalStatus> {
    driver.app().session().goal().map(|goal| goal.status)
}

fn objective(driver: &InteractiveDriver) -> Option<String> {
    driver.app().session().goal().map(|goal| goal.objective)
}

#[tokio::test(flavor = "current_thread")]
async fn the_goal_command_sets_pauses_resumes_and_clears_a_goal() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        assert_eq!(goal_status(&driver), None, "no goal to begin with");

        driver.submit("/goal").await;
        driver.wait_for("No goal.").await;

        driver
            .submit("/goal ship the release --turns 4 --tokens 500")
            .await;
        driver
            .wait_for("Goal [active · 0/4 turns · 0/500 tokens]")
            .await;
        assert_eq!(goal_status(&driver), Some(ThreadGoalStatus::Active));
        assert_eq!(objective(&driver).as_deref(), Some("ship the release"));

        driver.submit("/goal pause").await;
        driver.wait_for("Goal paused").await;
        assert_eq!(goal_status(&driver), Some(ThreadGoalStatus::Paused));

        driver.submit("/goal resume").await;
        driver.settle().await;
        assert_eq!(goal_status(&driver), Some(ThreadGoalStatus::Active));

        driver.submit("/goal clear").await;
        driver.wait_for("Goal cleared").await;
        assert_eq!(goal_status(&driver), None);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_second_goal_reports_the_running_one_unless_replace_is_given() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("/goal ship the release").await;
        driver.wait_for("ship the release").await;

        // Without `replace`, a typed goal never discards work in progress.
        driver.submit("/goal something else entirely").await;
        driver.wait_for("Use `/goal replace").await;
        assert_eq!(objective(&driver).as_deref(), Some("ship the release"));

        driver.submit("/goal replace something else entirely").await;
        driver.settle().await;
        assert_eq!(
            objective(&driver).as_deref(),
            Some("something else entirely")
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn strict_is_carried_onto_the_goal_and_an_unusable_argument_is_reported() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("/goal strict ship the release").await;
        driver.wait_for("Goal [active, strict").await;
        assert!(driver.app().session().goal().expect("goal").strict);

        driver.submit("/goal --turns nonsense more work").await;
        driver.wait_for("--turns needs a number").await;
        // The running goal is untouched by an unusable create.
        assert_eq!(objective(&driver).as_deref(), Some("ship the release"));
    })
    .await;
}
