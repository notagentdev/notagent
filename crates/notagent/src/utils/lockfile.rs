//! Inter-process file locking, the behaviour `proper-lockfile` gives the TS app.
//!
//! Tech substitution (master plan): `proper-lockfile` is replaced by a `<file>.lock`
//! directory. `mkdir` is atomic on every supported filesystem, the directory mtime
//! carries the heartbeat that makes an abandoned lock detectably stale, and a
//! background thread refreshes it while the lock is held.

use std::fs::{File, FileTimes};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};

pub type OnCompromised = Arc<dyn Fn(LockError) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LockError {
    /// `ELOCKED` — somebody else holds the lock.
    #[error("Lock file is already being held")]
    Locked,
    /// `ECOMPROMISED` — the lock was taken from us while we held it.
    #[error("{0}")]
    Compromised(String),
    #[error("{0}")]
    Io(String),
}

#[derive(Clone)]
pub struct LockOptions {
    /// How old a lock may get before another process may take it over.
    pub stale: Duration,
    /// Heartbeat interval; defaults to half the stale duration.
    pub update: Option<Duration>,
    /// Called once when the heartbeat notices that the lock is no longer ours.
    pub on_compromised: Option<OnCompromised>,
}

impl std::fmt::Debug for LockOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LockOptions")
            .field("stale", &self.stale)
            .field("update", &self.update)
            .field("on_compromised", &self.on_compromised.is_some())
            .finish()
    }
}

impl Default for LockOptions {
    fn default() -> Self {
        Self {
            stale: Duration::from_secs(10),
            update: None,
            on_compromised: None,
        }
    }
}

impl LockOptions {
    fn effective_stale(&self) -> Duration {
        self.stale.max(Duration::from_secs(5))
    }

    fn effective_update(&self) -> Duration {
        let stale = self.effective_stale();
        let default = stale / 2;
        // proper-lockfile clamps to [1s, stale/2]; the lower bound is relaxed so
        // that compromise handling stays testable without multi-second waits.
        self.update
            .unwrap_or(default)
            .clamp(Duration::from_millis(100), stale / 2)
    }
}

/// The path of the lock belonging to `path`.
pub fn lock_path_for(path: &Path) -> PathBuf {
    let mut lock_path = path.as_os_str().to_owned();
    lock_path.push(".lock");
    PathBuf::from(lock_path)
}

struct Heartbeat {
    stop: Arc<(Mutex<bool>, Condvar)>,
    handle: Option<std::thread::JoinHandle<()>>,
}

/// A held lock. Dropping it releases the lock.
pub struct LockGuard {
    lock_path: PathBuf,
    heartbeat: Option<Heartbeat>,
    compromised: Arc<AtomicBool>,
}

impl std::fmt::Debug for LockGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LockGuard")
            .field("lock_path", &self.lock_path)
            .field("compromised", &self.is_compromised())
            .finish()
    }
}

impl LockGuard {
    /// True once the heartbeat found the lock directory gone or taken over.
    pub fn is_compromised(&self) -> bool {
        self.compromised.load(Ordering::SeqCst)
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Release the lock. Idempotent; `Drop` calls it too.
    pub fn release(mut self) {
        self.release_in_place();
    }

    fn release_in_place(&mut self) {
        if let Some(mut heartbeat) = self.heartbeat.take() {
            {
                let (lock, condvar) = &*heartbeat.stop;
                *lock.lock().expect("heartbeat stop mutex") = true;
                condvar.notify_all();
            }
            if let Some(handle) = heartbeat.handle.take() {
                let _ = handle.join();
            }
        }
        // A compromised lock is no longer ours: the directory either vanished
        // or belongs to whoever took it over. proper-lockfile refuses the
        // release in this state ("Lock is not acquired/owned by you") instead
        // of deleting the successor's lock — removing it here would let a
        // third process acquire while the successor still believes it holds
        // the lock.
        if !self.compromised.load(Ordering::SeqCst) {
            let _ = std::fs::remove_dir_all(&self.lock_path);
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        self.release_in_place();
    }
}

/// Acquire the lock for `path` in a single attempt, taking over a stale lock.
pub fn try_lock(path: &Path, options: &LockOptions) -> Result<LockGuard, LockError> {
    let lock_path = lock_path_for(path);
    let stale = options.effective_stale();

    match std::fs::create_dir(&lock_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !is_lock_stale(&lock_path, stale) {
                return Err(LockError::Locked);
            }
            // Take over the abandoned lock, then compete for it once more.
            let _ = std::fs::remove_dir_all(&lock_path);
            std::fs::create_dir(&lock_path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    LockError::Locked
                } else {
                    LockError::Io(error.to_string())
                }
            })?;
        }
        Err(error) => return Err(LockError::Io(error.to_string())),
    }

    let mtime = touch(&lock_path).map_err(|error| LockError::Io(error.to_string()))?;
    let compromised = Arc::new(AtomicBool::new(false));
    let heartbeat = spawn_heartbeat(
        lock_path.clone(),
        options.effective_update(),
        mtime,
        Arc::clone(&compromised),
        options.on_compromised.clone(),
    );
    Ok(LockGuard {
        lock_path,
        heartbeat: Some(heartbeat),
        compromised,
    })
}

/// Acquire the lock, retrying `attempts` times with `delay` in between.
pub fn lock_with_retry(
    path: &Path,
    options: &LockOptions,
    attempts: u32,
    delay: Duration,
) -> Result<LockGuard, LockError> {
    let mut last_error = LockError::Locked;
    for attempt in 1..=attempts.max(1) {
        match try_lock(path, options) {
            Ok(guard) => return Ok(guard),
            Err(LockError::Locked) if attempt < attempts => {
                last_error = LockError::Locked;
                std::thread::sleep(delay);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

fn is_lock_stale(lock_path: &Path, stale: Duration) -> bool {
    let Ok(metadata) = std::fs::metadata(lock_path) else {
        // The lock vanished between `mkdir` and `stat`: treat it as free.
        return true;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    match SystemTime::now().duration_since(modified) {
        Ok(age) => age > stale,
        // An mtime in the future is not stale.
        Err(_) => false,
    }
}

/// Set the lock mtime to now and return the value the filesystem stored.
fn touch(lock_path: &Path) -> std::io::Result<SystemTime> {
    let now = SystemTime::now();
    let file = File::open(lock_path)?;
    if file
        .set_times(FileTimes::new().set_accessed(now).set_modified(now))
        .is_err()
    {
        // Some filesystems refuse `futimens` on a directory handle; creating and
        // removing an entry updates the directory mtime just as well.
        let marker = lock_path.join(".heartbeat");
        std::fs::write(&marker, b"")?;
        std::fs::remove_file(&marker)?;
    }
    std::fs::metadata(lock_path)?.modified()
}

fn spawn_heartbeat(
    lock_path: PathBuf,
    interval: Duration,
    initial_mtime: SystemTime,
    compromised: Arc<AtomicBool>,
    on_compromised: Option<OnCompromised>,
) -> Heartbeat {
    let stop = Arc::new((Mutex::new(false), Condvar::new()));
    let thread_stop = Arc::clone(&stop);
    let handle = std::thread::Builder::new()
        .name("notagent-lock-heartbeat".to_owned())
        .spawn(move || {
            let mut expected = initial_mtime;
            loop {
                let (lock, condvar) = &*thread_stop;
                let stopped = lock.lock().expect("heartbeat stop mutex");
                // Check before waiting: a release that happens between two ticks
                // must not be missed, or the guard would block for a full interval.
                if *stopped {
                    return;
                }
                let (stopped, _) = condvar
                    .wait_timeout(stopped, interval)
                    .expect("heartbeat wait");
                if *stopped {
                    return;
                }
                drop(stopped);

                let error =
                    match std::fs::metadata(&lock_path).and_then(|metadata| metadata.modified()) {
                        Err(_) => Some(LockError::Compromised("Lock file was deleted".to_owned())),
                        Ok(current) if current != expected => Some(LockError::Compromised(
                            "Lock file was updated by someone else".to_owned(),
                        )),
                        Ok(_) => match touch(&lock_path) {
                            Ok(updated) => {
                                expected = updated;
                                None
                            }
                            Err(error) => Some(LockError::Compromised(format!(
                                "Unable to update lock within the stale threshold: {error}"
                            ))),
                        },
                    };
                if let Some(error) = error {
                    compromised.store(true, Ordering::SeqCst);
                    if let Some(callback) = &on_compromised {
                        callback(error);
                    }
                    return;
                }
            }
        })
        .expect("spawn heartbeat thread");
    Heartbeat {
        stop,
        handle: Some(handle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::Builder::new()
            .prefix("notagent-lockfile-")
            .tempdir()
            .expect("temp dir");
        let file = directory.path().join("auth.json");
        std::fs::write(&file, "{}").expect("write");
        (directory, file)
    }

    #[test]
    fn acquires_and_releases_a_lock() {
        let (_directory, file) = temp_file();
        let guard = try_lock(&file, &LockOptions::default()).expect("lock");
        assert!(lock_path_for(&file).is_dir());
        guard.release();
        assert!(!lock_path_for(&file).exists());
    }

    #[test]
    fn a_second_holder_is_rejected() {
        let (_directory, file) = temp_file();
        let _guard = try_lock(&file, &LockOptions::default()).expect("lock");
        assert_eq!(
            try_lock(&file, &LockOptions::default()).expect_err("locked"),
            LockError::Locked
        );
    }

    #[test]
    fn dropping_the_guard_releases_the_lock() {
        let (_directory, file) = temp_file();
        {
            let _guard = try_lock(&file, &LockOptions::default()).expect("lock");
        }
        let _second = try_lock(&file, &LockOptions::default()).expect("lock again");
    }

    #[test]
    fn a_stale_lock_is_taken_over() {
        let (_directory, file) = temp_file();
        let lock_path = lock_path_for(&file);
        std::fs::create_dir(&lock_path).expect("create lock");
        let old = SystemTime::now() - Duration::from_secs(120);
        let handle = File::open(&lock_path).expect("open lock");
        handle
            .set_times(FileTimes::new().set_accessed(old).set_modified(old))
            .expect("set times");
        let _guard = try_lock(
            &file,
            &LockOptions {
                stale: Duration::from_secs(30),
                ..LockOptions::default()
            },
        )
        .expect("takes over");
    }

    #[test]
    fn retries_until_the_lock_is_free() {
        let (_directory, file) = temp_file();
        let guard = try_lock(&file, &LockOptions::default()).expect("lock");
        let released = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            guard.release();
        });
        let second = lock_with_retry(
            &file,
            &LockOptions::default(),
            10,
            Duration::from_millis(20),
        );
        released.join().expect("release thread");
        second.expect("acquires after the holder releases");
    }

    #[test]
    fn retrying_gives_up_while_the_lock_is_held() {
        let (_directory, file) = temp_file();
        let _guard = try_lock(&file, &LockOptions::default()).expect("lock");
        let error = lock_with_retry(&file, &LockOptions::default(), 3, Duration::from_millis(5))
            .expect_err("locked");
        assert_eq!(error, LockError::Locked);
    }

    #[test]
    fn the_heartbeat_reports_a_deleted_lock() {
        let (_directory, file) = temp_file();
        let seen: Arc<Mutex<Option<LockError>>> = Arc::new(Mutex::new(None));
        let recorder = Arc::clone(&seen);
        let options = LockOptions {
            stale: Duration::from_secs(5),
            update: Some(Duration::from_secs(1)),
            on_compromised: Some(Arc::new(move |error| {
                *recorder.lock().expect("recorder") = Some(error);
            })),
        };
        let guard = try_lock(&file, &options).expect("lock");
        std::fs::remove_dir_all(lock_path_for(&file)).expect("remove lock");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while seen.lock().expect("recorder").is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            seen.lock().expect("recorder").clone(),
            Some(LockError::Compromised("Lock file was deleted".to_owned()))
        );
        assert!(guard.is_compromised());
    }

    #[test]
    fn a_compromised_guard_leaves_the_successors_lock_alone() {
        let (_directory, file) = temp_file();
        let options = LockOptions {
            stale: Duration::from_secs(5),
            update: Some(Duration::from_millis(100)),
            on_compromised: None,
        };
        let guard = try_lock(&file, &options).expect("lock");
        // Steal the lock the way a stale takeover does: remove and re-create.
        std::fs::remove_dir_all(lock_path_for(&file)).expect("steal");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !guard.is_compromised() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(guard.is_compromised());
        std::fs::create_dir(lock_path_for(&file)).expect("successor acquires");
        guard.release();
        assert!(
            lock_path_for(&file).is_dir(),
            "the compromised guard must not delete the successor's lock"
        );
    }

    #[test]
    fn the_heartbeat_keeps_a_held_lock_fresh() {
        let (_directory, file) = temp_file();
        let options = LockOptions {
            stale: Duration::from_secs(5),
            update: Some(Duration::from_secs(1)),
            on_compromised: None,
        };
        let guard = try_lock(&file, &options).expect("lock");
        let first = std::fs::metadata(lock_path_for(&file))
            .expect("stat")
            .modified()
            .expect("mtime");
        std::thread::sleep(Duration::from_millis(1200));
        let second = std::fs::metadata(lock_path_for(&file))
            .expect("stat")
            .modified()
            .expect("mtime");
        assert!(second > first, "the heartbeat must refresh the lock mtime");
        assert!(!guard.is_compromised());
    }
}
