//! Port of `packages/coding-agent/src/utils/fs-watch.ts`.
//!
//! `fs.watch` throws on some platforms and file systems — a directory that
//! disappears, an inotify limit, a network mount — and emits `error` on others.
//! Both failures land in the same place here, so a caller registers one
//! recovery path instead of two.
//!
//! Deviation (class 3): Node's watcher is replaced by `notify`. The callback
//! receives the changed file's name relative to the watched path, which is the
//! only part of Node's `(eventType, filename)` pair any caller reads.

use std::path::Path;
use std::sync::Arc;

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

/// How long to wait before retrying a watcher that could not be installed.
pub const FS_WATCH_RETRY_DELAY_MS: u64 = 5000;

/// A live watcher. Dropping it stops the watch, which is what `close()` does in
/// TypeScript; `close_watcher` exists so call sites read the same.
pub struct FsWatcher {
    _inner: RecommendedWatcher,
}

/// Stops a watcher, ignoring any error — as in TypeScript, where close failures
/// are swallowed.
pub fn close_watcher(watcher: Option<FsWatcher>) {
    drop(watcher);
}

/// Watches `path`, calling `listener` with the changed entry's file name.
///
/// Returns `None` when the watch could not be installed, having called
/// `on_error` first — the same shape as the TypeScript, where a throw and an
/// asynchronous `error` event both reach the handler.
pub fn watch_with_error_handler(
    path: &Path,
    listener: Arc<dyn Fn(Option<String>) + Send + Sync>,
    on_error: Arc<dyn Fn() + Send + Sync>,
) -> Option<FsWatcher> {
    let watched = path.to_path_buf();
    let error_handler = Arc::clone(&on_error);
    let handler = move |event: notify::Result<notify::Event>| match event {
        Ok(event) => {
            for changed in &event.paths {
                let name = changed
                    .strip_prefix(&watched)
                    .ok()
                    .or_else(|| changed.file_name().map(Path::new))
                    .map(|name| name.to_string_lossy().into_owned());
                listener(name);
            }
            if event.paths.is_empty() {
                listener(None);
            }
        }
        Err(_) => error_handler(),
    };

    let mut watcher = match notify::recommended_watcher(handler) {
        Ok(watcher) => watcher,
        Err(_) => {
            on_error();
            return None;
        }
    };
    if watcher.watch(path, RecursiveMode::NonRecursive).is_err() {
        on_error();
        return None;
    }
    Some(FsWatcher { _inner: watcher })
}
