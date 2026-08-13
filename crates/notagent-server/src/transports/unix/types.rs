//! Port of `packages/server/src/transports/unix/types.ts`.

use crate::types::ErrorObserver;

#[derive(Clone, Default)]
pub struct UnixListenerOptions {
    pub path: String,
    /// Socket filesystem permissions. Defaults to owner read/write only (0o600).
    pub mode: Option<u32>,
    /// Maximum framed bytes queued per connection before a slow peer is disconnected.
    pub max_pending_bytes: Option<u64>,
    pub graceful_close_timeout_ms: Option<u64>,
    /// Used to derive and validate maxPendingBytes. Must match the server when customized.
    pub max_frame_length: Option<u64>,
    pub on_error: Option<ErrorObserver>,
}

impl UnixListenerOptions {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Self::default()
        }
    }
}

/// `UnixServerOptions extends Omit<PiServerOptions, "listeners">, UnixListenerOptions`
#[derive(Clone, Default)]
pub struct UnixServerOptions {
    pub path: String,
    pub mode: Option<u32>,
    pub max_pending_bytes: Option<u64>,
    pub graceful_close_timeout_ms: Option<u64>,
    pub max_frame_length: Option<u64>,
    pub handshake_timeout_ms: Option<u64>,
    pub server_id: Option<String>,
    pub on_error: Option<ErrorObserver>,
}

impl UnixServerOptions {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Self::default()
        }
    }
}
