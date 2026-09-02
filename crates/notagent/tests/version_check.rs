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
}

impl EnvGuard {
    fn acquire() -> Self {
        let lock = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        EnvGuard {
            _lock: lock,
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
    set_latest_version_url_for_tests(Some(format!("{}/api/latest-version.json", server.base_url)));
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
    let (_guard, server) = serve(Vec::new(), CannedResponse::json(r#"{"version":"1.2.3"}"#)).await;

    assert_eq!(check_for_new_pi_version("1.2.3").await, None);
    assert_eq!(
        check_for_new_pi_version("1.2.2")
            .await
            .map(|release| release.version),
        Some("1.2.3".to_owned())
    );
    assert_eq!(server.request_count(), 2);
}

#[tokio::test]
async fn uses_the_notagent_dev_version_check_api_with_a_notagent_user_agent() {
    let (_guard, server) = serve(Vec::new(), CannedResponse::json(r#"{"version":"1.2.4"}"#)).await;

    assert_eq!(
        get_latest_pi_version("1.2.3", VersionCheckOptions::default())
            .await
            .expect("request"),
        Some("1.2.4".to_owned())
    );
    let requests = server.requests();
    let request = requests.first().expect("request");
    assert_eq!(request.path, "/api/latest-version.json");
    assert!(
        request
            .header("user-agent")
            .is_some_and(|value| value.starts_with("notagent/1.2.3 ")),
        "{:?}",
        request.header("user-agent")
    );
    assert_eq!(request.header("accept"), Some("application/json"));
}

#[tokio::test]
async fn retries_a_transient_version_request_when_explicitly_requested() {
    let (_guard, server) = serve(
        vec![
            CannedResponse::status(503, "busy"),
            CannedResponse::status(503, "busy"),
        ],
        CannedResponse::json(r#"{"version":"1.2.4"}"#),
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
async fn returns_the_active_package_metadata_from_the_version_check_api() {
    let (_guard, _server) = serve(
        Vec::new(),
        CannedResponse::json(r#"{"packageName":"@new-scope/notagent","version":"1.2.4"}"#),
    )
    .await;

    let release = get_latest_pi_release("1.2.3", VersionCheckOptions::default())
        .await
        .expect("request")
        .expect("release");
    assert_eq!(release.version, "1.2.4");
    assert_eq!(release.package_name.as_deref(), Some("@new-scope/notagent"));
    assert_eq!(release.note, None);
}

#[tokio::test]
async fn returns_update_notes_from_the_version_check_api() {
    let (_guard, _server) = serve(
        Vec::new(),
        CannedResponse::json(r#"{"note":" **Read this** ","version":"1.2.4"}"#),
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
    let (guard, server) = serve(Vec::new(), CannedResponse::json(r#"{"version":"1.2.4"}"#)).await;
    guard.set_skip_version_check("1");

    assert_eq!(check_for_new_pi_version("1.2.3").await, None);
    assert_eq!(server.request_count(), 0);
}

#[tokio::test]
async fn allows_direct_api_calls_when_automatic_version_checks_are_disabled() {
    let (guard, server) = serve(Vec::new(), CannedResponse::json(r#"{"version":"1.2.4"}"#)).await;
    guard.set_skip_version_check("1");

    assert_eq!(
        get_latest_pi_version("1.2.3", VersionCheckOptions::default())
            .await
            .expect("request"),
        Some("1.2.4".to_owned())
    );
    assert_eq!(server.request_count(), 1);
}
