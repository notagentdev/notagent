mod support;

use std::sync::{Mutex, MutexGuard};

use notagent::utils::version_check::{
    VersionCheckOptions, check_for_new_pi_version, compare_package_versions, get_latest_pi_release,
    get_latest_pi_version, is_newer_package_version, set_latest_version_url_for_tests,
};
use support::{CannedResponse, TestServer};

/// The URL override and `NOTAGENT_SKIP_VERSION_CHECK` are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    skip_version_check: Option<String>,
    previous_agent_dir: Option<String>,
    previous_offline: Option<String>,
    directory: tempfile::TempDir,
}

impl EnvGuard {
    fn acquire() -> Self {
        let lock = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        let previous_offline = std::env::var("NOTAGENT_OFFLINE").ok();
        unsafe {
            std::env::remove_var("NOTAGENT_OFFLINE");
        }
        let directory = tempfile::tempdir().unwrap();
        let previous_agent_dir = std::env::var("NOTAGENT_CODING_AGENT_DIR").ok();
        unsafe {
            std::env::set_var("NOTAGENT_CODING_AGENT_DIR", directory.path());
        }
        EnvGuard {
            _lock: lock,
            previous_agent_dir,
            previous_offline,
            directory,
            skip_version_check: std::env::var("NOTAGENT_SKIP_VERSION_CHECK").ok(),
        }
    }

    fn set_skip_version_check(&self, value: &str) {
        // SAFETY: the suite is serialized by SERIAL and restores the value below.
        unsafe { std::env::set_var("NOTAGENT_SKIP_VERSION_CHECK", value) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        set_latest_version_url_for_tests(None);
        unsafe {
            match self.previous_offline.take() {
                Some(value) => std::env::set_var("NOTAGENT_OFFLINE", value),
                None => std::env::remove_var("NOTAGENT_OFFLINE"),
            }
            match self.previous_agent_dir.take() {
                Some(value) => std::env::set_var("NOTAGENT_CODING_AGENT_DIR", value),
                None => std::env::remove_var("NOTAGENT_CODING_AGENT_DIR"),
            }
            match self.skip_version_check.take() {
                Some(value) => std::env::set_var("NOTAGENT_SKIP_VERSION_CHECK", value),
                None => std::env::remove_var("NOTAGENT_SKIP_VERSION_CHECK"),
            }
        }
    }
}

async fn serve(responses: Vec<CannedResponse>, fallback: CannedResponse) -> (EnvGuard, TestServer) {
    let guard = EnvGuard::acquire();
    // SAFETY: serialized by the guard.
    unsafe { std::env::remove_var("NOTAGENT_SKIP_VERSION_CHECK") };
    let server = TestServer::start(responses, fallback).await;
    set_latest_version_url_for_tests(Some(format!(
        "{}/repos/notagentdev/notagent/releases/latest",
        server.base_url
    )));
    (guard, server)
}

#[test]
fn compares_package_versions() {
    assert!(
        compare_package_versions("0.70.6", "0.70.5")
            .expect("comparable")
            .is_gt()
    );
    assert!(
        compare_package_versions("0.70.5", "0.70.5")
            .expect("comparable")
            .is_eq()
    );
    assert!(
        compare_package_versions("0.70.4", "0.70.5")
            .expect("comparable")
            .is_lt()
    );
    assert!(
        compare_package_versions("5.0.0-beta.20", "5.0.0-beta.9")
            .expect("comparable")
            .is_gt()
    );
    assert!(!is_newer_package_version("0.70.5", "0.70.5"));
    assert!(is_newer_package_version("0.70.6", "0.70.5"));
}

#[tokio::test]
async fn returns_only_newer_versions() {
    let (_guard, server) =
        serve(Vec::new(), CannedResponse::json(r#"{"tag_name":"v1.2.3"}"#)).await;

    assert_eq!(check_for_new_pi_version("1.2.3").await, None);
    assert_eq!(
        check_for_new_pi_version("1.2.2")
            .await
            .map(|release| release.version),
        Some("1.2.3".to_owned())
    );
    assert_eq!(server.request_count(), 1);
}

#[tokio::test]
async fn uses_the_github_release_api_with_a_notagent_user_agent() {
    let (_guard, server) =
        serve(Vec::new(), CannedResponse::json(r#"{"tag_name":"v1.2.4"}"#)).await;

    assert_eq!(
        get_latest_pi_version("1.2.3", VersionCheckOptions::default())
            .await
            .expect("request"),
        Some("1.2.4".to_owned())
    );
    let requests = server.requests();
    let request = requests.first().expect("request");
    assert_eq!(request.path, "/repos/notagentdev/notagent/releases/latest");
    assert!(
        request
            .header("user-agent")
            .is_some_and(|value| value.starts_with("notagent/1.2.3 ")),
        "{:?}",
        request.header("user-agent")
    );
    assert_eq!(
        request.header("accept"),
        Some("application/vnd.github+json")
    );
}

#[tokio::test]
async fn retries_a_transient_version_request_when_explicitly_requested() {
    let (_guard, server) = serve(
        vec![
            CannedResponse::status(503, "busy"),
            CannedResponse::status(503, "busy"),
        ],
        CannedResponse::json(r#"{"tag_name":"v1.2.4"}"#),
    )
    .await;

    let release = get_latest_pi_release(
        "1.2.3",
        VersionCheckOptions {
            retry: true,
            ..VersionCheckOptions::default()
        },
    )
    .await
    .expect("request")
    .expect("release");
    assert_eq!(release.version, "1.2.4");
    assert_eq!(server.request_count(), 3);
}

#[tokio::test]
async fn keeps_automatic_version_checks_to_one_request() {
    let (_guard, server) = serve(Vec::new(), CannedResponse::status(503, "busy")).await;

    assert_eq!(check_for_new_pi_version("1.2.3").await, None);
    assert_eq!(server.request_count(), 1);
}

#[tokio::test]
async fn does_not_treat_github_release_names_as_package_names() {
    let (_guard, _server) = serve(
        Vec::new(),
        CannedResponse::json(r#"{"name":"Release title","tag_name":"v1.2.4"}"#),
    )
    .await;

    let release = get_latest_pi_release("1.2.3", VersionCheckOptions::default())
        .await
        .expect("request")
        .expect("release");
    assert_eq!(release.version, "1.2.4");
    assert_eq!(release.package_name, None);
    assert_eq!(release.note, None);
}

#[tokio::test]
async fn returns_the_github_release_body_as_update_notes() {
    let (_guard, _server) = serve(
        Vec::new(),
        CannedResponse::json(r#"{"body":" **Read this** ","tag_name":"v1.2.4"}"#),
    )
    .await;

    let release = get_latest_pi_release("1.2.3", VersionCheckOptions::default())
        .await
        .expect("request")
        .expect("release");
    assert_eq!(release.version, "1.2.4");
    assert_eq!(release.note.as_deref(), Some("**Read this**"));
}

#[tokio::test]
async fn skips_automatic_api_calls_when_version_checks_are_disabled() {
    let (guard, server) = serve(Vec::new(), CannedResponse::json(r#"{"tag_name":"v1.2.4"}"#)).await;
    guard.set_skip_version_check("1");

    assert_eq!(check_for_new_pi_version("1.2.3").await, None);
    assert_eq!(server.request_count(), 0);
}

#[tokio::test]
async fn allows_direct_api_calls_when_automatic_version_checks_are_disabled() {
    let (guard, server) = serve(Vec::new(), CannedResponse::json(r#"{"tag_name":"v1.2.4"}"#)).await;
    guard.set_skip_version_check("1");

    assert_eq!(
        get_latest_pi_version("1.2.3", VersionCheckOptions::default())
            .await
            .expect("request"),
        Some("1.2.4".to_owned())
    );
    assert_eq!(server.request_count(), 1);
}

#[tokio::test]
async fn reuses_the_disk_cache_and_refreshes_it_when_expired_or_explicitly_requested() {
    let (guard, server) = serve(Vec::new(), CannedResponse::json(r#"{"tag_name":"v1.2.4"}"#)).await;
    get_latest_pi_release("1.2.3", VersionCheckOptions::default())
        .await
        .unwrap();
    let path = guard.directory.path().join("cache/github-release.json");
    let mut cache: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    cache["release"]["version"] = serde_json::json!("1.2.5");
    std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
    assert_eq!(
        get_latest_pi_version("1.2.3", VersionCheckOptions::default())
            .await
            .unwrap()
            .as_deref(),
        Some("1.2.5"),
        "a fresh persisted result must survive without another request"
    );
    assert_eq!(server.request_count(), 1);
    get_latest_pi_release(
        "1.2.3",
        VersionCheckOptions {
            force_refresh: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        server.request_count(),
        2,
        "explicit refresh bypasses a fresh cache"
    );
    cache["checked_at"] = serde_json::json!(0);
    std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
    assert_eq!(
        get_latest_pi_version("1.2.3", VersionCheckOptions::default())
            .await
            .unwrap()
            .as_deref(),
        Some("1.2.4")
    );
    assert_eq!(server.request_count(), 3, "an expired cache must refresh");
}

#[tokio::test]
async fn rate_limits_do_not_cause_repeated_automatic_requests() {
    let (_guard, server) = serve(Vec::new(), CannedResponse::status(429, "rate limited")).await;
    assert_eq!(check_for_new_pi_version("1.2.3").await, None);
    assert_eq!(check_for_new_pi_version("1.2.3").await, None);
    assert_eq!(server.request_count(), 1);
}

#[tokio::test]
async fn ignores_drafts_prereleases_and_invalid_release_tags() {
    let (_guard, server) = serve(
        vec![
            CannedResponse::json(r#"{"tag_name":"v2.0.0","draft":true}"#),
            CannedResponse::json(r#"{"tag_name":"v2.0.0","prerelease":true}"#),
            CannedResponse::json(r#"{"tag_name":"v2.0.0-beta.1"}"#),
            CannedResponse::json(r#"{"tag_name":"nightly"}"#),
        ],
        CannedResponse::json(r#"{"tag_name":" 1.2.4 "}"#),
    )
    .await;
    let options = VersionCheckOptions {
        force_refresh: true,
        ..Default::default()
    };
    for _ in 0..4 {
        assert_eq!(get_latest_pi_release("1.2.3", options).await.unwrap(), None);
    }
    assert_eq!(
        get_latest_pi_version("1.2.3", options)
            .await
            .unwrap()
            .as_deref(),
        Some("1.2.4")
    );
    assert_eq!(server.request_count(), 5);
}

#[tokio::test]
async fn corrupt_cache_and_unwritable_cache_do_not_break_a_successful_check() {
    let (guard, server) = serve(Vec::new(), CannedResponse::json(r#"{"tag_name":"v1.2.4"}"#)).await;
    let path = guard.directory.path().join("cache/github-release.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "broken").unwrap();
    assert_eq!(
        get_latest_pi_version("1.2.3", VersionCheckOptions::default())
            .await
            .unwrap()
            .as_deref(),
        Some("1.2.4")
    );
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        get_latest_pi_version("1.2.3", VersionCheckOptions::default())
            .await
            .unwrap()
            .as_deref(),
        Some("1.2.4")
    );
    assert_eq!(server.request_count(), 2);
}

#[tokio::test]
async fn offline_mode_neither_fetches_nor_reports_a_cached_release() {
    let (_guard, server) =
        serve(Vec::new(), CannedResponse::json(r#"{"tag_name":"v1.2.4"}"#)).await;
    assert!(check_for_new_pi_version("1.2.3").await.is_some());
    // SAFETY: EnvGuard serializes and restores the environment for this suite.
    unsafe {
        std::env::set_var("NOTAGENT_OFFLINE", "1");
    }
    assert_eq!(check_for_new_pi_version("1.2.3").await, None);
    assert_eq!(server.request_count(), 1);
}
