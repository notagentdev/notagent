use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::config::CONFIG_DIR_NAME;

/// Reads whether atomic leases are enabled, at call time, so `/leases on|off`
/// applies without a session restart. An absent gate means disabled, matching
pub type LeaseGate = Arc<dyn Fn() -> bool + Send + Sync>;

/// Extra time granted by [`LeaseCoordinator::prepare_commit`] to finish the
/// actual write after the pre-commit checks passed.
const COMMIT_GRACE_MS: u64 = 5_000;

/// Lifecycle phase of a [`FileLease`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseStatus {
    /// The holder has reserved the file and is preparing its change.
    Working,
    /// The holder passed the pre-commit checks and is about to write.
    Committing,
}

impl fmt::Display for LeaseStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeaseStatus::Working => write!(formatter, "working"),
            LeaseStatus::Committing => write!(formatter, "committing"),
        }
    }
}

/// An advisory, time-bounded reservation of a single file.
/// Only one holder may modify a file at a time, and the content hash taken at
/// reservation time (`base_hash`) detects external modifications before the
/// write is committed. A lease expires automatically at `lease_until_ms`, so a
/// crashed holder never blocks a file for longer than its TTL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileLease {
    pub lease_id: String,
    /// Identity of the holder, formatted as `pid:<process id>`.
    pub agent_id: String,
    pub file_path: PathBuf,
    pub tool_name: String,
    /// Content hash of the file at reservation time; `None` when the file did
    /// not exist.
    pub base_hash: Option<String>,
    pub acquired_at_ms: u64,
    pub expected_duration_ms: u64,
    pub lease_until_ms: u64,
    pub status: LeaseStatus,
}

impl FileLease {
    pub fn new(lease_id: impl Into<String>, file_path: PathBuf) -> Self {
        Self {
            lease_id: lease_id.into(),
            agent_id: String::new(),
            file_path,
            tool_name: String::new(),
            base_hash: None,
            acquired_at_ms: 0,
            expected_duration_ms: 0,
            lease_until_ms: 0,
            status: LeaseStatus::Working,
        }
    }

    #[must_use]
    pub fn agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = agent_id.into();
        self
    }

    #[must_use]
    pub fn tool_name(mut self, tool_name: impl Into<String>) -> Self {
        self.tool_name = tool_name.into();
        self
    }

    #[must_use]
    pub fn base_hash(mut self, base_hash: Option<String>) -> Self {
        self.base_hash = base_hash;
        self
    }

    #[must_use]
    pub fn acquired_at_ms(mut self, acquired_at_ms: u64) -> Self {
        self.acquired_at_ms = acquired_at_ms;
        self
    }

    #[must_use]
    pub fn expected_duration_ms(mut self, expected_duration_ms: u64) -> Self {
        self.expected_duration_ms = expected_duration_ms;
        self
    }

    #[must_use]
    pub fn lease_until_ms(mut self, lease_until_ms: u64) -> Self {
        self.lease_until_ms = lease_until_ms;
        self
    }

    #[must_use]
    pub fn status(mut self, status: LeaseStatus) -> Self {
        self.status = status;
        self
    }

    pub fn is_expired(&self, now_ms: u64) -> bool {
        now_ms >= self.lease_until_ms
    }

    pub fn is_active(&self, now_ms: u64) -> bool {
        !self.is_expired(now_ms)
    }

    /// Process id of the holder, when `agent_id` follows the `pid:<n>`
    /// convention.
    pub fn holder_pid(&self) -> Option<u32> {
        self.agent_id.strip_prefix("pid:")?.parse().ok()
    }

    /// Whole seconds until the lease expires; 0 when already expired.
    pub fn remaining_secs(&self, now_ms: u64) -> u64 {
        self.lease_until_ms.saturating_sub(now_ms) / 1000
    }
}

/// The lease failures a tool reports to the model.
/// The four lifecycle messages are the reference's, word for word: they are
/// what the model reads and act on ("retry shortly"). The storage variants
/// carry the contexts the reference attaches with `anyhow`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LeaseError {
    #[error(
        "file is currently leased: {path} (held by {holder}, free in ~{remaining_secs}s — retry shortly)"
    )]
    AlreadyLeased {
        path: String,
        holder: String,
        remaining_secs: u64,
    },
    #[error("lease expired for file: {path}")]
    Expired { path: String, lease_id: String },
    #[error("lease base hash mismatch for file: {path}")]
    BaseHashMismatch {
        path: String,
        expected: Option<String>,
        actual: Option<String>,
    },
    #[error("lease is no longer owned by the current operation: {path}")]
    NotOwner { path: String, lease_id: String },
    #[error("Failed to create lease directory: {0}")]
    CreateDirectory(String),
    #[error("Failed to create lease file: {0}")]
    CreateFile(String),
    #[error("Failed to read lease file: {0}")]
    ReadFile(String),
    #[error("Failed to write lease file: {0}")]
    WriteFile(String),
    #[error("Failed to list lease directory: {0}")]
    ListDirectory(String),
    #[error("Failed to encode lease: {0}")]
    Encode(String),
    #[error("Failed to decode lease file: {0}")]
    Decode(String),
    #[error("Failed to acquire lease after stale cleanup retry")]
    StaleCleanupExhausted,
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Content hash of a file, as the lease records it at reservation time.
fn compute_hash_bytes(content: &[u8]) -> String {
    hex_encode(&Sha256::digest(content))
}

/// Derives the lease file name for a target path.
/// The absolute path string is hashed so the lease directory stays flat and
/// file-system safe regardless of the target's depth or characters.
fn compute_path_key(path: &Path) -> String {
    hex_encode(&Sha256::digest(path.to_string_lossy().as_bytes()))
}

/// Milliseconds since the Unix epoch.
/// If the system clock is set before 1970-01-01 (clock skew, misconfigured
/// hardware) `duration_since` returns an error; we fall back to 0 instead of
/// panicking so lease bookkeeping never takes the process down. A 0 value
/// makes existing leases appear acquired at the epoch, which naturally flows
/// through the expiry logic as "very old" and lets the system recover once the
/// clock is corrected.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Returns whether the process with the given pid is still running.
/// Used to treat leases of crashed holders as stale immediately instead of
/// waiting out their TTL. On Unix this probes with `kill(pid, 0)`: success or
/// `EPERM` (process exists but belongs to another user) both mean alive. On
/// other platforms there is no cheap probe, so we conservatively report alive
/// and fall back to TTL-based expiry.
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(pid) else {
            return true;
        };
        // Signal 0 sends nothing; it only reports whether the pid can be
        // signalled, which is the existence probe we want.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Returns whether the stored lease still blocks a new acquisition at
/// `now_ms`: it must be unexpired and its holder process still alive.
fn is_blocking(lease: &FileLease, now_ms: u64) -> bool {
    if lease.is_expired(now_ms) {
        return false;
    }
    match lease.holder_pid() {
        Some(pid) => is_process_alive(pid),
        None => true,
    }
}

/// File-system backed lease store. All readers and mutations of one lease share
/// an OS lock on a separate, stable file; replacing JSON never replaces the lock.
pub struct FileLeaseStore {
    lease_dir: PathBuf,
}

impl FileLeaseStore {
    pub fn new(lease_dir: PathBuf) -> Self {
        Self { lease_dir }
    }

    pub fn for_workspace(cwd: &str) -> Self {
        Self::new(Path::new(cwd).join(CONFIG_DIR_NAME).join("leases"))
    }

    fn lease_file_path(&self, path: &Path) -> PathBuf {
        self.lease_dir
            .join(format!("{}.json", compute_path_key(path)))
    }

    async fn with_locked_file<T: Send + 'static>(
        lease_path: PathBuf,
        body: impl FnOnce(&Path) -> Result<T, LeaseError> + Send + 'static,
    ) -> Result<T, LeaseError> {
        // Keep the lock and all filesystem work in the same blocking job.
        // Cancelling its await cannot unlock while a write is still in flight.
        tokio::task::spawn_blocking(move || {
            let parent = lease_path.parent().ok_or_else(|| {
                LeaseError::CreateDirectory("Lease has no parent directory".to_owned())
            })?;
            std::fs::create_dir_all(parent)
                .map_err(|error| LeaseError::CreateDirectory(error.to_string()))?;
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lease_path.with_extension("lock"))
                .map_err(|error| LeaseError::CreateFile(error.to_string()))?;
            lock.lock()
                .map_err(|error| LeaseError::CreateFile(error.to_string()))?;
            // Lock files must not be unlinked: waiters hold their original inode.
            body(&lease_path)
        })
        .await
        .map_err(|error| LeaseError::WriteFile(error.to_string()))?
    }

    fn read_lease(path: &Path) -> Result<Option<FileLease>, LeaseError> {
        match std::fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|error| LeaseError::Decode(error.to_string())),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(LeaseError::ReadFile(error.to_string())),
        }
    }

    fn write_lease(path: &Path, lease: &FileLease) -> Result<(), LeaseError> {
        use std::io::Write;
        let content =
            serde_json::to_vec(lease).map_err(|error| LeaseError::Encode(error.to_string()))?;
        let parent = path.parent().ok_or_else(|| {
            LeaseError::CreateDirectory("Lease has no parent directory".to_owned())
        })?;
        let mut file = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| LeaseError::CreateFile(error.to_string()))?;
        file.write_all(&content)
            .and_then(|()| file.as_file().sync_all())
            .map_err(|error| LeaseError::WriteFile(error.to_string()))?;
        file.persist(path)
            .map_err(|error| LeaseError::WriteFile(error.error.to_string()))?;
        Ok(())
    }

    pub async fn try_acquire(&self, lease: FileLease) -> Result<FileLease, LeaseError> {
        Self::with_locked_file(self.lease_file_path(&lease.file_path), move |path| {
            if let Some(current) = Self::read_lease(path)?
                && is_blocking(&current, lease.acquired_at_ms)
            {
                return Err(LeaseError::AlreadyLeased {
                    path: lease.file_path.display().to_string(),
                    remaining_secs: current.remaining_secs(lease.acquired_at_ms),
                    holder: current.agent_id,
                });
            }
            Self::write_lease(path, &lease)?;
            Ok(lease)
        })
        .await
    }

    pub async fn get_by_path(&self, path: &Path) -> Result<Option<FileLease>, LeaseError> {
        Self::with_locked_file(self.lease_file_path(path), Self::read_lease).await
    }

    pub async fn update(&self, lease: FileLease) -> Result<FileLease, LeaseError> {
        Self::with_locked_file(self.lease_file_path(&lease.file_path), move |path| {
            let current = Self::read_lease(path)?;
            if !current.is_some_and(|current| current.lease_id == lease.lease_id) {
                return Err(LeaseError::NotOwner {
                    path: lease.file_path.display().to_string(),
                    lease_id: lease.lease_id.clone(),
                });
            }
            Self::write_lease(path, &lease)?;
            Ok(lease)
        })
        .await
    }

    pub async fn release(&self, path: &Path, lease_id: &str) -> Result<(), LeaseError> {
        let target = path.to_path_buf();
        let lease_id = lease_id.to_owned();
        Self::with_locked_file(self.lease_file_path(path), move |path| {
            if let Some(current) = Self::read_lease(path)? {
                if current.lease_id != lease_id {
                    return Err(LeaseError::NotOwner {
                        path: target.display().to_string(),
                        lease_id,
                    });
                }
                std::fs::remove_file(path)
                    .map_err(|error| LeaseError::WriteFile(error.to_string()))?;
            }
            Ok(())
        })
        .await
    }

    /// Lists active leases and cleans up expired ones under the same lock used
    /// by acquisition, updates and release.
    pub async fn list(&self, path: Option<&Path>) -> Result<Vec<FileLease>, LeaseError> {
        let mut entries = match fs::read_dir(&self.lease_dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(LeaseError::ListDirectory(error.to_string())),
        };
        let mut leases = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| LeaseError::ListDirectory(error.to_string()))?
        {
            let entry_path = entry.path();
            if entry_path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let lease = Self::with_locked_file(entry_path, |path| {
                let lease = match Self::read_lease(path) {
                    Ok(Some(lease)) => lease,
                    Ok(None) | Err(LeaseError::Decode(_) | LeaseError::ReadFile(_)) => {
                        return Ok(None);
                    }
                    Err(error) => return Err(error),
                };
                if lease.is_expired(now_ms()) {
                    std::fs::remove_file(path)
                        .map_err(|error| LeaseError::WriteFile(error.to_string()))?;
                    return Ok(None);
                }
                Ok(Some(lease))
            })
            .await?;
            if let Some(lease) = lease
                && path.is_none_or(|filter| lease.file_path.starts_with(filter))
            {
                leases.push(lease);
            }
        }
        leases.sort_by(|left, right| left.file_path.cmp(&right.file_path));
        Ok(leases)
    }
}

/// Coordinates the two-phase lease protocol used by the mutating file tools.
/// Phase one ([`Self::reserve`]) records a content hash of the target file and
/// acquires the lease; phase two ([`Self::prepare_commit`]) re-validates
/// ownership and content immediately before the write. Releasing is explicit
/// so callers can clean up on both the success and the error path.
pub struct LeaseCoordinator {
    store: FileLeaseStore,
    enabled: Option<LeaseGate>,
}

impl LeaseCoordinator {
    pub fn new(cwd: &str, enabled: Option<LeaseGate>) -> Self {
        Self {
            store: FileLeaseStore::for_workspace(cwd),
            enabled,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.as_ref().is_some_and(|enabled| enabled())
    }

    /// Returns `None` when atomic leases are disabled, `Some(lease)` when
    /// enabled and the file could be reserved.
    pub async fn reserve(
        &self,
        path: &Path,
        tool_name: &str,
        expected_duration_ms: u64,
    ) -> Result<Option<FileLease>, LeaseError> {
        if !self.is_enabled() {
            return Ok(None);
        }

        let now_ms = now_ms();
        let base_hash = read_hash(path).await;

        let lease = FileLease::new(uuid::Uuid::new_v4().to_string(), path.to_path_buf())
            .agent_id(format!("pid:{}", std::process::id()))
            .tool_name(tool_name)
            .base_hash(base_hash)
            .acquired_at_ms(now_ms)
            .expected_duration_ms(expected_duration_ms)
            .lease_until_ms(now_ms + expected_duration_ms)
            .status(LeaseStatus::Working);

        self.store.try_acquire(lease).await.map(Some)
    }

    /// Re-validates the lease right before the write: it must be unexpired,
    /// still owned by this operation, and the file content must match the hash
    /// recorded at reservation time.
    pub async fn prepare_commit(&self, lease: FileLease) -> Result<FileLease, LeaseError> {
        let now_ms = now_ms();
        if lease.is_expired(now_ms) {
            return Err(LeaseError::Expired {
                path: lease.file_path.display().to_string(),
                lease_id: lease.lease_id,
            });
        }

        let current = self
            .store
            .get_by_path(&lease.file_path)
            .await?
            .ok_or_else(|| LeaseError::NotOwner {
                path: lease.file_path.display().to_string(),
                lease_id: lease.lease_id.clone(),
            })?;

        if current.lease_id != lease.lease_id {
            return Err(LeaseError::NotOwner {
                path: lease.file_path.display().to_string(),
                lease_id: lease.lease_id.clone(),
            });
        }

        let actual_hash = read_hash(&lease.file_path).await;
        if actual_hash != lease.base_hash {
            return Err(LeaseError::BaseHashMismatch {
                path: lease.file_path.display().to_string(),
                expected: lease.base_hash,
                actual: actual_hash,
            });
        }

        self.store
            .update(
                lease
                    .status(LeaseStatus::Committing)
                    .lease_until_ms(now_ms + COMMIT_GRACE_MS),
            )
            .await
    }

    pub async fn release(&self, lease: &FileLease) -> Result<(), LeaseError> {
        self.store.release(&lease.file_path, &lease.lease_id).await
    }

    /// Releases the lease (best effort) when `result` is an error, so a
    /// failure inside the tool never leaves the file blocked until the lease's
    /// TTL runs out. `Ok` results keep the lease, because the success path
    /// releases it itself once the write has landed.
    pub async fn release_on_error<T, E>(
        &self,
        lease: Option<&FileLease>,
        result: Result<T, E>,
    ) -> Result<T, E> {
        if result.is_err()
            && let Some(lease) = lease
        {
            let _ = self.release(lease).await;
        }
        result
    }
}

/// The content hash of `path`, or `None` when it cannot be read (it does not
/// exist yet, or it is not readable) — the reference treats both the same way.
async fn read_hash(path: &Path) -> Option<String> {
    fs::read(path)
        .await
        .ok()
        .map(|bytes| compute_hash_bytes(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_waiting_process_cannot_replace_a_renewed_lease() {
        let fixture = Fixture::new();
        let old = fixture.live_lease().lease_until_ms(1);
        fixture.store.try_acquire(old).await.expect("old lease");
        let lease_path = fixture.store.lease_file_path(&fixture.file_path);
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(lease_path.with_extension("lock"))
            .expect("lock file");
        lock.lock().expect("hold lock");
        let mut child =
            std::process::Command::new(std::env::current_exe().expect("test executable"))
                .args([
                    "--exact",
                    "core::tools::file_lease::tests::lease_acquisition_process_probe",
                    "--nocapture",
                ])
                .env("NOTAGENT_LEASE_PROBE_DIR", &fixture.directory.path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("child");
        let ready = fixture.directory.path.join("ready");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let completed_while_locked = fixture.directory.path.join("result").exists();
        let current = fixture
            .live_lease()
            .lease_id("renewed")
            .acquired_at_ms(now_ms())
            .lease_until_ms(now_ms() + 60_000);
        FileLeaseStore::write_lease(&lease_path, &current).expect("renew under lock");
        drop(lock);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().expect("child status") {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().expect("stop timed-out child");
                child.wait().expect("reap child");
                panic!("lease acquisition must not deadlock");
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        assert!(
            ready.exists() && status.success(),
            "probe must run successfully"
        );
        assert!(
            !completed_while_locked,
            "a different process must honor the metadata lock"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.directory.path.join("result")).expect("result"),
            "blocked"
        );
        assert_eq!(
            fixture
                .store
                .get_by_path(&fixture.file_path)
                .await
                .expect("lease"),
            Some(current)
        );
    }

    #[tokio::test]
    async fn lease_acquisition_process_probe() {
        let Some(directory) = std::env::var_os("NOTAGENT_LEASE_PROBE_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let store = FileLeaseStore::for_workspace(&directory.to_string_lossy());
        let lease = FileLease::new("contender", directory.join("test.rs"))
            .agent_id(format!("pid:{}", std::process::id()))
            .acquired_at_ms(now_ms())
            .lease_until_ms(now_ms() + 60_000);
        std::fs::write(directory.join("ready"), "ready").expect("ready");
        let outcome = match store.try_acquire(lease).await {
            Err(LeaseError::AlreadyLeased { .. }) => "blocked",
            Ok(_) => "acquired",
            Err(error) => panic!("unexpected lease error: {error}"),
        };
        std::fs::write(directory.join("result"), outcome).expect("result");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_stale_cleanup_has_exactly_one_new_owner() {
        let fixture = Fixture::new();
        fixture
            .store
            .try_acquire(fixture.live_lease().lease_until_ms(1))
            .await
            .expect("expired lease");
        let barrier = Arc::new(tokio::sync::Barrier::new(8));
        let mut tasks = Vec::new();
        for id in 0..8 {
            let store = FileLeaseStore::for_workspace(&fixture.directory.cwd());
            let lease = fixture
                .live_lease()
                .lease_id(format!("owner-{id}"))
                .acquired_at_ms(now_ms())
                .lease_until_ms(now_ms() + 60_000);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                store.try_acquire(lease).await
            }));
        }
        let mut winners = Vec::new();
        for task in tasks {
            match task.await.expect("contender") {
                Ok(lease) => winners.push(lease),
                Err(LeaseError::AlreadyLeased { .. }) => {}
                Err(error) => panic!("contention must not produce corrupt leases: {error}"),
            }
        }
        assert_eq!(
            winners.len(),
            1,
            "only one contender may own the replacement"
        );
        fixture.store.list(None).await.expect("cleanup");
        assert_eq!(
            fixture
                .store
                .get_by_path(&fixture.file_path)
                .await
                .expect("lease"),
            winners.pop()
        );
    }

    impl FileLease {
        fn test() -> Self {
            Self::new("lease-1", PathBuf::from("/tmp/file.rs"))
                .agent_id("pid:42")
                .tool_name("write")
                .base_hash(Some("base123".to_owned()))
                .acquired_at_ms(1)
                .expected_duration_ms(10_000)
                .lease_until_ms(10_001)
                .status(LeaseStatus::Working)
        }

        fn file_path(mut self, file_path: PathBuf) -> Self {
            self.file_path = file_path;
            self
        }

        fn lease_id(mut self, lease_id: impl Into<String>) -> Self {
            self.lease_id = lease_id.into();
            self
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let directory = tempfile::Builder::new()
                .prefix("notagent-file-lease-")
                .tempdir()
                .expect("temp dir");
            let path = directory.path().to_path_buf();
            let _ = directory.keep();
            Self { path }
        }

        fn cwd(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// A workspace with one file, its store, and leases held by this very
    /// process, so the holder is alive.
    struct Fixture {
        store: FileLeaseStore,
        file_path: PathBuf,
        directory: TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = TempDir::new();
            let file_path = directory.path.join("test.rs");
            std::fs::write(&file_path, "content").expect("writes");
            let store = FileLeaseStore::for_workspace(&directory.cwd());
            Self {
                store,
                file_path,
                directory,
            }
        }

        fn live_lease(&self) -> FileLease {
            FileLease::test()
                .file_path(self.file_path.clone())
                .agent_id(format!("pid:{}", std::process::id()))
        }

        fn coordinator(&self, enabled: bool) -> LeaseCoordinator {
            LeaseCoordinator::new(
                &self.directory.cwd(),
                Some(Arc::new(move || enabled) as LeaseGate),
            )
        }

        fn set_content(&self, content: &str) {
            std::fs::write(&self.file_path, content).expect("writes");
        }
    }

    // ---- the value type ---------------------------------------------------

    #[test]
    fn test_is_expired() {
        let fixture = FileLease::test().lease_until_ms(10);
        assert!(fixture.is_expired(11));
    }

    #[test]
    fn test_is_active() {
        let fixture = FileLease::test().lease_until_ms(20);
        assert!(fixture.is_active(10));
    }

    #[test]
    fn test_holder_pid() {
        let fixture = FileLease::test().agent_id("pid:1234");
        assert_eq!(fixture.holder_pid(), Some(1234));
    }

    #[test]
    fn test_holder_pid_invalid() {
        let fixture = FileLease::test().agent_id("worker-7");
        assert_eq!(fixture.holder_pid(), None);
    }

    #[test]
    fn test_remaining_secs() {
        let fixture = FileLease::test().lease_until_ms(12_500);
        assert_eq!(fixture.remaining_secs(2_000), 10);
    }

    #[test]
    fn test_remaining_secs_expired() {
        let fixture = FileLease::test().lease_until_ms(10);
        assert_eq!(fixture.remaining_secs(11), 0);
    }

    #[test]
    fn test_already_leased_message_is_actionable() {
        let actual = LeaseError::AlreadyLeased {
            path: "/tmp/file.rs".to_owned(),
            holder: "pid:42".to_owned(),
            remaining_secs: 12,
        }
        .to_string();

        assert!(actual.contains("free in ~12s"), "{actual}");
        assert!(actual.contains("pid:42"), "{actual}");
    }

    #[test]
    fn the_status_renders_the_reference_wording() {
        assert_eq!(LeaseStatus::Working.to_string(), "working");
        assert_eq!(LeaseStatus::Committing.to_string(), "committing");
    }

    // ---- the store --------------------------------------------------------

    #[test]
    fn the_lease_directory_is_workspace_local() {
        let store = FileLeaseStore::for_workspace("/workspace");
        assert_eq!(
            store.lease_dir,
            PathBuf::from("/workspace/.notagent/leases")
        );
    }

    #[test]
    fn the_lease_file_name_is_the_hash_of_the_target_path() {
        let store = FileLeaseStore::for_workspace("/workspace");
        let first = store.lease_file_path(Path::new("/a/deep/../file.rs"));
        let second = store.lease_file_path(Path::new("/a/other.rs"));
        assert_ne!(first, second);
        // Flat directory, one JSON file per target path.
        assert_eq!(first.parent(), Some(store.lease_dir.as_path()));
        assert_eq!(
            first.extension().and_then(|value| value.to_str()),
            Some("json")
        );
    }

    #[tokio::test]
    async fn test_try_acquire_rejects_active_lease() {
        let fixture = Fixture::new();
        let acquired = fixture
            .store
            .try_acquire(fixture.live_lease())
            .await
            .expect("first acquire");
        assert_eq!(acquired.lease_id, "lease-1");

        let actual = fixture
            .store
            .try_acquire(fixture.live_lease().lease_id("lease-2"))
            .await
            .expect_err("second acquire")
            .to_string();

        assert!(actual.contains("file is currently leased"), "{actual}");
    }

    #[tokio::test]
    async fn test_try_acquire_replaces_expired_lease() {
        let fixture = Fixture::new();
        let expired = fixture.live_lease().lease_until_ms(1);
        fixture.store.try_acquire(expired).await.expect("acquire");

        let actual = fixture
            .store
            .try_acquire(fixture.live_lease().lease_id("lease-2"))
            .await
            .expect("replaces the expired lease");

        assert_eq!(actual.lease_id, "lease-2");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_try_acquire_replaces_lease_of_dead_holder() {
        let fixture = Fixture::new();
        // Unexpired lease, but held by a pid that cannot exist.
        let dead = fixture.live_lease().agent_id("pid:99999999");
        fixture.store.try_acquire(dead).await.expect("acquire");

        let actual = fixture
            .store
            .try_acquire(fixture.live_lease().lease_id("lease-2"))
            .await
            .expect("replaces the dead holder's lease");

        assert_eq!(actual.lease_id, "lease-2");
    }

    #[tokio::test]
    async fn test_already_leased_reports_remaining_seconds() {
        let fixture = Fixture::new();
        // acquired_at_ms = 1, lease_until_ms = 10_001 -> ~10s remaining
        fixture
            .store
            .try_acquire(fixture.live_lease())
            .await
            .expect("acquire");

        let actual = fixture
            .store
            .try_acquire(fixture.live_lease().lease_id("lease-2"))
            .await
            .expect_err("blocked")
            .to_string();

        assert!(actual.contains("free in ~10s"), "{actual}");
    }

    #[tokio::test]
    async fn test_release_requires_ownership() {
        let fixture = Fixture::new();
        fixture
            .store
            .try_acquire(fixture.live_lease())
            .await
            .expect("acquire");

        let actual = fixture
            .store
            .release(&fixture.file_path, "other-lease")
            .await
            .expect_err("not the owner")
            .to_string();

        assert!(actual.contains("no longer owned"), "{actual}");
    }

    #[tokio::test]
    async fn releasing_removes_the_lease_file() {
        let fixture = Fixture::new();
        let lease = fixture
            .store
            .try_acquire(fixture.live_lease())
            .await
            .expect("acquire");

        fixture
            .store
            .release(&lease.file_path, &lease.lease_id)
            .await
            .expect("release");

        assert_eq!(
            fixture
                .store
                .get_by_path(&fixture.file_path)
                .await
                .expect("get"),
            None
        );
    }

    #[tokio::test]
    async fn test_list_skips_expired_leases() {
        let fixture = Fixture::new();
        let expired = fixture.live_lease().lease_until_ms(1);
        fixture.store.try_acquire(expired).await.expect("acquire");

        let actual = fixture.store.list(None).await.expect("list");

        assert_eq!(actual, Vec::new());
    }

    #[tokio::test]
    async fn listing_an_absent_lease_directory_is_empty() {
        let store = FileLeaseStore::for_workspace("/definitely/not/a/workspace");
        assert_eq!(store.list(None).await.expect("list"), Vec::new());
    }

    #[tokio::test]
    async fn listing_filters_by_path_prefix() {
        let fixture = Fixture::new();
        // `list` measures expiry against the wall clock, so this lease has to
        // outlive it — the fixture's epoch-relative timestamps would be purged.
        fixture
            .store
            .try_acquire(fixture.live_lease().lease_until_ms(u64::MAX))
            .await
            .expect("acquire");

        let matching = fixture
            .store
            .list(Some(&fixture.directory.path))
            .await
            .expect("list");
        assert_eq!(matching.len(), 1);

        let other = fixture
            .store
            .list(Some(Path::new("/somewhere/else")))
            .await
            .expect("list");
        assert_eq!(other, Vec::new());
    }

    // ---- the coordinator --------------------------------------------------

    #[tokio::test]
    async fn test_reserve_returns_none_when_disabled() {
        let fixture = Fixture::new();
        let coordinator = fixture.coordinator(false);

        let actual = coordinator
            .reserve(&fixture.file_path, "write", 10_000)
            .await
            .expect("reserve");

        assert_eq!(actual, None);
    }

    #[tokio::test]
    async fn an_absent_gate_leaves_leases_off() {
        let fixture = Fixture::new();
        let coordinator = LeaseCoordinator::new(&fixture.directory.cwd(), None);

        assert!(!coordinator.is_enabled());
        assert_eq!(
            coordinator
                .reserve(&fixture.file_path, "write", 10_000)
                .await
                .expect("reserve"),
            None
        );
    }

    #[tokio::test]
    async fn test_reserve_and_commit_roundtrip() {
        let fixture = Fixture::new();
        let coordinator = fixture.coordinator(true);

        let lease = coordinator
            .reserve(&fixture.file_path, "write", 10_000)
            .await
            .expect("reserve")
            .expect("lease should be Some when enabled");

        let actual = coordinator.prepare_commit(lease).await.expect("commit");

        assert_eq!(actual.status, LeaseStatus::Committing);
    }

    #[tokio::test]
    async fn test_prepare_commit_fails_on_base_hash_change() {
        let fixture = Fixture::new();
        let coordinator = fixture.coordinator(true);

        let lease = coordinator
            .reserve(&fixture.file_path, "write", 10_000)
            .await
            .expect("reserve")
            .expect("lease should be Some when enabled");

        fixture.set_content("changed outside\n");

        let actual = coordinator
            .prepare_commit(lease)
            .await
            .expect_err("the file moved underneath the holder")
            .to_string();

        assert!(actual.contains("lease base hash mismatch"), "{actual}");
    }

    #[tokio::test]
    async fn test_reserve_fails_when_file_is_already_leased() {
        let fixture = Fixture::new();
        let coordinator = fixture.coordinator(true);

        coordinator
            .reserve(&fixture.file_path, "write", 60_000)
            .await
            .expect("reserve")
            .expect("first reserve should succeed");

        let actual = coordinator
            .reserve(&fixture.file_path, "edit", 10_000)
            .await
            .expect_err("blocked")
            .to_string();

        assert!(actual.contains("file is currently leased"), "{actual}");
        assert!(actual.contains("held by"), "{actual}");
    }

    #[tokio::test]
    async fn a_file_that_does_not_exist_yet_reserves_with_an_empty_base_hash() {
        let fixture = Fixture::new();
        let coordinator = fixture.coordinator(true);
        let new_file = fixture.directory.path.join("new.rs");

        let lease = coordinator
            .reserve(&new_file, "write", 10_000)
            .await
            .expect("reserve")
            .expect("lease");
        assert_eq!(lease.base_hash, None);

        // Creating it is exactly what the holder is about to do, so the
        // pre-commit check must still pass.
        let committed = coordinator.prepare_commit(lease).await.expect("commit");
        assert_eq!(committed.status, LeaseStatus::Committing);
    }

    #[tokio::test]
    async fn prepare_commit_rejects_an_expired_lease() {
        let fixture = Fixture::new();
        let coordinator = fixture.coordinator(true);

        let lease = coordinator
            .reserve(&fixture.file_path, "write", 10_000)
            .await
            .expect("reserve")
            .expect("lease")
            .lease_until_ms(1);

        let actual = coordinator
            .prepare_commit(lease)
            .await
            .expect_err("expired")
            .to_string();

        assert!(actual.contains("lease expired for file"), "{actual}");
    }

    #[tokio::test]
    async fn release_on_error_frees_the_file_again() {
        let fixture = Fixture::new();
        let coordinator = fixture.coordinator(true);

        let lease = coordinator
            .reserve(&fixture.file_path, "write", 60_000)
            .await
            .expect("reserve")
            .expect("lease");

        let result: Result<(), &str> = coordinator
            .release_on_error(Some(&lease), Err("boom"))
            .await;
        assert_eq!(result, Err("boom"));

        // The next reserve succeeds because the failed one released.
        coordinator
            .reserve(&fixture.file_path, "edit", 10_000)
            .await
            .expect("reserve")
            .expect("lease");
    }

    #[tokio::test]
    async fn release_on_error_keeps_the_lease_when_the_operation_succeeded() {
        let fixture = Fixture::new();
        let coordinator = fixture.coordinator(true);

        let lease = coordinator
            .reserve(&fixture.file_path, "write", 60_000)
            .await
            .expect("reserve")
            .expect("lease");

        let result: Result<u8, &str> = coordinator.release_on_error(Some(&lease), Ok(1)).await;
        assert_eq!(result, Ok(1));

        let actual = coordinator
            .reserve(&fixture.file_path, "edit", 10_000)
            .await
            .expect_err("still held")
            .to_string();
        assert!(actual.contains("file is currently leased"), "{actual}");
    }
}
