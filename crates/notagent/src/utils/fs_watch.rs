use std::path::Path;
use std::sync::Arc;

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

/// How long to wait before retrying a watcher that could not be installed.
pub const FS_WATCH_RETRY_DELAY_MS: u64 = 5000;

/// A live watcher. Dropping it stops the watch, which is what `close()` does in
pub struct FsWatcher {
    _inner: RecommendedWatcher,
}

/// are swallowed.
pub fn close_watcher(watcher: Option<FsWatcher>) {
    drop(watcher);
}

/// Watches `path`, calling `listener` with the changed entry's file name.
/// Returns `None` when the watch could not be installed, having called
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
