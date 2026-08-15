//! Port of `packages/coding-agent/test/first-time-setup.test.ts` and of
//! `test/first-time-setup-fork.test.ts`.
//!
//! The fork suite mocks the config module to rebrand the package; Rust cannot
//! swap a `const`, so it calls `is_official_distribution` with the forked name
//! instead (deviation class 1, documented in `cli/startup_ui.rs`).

use notagent::cli::startup_ui::{is_official_distribution, should_run_first_time_setup};
use notagent::config::{APP_NAME, CONFIG_DIR_NAME, PACKAGE_NAME, env_agent_dir};
use notagent::core::settings_manager::SettingsManager;

/// The suites mutate process-wide environment variables, so they run one after
/// another rather than in parallel.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct EnvGuard {
    agent_dir: Option<String>,
    experimental: Option<String>,
}

impl EnvGuard {
    fn new() -> Self {
        let guard = EnvGuard {
            agent_dir: std::env::var(env_agent_dir()).ok(),
            experimental: std::env::var("NOTAGENT_EXPERIMENTAL").ok(),
        };
        unsafe {
            std::env::set_var("NOTAGENT_EXPERIMENTAL", "1");
            std::env::remove_var(env_agent_dir());
        }
        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.agent_dir {
                Some(value) => std::env::set_var(env_agent_dir(), value),
                None => std::env::remove_var(env_agent_dir()),
            }
            match &self.experimental {
                Some(value) => std::env::set_var("NOTAGENT_EXPERIMENTAL", value),
                None => std::env::remove_var("NOTAGENT_EXPERIMENTAL"),
            }
        }
    }
}

#[test]
fn returns_true_when_experimental_default_agent_dir_and_no_settings_json() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _guard = EnvGuard::new();
    let temp = tempfile::tempdir().expect("temp dir");
    let settings_path = temp.path().join("settings.json");

    assert!(should_run_first_time_setup(Some(&settings_path)));
}

#[test]
fn returns_false_when_experimental_features_are_disabled() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _guard = EnvGuard::new();
    let temp = tempfile::tempdir().expect("temp dir");
    let settings_path = temp.path().join("settings.json");
    unsafe {
        std::env::remove_var("NOTAGENT_EXPERIMENTAL");
    }

    assert!(!should_run_first_time_setup(Some(&settings_path)));
}

#[test]
fn returns_false_when_a_custom_agent_dir_is_set() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _guard = EnvGuard::new();
    let temp = tempfile::tempdir().expect("temp dir");
    let settings_path = temp.path().join("settings.json");
    unsafe {
        std::env::set_var(env_agent_dir(), temp.path());
    }

    assert!(!should_run_first_time_setup(Some(&settings_path)));
}

#[test]
fn returns_false_when_settings_json_already_exists() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _guard = EnvGuard::new();
    let temp = tempfile::tempdir().expect("temp dir");
    let settings_path = temp.path().join("settings.json");
    std::fs::write(&settings_path, "{}").expect("write settings");

    assert!(!should_run_first_time_setup(Some(&settings_path)));
}

#[test]
fn returns_false_for_a_forked_package() {
    assert!(!is_official_distribution(
        "@example/notagent-coding-agent",
        APP_NAME,
        CONFIG_DIR_NAME
    ));
    assert!(is_official_distribution(
        PACKAGE_NAME,
        APP_NAME,
        CONFIG_DIR_NAME
    ));
}

// ---------------------------------------------------------------------------
// analytics settings (same TypeScript file)
// ---------------------------------------------------------------------------

#[test]
fn defaults_to_disabled_with_no_tracking_identifier() {
    let manager = SettingsManager::in_memory(&Default::default(), Default::default());

    assert!(!manager.get_enable_analytics());
    assert_eq!(manager.get_tracking_id(), None);
}

#[test]
fn generates_a_tracking_identifier_on_opt_in() {
    let manager = SettingsManager::in_memory(&Default::default(), Default::default());

    manager.set_enable_analytics(true);

    assert!(manager.get_enable_analytics());
    let tracking_id = manager.get_tracking_id().expect("tracking id");
    assert_eq!(tracking_id.len(), 36);
    assert!(
        tracking_id
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
    );
}

#[test]
fn does_not_generate_a_tracking_identifier_on_opt_out() {
    let manager = SettingsManager::in_memory(&Default::default(), Default::default());

    manager.set_enable_analytics(false);

    assert!(!manager.get_enable_analytics());
    assert_eq!(manager.get_tracking_id(), None);
}

#[test]
fn keeps_the_tracking_identifier_when_toggling_analytics() {
    let manager = SettingsManager::in_memory(&Default::default(), Default::default());

    manager.set_enable_analytics(true);
    let tracking_id = manager.get_tracking_id();
    manager.set_enable_analytics(false);
    manager.set_enable_analytics(true);

    assert_eq!(manager.get_tracking_id(), tracking_id);
}
