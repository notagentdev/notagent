use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use semver::Version;

use crate::utils::management_http::{FetchRetryOptions, fetch_with_retry};
use crate::utils::notagent_user_agent::get_pi_user_agent;

const LATEST_VERSION_URL: &str =
    "https://api.github.com/repos/notagentdev/notagent/releases/latest";

/// `crates/notagent/tests/support/mod.rs`). Unset in every other build path.
static LATEST_VERSION_URL_OVERRIDE: std::sync::RwLock<Option<String>> =
    std::sync::RwLock::new(None);

/// Point [`get_latest_pi_release`] at another endpoint, or back at the default
/// with `None`. Test-only; see [`LATEST_VERSION_URL_OVERRIDE`].
#[doc(hidden)]
pub fn set_latest_version_url_for_tests(url: Option<String>) {
    *LATEST_VERSION_URL_OVERRIDE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = url;
}

fn latest_version_url() -> String {
    LATEST_VERSION_URL_OVERRIDE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| LATEST_VERSION_URL.to_owned())
}
const DEFAULT_VERSION_CHECK_TIMEOUT_MS: u64 = 10_000;

/// `LatestPiRelease`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Explicit update commands bypass the automatic check cache.
    pub force_refresh: bool,
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

    let url = latest_version_url();
    let path = crate::config::get_agent_dir().join("cache/github-release.json");
    // Multiple startup consumers share one fetch; disk caching also covers restarts.
    let _guard = VERSION_CHECK_LOCK.lock().await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if !options.force_refresh
        && let Ok(bytes) = tokio::fs::read(&path).await
        && let Ok(cache) = serde_json::from_slice::<ReleaseCache>(&bytes)
        && cache.is_fresh(&url, now)
    {
        return Ok(cache.release);
    }
    let result = fetch_release(current_version, options, &url).await;
    let cache = ReleaseCache {
        source: url,
        checked_at: now,
        release: result.as_ref().ok().cloned().flatten(),
    };
    save_cache(&path, &cache).await;
    result
}

static VERSION_CHECK_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const SUCCESS_CACHE_SECONDS: u64 = 60 * 60;
const FAILURE_CACHE_SECONDS: u64 = 5 * 60;

#[derive(Serialize, Deserialize)]
struct ReleaseCache {
    source: String,
    checked_at: u64,
    release: Option<LatestPiRelease>,
}

impl ReleaseCache {
    fn is_fresh(&self, url: &str, now: u64) -> bool {
        let ttl = if self.release.is_some() {
            SUCCESS_CACHE_SECONDS
        } else {
            FAILURE_CACHE_SECONDS
        };
        self.source == url
            && now
                .checked_sub(self.checked_at)
                .is_some_and(|age| age < ttl)
            && self.release.as_ref().is_none_or(|release| {
                stable_version(&release.version).as_deref() == Some(release.version.as_str())
                    && release.package_name.is_none()
            })
    }
}

async fn save_cache(path: &Path, cache: &ReleaseCache) {
    let Some(parent) = path.parent() else {
        return;
    };
    let Ok(bytes) = serde_json::to_vec(cache) else {
        return;
    };
    if tokio::fs::create_dir_all(parent).await.is_err() {
        return;
    }
    // Unique temporary files keep independent app processes from clobbering writes.
    let temporary = path.with_extension(format!("{}.tmp", notagent_ai::uuidv7()));
    if tokio::fs::write(&temporary, bytes).await.is_err()
        || tokio::fs::rename(&temporary, path).await.is_err()
    {
        let _ = tokio::fs::remove_file(temporary).await;
    }
}

fn stable_version(tag: &str) -> Option<String> {
    let tag = tag.trim();
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()?;
    version.pre.is_empty().then(|| version.to_string())
}

async fn fetch_release(
    current_version: &str,
    options: VersionCheckOptions,
    url: &str,
) -> Result<Option<LatestPiRelease>, String> {
    let client = reqwest::Client::new();
    let user_agent = get_pi_user_agent(current_version);
    let response = fetch_with_retry(
        &client,
        || {
            client
                .get(url)
                .header("User-Agent", user_agent.clone())
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2026-03-10")
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
    if data.get("draft").and_then(serde_json::Value::as_bool) == Some(true)
        || data.get("prerelease").and_then(serde_json::Value::as_bool) == Some(true)
    {
        return Ok(None);
    }
    let Some(version) = data
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .and_then(stable_version)
    else {
        return Ok(None);
    };
    Ok(Some(LatestPiRelease {
        version,
        package_name: None,
        note: data
            .get("body")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|body| !body.is_empty())
            .map(str::to_owned),
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
    fn cache_expiry_rejects_other_sources_and_future_timestamps() {
        let mut cache = ReleaseCache {
            source: LATEST_VERSION_URL.to_owned(),
            checked_at: 10_000,
            release: Some(LatestPiRelease {
                version: "1.2.3".to_owned(),
                package_name: None,
                note: None,
            }),
        };
        assert!(cache.is_fresh(LATEST_VERSION_URL, 13_599));
        assert!(!cache.is_fresh(LATEST_VERSION_URL, 13_600));
        assert!(!cache.is_fresh(LATEST_VERSION_URL, 9_999));
        assert!(!cache.is_fresh("https://example.invalid", 10_001));
        cache.release = None;
        assert!(cache.is_fresh(LATEST_VERSION_URL, 10_299));
        assert!(!cache.is_fresh(LATEST_VERSION_URL, 10_300));
    }

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
