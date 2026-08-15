//! Port of `packages/coding-agent/src/cli/startup-ui.ts`.
//!
//! The dialogs that run before the interactive mode exists: first-time setup,
//! the startup selector and the session picker's screen. What is ported here is
//! the decision half — whether first-time setup runs at all — because it is
//! reached from the headless paths too.
//!
//! The dialog half needs a render loop over `ProcessTerminal`, and the terminal
//! lives inside the TUI core once it is handed over; there is no seam to pump it
//! from the outside yet (interface request C-14). It therefore lands with the
//! interactive main loop in plan task 13, which is where that loop is built.
//!
//! Deviation (class 2): `showStartupInput` and its `ExtensionInputComponent`
//! went with the extension system — the only caller was the extension half of
//! the project-trust context (`plans/facts/extension-boundary.md` §3).

use std::path::Path;

use crate::config::{APP_NAME, CONFIG_DIR_NAME, PACKAGE_NAME, env_agent_dir, get_settings_path};
use crate::core::experimental::are_experimental_features_enabled;

const OFFICIAL_PACKAGE_NAME: &str = "@notagent/coding-agent";
const OFFICIAL_APP_NAME: &str = "notagent";
const OFFICIAL_CONFIG_DIR_NAME: &str = ".notagent";

/// Whether this build is the official distribution rather than a fork.
///
/// Deviation (class 1): the arguments are explicit instead of read from the
/// module constants, because a Rust test cannot swap a `const` the way the
/// TypeScript suite mocks the config module.
pub fn is_official_distribution(package_name: &str, app_name: &str, config_dir_name: &str) -> bool {
    package_name == OFFICIAL_PACKAGE_NAME
        && app_name == OFFICIAL_APP_NAME
        && config_dir_name == OFFICIAL_CONFIG_DIR_NAME
}

/// First-time setup runs when all of these hold:
/// - this is the official distribution (not a fork or rebrand)
/// - experimental features are on (`NOTAGENT_EXPERIMENTAL=1`)
/// - the default agent directory is in use (no override)
/// - setup never completed before (no `settings.json`)
pub fn should_run_first_time_setup(settings_path: Option<&Path>) -> bool {
    if !is_official_distribution(PACKAGE_NAME, APP_NAME, CONFIG_DIR_NAME) {
        return false;
    }
    if !are_experimental_features_enabled() {
        return false;
    }
    if std::env::var(env_agent_dir()).is_ok_and(|value| !value.is_empty()) {
        return false;
    }
    match settings_path {
        Some(settings_path) => !settings_path.exists(),
        None => !get_settings_path().exists(),
    }
}
