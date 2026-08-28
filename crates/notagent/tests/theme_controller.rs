use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use notagent::core::settings_manager::{SettingsManager, SettingsManagerCreateOptions};
use notagent::modes::interactive::theme::theme::theme;
use notagent::modes::interactive::theme::theme_controller::InteractiveThemeController;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui_main_screen::TuiMainScreen;

/// The theme registry is a process global, so the cases run one at a time.
fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

struct Harness {
    _directory: tempfile::TempDir,
    settings: Arc<SettingsManager>,
    _tui: TuiMainScreen,
    controller: InteractiveThemeController,
    errors: Arc<Mutex<Vec<String>>>,
    changes: Arc<Mutex<usize>>,
}

fn harness(theme_setting: Option<&str>) -> Harness {
    let directory = tempfile::Builder::new()
        .prefix("notagent-theme-controller-")
        .tempdir()
        .expect("temp dir");
    let agent_dir = directory.path().join("agent");
    let project_dir = directory.path().join("project");
    std::fs::create_dir_all(&agent_dir).expect("creates");
    std::fs::create_dir_all(project_dir.join(".notagent")).expect("creates");
    if let Some(setting) = theme_setting {
        std::fs::write(
            agent_dir.join("settings.json"),
            serde_json::json!({ "theme": setting }).to_string(),
        )
        .expect("writes");
    }

    let settings = Arc::new(SettingsManager::create(
        &project_dir,
        Some(&agent_dir),
        SettingsManagerCreateOptions::default(),
    ));
    let tui = TuiMainScreen::new(Box::new(VirtualTerminal::new(80, 24)));

    let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let changes = Arc::new(Mutex::new(0usize));
    let error_sink = Arc::clone(&errors);
    let change_sink = Arc::clone(&changes);
    let controller = InteractiveThemeController::new(
        tui.core().clone(),
        Arc::clone(&settings),
        Box::new(move |message: &str| error_sink.lock().unwrap().push(message.to_string())),
        Box::new(move || *change_sink.lock().unwrap() += 1),
    );

    Harness {
        _directory: directory,
        settings,
        _tui: tui,
        controller,
        errors,
        changes,
    }
}

#[test]
fn the_constructor_installs_the_theme_the_setting_names() {
    let _guard = theme_lock();
    let harness = harness(Some("light"));

    assert_eq!(theme().name.as_deref(), Some("light"));
    assert!(harness.errors.lock().unwrap().is_empty());
}

#[test]
fn setting_a_theme_by_name_swaps_the_instance_and_reports_the_change() {
    let _guard = theme_lock();
    let harness = harness(Some("light"));

    let result = harness.controller.set_theme_name("dark", true);

    assert!(result.success, "{:?}", result.error);
    assert_eq!(theme().name.as_deref(), Some("dark"));
    assert_eq!(*harness.changes.lock().unwrap(), 1);
    // `setThemeName` does not write the settings — the caller does.
    assert_eq!(harness.settings.get_theme().as_deref(), Some("light"));
}

#[test]
fn an_unknown_theme_name_reports_an_error_and_keeps_the_current_theme() {
    let _guard = theme_lock();
    let harness = harness(Some("dark"));

    let result = harness.controller.set_theme_name("no-such-theme", true);

    assert!(!result.success);
    assert_eq!(theme().name.as_deref(), Some("dark"));
    assert_eq!(
        harness.errors.lock().unwrap().len(),
        1,
        "the error goes to showError: {:?}",
        harness.errors.lock().unwrap()
    );
}

#[test]
fn a_preview_changes_the_instance_but_not_the_settings() {
    let _guard = theme_lock();
    let harness = harness(Some("dark"));

    harness.controller.preview("light");

    assert_eq!(theme().name.as_deref(), Some("light"));
    assert_eq!(
        harness.settings.get_theme().as_deref(),
        Some("dark"),
        "a preview never persists"
    );
}

#[test]
fn a_preview_of_an_unknown_name_falls_back_to_the_active_theme() {
    let _guard = theme_lock();
    let harness = harness(Some("dark"));

    // `resolveThemeSetting` answers `undefined`, so the controller previews
    // `activeThemeName` — the theme stays where it is, without an error.
    harness.controller.preview("");

    assert_eq!(theme().name.as_deref(), Some("dark"));
    assert!(harness.errors.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "current_thread")]
// The guard serialises the process-global theme registry; the runtime is
// single-threaded, so nothing else can run while it is held.
#[allow(clippy::await_holding_lock)]
async fn apply_from_settings_takes_a_fixed_setting_as_it_is() {
    let _guard = theme_lock();
    let harness = harness(Some("light"));
    harness.controller.set_theme_name("dark", true);

    harness.controller.apply_from_settings().await;

    assert_eq!(theme().name.as_deref(), Some("light"));
    assert_eq!(
        harness.controller.get_terminal_theme(),
        notagent::modes::interactive::theme::theme::TerminalTheme::Dark,
        "no automatic setting, so the terminal was never asked"
    );
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::await_holding_lock)]
async fn an_automatic_setting_picks_the_half_that_matches_the_terminal() {
    let _guard = theme_lock();
    // `light/dark` is the shape `parseAutoThemeSetting` accepts. The virtual
    // terminal answers no color-scheme query, so the detection falls back to
    // the environment default (dark) and the dark half wins.
    let harness = harness(Some("light/dark"));

    harness.controller.apply_from_settings().await;

    assert_eq!(theme().name.as_deref(), Some("dark"));
}
