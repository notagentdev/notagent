//! Ported from `packages/coding-agent/test/footer-data-provider.test.ts`.
//!
//! Deviation (class 1): the TypeScript suite mocks `child_process` so it can
//! assert whether git was consulted. Rust has no module mocking, and a fake
//! `git` on `PATH` would mean mutating the process environment while other
//! tests run. The fixtures do the same job instead: a repository whose `.git`
//! holds nothing but a HEAD file is one real git refuses to open, so a branch
//! that still comes back proves the file was read and git was not asked, while
//! the reftable cases use repositories real git does open.
//!
//! Not ported: "retries git watchers 5 seconds after an async fs.watch error",
//! which drives the watcher's `error` event by hand through the mocked handle.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use notagent::core::footer_data_provider::{FooterDataProvider, find_git_paths};

fn git(args: &[&str], cwd: &Path) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// A `.git` directory with nothing in it but HEAD: enough for the provider to
/// find and read, and not a repository real git will open.
fn create_plain_repo(temp: &Path) -> PathBuf {
    let repo_dir = temp.join("repo");
    std::fs::create_dir_all(repo_dir.join(".git")).expect("create .git");
    std::fs::write(repo_dir.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
    repo_dir
}

/// The same, plus the reftable marker HEAD — so the provider asks git, and git
/// declines.
fn create_plain_reftable_repo(temp: &Path) -> PathBuf {
    let repo_dir = temp.join("repo");
    std::fs::create_dir_all(repo_dir.join(".git/reftable")).expect("create reftable");
    std::fs::write(repo_dir.join(".git/HEAD"), "ref: refs/heads/.invalid\n").expect("write HEAD");
    repo_dir
}

/// A real reftable-backed repository. Its HEAD file says `.invalid` — that is
/// what the format writes — so only git can name the branch.
fn create_real_reftable_repo(temp: &Path, branch: &str) -> Option<PathBuf> {
    let repo_dir = temp.join("repo");
    std::fs::create_dir_all(&repo_dir).expect("create repo dir");
    let status = std::process::Command::new("git")
        .args(["init", "--ref-format=reftable", "-b", branch, "."])
        .current_dir(&repo_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        // git is too old for the reftable format; the case cannot run here.
        return None;
    }
    let head = std::fs::read_to_string(repo_dir.join(".git/HEAD")).ok()?;
    if !head.contains(".invalid") {
        return None;
    }
    Some(repo_dir)
}

fn create_real_reftable_worktree(temp: &Path) -> Option<(PathBuf, PathBuf)> {
    let repo_dir = create_real_reftable_repo(temp, "main")?;
    git(
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=test",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
        &repo_dir,
    );
    let worktree_dir = temp.join("worktree");
    git(
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            worktree_dir.to_str().expect("utf-8 path"),
        ],
        &repo_dir,
    );
    Some((repo_dir, worktree_dir))
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

async fn wait_for(mut condition: impl FnMut() -> bool, timeout: Duration) -> bool {
    let started = Instant::now();
    while !condition() {
        if started.elapsed() > timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

// ------------------------------------------------------------- findGitPaths

#[test]
fn finds_the_git_paths_of_a_regular_repository_from_a_nested_directory() {
    let temp = tempfile::tempdir().expect("temp dir");
    let repo_dir = create_plain_repo(temp.path());
    let nested = repo_dir.join("src/nested");
    std::fs::create_dir_all(&nested).expect("create nested");

    let paths = find_git_paths(&text(&nested)).expect("git paths");
    assert_eq!(paths.repo_dir, text(&repo_dir));
    assert_eq!(paths.common_git_dir, text(&repo_dir.join(".git")));
    assert_eq!(paths.head_path, text(&repo_dir.join(".git/HEAD")));
}

#[test]
fn resolves_a_worktrees_common_git_dir_through_the_gitdir_pointer() {
    let temp = tempfile::tempdir().expect("temp dir");
    let Some((repo_dir, worktree_dir)) = create_real_reftable_worktree(temp.path()) else {
        return;
    };

    let paths = find_git_paths(&text(&worktree_dir)).expect("git paths");
    assert_eq!(paths.repo_dir, text(&worktree_dir));
    // `commondir` points back at the main repository, not at the worktree's own
    // git directory.
    assert_eq!(
        std::fs::canonicalize(&paths.common_git_dir).expect("canonical"),
        std::fs::canonicalize(repo_dir.join(".git")).expect("canonical")
    );
    assert!(paths.head_path.contains("worktrees"));
}

#[test]
fn finds_nothing_outside_a_repository() {
    let temp = tempfile::tempdir().expect("temp dir");
    let outside = temp.path().join("plain");
    std::fs::create_dir_all(&outside).expect("create dir");
    // A temp directory can still sit inside a checkout on some machines; only
    // assert the negative when the walk really leaves every repository behind.
    if let Some(paths) = find_git_paths(&text(&outside)) {
        assert!(!paths.repo_dir.starts_with(&text(&outside)));
    }
}

// ------------------------------------------------------- branch resolution

#[test]
fn uses_head_directly_in_a_regular_repo_from_a_nested_directory() {
    let temp = tempfile::tempdir().expect("temp dir");
    let repo_dir = create_plain_repo(temp.path());
    let nested = repo_dir.join("src/nested");
    std::fs::create_dir_all(&nested).expect("create nested");

    let provider = FooterDataProvider::new(&text(&nested));
    // git cannot open this repository, so "main" can only have come from the
    // HEAD file.
    assert_eq!(provider.get_git_branch().as_deref(), Some("main"));
    provider.dispose();
}

#[test]
fn resolves_the_branch_via_git_when_head_is_invalid_in_a_reftable_repo() {
    let temp = tempfile::tempdir().expect("temp dir");
    let Some(repo_dir) = create_real_reftable_repo(temp.path(), "main") else {
        return;
    };

    let provider = FooterDataProvider::new(&text(&repo_dir));
    assert_eq!(provider.get_git_branch().as_deref(), Some("main"));
    provider.dispose();
}

#[test]
fn resolves_the_branch_via_git_in_a_reftable_backed_worktree() {
    let temp = tempfile::tempdir().expect("temp dir");
    let Some((_repo_dir, worktree_dir)) = create_real_reftable_worktree(temp.path()) else {
        return;
    };

    let provider = FooterDataProvider::new(&text(&worktree_dir));
    assert_eq!(provider.get_git_branch().as_deref(), Some("feature"));
    provider.dispose();
}

#[test]
fn treats_an_unresolved_invalid_reftable_head_as_detached() {
    let temp = tempfile::tempdir().expect("temp dir");
    let repo_dir = create_plain_reftable_repo(temp.path());

    let provider = FooterDataProvider::new(&text(&repo_dir));
    assert_eq!(provider.get_git_branch().as_deref(), Some("detached"));
    provider.dispose();
}

#[test]
fn reports_no_branch_outside_a_repository() {
    let temp = tempfile::tempdir().expect("temp dir");
    let outside = temp.path().join("plain");
    std::fs::create_dir_all(&outside).expect("create dir");
    if find_git_paths(&text(&outside)).is_some() {
        return;
    }

    let provider = FooterDataProvider::new(&text(&outside));
    assert_eq!(provider.get_git_branch(), None);
    provider.dispose();
}

#[test]
fn treats_a_detached_head_as_detached() {
    let temp = tempfile::tempdir().expect("temp dir");
    let repo_dir = temp.path().join("repo");
    std::fs::create_dir_all(repo_dir.join(".git")).expect("create .git");
    std::fs::write(
        repo_dir.join(".git/HEAD"),
        "1234567890123456789012345678901234567890\n",
    )
    .expect("write HEAD");

    let provider = FooterDataProvider::new(&text(&repo_dir));
    assert_eq!(provider.get_git_branch().as_deref(), Some("detached"));
    provider.dispose();
}

// ----------------------------------------------------------- watcher paths

#[tokio::test]
async fn updates_the_cached_branch_when_the_reftable_directory_changes() {
    let temp = tempfile::tempdir().expect("temp dir");
    let Some(repo_dir) = create_real_reftable_repo(temp.path(), "main") else {
        return;
    };
    git(
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=test",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
        &repo_dir,
    );

    let provider = FooterDataProvider::new(&text(&repo_dir));
    assert_eq!(provider.get_git_branch().as_deref(), Some("main"));

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let _subscription = provider.on_branch_change(Arc::new(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    }));

    git(&["switch", "-q", "-c", "foo"], &repo_dir);

    assert!(
        wait_for(
            || provider.get_git_branch().as_deref() == Some("foo"),
            Duration::from_secs(10)
        )
        .await,
        "branch never refreshed"
    );
    assert!(calls.load(Ordering::Relaxed) >= 1);
    provider.dispose();
}

#[tokio::test]
async fn does_not_notify_listeners_when_reftable_updates_keep_the_same_branch() {
    let temp = tempfile::tempdir().expect("temp dir");
    let Some(repo_dir) = create_real_reftable_repo(temp.path(), "main") else {
        return;
    };
    git(
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=test",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
        &repo_dir,
    );

    let provider = FooterDataProvider::new(&text(&repo_dir));
    assert_eq!(provider.get_git_branch().as_deref(), Some("main"));

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let _subscription = provider.on_branch_change(Arc::new(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    }));

    // A tag writes the reftable stack without moving HEAD.
    git(&["tag", "marker"], &repo_dir);
    tokio::time::sleep(Duration::from_millis(1500)).await;

    assert_eq!(provider.get_git_branch().as_deref(), Some("main"));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    provider.dispose();
}

#[tokio::test]
async fn stops_notifying_once_the_subscription_is_dropped() {
    let temp = tempfile::tempdir().expect("temp dir");
    let repo_dir = create_plain_repo(temp.path());
    let provider = FooterDataProvider::new(&text(&repo_dir));

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let subscription = provider.on_branch_change(Arc::new(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    }));
    drop(subscription);

    // A cwd change always notifies; with no subscriber left, nothing is called.
    let other = temp.path().join("elsewhere");
    std::fs::create_dir_all(&other).expect("create dir");
    provider.set_cwd(&text(&other));

    assert_eq!(calls.load(Ordering::Relaxed), 0);
    provider.dispose();
}

#[tokio::test]
async fn rereads_the_branch_after_the_working_directory_changes() {
    let temp = tempfile::tempdir().expect("temp dir");
    let first = temp.path().join("first");
    std::fs::create_dir_all(&first).expect("create dir");
    let repo_dir = create_plain_repo(&first);

    let second = temp.path().join("second");
    std::fs::create_dir_all(second.join("repo/.git")).expect("create .git");
    std::fs::write(second.join("repo/.git/HEAD"), "ref: refs/heads/other\n").expect("write HEAD");

    let provider = FooterDataProvider::new(&text(&repo_dir));
    assert_eq!(provider.get_git_branch().as_deref(), Some("main"));

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let _subscription = provider.on_branch_change(Arc::new(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    }));

    provider.set_cwd(&text(&second.join("repo")));
    assert_eq!(provider.get_git_branch().as_deref(), Some("other"));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    provider.dispose();
}

#[test]
fn carries_the_available_provider_count_for_the_footer() {
    let temp = tempfile::tempdir().expect("temp dir");
    let repo_dir = create_plain_repo(temp.path());
    let provider = FooterDataProvider::new(&text(&repo_dir));

    assert_eq!(provider.get_available_provider_count(), 0);
    provider.set_available_provider_count(3);
    assert_eq!(provider.get_available_provider_count(), 3);
    provider.dispose();
}
