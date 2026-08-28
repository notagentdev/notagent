use std::time::Duration;

use crate::core::remote_catalog_provider::urlencode;
use crate::core::settings_manager::SettingsManager;
use crate::utils::notagent_user_agent::get_pi_user_agent;

const REPORT_INSTALL_URL: &str = "https://notagent.dev/api/report-install";
const INSTALL_PING_TIMEOUT: Duration = Duration::from_secs(5);

/// `isTruthyEnvFlag(value)`
/// opt-out rather than as an unset variable.
fn is_truthy_env_flag(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    value == "1" || value.to_lowercase() == "true" || value.to_lowercase() == "yes"
}

/// `isInstallTelemetryEnabled(settingsManager, telemetryEnv)`
pub fn is_install_telemetry_enabled(settings_manager: &SettingsManager) -> bool {
    is_install_telemetry_enabled_with_env(
        settings_manager,
        std::env::var("NOTAGENT_TELEMETRY").ok().as_deref(),
    )
}

/// The same with the environment value passed explicitly, mirroring the second
pub fn is_install_telemetry_enabled_with_env(
    settings_manager: &SettingsManager,
    telemetry_env: Option<&str>,
) -> bool {
    match telemetry_env {
        Some(value) => is_truthy_env_flag(value),
        None => settings_manager.get_enable_install_telemetry(),
    }
}

/// `reportInstallTelemetry(version)` — fire and forget, failures are ignored.
/// Returns without spawning anything when the ping is switched off, so a caller
/// on a runtime without a reactor stays safe.
pub fn report_install_telemetry(settings_manager: &SettingsManager, version: &str) {
    if std::env::var("NOTAGENT_OFFLINE").is_ok_and(|value| !value.is_empty()) {
        return;
    }
    if !is_install_telemetry_enabled(settings_manager) {
        return;
    }

    let url = format!("{REPORT_INSTALL_URL}?version={}", urlencode(version));
    let user_agent = get_pi_user_agent(version);
    tokio::spawn(async move {
        let Ok(client) = reqwest::Client::builder()
            .timeout(INSTALL_PING_TIMEOUT)
            .build()
        else {
            return;
        };
        let _ = client
            .get(url)
            .header("User-Agent", user_agent)
            .send()
            .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_env_flag_accepts_exactly_the_three_truthy_spellings() {
        assert!(is_truthy_env_flag("1"));
        assert!(is_truthy_env_flag("true"));
        assert!(is_truthy_env_flag("TRUE"));
        assert!(is_truthy_env_flag("yes"));
        assert!(is_truthy_env_flag("Yes"));
        assert!(!is_truthy_env_flag(""));
        assert!(!is_truthy_env_flag("0"));
        assert!(!is_truthy_env_flag("on"));
        assert!(!is_truthy_env_flag(" true"));
    }
}
