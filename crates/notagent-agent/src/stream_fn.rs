use std::sync::{Mutex, OnceLock};

use crate::types::StreamFn;

fn slot() -> &'static Mutex<Option<StreamFn>> {
    static DEFAULT_STREAM_FN: OnceLock<Mutex<Option<StreamFn>>> = OnceLock::new();
    DEFAULT_STREAM_FN.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, thiserror::Error)]
#[error(
    "No default stream function configured. Pass streamFn explicitly or call setDefaultStreamFn()."
)]
pub struct NoDefaultStreamFn;

/// `setDefaultStreamFn(streamFn)`
pub fn set_default_stream_fn(stream_fn: Option<StreamFn>) {
    *slot().lock().expect("Default-StreamFn vergiftet") = stream_fn;
}

/// `getDefaultStreamFn()`
pub fn get_default_stream_fn() -> Result<StreamFn, NoDefaultStreamFn> {
    slot()
        .lock()
        .expect("Default-StreamFn vergiftet")
        .clone()
        .ok_or(NoDefaultStreamFn)
}
