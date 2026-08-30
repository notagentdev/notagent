use notagent::config::APP_NAME;
use notagent::modes::interactive::theme::theme::{ThemeColor, theme};

use super::harness::{InteractiveE2e, run_local};

#[tokio::test(flavor = "current_thread")]
async fn the_settings_menu_switches_the_theme_and_repaints_the_screen() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        let before = theme();
        let before_paint = driver.writes().len();

        driver.submit("/settings").await;
        // "Auto-compact" is the first row of the settings list, so it says the
        // menu is open. The theme row is the 22nd of 28 and therefore below the
        // visible window — `choose` walks the selection down to it, which
        // scrolls the list the way a user does.
        driver.wait_for("Auto-compact").await;
        driver.choose("Theme").await;

        // The submenu lists the theme names; "light" is built in, so it is
        // there whatever the resource loader found.
        driver.wait_for("light").await;
        driver.choose("light").await;

        // The instance the components draw through is a different one now …
        let after = theme();
        assert_eq!(after.name.as_deref(), Some("light"));
        assert_ne!(
            before.get_fg_ansi(ThemeColor::Text),
            after.get_fg_ansi(ThemeColor::Text),
            "the light theme paints text differently than {:?}",
            before.name
        );

        // … the choice survives in the settings …
        assert_eq!(
            driver
                .app()
                .session()
                .settings_manager()
                .get_theme()
                .as_deref(),
            Some("light")
        );

        // … and the screen was redrawn afterwards.
        assert!(
            driver.writes().len() > before_paint,
            "nothing was written after the theme changed"
        );
    })
    .await;
}
