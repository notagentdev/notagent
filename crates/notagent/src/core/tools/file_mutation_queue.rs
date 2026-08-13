//! Port of `packages/coding-agent/src/core/tools/file-mutation-queue.ts`.
//!
//! Mutations of the same file are serialized; mutations of different files still
//! run in parallel.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};

use tokio::sync::Mutex as AsyncMutex;

type QueueMap = HashMap<String, Arc<AsyncMutex<()>>>;

static FILE_MUTATION_QUEUES: LazyLock<Mutex<QueueMap>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The queue key is the real path, so hard links and symlinks share one queue.
///
/// Deviation (class 1): TS resolves the real path asynchronously and therefore
/// needs a registration queue to keep call order. Resolving synchronously keeps
/// the same order without one.
fn mutation_queue_key(file_path: &str) -> String {
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let resolved = crate::utils::paths::resolve_path_default(file_path, &cwd)
        .unwrap_or_else(|_| file_path.to_owned());
    match std::fs::canonicalize(&resolved) {
        Ok(real) => real.to_string_lossy().into_owned(),
        // A path that does not exist yet keeps its resolved form.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            resolved
        }
        Err(_) => resolved,
    }
}

fn queue_for(key: &str) -> Arc<AsyncMutex<()>> {
    let mut queues = FILE_MUTATION_QUEUES.lock().expect("file mutation queues");
    Arc::clone(queues.entry(key.to_owned()).or_default())
}

fn release(key: &str) {
    let mut queues = FILE_MUTATION_QUEUES.lock().expect("file mutation queues");
    // The map only keeps queues that still have waiters.
    if queues
        .get(key)
        .is_some_and(|queue| Arc::strong_count(queue) == 1)
    {
        queues.remove(key);
    }
}

/// Serialize file mutation operations targeting the same file.
pub async fn with_file_mutation_queue<T, F>(file_path: &str, operation: F) -> T
where
    F: Future<Output = T>,
{
    let key = mutation_queue_key(file_path);
    let queue = queue_for(&key);
    let result = {
        let _guard = queue.lock().await;
        operation.await
    };
    drop(queue);
    release(&key);
    result
}

/// True while a queue is being tracked for `file_path`; used by the tests.
pub fn is_queue_tracked(file_path: &str) -> bool {
    let key = mutation_queue_key(file_path);
    FILE_MUTATION_QUEUES
        .lock()
        .expect("file mutation queues")
        .contains_key(&key)
}

/// True when `path` resolves to the same queue key as `other`.
pub fn shares_queue(path: &str, other: &str) -> bool {
    mutation_queue_key(path) == mutation_queue_key(other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let directory = tempfile::Builder::new()
                .prefix("notagent-mutation-queue-")
                .tempdir()
                .expect("temp dir");
            let path = directory.path().to_path_buf();
            let _ = directory.keep();
            Self { path }
        }

        fn write(&self, name: &str) -> String {
            let path = self.path.join(name);
            std::fs::write(&path, "x").expect("writes");
            path.to_string_lossy().into_owned()
        }

        fn join(&self, name: &str) -> String {
            self.path.join(name).to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[tokio::test]
    async fn serializes_operations_on_the_same_file() {
        let directory = TempDir::new();
        let file = directory.write("file.txt");
        let running = Arc::new(AtomicUsize::new(0));
        let max_running = Arc::new(AtomicUsize::new(0));

        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let file = file.clone();
                let running = Arc::clone(&running);
                let max_running = Arc::clone(&max_running);
                tokio::spawn(async move {
                    with_file_mutation_queue(&file, async {
                        let current = running.fetch_add(1, Ordering::SeqCst) + 1;
                        max_running.fetch_max(current, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                        running.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await;
                })
            })
            .collect();
        for task in tasks {
            task.await.expect("join");
        }
        assert_eq!(max_running.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn runs_operations_on_different_files_in_parallel() {
        let directory = TempDir::new();
        let first = directory.write("first.txt");
        let second = directory.write("second.txt");
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());

        let blocking = tokio::spawn({
            let first = first.clone();
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            async move {
                with_file_mutation_queue(&first, async {
                    started.notify_one();
                    release.notified().await;
                })
                .await;
            }
        });
        started.notified().await;

        // The second file must not wait for the first.
        with_file_mutation_queue(&second, async {}).await;
        release.notify_one();
        blocking.await.expect("join");
    }

    #[tokio::test]
    async fn shares_a_queue_across_links_to_the_same_file() {
        let directory = TempDir::new();
        let target = directory.write("target.txt");
        let link = directory.join("link.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        #[cfg(not(unix))]
        std::fs::copy(&target, &link).expect("copy");

        #[cfg(unix)]
        assert!(shares_queue(&target, &link));
        assert!(shares_queue(&target, &target));
        assert!(!shares_queue(&target, &directory.join("other.txt")));
    }

    #[tokio::test]
    async fn forgets_the_queue_once_the_last_operation_finished() {
        let directory = TempDir::new();
        let file = directory.write("done.txt");
        with_file_mutation_queue(&file, async {}).await;
        assert!(!is_queue_tracked(&file));
    }
}
