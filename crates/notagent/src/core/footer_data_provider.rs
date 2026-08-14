//! Port of `packages/coding-agent/src/core/footer-data-provider.ts`.
//!
//! The git branch shown in the footer, and the machinery that keeps it honest.
//!
//! Two things here are less obvious than they look. The first is that the
//! branch is read from `.git/HEAD` rather than by running git: the footer
//! repaints on every keystroke, and a subprocess per repaint is not affordable.
//! git is only consulted for the one case the file cannot answer — a HEAD
//! pointing at `refs/heads/.invalid`, which is what a repository mid-rebase
//! looks like.
//!
//! The second is that the watcher is installed on the *directory* holding HEAD,
//! not on HEAD itself. Git writes HEAD by renaming a temporary file over it,
//! which changes the inode, and a watch on the old inode goes quiet forever.
//!
//! Deviation (class 2): the extension status map (`setExtensionStatus`,
//! `getExtensionStatuses`, `clearExtensionStatuses`) is gone. Its only producer
//! was `ctx.ui.setStatus`, which is extension API — see
//! `plans/facts/extension-boundary.md` §6, "Custom Footer/Header/Widgets".

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::utils::fs_watch::{FS_WATCH_RETRY_DELAY_MS, FsWatcher, watch_with_error_handler};

/// Where the three git files this module cares about live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPaths {
    /// The directory holding `.git` — the worktree root.
    pub repo_dir: String,
    /// The repository's shared git directory. For a linked worktree this is the
    /// main repository's, not the worktree's own.
    pub common_git_dir: String,
    pub head_path: String,
}

fn to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Walks up from `cwd` looking for git metadata.
///
/// Handles both layouts: a `.git` directory (ordinary clone) and a `.git` file
/// holding a `gitdir:` pointer (linked worktree or submodule).
pub fn find_git_paths(cwd: &str) -> Option<GitPaths> {
    let mut dir = PathBuf::from(cwd);
    loop {
        let git_path = dir.join(".git");
        if git_path.exists() {
            let stat = std::fs::metadata(&git_path).ok()?;
            if stat.is_file() {
                let content = std::fs::read_to_string(&git_path).ok()?;
                let content = content.trim();
                if let Some(target) = content.strip_prefix("gitdir: ") {
                    let git_dir = resolve_against(&dir, target.trim());
                    let head_path = git_dir.join("HEAD");
                    if !head_path.exists() {
                        return None;
                    }
                    let common_dir_path = git_dir.join("commondir");
                    let common_git_dir = if common_dir_path.exists() {
                        let target = std::fs::read_to_string(&common_dir_path).ok()?;
                        resolve_against(&git_dir, target.trim())
                    } else {
                        git_dir.clone()
                    };
                    return Some(GitPaths {
                        repo_dir: to_string(&dir),
                        common_git_dir: to_string(&common_git_dir),
                        head_path: to_string(&head_path),
                    });
                }
            } else if stat.is_dir() {
                let head_path = git_path.join("HEAD");
                if !head_path.exists() {
                    return None;
                }
                return Some(GitPaths {
                    repo_dir: to_string(&dir),
                    common_git_dir: to_string(&git_path),
                    head_path: to_string(&head_path),
                });
            }
        }
        let parent = dir.parent().map(Path::to_path_buf);
        match parent {
            Some(parent) if parent != dir => dir = parent,
            _ => return None,
        }
    }
}

fn resolve_against(base: &Path, target: &str) -> PathBuf {
    let candidate = Path::new(target);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        crate::utils::paths::resolve_path_default(target, &to_string(base))
            .map(PathBuf::from)
            .unwrap_or_else(|_| base.join(target))
    }
}

const GIT_BRANCH_ARGS: [&str; 4] = [
    "--no-optional-locks",
    "symbolic-ref",
    "--quiet",
    "--short",
];

/// Asks git for the current branch. `None` on a detached HEAD or when git is
/// unavailable.
fn resolve_branch_with_git_sync(repo_dir: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(GIT_BRANCH_ARGS)
        .arg("HEAD")
        .current_dir(repo_dir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!branch.is_empty()).then_some(branch)
}

async fn resolve_branch_with_git_async(repo_dir: &str) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(GIT_BRANCH_ARGS)
        .arg("HEAD")
        .current_dir(repo_dir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!branch.is_empty()).then_some(branch)
}

fn is_wsl_environment() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var_os("WSL_DISTRO_NAME").is_some()
            || std::env::var_os("WSL_INTEROP").is_some())
}

fn is_windows_mounted_repo_path(repo_dir: &str) -> bool {
    let bytes = repo_dir.as_bytes();
    if !repo_dir.starts_with("/mnt/") || bytes.len() < 6 {
        return false;
    }
    let drive = bytes[5] as char;
    if !drive.is_ascii_alphabetic() {
        return false;
    }
    bytes.len() == 6 || bytes[6] == b'/'
}

/// Whether HEAD has to be polled instead of watched. Inotify does not see
/// changes a Windows process makes under `/mnt`.
fn should_poll_git_head(repo_dir: &str) -> bool {
    is_wsl_environment() && is_windows_mounted_repo_path(repo_dir)
}

const WATCH_DEBOUNCE_MS: u64 = 500;

type BranchCallback = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Watchers {
    head: Option<FsWatcher>,
    head_poll: Option<JoinHandle<()>>,
    reftable: Option<FsWatcher>,
    reftable_tables_list: Option<FsWatcher>,
    reftable_tables_list_poll: Option<JoinHandle<()>>,
    retry: Option<JoinHandle<()>>,
    refresh: Option<JoinHandle<()>>,
}

impl Watchers {
    fn clear_git_watchers(&mut self) {
        self.head = None;
        if let Some(handle) = self.head_poll.take() {
            handle.abort();
        }
        self.reftable = None;
        self.reftable_tables_list = None;
        if let Some(handle) = self.reftable_tables_list_poll.take() {
            handle.abort();
        }
        if let Some(handle) = self.retry.take() {
            handle.abort();
        }
    }
}

struct Inner {
    /// Captured when the provider is built. The watcher callbacks run on the
    /// file-watcher's own thread, where `Handle::try_current()` would find no
    /// runtime, so the debounce and retry timers need a handle to spawn onto.
    runtime: Option<tokio::runtime::Handle>,
    cwd: Mutex<String>,
    /// `None` = not read yet (TS `undefined`); `Some(None)` = not in a repo.
    cached_branch: Mutex<Option<Option<String>>>,
    git_paths: Mutex<Option<GitPaths>>,
    watchers: Mutex<Watchers>,
    branch_change_callbacks: Mutex<Vec<(u64, BranchCallback)>>,
    next_callback_id: AtomicU64,
    available_provider_count: AtomicU64,
    refresh_in_flight: AtomicBool,
    refresh_pending: AtomicBool,
    disposed: AtomicBool,
}

/// Provides the git branch, plus the provider count the footer prints beside it.
pub struct FooterDataProvider {
    inner: Arc<Inner>,
}

impl FooterDataProvider {
    pub fn new(cwd: &str) -> Self {
        let inner = Arc::new(Inner {
            runtime: tokio::runtime::Handle::try_current().ok(),
            cwd: Mutex::new(cwd.to_string()),
            cached_branch: Mutex::new(None),
            git_paths: Mutex::new(find_git_paths(cwd)),
            watchers: Mutex::new(Watchers::default()),
            branch_change_callbacks: Mutex::new(Vec::new()),
            next_callback_id: AtomicU64::new(0),
            available_provider_count: AtomicU64::new(0),
            refresh_in_flight: AtomicBool::new(false),
            refresh_pending: AtomicBool::new(false),
            disposed: AtomicBool::new(false),
        });
        Inner::setup_git_watcher(&inner);
        Self { inner }
    }

    /// The current git branch, `None` outside a repository, `"detached"` on a
    /// detached HEAD.
    pub fn get_git_branch(&self) -> Option<String> {
        let mut cached = self.inner.cached_branch.lock().expect("poisoned");
        if cached.is_none() {
            *cached = Some(self.inner.resolve_git_branch_sync());
        }
        cached.clone().flatten()
    }

    /// Subscribes to branch changes. The returned handle unsubscribes.
    pub fn on_branch_change(&self, callback: BranchCallback) -> BranchChangeSubscription {
        let id = self.inner.next_callback_id.fetch_add(1, Ordering::Relaxed);
        self.inner
            .branch_change_callbacks
            .lock()
            .expect("poisoned")
            .push((id, callback));
        BranchChangeSubscription {
            inner: Arc::downgrade(&self.inner),
            id,
        }
    }

    /// Number of distinct providers with available models, for the footer.
    pub fn get_available_provider_count(&self) -> u64 {
        self.inner.available_provider_count.load(Ordering::Relaxed)
    }

    pub fn set_available_provider_count(&self, count: u64) {
        self.inner
            .available_provider_count
            .store(count, Ordering::Relaxed);
    }

    /// The git paths the provider resolved for its cwd, if any.
    pub fn git_paths(&self) -> Option<GitPaths> {
        self.inner.git_paths.lock().expect("poisoned").clone()
    }

    pub fn set_cwd(&self, cwd: &str) {
        {
            let mut current = self.inner.cwd.lock().expect("poisoned");
            if *current == cwd {
                return;
            }
            *current = cwd.to_string();
        }

        {
            let mut watchers = self.inner.watchers.lock().expect("poisoned");
            if let Some(handle) = watchers.refresh.take() {
                handle.abort();
            }
            watchers.clear_git_watchers();
        }
        *self.inner.cached_branch.lock().expect("poisoned") = None;
        *self.inner.git_paths.lock().expect("poisoned") = find_git_paths(cwd);
        Inner::setup_git_watcher(&self.inner);
        self.inner.notify_branch_change();
    }

    pub fn dispose(&self) {
        self.inner.disposed.store(true, Ordering::Relaxed);
        let mut watchers = self.inner.watchers.lock().expect("poisoned");
        if let Some(handle) = watchers.refresh.take() {
            handle.abort();
        }
        watchers.clear_git_watchers();
        drop(watchers);
        self.inner
            .branch_change_callbacks
            .lock()
            .expect("poisoned")
            .clear();
    }
}

impl Drop for FooterDataProvider {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// Handle returned by [`FooterDataProvider::on_branch_change`]; drop it or call
/// [`BranchChangeSubscription::unsubscribe`] to stop being called.
pub struct BranchChangeSubscription {
    inner: Weak<Inner>,
    id: u64,
}

impl BranchChangeSubscription {
    pub fn unsubscribe(self) {}
}

impl Drop for BranchChangeSubscription {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner
                .branch_change_callbacks
                .lock()
                .expect("poisoned")
                .retain(|(id, _)| *id != self.id);
        }
    }
}

impl Inner {
    fn notify_branch_change(&self) {
        let callbacks: Vec<BranchCallback> = self
            .branch_change_callbacks
            .lock()
            .expect("poisoned")
            .iter()
            .map(|(_, callback)| Arc::clone(callback))
            .collect();
        for callback in callbacks {
            callback();
        }
    }

    fn schedule_refresh(inner: &Arc<Self>) {
        if inner.disposed.load(Ordering::Relaxed) {
            return;
        }
        if inner.refresh_in_flight.load(Ordering::Relaxed) {
            inner.refresh_pending.store(true, Ordering::Relaxed);
            return;
        }
        let Some(handle) = inner.runtime.as_ref() else {
            return;
        };
        let mut watchers = inner.watchers.lock().expect("poisoned");
        if watchers.refresh.is_some() {
            return;
        }
        let weak = Arc::downgrade(inner);
        watchers.refresh = Some(handle.spawn(async move {
            tokio::time::sleep(Duration::from_millis(WATCH_DEBOUNCE_MS)).await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            inner.watchers.lock().expect("poisoned").refresh = None;
            Inner::refresh_git_branch_async(&inner).await;
        }));
    }

    async fn refresh_git_branch_async(inner: &Arc<Self>) {
        if inner.disposed.load(Ordering::Relaxed) {
            return;
        }
        if inner.refresh_in_flight.load(Ordering::Relaxed) {
            inner.refresh_pending.store(true, Ordering::Relaxed);
            return;
        }

        inner.refresh_in_flight.store(true, Ordering::Relaxed);
        let next_branch = inner.resolve_git_branch_async().await;
        let mut changed = false;
        if !inner.disposed.load(Ordering::Relaxed) {
            let mut cached = inner.cached_branch.lock().expect("poisoned");
            if let Some(previous) = cached.clone()
                && previous != next_branch
            {
                changed = true;
            }
            *cached = Some(next_branch);
        }
        inner.refresh_in_flight.store(false, Ordering::Relaxed);
        if changed {
            inner.notify_branch_change();
        }
        if inner.refresh_pending.load(Ordering::Relaxed) && !inner.disposed.load(Ordering::Relaxed) {
            inner.refresh_pending.store(false, Ordering::Relaxed);
            Inner::schedule_refresh(inner);
        }
    }

    fn head_content(&self) -> Option<(String, String)> {
        let paths = self.git_paths.lock().expect("poisoned").clone()?;
        let content = std::fs::read_to_string(&paths.head_path).ok()?;
        Some((content.trim().to_string(), paths.repo_dir))
    }

    fn resolve_git_branch_sync(&self) -> Option<String> {
        let (content, repo_dir) = self.head_content()?;
        match content.strip_prefix("ref: refs/heads/") {
            Some(branch) if branch == ".invalid" => Some(
                resolve_branch_with_git_sync(&repo_dir).unwrap_or_else(|| "detached".to_string()),
            ),
            Some(branch) => Some(branch.to_string()),
            None => Some("detached".to_string()),
        }
    }

    async fn resolve_git_branch_async(&self) -> Option<String> {
        let (content, repo_dir) = self.head_content()?;
        match content.strip_prefix("ref: refs/heads/") {
            Some(branch) if branch == ".invalid" => Some(
                resolve_branch_with_git_async(&repo_dir)
                    .await
                    .unwrap_or_else(|| "detached".to_string()),
            ),
            Some(branch) => Some(branch.to_string()),
            None => Some("detached".to_string()),
        }
    }

    fn schedule_git_watcher_retry(inner: &Arc<Self>) {
        if inner.disposed.load(Ordering::Relaxed) {
            return;
        }
        let Some(handle) = inner.runtime.as_ref() else {
            return;
        };
        let mut watchers = inner.watchers.lock().expect("poisoned");
        if watchers.retry.is_some() {
            return;
        }
        let weak = Arc::downgrade(inner);
        watchers.retry = Some(handle.spawn(async move {
            tokio::time::sleep(Duration::from_millis(FS_WATCH_RETRY_DELAY_MS)).await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            inner.watchers.lock().expect("poisoned").retry = None;
            Inner::setup_git_watcher(&inner);
        }));
    }

    fn handle_git_watcher_error(inner: &Arc<Self>) {
        inner
            .watchers
            .lock()
            .expect("poisoned")
            .clear_git_watchers();
        Inner::schedule_git_watcher_retry(inner);
    }

    fn setup_git_watcher(inner: &Arc<Self>) {
        inner
            .watchers
            .lock()
            .expect("poisoned")
            .clear_git_watchers();
        let Some(paths) = inner.git_paths.lock().expect("poisoned").clone() else {
            return;
        };

        let poll_git_head = should_poll_git_head(&paths.repo_dir);

        let refresh: Arc<dyn Fn(Option<String>) + Send + Sync> = {
            let weak = Arc::downgrade(inner);
            Arc::new(move |_name: Option<String>| {
                if let Some(inner) = weak.upgrade() {
                    Inner::schedule_refresh(&inner);
                }
            })
        };
        let on_error: Arc<dyn Fn() + Send + Sync> = {
            let weak = Arc::downgrade(inner);
            Arc::new(move || {
                if let Some(inner) = weak.upgrade() {
                    Inner::handle_git_watcher_error(&inner);
                }
            })
        };

        // Watch the directory holding HEAD, not HEAD itself: git writes HEAD by
        // renaming a temporary over it, which changes the inode.
        let head_dir = Path::new(&paths.head_path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let head_listener: Arc<dyn Fn(Option<String>) + Send + Sync> = {
            let refresh = Arc::clone(&refresh);
            Arc::new(move |name: Option<String>| {
                if name.as_deref().is_none_or(|name| name == "HEAD") {
                    refresh(None);
                }
            })
        };
        let head_watcher =
            watch_with_error_handler(&head_dir, head_listener, Arc::clone(&on_error));
        let has_head_watcher = head_watcher.is_some();
        inner.watchers.lock().expect("poisoned").head = head_watcher;

        if poll_git_head {
            let handle = Self::spawn_file_poll(
                inner,
                PathBuf::from(&paths.head_path),
                1000,
                Arc::clone(&refresh),
            );
            inner.watchers.lock().expect("poisoned").head_poll = handle;
        }
        if !has_head_watcher && !poll_git_head {
            return;
        }

        // Reftable repositories record branch switches in the reftable
        // directory instead of HEAD.
        let reftable_dir = Path::new(&paths.common_git_dir).join("reftable");
        if !reftable_dir.exists() {
            return;
        }

        let reftable_watcher =
            watch_with_error_handler(&reftable_dir, Arc::clone(&refresh), Arc::clone(&on_error));
        if reftable_watcher.is_none() {
            return;
        }
        inner.watchers.lock().expect("poisoned").reftable = reftable_watcher;

        let tables_list_path = reftable_dir.join("tables.list");
        if !tables_list_path.exists() {
            return;
        }
        let tables_list_watcher =
            watch_with_error_handler(&tables_list_path, Arc::clone(&refresh), on_error);
        if tables_list_watcher.is_none() {
            return;
        }
        let poll = Self::spawn_file_poll(inner, tables_list_path, 250, refresh);
        let mut watchers = inner.watchers.lock().expect("poisoned");
        watchers.reftable_tables_list = tables_list_watcher;
        watchers.reftable_tables_list_poll = poll;
    }

    /// `fs.watchFile`: polls the file and reports a change when its metadata
    /// moved.
    ///
    /// Deviation (class 1): the comparison is modification time plus size. Node
    /// also compares the inode change time, which Rust exposes only per
    /// platform; every write that changes ctime here also changes mtime.
    fn spawn_file_poll(
        inner: &Arc<Self>,
        path: PathBuf,
        interval_ms: u64,
        on_change: Arc<dyn Fn(Option<String>) + Send + Sync>,
    ) -> Option<JoinHandle<()>> {
        let handle = inner.runtime.as_ref()?;
        let weak = Arc::downgrade(inner);
        Some(handle.spawn(async move {
            let mut previous = file_stamp(&path);
            let mut ticker = tokio::time::interval(Duration::from_millis(interval_ms));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if weak.upgrade().is_none() {
                    return;
                }
                let current = file_stamp(&path);
                if current != previous {
                    previous = current;
                    on_change(None);
                }
            }
        }))
    }
}

fn file_stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}
