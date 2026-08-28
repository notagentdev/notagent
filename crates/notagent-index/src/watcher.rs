//! File watcher for incremental index updates

use crate::index::SymbolIndex;
use crate::parser::{MultiParser, SupportedLanguage};
use crate::persistence::{IndexCache, hash_content};
use crate::{Bm25Hit, Bm25Index, ChunkDocument, chunk_file};
use anyhow::{Context, Result};
use fs2::FileExt;
use ignore::WalkBuilder;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

/// Debounce delay for file changes
const DEBOUNCE_DELAY_MS: u64 = 500;

/// File watcher for incremental index updates
pub struct IndexWatcher {
    watcher: Option<RecommendedWatcher>,
    event_tx: mpsc::Sender<PathBuf>,
    stop_tx: Option<mpsc::Sender<()>>,
}

impl IndexWatcher {
    /// Create a new index watcher
    /// Returns the watcher and a receiver for file change events
    pub fn new() -> Result<(Self, mpsc::Receiver<PathBuf>)> {
        let (event_tx, event_rx) = mpsc::channel(100);

        let watcher = IndexWatcher {
            watcher: None,
            event_tx,
            stop_tx: None,
        };

        Ok((watcher, event_rx))
    }

    /// Start watching a directory
    pub fn watch(&mut self, path: &Path) -> Result<()> {
        let tx = self.event_tx.clone();

        // Create debounced event handler
        let (debounce_tx, mut debounce_rx) = mpsc::channel::<PathBuf>(100);
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        self.stop_tx = Some(stop_tx);

        // Spawn debounce task
        let event_tx = tx.clone();
        tokio::spawn(async move {
            let mut pending: HashSet<PathBuf> = HashSet::new();
            let debounce_duration = Duration::from_millis(DEBOUNCE_DELAY_MS);

            loop {
                tokio::select! {
                    Some(path) = debounce_rx.recv() => {
                        pending.insert(path);
                    }
                    _ = tokio::time::sleep(debounce_duration), if !pending.is_empty() => {
                        // Send all pending paths
                        for path in pending.drain() {
                            if let Err(e) = event_tx.send(path).await {
                                tracing::trace!("Failed to send debounced file event: {}", e);
                            }
                        }
                    }
                    _ = stop_rx.recv() => {
                        break;
                    }
                }
            }
        });

        // Create file watcher
        let watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    // Only handle modify/create events for supported files
                    if event.kind.is_modify() || event.kind.is_create() {
                        for path in event.paths {
                            if Self::is_indexable(&path)
                                && let Err(e) = debounce_tx.blocking_send(path)
                            {
                                tracing::trace!(
                                    "Failed to send file event to debounce task: {}",
                                    e
                                );
                            }
                        }
                    }
                }
            },
            Config::default(),
        )?;

        self.watcher = Some(watcher);

        if let Some(ref mut w) = self.watcher {
            w.watch(path, RecursiveMode::Recursive)?;
            tracing::info!("Started watching: {}", path.display());
        }

        Ok(())
    }

    /// Stop watching
    pub fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take()
            && let Err(e) = tx.blocking_send(())
        {
            tracing::trace!("Failed to send stop signal to watcher: {}", e);
        }
        self.watcher = None;
        tracing::info!("Stopped file watcher");
    }

    /// Check if a file should be indexed
    fn is_indexable(path: &Path) -> bool {
        // Must be a file
        if !path.is_file() {
            return false;
        }

        // Must be a supported language
        SupportedLanguage::from_path(path).is_some()
    }
}

impl Default for IndexWatcher {
    fn default() -> Self {
        Self {
            watcher: None,
            event_tx: mpsc::channel(1).0,
            stop_tx: None,
        }
    }
}

impl Drop for IndexWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Result from parallel file processing
struct FileResult {
    path: PathBuf,
    hash: u64,
    symbols: Vec<Arc<crate::symbol::Symbol>>,
    chunks: Vec<ChunkDocument>,
}

/// Manager that combines index, cache, and watcher
pub struct IndexManager {
    index: Arc<SymbolIndex>,
    cache: IndexCache,
    bm25: Option<Bm25Index>,
    workspace_root: PathBuf,
    /// Directory holding the persisted index state (`index.bin`, `bm25/`).
    /// Lives outside the workspace tree so repositories stay clean.
    state_dir: PathBuf,
}

impl IndexManager {
    /// Create a new index manager, loading from cache with graceful fallback
    pub fn new(workspace_root: PathBuf, state_dir: PathBuf) -> Result<Self> {
        let cache_path = IndexCache::default_path(&state_dir);

        // Graceful fallback - never fails on corrupt cache
        let cache = IndexCache::load_or_default(&cache_path);

        if cache.file_count() > 0 {
            tracing::info!(
                "Loaded index cache: {} files, {} symbols",
                cache.file_count(),
                cache.symbol_count()
            );
        } else {
            tracing::info!("No existing cache, will perform full index");
        }

        let index = Arc::new(SymbolIndex::new());

        // Populate index from cache, per file rather than per symbol, so the
        // per-file entry exists and a later removal can find what to remove.
        for (path, file_cache) in &cache.files {
            let symbols = file_cache
                .symbols
                .iter()
                .map(|symbol| Arc::new(symbol.clone()))
                .collect();
            index.restore_file(std::path::Path::new(path), symbols, file_cache.content_hash);
        }

        Ok(Self {
            index,
            cache,
            bm25: None,
            workspace_root,
            state_dir,
        })
    }
    /// Refreshes a workspace while holding its cross-process index lock.
    /// The cache and BM25 reader are reopened only after the lock is acquired,
    /// so a process never refreshes from state made stale by another writer.
    /// # Arguments
    /// * `workspace_root` - Root directory whose supported source files are indexed.
    /// * `state_dir` - Persistent state directory shared by index users.
    /// * `on_progress` - Optional callback receiving indexing progress.
    /// # Errors
    /// Returns an error when the refresh lock, index, workspace scan, or cache write fails.
    pub fn refresh_workspace<F>(
        workspace_root: PathBuf,
        state_dir: PathBuf,
        on_progress: Option<F>,
    ) -> Result<(Self, usize)>
    where
        F: FnMut(IndexProgress),
    {
        let _lock = acquire_refresh_lock(&state_dir)?;
        let mut manager = Self::new(workspace_root, state_dir)?;
        manager.init_bm25()?;
        let indexed = manager.index_workspace(on_progress)?;
        manager.save_cache()?;
        Ok((manager, indexed))
    }
    /// Initialize BM25 index (stored in the state directory)
    pub fn init_bm25(&mut self) -> Result<()> {
        let bm25_path = self.state_dir.join("bm25");
        let bm25 = Bm25Index::open_or_create(bm25_path)?;
        self.bm25 = Some(bm25);
        Ok(())
    }

    pub fn bm25_search(&self, query: &str, max_results: usize) -> Result<Option<Vec<Bm25Hit>>> {
        if let Some(ref bm25) = self.bm25 {
            return Ok(Some(bm25.search(query, max_results)?));
        }
        Ok(None)
    }

    /// Full workspace indexing with optional progress callback.
    /// Uses rayon to parallelize file I/O, hashing, parsing, and chunking.
    /// Results are collected and applied sequentially; BM25 uses a single
    /// batched commit at the end.
    pub fn index_workspace<F>(&mut self, mut on_progress: Option<F>) -> Result<usize>
    where
        F: FnMut(IndexProgress),
    {
        // Standard filters honor .gitignore/.ignore and skip hidden entries,
        // so build output and vendored dependencies never enter the index.
        let mut files: Vec<PathBuf> = Vec::new();
        for entry in WalkBuilder::new(&self.workspace_root)
            .standard_filters(true)
            .require_git(false)
            .build()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.is_file() && SupportedLanguage::from_path(path).is_some() {
                files.push(path.to_path_buf());
            }
        }

        let total = files.len();
        if let Some(ref mut cb) = on_progress {
            cb(IndexProgress::new(IndexingPhase::Scanning, total, 0));
        }

        // Files the cache knows but the scan no longer found: gone from disk.
        // Nothing else would notice. The phase below only walks what it just
        // scanned, so a deleted file's symbols and BM25 documents stayed
        // searchable for good and the cache grew with every rename.
        let present: std::collections::HashSet<&Path> =
            files.iter().map(PathBuf::as_path).collect();
        let removed: Vec<PathBuf> = self
            .cache
            .files
            .keys()
            .map(PathBuf::from)
            .filter(|path| !present.contains(path.as_path()))
            .collect();
        for path in &removed {
            self.cache.remove_file(path);
            self.index.remove_file(path);
        }
        if !removed.is_empty() {
            tracing::debug!("Dropped {} file(s) that no longer exist", removed.len());
        }

        // Atomic counter for progress from parallel threads
        let progress_counter = Arc::new(AtomicUsize::new(0));

        // Snapshot current cache hashes so rayon threads can check without &mut self
        let cache = &self.cache;
        let index = &self.index;

        // --- Parallel phase: read, hash, parse, extract symbols, chunk ---
        let results: Vec<_> = files
            .par_iter()
            .filter_map(|path| {
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => {
                        progress_counter.fetch_add(1, Ordering::Relaxed);
                        return None;
                    }
                };

                let new_hash = hash_content(&content);

                // Skip unchanged files
                if let Some(cached_hash) = cache.get_hash(path)
                    && cached_hash == new_hash
                {
                    progress_counter.fetch_add(1, Ordering::Relaxed);
                    return None;
                }

                // Parse once — reuse tree for both symbols and chunking
                let mut parser = MultiParser::new();
                let tree = match parser.parse_file(&content, path) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::trace!("Failed to parse {:?}: {}", path, e);
                        progress_counter.fetch_add(1, Ordering::Relaxed);
                        return None;
                    }
                };

                // Extract symbols using the parsed tree
                let symbols = index.index_file_with_tree(path, &content, &tree);

                // Chunk using the same tree (no double parsing)
                let chunks = match chunk_file(path, &content, Some(&tree)) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::trace!("Failed to chunk {:?}: {}", path, e);
                        Vec::new()
                    }
                };

                progress_counter.fetch_add(1, Ordering::Relaxed);

                Some(FileResult {
                    path: path.clone(),
                    hash: new_hash,
                    symbols,
                    chunks,
                })
            })
            .collect();

        // Report progress after parallel phase
        let indexed = results.len();
        if let Some(ref mut cb) = on_progress {
            cb(IndexProgress::new(
                IndexingPhase::Indexing,
                total,
                progress_counter.load(Ordering::Relaxed),
            ));
        }

        // --- Sequential phase: update cache + collect BM25 batches ---
        let mut all_chunks: Vec<ChunkDocument> = Vec::new();
        // The vanished files go into the same batch as the re-indexed ones, so
        // one commit settles both.
        let mut delete_paths: Vec<String> = removed
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();

        for result in &results {
            // Update cache
            let symbols_for_cache: Vec<_> = result.symbols.iter().map(|s| (**s).clone()).collect();
            self.cache
                .update_file(&result.path, result.hash, symbols_for_cache);

            // Collect BM25 data
            if !result.chunks.is_empty() {
                delete_paths.push(result.path.to_string_lossy().to_string());
                all_chunks.extend(result.chunks.iter().cloned());
            }

            tracing::debug!(
                "Indexed file: {} ({} symbols, {} chunks)",
                result.path.display(),
                result.symbols.len(),
                result.chunks.len()
            );
        }

        // --- Single BM25 batch commit ---
        // A refresh that only deleted files still has to commit: `delete_paths`
        // carries them even when nothing new was chunked.
        if let Some(ref bm25) = self.bm25
            && !(all_chunks.is_empty() && delete_paths.is_empty())
        {
            bm25.batch_delete_and_add(&delete_paths, &all_chunks)?;
            bm25.reload()?;
        }

        if let Some(ref mut cb) = on_progress {
            cb(IndexProgress::new(IndexingPhase::Done, total, total));
        }

        Ok(indexed)
    }

    /// Get the symbol index
    pub fn index(&self) -> Arc<SymbolIndex> {
        self.index.clone()
    }

    /// Update a file in the index (checks hash first, uses fast xxHash)
    pub fn update_file(&mut self, path: &Path, content: &str) -> Result<bool> {
        use crate::persistence::hash_content;

        // Calculate content hash (xxHash64 - very fast)
        let new_hash = hash_content(content);

        // Check if file has changed
        if let Some(cached_hash) = self.cache.get_hash(path)
            && cached_hash == new_hash
        {
            tracing::debug!("File unchanged, skipping: {}", path.display());
            return Ok(false);
        }

        // Remove old symbols
        self.index.remove_file(path);

        // Index the file
        let symbols = self.index.index_file(path, content)?;

        if let Some(ref bm25) = self.bm25 {
            let mut parser = crate::parser::MultiParser::new();
            let tree = match parser.parse_file(content, path) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!(
                        "[index] Failed to parse file {:?} for chunking: {}",
                        path,
                        e
                    );
                    None
                }
            };
            let chunks: Vec<ChunkDocument> = chunk_file(path, content, tree.as_ref())?;
            bm25.delete_by_path(&path.to_string_lossy())?;
            bm25.add_chunks(&chunks)?;
            bm25.reload()?;
        }

        // Update cache
        let symbols_for_cache: Vec<_> = symbols.iter().map(|s| (**s).clone()).collect();
        self.cache.update_file(path, new_hash, symbols_for_cache);

        tracing::debug!(
            "Indexed file: {} ({} symbols)",
            path.display(),
            symbols.len()
        );

        Ok(true)
    }

    /// Save cache to disk
    pub fn save_cache(&self) -> Result<()> {
        let cache_path = IndexCache::default_path(&self.state_dir);
        self.cache.save(&cache_path)
    }

    /// Get cache statistics
    pub fn stats(&self) -> (usize, usize) {
        (self.cache.file_count(), self.cache.symbol_count())
    }
}
fn acquire_refresh_lock(state_dir: &Path) -> Result<File> {
    std::fs::create_dir_all(state_dir).with_context(|| {
        format!(
            "Failed to create index state directory {}",
            state_dir.display()
        )
    })?;
    let lock_path = state_dir.join("refresh.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("Failed to open index refresh lock {}", lock_path.display()))?;
    FileExt::lock_exclusive(&lock).with_context(|| {
        format!(
            "Failed to acquire index refresh lock {}",
            lock_path.display()
        )
    })?;
    Ok(lock)
}
#[derive(Debug, Clone, Copy)]
pub enum IndexingPhase {
    Scanning,
    Indexing,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy)]
pub struct IndexProgress {
    pub phase: IndexingPhase,
    pub total_files: usize,
    pub indexed_files: usize,
}

impl IndexProgress {
    pub fn new(phase: IndexingPhase, total_files: usize, indexed_files: usize) -> Self {
        Self {
            phase,
            total_files,
            indexed_files,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent dir");
        }
        std::fs::write(path, content).expect("write fixture");
    }

    #[test]
    fn is_indexable_requires_existing_supported_files() {
        let tempdir = TempDir::new().expect("tempdir");
        let rust_file = tempdir.path().join("src/lib.rs");
        let text_file = tempdir.path().join("README.txt");
        let missing_file = tempdir.path().join("missing.rs");
        std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir");
        write_file(&rust_file, "pub fn hello() {}\n");
        write_file(&text_file, "plain text\n");

        assert!(IndexWatcher::is_indexable(&rust_file));
        assert!(!IndexWatcher::is_indexable(&text_file));
        assert!(!IndexWatcher::is_indexable(
            tempdir.path().join("src").as_path()
        ));
        assert!(!IndexWatcher::is_indexable(&missing_file));
    }

    #[test]
    fn a_refresh_drops_files_that_disappeared_from_disk() {
        let tempdir = TempDir::new().expect("tempdir");
        // Outside the workspace, as the field doc requires — otherwise the
        // scan finds the index's own state files.
        let state_dir = TempDir::new().expect("state tempdir");
        let state = state_dir.path().to_path_buf();
        let kept = tempdir.path().join("src/kept.rs");
        let doomed = tempdir.path().join("src/doomed.rs");
        write_file(&kept, "pub fn kept() {}\n");
        write_file(&doomed, "pub fn doomed() {}\n");

        let (manager, _) = IndexManager::refresh_workspace(
            tempdir.path().to_path_buf(),
            state.clone(),
            None::<fn(IndexProgress)>,
        )
        .expect("first refresh");
        assert_eq!(manager.stats().0, 2, "both files indexed");
        drop(manager);

        std::fs::remove_file(&doomed).expect("delete fixture");

        let (manager, _) = IndexManager::refresh_workspace(
            tempdir.path().to_path_buf(),
            state,
            None::<fn(IndexProgress)>,
        )
        .expect("second refresh");

        assert_eq!(
            manager.stats().0,
            1,
            "the deleted file is gone from the cache"
        );
        assert!(
            manager
                .index()
                .all_symbols()
                .iter()
                .all(|symbol| symbol.file_path != doomed),
            "no symbol of the deleted file survives"
        );
    }

    #[test]
    fn update_file_skips_unchanged_content() {
        let tempdir = TempDir::new().expect("tempdir");
        let file_path = tempdir.path().join("src/lib.rs");
        let content = "pub fn hello() {}\n";
        write_file(&file_path, content);

        let mut manager =
            IndexManager::new(tempdir.path().to_path_buf(), tempdir.path().join("state"))
                .expect("manager");

        let indexed = manager
            .update_file(&file_path, content)
            .expect("initial update");
        assert!(indexed);
        let initial_stats = manager.stats();
        assert_eq!(initial_stats.0, 1);
        assert!(initial_stats.1 >= 1);

        let changed = manager
            .update_file(&file_path, content)
            .expect("repeat update");
        assert!(!changed);
        assert_eq!(manager.stats(), initial_stats);
    }

    #[test]
    fn index_workspace_reports_progress_and_indexes_supported_files() {
        let tempdir = TempDir::new().expect("tempdir");
        write_file(
            &tempdir.path().join("src/lib.rs"),
            "pub fn hello() {}\nstruct Person {}\n",
        );
        write_file(&tempdir.path().join("notes.txt"), "ignored\n");

        let mut manager =
            IndexManager::new(tempdir.path().to_path_buf(), tempdir.path().join("state"))
                .expect("manager");
        let mut progress = Vec::new();

        let indexed = manager
            .index_workspace(Some(|p: IndexProgress| {
                progress.push((p.phase, p.total_files, p.indexed_files))
            }))
            .expect("workspace indexing");

        assert_eq!(indexed, 1);
        assert_eq!(manager.stats().0, 1);
        assert!(manager.stats().1 >= 2);
        assert_eq!(progress.len(), 3);
        assert!(matches!(progress[0].0, IndexingPhase::Scanning));
        assert!(matches!(progress[1].0, IndexingPhase::Indexing));
        assert!(matches!(progress[2].0, IndexingPhase::Done));
        assert_eq!(progress[0].1, 1);
        assert_eq!(progress[2].2, 1);
    }
    #[test]
    fn concurrent_workspace_refreshes_share_persistent_state_without_lock_conflicts() {
        let workspace = TempDir::new().expect("workspace");
        let state = TempDir::new().expect("state");
        write_file(
            &workspace.path().join("src/lib.rs"),
            "pub fn concurrent_refresh_symbol() {}\n",
        );
        let workspace_path = workspace.path().to_path_buf();
        let state_path = state.path().to_path_buf();
        let first_workspace = workspace_path.clone();
        let first_state = state_path.clone();
        let first = std::thread::spawn(move || {
            IndexManager::refresh_workspace::<fn(IndexProgress)>(first_workspace, first_state, None)
        });
        let second = std::thread::spawn(move || {
            IndexManager::refresh_workspace::<fn(IndexProgress)>(workspace_path, state_path, None)
        });
        let first = first
            .join()
            .expect("first refresh thread")
            .expect("first refresh");
        let second = second
            .join()
            .expect("second refresh thread")
            .expect("second refresh");
        let actual = [first.0.stats().0, second.0.stats().0];
        let expected = [1, 1];
        assert_eq!(actual, expected);
    }
}
