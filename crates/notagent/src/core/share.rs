//! The TUI-free half of `/share` —
//! `packages/coding-agent/src/modes/interactive/interactive-mode.ts:6140-6236`
//! (`handleShareCommand`) plus `getShareViewerUrl` from `config.ts`.
//!
//! What stays in the interactive mode: the cancellable loader, the editor
//! swap and the status lines. What is here: the `gh` calls, the gist id and the
//! viewer URL (interface request B-9).

use std::path::{Path, PathBuf};

use futures::future::BoxFuture;

use crate::config::get_share_viewer_url;

/// The result of a successful `gh gist create`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedGist {
    /// The URL `gh` printed, e.g. `https://gist.github.com/user/GIST_ID`.
    pub gist_url: String,
    pub gist_id: String,
    /// `getShareViewerUrl(gistId)`
    pub viewer_url: String,
}

/// The failures the command reports, with the TypeScript's wording.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShareError {
    #[error("GitHub CLI is not logged in. Run 'gh auth login' first.")]
    NotLoggedIn,
    #[error("GitHub CLI (gh) is not installed. Install it from https://cli.github.com/")]
    NotInstalled,
    #[error("Failed to create gist: {0}")]
    GistFailed(String),
    #[error("Failed to parse gist ID from gh output")]
    UnparsableGistId,
}

/// What a finished `gh gist create` left behind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GhOutput {
    pub stdout: String,
    pub stderr: String,
    /// `null` when the process was killed before it exited.
    pub code: Option<i32>,
}

/// The two `gh` calls, behind a seam so the suite can drive them without a
/// GitHub CLI (deviation class 1, the same seam the package manager uses).
pub trait ShareCommandRunner: Send + Sync {
    /// `spawnSync("gh", ["auth", "status"])` — the exit status, or `None` when
    /// the process produced none (missing binary included).
    fn gh_auth_status(&self) -> Option<i32>;
    /// `spawn("gh", ["gist", "create", "--public=false", file])`.
    fn gh_create_gist<'a>(&'a self, file: &'a Path) -> BoxFuture<'a, GhOutput>;
}

/// `os.tmpdir()/session.html` — where the command parks the export.
pub fn share_temp_html_path() -> PathBuf {
    std::env::temp_dir().join("session.html")
}

/// The `gh auth status` gate.
///
/// Bug compatibility: `spawnSync` does not throw when `gh` is missing, it
/// returns a result without a status, so the TypeScript falls into the
/// "not logged in" branch rather than the "not installed" one it has ready.
/// [`ShareError::NotInstalled`] therefore stays unreachable from here, exactly
/// as in the original.
pub fn check_gh_auth(runner: &dyn ShareCommandRunner) -> Result<(), ShareError> {
    match runner.gh_auth_status() {
        Some(0) => Ok(()),
        _ => Err(ShareError::NotLoggedIn),
    }
}

/// Create the secret gist for an exported HTML file and derive its viewer URL.
pub async fn create_secret_gist(
    file: &Path,
    runner: &dyn ShareCommandRunner,
) -> Result<SharedGist, ShareError> {
    let result = runner.gh_create_gist(file).await;

    if result.code != Some(0) {
        let message = result.stderr.trim();
        let message = if message.is_empty() {
            "Unknown error"
        } else {
            message
        };
        return Err(ShareError::GistFailed(message.to_owned()));
    }

    // Extract gist ID from the URL returned by gh
    // gh returns something like: https://gist.github.com/username/GIST_ID
    let gist_url = result.stdout.trim().to_owned();
    let gist_id = gist_url.rsplit('/').next().unwrap_or_default().to_owned();
    if gist_id.is_empty() {
        return Err(ShareError::UnparsableGistId);
    }

    let viewer_url = get_share_viewer_url(&gist_id);
    Ok(SharedGist {
        gist_url,
        gist_id,
        viewer_url,
    })
}

/// The production runner: the real `gh` binary.
#[derive(Debug, Default)]
pub struct ProcessShareCommandRunner;

impl ShareCommandRunner for ProcessShareCommandRunner {
    fn gh_auth_status(&self) -> Option<i32> {
        std::process::Command::new("gh")
            .args(["auth", "status"])
            .output()
            .ok()
            .and_then(|output| output.status.code())
    }

    fn gh_create_gist<'a>(&'a self, file: &'a Path) -> BoxFuture<'a, GhOutput> {
        Box::pin(async move {
            let output = tokio::process::Command::new("gh")
                .args(["gist", "create", "--public=false"])
                .arg(file)
                .kill_on_drop(true)
                .output()
                .await;
            match output {
                Ok(output) => GhOutput {
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    code: output.status.code(),
                },
                // A spawn failure is what the TypeScript sees as a closed process
                // without an exit code.
                Err(error) => GhOutput {
                    stdout: String::new(),
                    stderr: error.to_string(),
                    code: None,
                },
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeRunner {
        status: Option<i32>,
        output: GhOutput,
    }

    impl ShareCommandRunner for FakeRunner {
        fn gh_auth_status(&self) -> Option<i32> {
            self.status
        }

        fn gh_create_gist<'a>(&'a self, _file: &'a Path) -> BoxFuture<'a, GhOutput> {
            Box::pin(async move { self.output.clone() })
        }
    }

    fn runner(status: Option<i32>, output: GhOutput) -> FakeRunner {
        FakeRunner { status, output }
    }

    #[test]
    fn the_auth_gate_only_passes_on_a_zero_exit() {
        assert_eq!(check_gh_auth(&runner(Some(0), GhOutput::default())), Ok(()));
        assert_eq!(
            check_gh_auth(&runner(Some(1), GhOutput::default())),
            Err(ShareError::NotLoggedIn)
        );
        // A missing binary lands in the same branch as a logged-out CLI.
        assert_eq!(
            check_gh_auth(&runner(None, GhOutput::default())),
            Err(ShareError::NotLoggedIn)
        );
    }

    #[tokio::test]
    async fn derives_the_viewer_url_from_the_last_path_segment() {
        let shared = create_secret_gist(
            Path::new("/tmp/session.html"),
            &runner(
                Some(0),
                GhOutput {
                    stdout: "https://gist.github.com/octocat/abc123\n".to_owned(),
                    stderr: String::new(),
                    code: Some(0),
                },
            ),
        )
        .await
        .expect("gist");
        assert_eq!(shared.gist_url, "https://gist.github.com/octocat/abc123");
        assert_eq!(shared.gist_id, "abc123");
        assert_eq!(shared.viewer_url, get_share_viewer_url("abc123"));
        assert!(shared.viewer_url.ends_with("#abc123"));
    }

    #[tokio::test]
    async fn reports_the_stderr_of_a_failed_gist() {
        let error = create_secret_gist(
            Path::new("/tmp/session.html"),
            &runner(
                Some(0),
                GhOutput {
                    stdout: String::new(),
                    stderr: "  gh: rate limited  ".to_owned(),
                    code: Some(1),
                },
            ),
        )
        .await
        .expect_err("failure");
        assert_eq!(error, ShareError::GistFailed("gh: rate limited".to_owned()));
    }

    #[tokio::test]
    async fn falls_back_to_unknown_error_and_reports_an_unparsable_id() {
        let error = create_secret_gist(
            Path::new("/tmp/session.html"),
            &runner(Some(0), GhOutput::default()),
        )
        .await
        .expect_err("failure");
        assert_eq!(error, ShareError::GistFailed("Unknown error".to_owned()));

        let error = create_secret_gist(
            Path::new("/tmp/session.html"),
            &runner(
                Some(0),
                GhOutput {
                    stdout: "   \n".to_owned(),
                    stderr: String::new(),
                    code: Some(0),
                },
            ),
        )
        .await
        .expect_err("failure");
        assert_eq!(error, ShareError::UnparsableGistId);
    }

    #[test]
    fn the_export_lands_in_the_system_temp_directory() {
        assert_eq!(
            share_temp_html_path(),
            std::env::temp_dir().join("session.html")
        );
    }
}
