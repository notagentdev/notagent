use std::time::Duration;

use semver::Version;

use crate::utils::management_http::{FetchRetryOptions, fetch_with_retry};
use crate::utils::notagent_user_agent::get_pi_user_agent;

const LATEST_VERSION_URL: &str = "https://notagent.dev/api/latest-version.json";

/// `crates/notagent/tests/support/mod.rs`). Unset in every other build path.
static LATEST_VERSION_URL_OVERRIDE: std::sync::RwLock<Option<String>> =
    std::sync::RwLock::new(None);

/// Point [`get_latest_pi_release`] at another endpoint, or back at the default
/// with `None`. Test-only; see [`LATEST_VERSION_URL_OVERRIDE`].
#[doc(hidden)]
pub fn set_latest_version_url_for_tests(url: Option<String>) {
    *LATEST_VERSION_URL_OVERRIDE.write().expect("poisoned") = url;
}

fn latest_version_url() -> String {
    LATEST_VERSION_URL_OVERRIDE
        .read()
        .expect("poisoned")
        .clone()
        .unwrap_or_else(|| LATEST_VERSION_URL.to_owned())
}
const DEFAULT_VERSION_CHECK_TIMEOUT_MS: u64 = 10_000;

/// `LatestPiRelease`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestPiRelease {
    pub version: String,
    pub package_name: Option<String>,
    pub note: Option<String>,
}

/// `{ timeoutMs?, retry? }`
#[derive(Debug, Clone, Copy, Default)]
pub struct VersionCheckOptions {
    pub timeout_ms: Option<u64>,
    pub retry: bool,
}

/// `comparePackageVersions(left, right)`
pub fn compare_package_versions(
    left_version: &str,
    right_version: &str,
) -> Option<std::cmp::Ordering> {
    let left = Version::parse(left_version.trim()).ok()?;
    let right = Version::parse(right_version.trim()).ok()?;
    Some(left.cmp(&right))
}

/// `isNewerPackageVersion(candidate, current)`
pub fn is_newer_package_version(candidate_version: &str, current_version: &str) -> bool {
    match compare_package_versions(candidate_version, current_version) {
        Some(ordering) => ordering.is_gt(),
        None => candidate_version.trim() != current_version.trim(),
    }
}

/// `getLatestPiRelease(currentVersion, options)`
pub async fn get_latest_pi_release(
    current_version: &str,
    options: VersionCheckOptions,
) -> Result<Option<LatestPiRelease>, String> {
    if std::env::var("NOTAGENT_OFFLINE").is_ok_and(|value| !value.is_empty()) {
        return Ok(None);
    }

    let client = reqwest::Client::new();
    let user_agent = get_pi_user_agent(current_version);
    let url = latest_version_url();
    let response = fetch_with_retry(
        &client,
        || {
            client
                .get(&url)
                .header("User-Agent", user_agent.clone())
                .header("accept", "application/json")
        },
        FetchRetryOptions {
            max_retries: if options.retry { 2 } else { 0 },
            retry_on_status: true,
            timeout: Some(Duration::from_millis(
                options
                    .timeout_ms
                    .unwrap_or(DEFAULT_VERSION_CHECK_TIMEOUT_MS),
            )),
        },
    )
    .await?;
    if !response.status().is_success() {
        return Ok(None);
    }

    let data: serde_json::Value = response.json().await.map_err(|error| error.to_string())?;
    let trimmed = |key: &str| {
        data.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let Some(version) = trimmed("version") else {
        return Ok(None);
    };
    Ok(Some(LatestPiRelease {
        version,
        package_name: trimmed("packageName"),
        note: trimmed("note"),
    }))
}

/// `getLatestPiVersion(currentVersion, options)`
pub async fn get_latest_pi_version(
    current_version: &str,
    options: VersionCheckOptions,
) -> Result<Option<String>, String> {
    Ok(get_latest_pi_release(current_version, options)
        .await?
        .map(|release| release.version))
}

/// `checkForNewPiVersion(currentVersion)`
pub async fn check_for_new_pi_version(current_version: &str) -> Option<LatestPiRelease> {
    if std::env::var("NOTAGENT_SKIP_VERSION_CHECK").is_ok_and(|value| !value.is_empty()) {
        return None;
    }

    let latest_release = get_latest_pi_release(current_version, VersionCheckOptions::default())
        .await
        .ok()??;
    is_newer_package_version(&latest_release.version, current_version).then_some(latest_release)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_only_valid_versions() {
        assert_eq!(
            compare_package_versions("1.2.3", "1.2.4"),
            Some(std::cmp::Ordering::Less)
        );
        assert_eq!(
            compare_package_versions(" 1.2.3 ", "1.2.3"),
            Some(std::cmp::Ordering::Equal)
        );
        assert_eq!(compare_package_versions("latest", "1.2.3"), None);
    }

    #[test]
    fn falls_back_to_string_inequality_for_unparsable_versions() {
        assert!(is_newer_package_version("1.2.4", "1.2.3"));
        assert!(!is_newer_package_version("1.2.3", "1.2.4"));
        assert!(!is_newer_package_version("1.2.3", "1.2.3"));
        assert!(is_newer_package_version("nightly", "1.2.3"));
        assert!(!is_newer_package_version("nightly", " nightly "));
    }
}
