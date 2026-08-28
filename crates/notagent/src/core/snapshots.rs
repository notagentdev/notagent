//! Copies of a file taken before it is changed, so a change can be taken back.
//! Every mutating tool writes one of these before it touches a file. `undo`
//! restores the newest and deletes it, so calling it repeatedly walks backwards
//! through the history one step at a time.
//! The store has no index, and that is what makes it worth trusting. A snapshot
//! lands at `<base>/<hash of the path>/<timestamp>.snap`, where the timestamp is
//! zero-padded down to nanoseconds — so the lexical order of the file names is
//! their chronological order, and finding the newest snapshot is the maximum of
//! a directory listing. There is no second record that could disagree with the
//! files on disk.
//! Snapshots live outside the workspace, under the agent directory. A snapshot
//! inside the repository would be a file the agent might read, search, or
//! commit.

use std::path::{Path, PathBuf};

use futures::future::BoxFuture;

/// Extension every snapshot carries, and the only thing read back out of the
/// directory. A stray file with another name is ignored rather than restored.
pub const SNAPSHOT_EXTENSION: &str = "snap";

/// Name of the marker naming the file a directory belongs to.
/// The directory name is a hash, and a hash can in principle collide. Restoring
/// the wrong file would be silent and destructive, so the original path is
/// written next to the snapshots and checked before anything is put back. It
/// costs one small file per tracked path.
pub const ORIGIN_MARKER: &str = "path";

/// FNV-1a over the path, which is what names its directory.
/// Chosen to match the reference implementation's layout. Any stable hash would
/// do — the two stores are never shared — but a fixed choice means a snapshot
/// taken by one version of this program is still found by the next.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// The path as it identifies a file for snapshot purposes.
/// The *directory* is resolved, not the file, and the file name is put back
/// afterwards. Resolving the file itself would be the obvious thing and is what
/// the reference does (`snapshot.rs:65`), but it breaks the case undo exists
/// for: a file that has been deleted no longer resolves, so the key computed
/// when the snapshot was taken and the key computed when it is wanted back
/// disagree — on macOS by the whole `/var` → `/private/var` prefix — and the
/// snapshot cannot be found. A directory outlives the file inside it.
/// A relative path is refused. It would hash differently depending on where the
/// process happened to be, which is a way to lose a snapshot without any error.
pub fn canonical_key(path: &Path) -> Result<String, String> {
    if !path.is_absolute() {
        return Err(format!(
            "Snapshot paths must be absolute, got \"{}\".",
            path.display()
        ));
    }
    let resolved = match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => match parent.canonicalize() {
            Ok(parent) => parent.join(name),
            // A directory that does not resolve either — the whole tree was
            // removed, or it never existed. The path as written is all there
            // is, and it is consistent with itself.
            Err(_) => path.to_path_buf(),
        },
        // A root has no file name to reattach.
        _ => path.to_path_buf(),
    };
    Ok(resolved.display().to_string())
}

/// Directory name holding every snapshot of one file.
pub fn path_hash(key: &str) -> String {
    format!("{:x}", fnv1a_64(key.as_bytes()))
}

/// Snapshot file name for a moment in time.
/// Nanoseconds are zero-padded so the names sort chronologically as text. Two
/// snapshots of one file within the same nanosecond would collide; the caller
/// takes them around file writes, which are orders of magnitude slower.
pub fn snapshot_file_name(at: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "{}.{SNAPSHOT_EXTENSION}",
        at.format("%Y-%m-%d_%H-%M-%S-%9f")
    )
}

/// The filesystem, so the store can be exercised without one.
pub trait SnapshotOperations: Send + Sync {
    fn read<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, String>>;
    fn write<'a>(&'a self, path: &'a str, bytes: &'a [u8]) -> BoxFuture<'a, Result<(), String>>;
    fn mkdir<'a>(&'a self, directory: &'a str) -> BoxFuture<'a, Result<(), String>>;
    fn remove_file<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), String>>;
    fn exists<'a>(&'a self, path: &'a str) -> BoxFuture<'a, bool>;
    /// File names directly inside `directory`, in any order.
    fn list<'a>(&'a self, directory: &'a str) -> BoxFuture<'a, Result<Vec<String>, String>>;
}

pub struct LocalSnapshotOperations;

impl SnapshotOperations for LocalSnapshotOperations {
    fn read<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        Box::pin(async move {
            tokio::fs::read(path)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn write<'a>(&'a self, path: &'a str, bytes: &'a [u8]) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            tokio::fs::write(path, bytes)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn mkdir<'a>(&'a self, directory: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            tokio::fs::create_dir_all(directory)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn remove_file<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            tokio::fs::remove_file(path)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn exists<'a>(&'a self, path: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move { tokio::fs::metadata(path).await.is_ok() })
    }

    fn list<'a>(&'a self, directory: &'a str) -> BoxFuture<'a, Result<Vec<String>, String>> {
        Box::pin(async move {
            let mut entries = tokio::fs::read_dir(directory)
                .await
                .map_err(|error| error.to_string())?;
            let mut names = Vec::new();
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| error.to_string())?
            {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
            Ok(names)
        })
    }
}

/// Where snapshots are kept, and how they are taken and put back.
#[derive(Clone)]
pub struct SnapshotStore {
    base: PathBuf,
    operations: std::sync::Arc<dyn SnapshotOperations>,
}

/// What `restore` put back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The bytes written back over the file.
    pub bytes: Vec<u8>,
    /// Whether any snapshot remains for this path afterwards, so a caller can
    /// say whether there is another step to take back.
    pub more_remain: bool,
}

impl SnapshotStore {
    pub fn new(base: PathBuf) -> Self {
        Self {
            base,
            operations: std::sync::Arc::new(LocalSnapshotOperations),
        }
    }

    pub fn with_operations(
        base: PathBuf,
        operations: std::sync::Arc<dyn SnapshotOperations>,
    ) -> Self {
        Self { base, operations }
    }

    fn directory_for(&self, key: &str) -> PathBuf {
        self.base.join(path_hash(key))
    }

    /// Copies a file's current bytes into the store.
    /// A file that does not exist is not an error and leaves nothing behind:
    /// `write` calls this before creating a file, and there is no earlier state
    /// to return to.
    pub async fn capture(&self, path: &Path) -> Result<Option<PathBuf>, String> {
        let key = canonical_key(path)?;
        let source = path.display().to_string();
        if !self.operations.exists(&source).await {
            return Ok(None);
        }
        let bytes = self.operations.read(&source).await?;
        let directory = self.directory_for(&key);
        let directory_text = directory.display().to_string();
        self.operations.mkdir(&directory_text).await?;

        // Written every time rather than once: a directory that lost it — an
        // interrupted first capture, a manual clean-up — would otherwise refuse
        // every restore from then on.
        let marker = directory.join(ORIGIN_MARKER);
        self.operations
            .write(&marker.display().to_string(), key.as_bytes())
            .await?;

        let target = directory.join(snapshot_file_name(chrono::Utc::now()));
        self.operations
            .write(&target.display().to_string(), &bytes)
            .await?;
        Ok(Some(target))
    }

    /// The newest snapshot for a path, or `None` when there is none.
    async fn newest(&self, key: &str) -> Result<Option<PathBuf>, String> {
        let directory = self.directory_for(key);
        let directory_text = directory.display().to_string();
        if !self.operations.exists(&directory_text).await {
            return Ok(None);
        }
        let names = self.operations.list(&directory_text).await?;
        Ok(names
            .into_iter()
            .filter(|name| name.ends_with(&format!(".{SNAPSHOT_EXTENSION}")))
            .max()
            .map(|name| directory.join(name)))
    }

    /// Whether the directory belongs to the path it is about to restore.
    /// A hash collision would otherwise put one file's content into another,
    /// silently. A directory with no marker is trusted — it was written by a
    /// version that did not keep one — since refusing would break an undo that
    /// is otherwise perfectly good.
    async fn belongs_to(&self, key: &str) -> Result<bool, String> {
        let marker = self.directory_for(key).join(ORIGIN_MARKER);
        let marker_text = marker.display().to_string();
        if !self.operations.exists(&marker_text).await {
            return Ok(true);
        }
        let recorded = self.operations.read(&marker_text).await?;
        Ok(String::from_utf8_lossy(&recorded) == key)
    }

    /// Puts the newest snapshot back and consumes it.
    /// Consuming is what makes repeated calls walk backwards. Returning the
    /// snapshot to the store on a failed write would be the safer-looking
    /// choice, but the write is the last step and a failure there means the
    /// file could not be opened at all — in which case the snapshot is still
    /// on disk, because it is only removed afterwards.
    pub async fn restore(&self, path: &Path) -> Result<Option<Restored>, String> {
        let key = canonical_key(path)?;
        let Some(snapshot) = self.newest(&key).await? else {
            return Ok(None);
        };
        if !self.belongs_to(&key).await? {
            return Err(format!(
                "The snapshot directory for \"{}\" records a different file. Refusing to restore.",
                path.display()
            ));
        }
        let bytes = self
            .operations
            .read(&snapshot.display().to_string())
            .await?;
        self.operations
            .write(&path.display().to_string(), &bytes)
            .await?;
        self.operations
            .remove_file(&snapshot.display().to_string())
            .await?;
        let more_remain = self.newest(&key).await?.is_some();
        Ok(Some(Restored { bytes, more_remain }))
    }
}

/// The store this installation keeps its snapshots in.
/// Under the agent directory, never inside the workspace: a snapshot in the
/// repository would be a file the agent can read, search and commit, and one
/// bad `git add -A` would put every previous version of every file into history.
pub fn snapshot_store() -> SnapshotStore {
    SnapshotStore::new(crate::config::get_agent_dir().join("snapshots"))
}

/// Takes a snapshot before a tool changes a file, when the session has a store.
/// A session without one — a test, or a build with the feature off — writes
/// nothing and reports nothing, so the mutation goes ahead exactly as before.
/// A store that is present and fails, however, fails the mutation with it. The
/// alternative is changing a file after quietly losing the ability to take the
/// change back, which is the one outcome nobody would choose if asked. It is
/// also the reference's behaviour (`fs_write.rs:139` propagates with `?`).
pub async fn capture_before_mutation(
    store: Option<&SnapshotStore>,
    path: &Path,
) -> Result<(), String> {
    let Some(store) = store else { return Ok(()) };
    store.capture(path).await.map(|_| ()).map_err(|error| {
        format!(
            "Could not snapshot \"{}\" before changing it, so the change was not made: {error}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(base: &Path) -> SnapshotStore {
        SnapshotStore::new(base.to_path_buf())
    }

    async fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.expect("parent");
        }
        tokio::fs::write(path, content).await.expect("write");
    }

    async fn read(path: &Path) -> String {
        String::from_utf8_lossy(&tokio::fs::read(path).await.expect("read")).into_owned()
    }

    #[test]
    fn sorts_snapshot_names_chronologically_as_text() {
        let earlier = chrono::DateTime::from_timestamp(1_700_000_000, 1).expect("a time");
        let later = chrono::DateTime::from_timestamp(1_700_000_000, 2).expect("a time");
        assert!(snapshot_file_name(earlier) < snapshot_file_name(later));

        // A second boundary is where an unpadded format would sort wrongly.
        let before = chrono::DateTime::from_timestamp(1_700_000_000, 999_999_999).expect("a time");
        let after = chrono::DateTime::from_timestamp(1_700_000_001, 0).expect("a time");
        assert!(snapshot_file_name(before) < snapshot_file_name(after));
    }

    #[test]
    fn refuses_a_relative_path() {
        let error = canonical_key(Path::new("src/main.rs")).expect_err("refused");
        assert!(error.contains("must be absolute"), "{error}");
    }

    /// The key has to survive the file it names. Anything resolved from the
    /// file itself changes the moment the file is gone, which is exactly when
    /// undo is asked for it.
    #[tokio::test]
    async fn keys_a_file_the_same_before_and_after_it_is_deleted() {
        let directory = tempfile::tempdir().expect("temp dir");
        let file = directory.path().join("note.txt");
        tokio::fs::write(&file, "here").await.expect("write");
        let while_present = canonical_key(&file).expect("key");
        tokio::fs::remove_file(&file).await.expect("removed");
        assert_eq!(canonical_key(&file).expect("key"), while_present);
    }

    #[test]
    fn gives_two_paths_two_directories() {
        assert_ne!(path_hash("/a/one.txt"), path_hash("/a/two.txt"));
        assert_eq!(path_hash("/a/one.txt"), path_hash("/a/one.txt"));
    }

    #[tokio::test]
    async fn captures_nothing_for_a_file_that_does_not_exist() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(&directory.path().join("snapshots"));
        let missing = directory.path().join("absent.txt");
        assert_eq!(store.capture(&missing).await.expect("captured"), None);
    }

    #[tokio::test]
    async fn puts_back_the_content_a_file_had() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(&directory.path().join("snapshots"));
        let file = directory.path().join("note.txt");

        write(&file, "first").await;
        store.capture(&file).await.expect("captured");
        write(&file, "second").await;

        let restored = store.restore(&file).await.expect("restored").expect("some");
        assert_eq!(read(&file).await, "first");
        assert_eq!(restored.bytes, b"first");
        assert!(!restored.more_remain);
    }

    #[tokio::test]
    async fn reports_nothing_to_undo_when_no_snapshot_was_taken() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(&directory.path().join("snapshots"));
        let file = directory.path().join("note.txt");
        write(&file, "only ever this").await;
        assert_eq!(store.restore(&file).await.expect("restored"), None);
    }

    /// The case a delete has to survive: by the time it is undone the path no
    /// longer resolves, so the key has to come out the same anyway.
    #[tokio::test]
    async fn brings_back_a_file_that_was_deleted_after_its_snapshot() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(&directory.path().join("snapshots"));
        let file = directory.path().join("note.txt");

        write(&file, "worth keeping").await;
        store.capture(&file).await.expect("captured");
        tokio::fs::remove_file(&file).await.expect("removed");

        store.restore(&file).await.expect("restored").expect("some");
        assert_eq!(read(&file).await, "worth keeping");
    }

    #[tokio::test]
    async fn walks_backwards_one_step_per_call() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(&directory.path().join("snapshots"));
        let file = directory.path().join("note.txt");

        write(&file, "one").await;
        store.capture(&file).await.expect("captured");
        write(&file, "two").await;
        store.capture(&file).await.expect("captured");
        write(&file, "three").await;

        let first = store.restore(&file).await.expect("restored").expect("some");
        assert_eq!(read(&file).await, "two");
        assert!(first.more_remain, "the earlier snapshot is still there");

        let second = store.restore(&file).await.expect("restored").expect("some");
        assert_eq!(read(&file).await, "one");
        assert!(!second.more_remain);

        assert_eq!(store.restore(&file).await.expect("restored"), None);
    }

    #[tokio::test]
    async fn keeps_two_files_histories_apart() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(&directory.path().join("snapshots"));
        let one = directory.path().join("a/note.txt");
        let two = directory.path().join("b/note.txt");

        write(&one, "from a").await;
        write(&two, "from b").await;
        store.capture(&one).await.expect("captured");
        store.capture(&two).await.expect("captured");
        write(&one, "changed").await;
        write(&two, "changed").await;

        store.restore(&one).await.expect("restored").expect("some");
        assert_eq!(read(&one).await, "from a");
        assert_eq!(read(&two).await, "changed", "b was left alone");
    }

    /// Binary content has to survive the round trip. The store works on bytes
    /// for this reason — only the reporting above it needs text.
    #[tokio::test]
    async fn restores_bytes_that_are_not_text() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(&directory.path().join("snapshots"));
        let file = directory.path().join("image.bin");
        let original: Vec<u8> = vec![0x00, 0xff, 0xfe, 0x80, 0x01];

        tokio::fs::write(&file, &original).await.expect("write");
        store.capture(&file).await.expect("captured");
        tokio::fs::write(&file, b"replaced").await.expect("write");

        let restored = store.restore(&file).await.expect("restored").expect("some");
        assert_eq!(restored.bytes, original);
        assert_eq!(tokio::fs::read(&file).await.expect("read"), original);
    }

    /// A directory whose marker names another file is a hash collision, and
    /// restoring from it would put one file's content into another.
    #[tokio::test]
    async fn refuses_a_directory_that_records_a_different_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let base = directory.path().join("snapshots");
        let store = store(&base);
        let file = directory.path().join("note.txt");

        write(&file, "mine").await;
        store.capture(&file).await.expect("captured");

        let key = canonical_key(&file).expect("key");
        let marker = base.join(path_hash(&key)).join(ORIGIN_MARKER);
        tokio::fs::write(&marker, b"/somewhere/else.txt")
            .await
            .expect("write");

        let error = store.restore(&file).await.expect_err("refused");
        assert!(error.contains("different file"), "{error}");
    }
}
