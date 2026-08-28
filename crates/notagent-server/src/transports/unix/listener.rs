use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use notagent_protocol::DEFAULT_MAX_FRAME_LENGTH;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener as TokioUnixListener, UnixStream};

use crate::connection::{ByteConnection, ByteConnectionAcceptor};
use crate::errors::ServerError;
use crate::listener::{PiServerListener, SharedListener};
use crate::types::ErrorObserver;

use super::types::UnixListenerOptions;

const DEFAULT_SOCKET_MODE: u32 = 0o600;
const DEFAULT_GRACEFUL_CLOSE_TIMEOUT_MS: u64 = 5_000;
const MAX_UINT32: u64 = 0xffff_ffff;
const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;
const SOCKET_PROBE_TIMEOUT_MS: u64 = 1_000;
const MAX_UNIX_SOCKET_PATH_BYTES: usize = if cfg!(target_os = "linux") { 107 } else { 103 };

pub fn validate_unix_socket_path(path: &str, description: &str) -> Result<(), ServerError> {
    if path.is_empty() {
        return Err(ServerError::other(format!(
            "{description} must not be empty"
        )));
    }
    if path.len() > MAX_UNIX_SOCKET_PATH_BYTES {
        return Err(ServerError::other(format!(
            "{description} is too long; maximum is {MAX_UNIX_SOCKET_PATH_BYTES} UTF-8 bytes"
        )));
    }
    Ok(())
}

struct ResolvedUnixListenerOptions {
    path: String,
    mode: u32,
    graceful_close_timeout_ms: u64,
    max_pending_bytes: u64,
    on_error: Option<ErrorObserver>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    dev: u64,
    ino: u64,
}

struct UnixListenerState {
    listener: Option<Arc<TokioUnixListener>>,
    accept_task: Option<tokio::task::JoinHandle<()>>,
    socket_identity: Option<FileIdentity>,
    owned_bind_path: Option<String>,
    bound_path: Option<String>,
    connections: Vec<Arc<UnixByteConnection>>,
}

pub(crate) struct UnixListener {
    options: ResolvedUnixListenerOptions,
    state: Mutex<UnixListenerState>,
    closing: AtomicBool,
}

#[async_trait]
impl PiServerListener for UnixListener {
    fn address(&self) -> Option<String> {
        self.state
            .lock()
            .expect("listener mutex")
            .bound_path
            .clone()
    }

    async fn start(&self, accept: ByteConnectionAcceptor) -> Result<(), ServerError> {
        if self
            .state
            .lock()
            .expect("listener mutex")
            .listener
            .is_some()
        {
            return Err(ServerError::other("Unix listener is already started"));
        }
        if self.closing.load(Ordering::SeqCst) {
            return Err(ServerError::other("Unix listener is closing or closed"));
        }
        let path = self.options.path.clone();
        let owned_bind_path = owned_bind_path(&path);
        validate_unix_socket_path(&owned_bind_path, "PiServer private Unix bind path")?;
        create_private_directory(&path).await?;
        remove_stale_socket(&path).await?;
        remove_stale_socket(&owned_bind_path).await?;
        self.state.lock().expect("listener mutex").owned_bind_path = Some(owned_bind_path.clone());

        let listener = TokioUnixListener::bind(&owned_bind_path).map_err(io_error)?;
        let identity = match tokio::fs::symlink_metadata(&owned_bind_path).await {
            Ok(metadata) if metadata.file_type().is_socket() => FileIdentity {
                dev: metadata.dev(),
                ino: metadata.ino(),
            },
            Ok(_) => {
                self.cleanup_after_failed_start(None).await;
                return Err(ServerError::other(format!(
                    "Unix listener path is not a socket after binding: {owned_bind_path}"
                )));
            }
            Err(error) => {
                self.cleanup_after_failed_start(None).await;
                return Err(io_error(error));
            }
        };
        let listener = Arc::new(listener);
        {
            let mut state = self.state.lock().expect("listener mutex");
            state.socket_identity = Some(identity);
            state.listener = Some(Arc::clone(&listener));
        }
        if let Err(error) = tokio::fs::hard_link(&owned_bind_path, &path).await {
            self.cleanup_after_failed_start(Some(identity)).await;
            return Err(io_error(error));
        }
        if let Err(error) = set_socket_mode(&path, self.options.mode).await {
            self.cleanup_after_failed_start(Some(identity)).await;
            return Err(error);
        }
        self.state.lock().expect("listener mutex").bound_path = Some(path);
        self.spawn_accept_loop(listener, accept);
        Ok(())
    }

    async fn close(&self) -> Result<(), ServerError> {
        if self.closing.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let (accept_task, connections, owned_bind_path, identity) = {
            let mut state = self.state.lock().expect("listener mutex");
            state.bound_path = None;
            state.listener = None;
            (
                state.accept_task.take(),
                std::mem::take(&mut state.connections),
                state.owned_bind_path.take(),
                state.socket_identity.take(),
            )
        };
        if let Some(task) = accept_task {
            task.abort();
        }
        for connection in connections {
            let _ = connection.close(None).await;
        }
        if let Some(identity) = identity {
            self.cleanup_owned_socket(identity).await?;
        }
        if let Some(owned_bind_path) = owned_bind_path {
            remove_path(&owned_bind_path).await?;
        }
        Ok(())
    }
}

impl UnixListener {
    fn spawn_accept_loop(&self, listener: Arc<TokioUnixListener>, accept: ByteConnectionAcceptor) {
        let graceful_close_timeout_ms = self.options.graceful_close_timeout_ms;
        let max_pending_bytes = self.options.max_pending_bytes;
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let connection = Arc::new(UnixByteConnection::new(
                    stream,
                    graceful_close_timeout_ms,
                    max_pending_bytes,
                ));
                let handler = accept(Arc::clone(&connection) as Arc<dyn ByteConnection>);
                connection.spawn_read_loop(handler);
            }
        });
        self.state.lock().expect("listener mutex").accept_task = Some(task);
    }

    async fn cleanup_after_failed_start(&self, identity: Option<FileIdentity>) {
        let owned_bind_path = {
            let mut state = self.state.lock().expect("listener mutex");
            state.listener = None;
            state.socket_identity = None;
            state.owned_bind_path.take()
        };
        if let Some(identity) = identity
            && let Err(error) = self.cleanup_owned_socket(identity).await
        {
            self.report_error(error);
        }
        if let Some(owned_bind_path) = owned_bind_path
            && let Err(error) = remove_path(&owned_bind_path).await
        {
            self.report_error(error);
        }
    }

    /// Removes the published path only when it still is exactly the socket this
    /// listener bound (dev/ino verified through a rename).
    async fn cleanup_owned_socket(&self, identity: FileIdentity) -> Result<(), ServerError> {
        let path = self.options.path.clone();
        let current = match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error(error)),
        };
        if !current.file_type().is_socket()
            || current.dev() != identity.dev
            || current.ino() != identity.ino
        {
            return Ok(());
        }
        let preserved = sibling_path(&path, ".c-");
        match tokio::fs::rename(&path, &preserved).await {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error(error)),
        }
        let moved = tokio::fs::symlink_metadata(&preserved)
            .await
            .map_err(io_error)?;
        if moved.file_type().is_socket()
            && moved.dev() == identity.dev
            && moved.ino() == identity.ino
        {
            remove_path(&preserved).await?;
            return Ok(());
        }
        match tokio::fs::symlink_metadata(&path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::rename(&preserved, &path)
                    .await
                    .map_err(io_error)?;
            }
            Err(error) => return Err(io_error(error)),
            Ok(_) => (),
        }
        Err(ServerError::other(format!(
            "Unix listener path changed during cleanup; preserved replacement at {preserved}"
        )))
    }

    fn report_error(&self, error: ServerError) {
        let Some(observer) = &self.options.on_error else {
            return;
        };
        // Error observers cannot affect listener state.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| observer(error)));
    }
}

/// Exported only for transport-level verification.
pub struct UnixByteConnection {
    writer: tokio::sync::Mutex<Option<OwnedWriteHalf>>,
    reader: Mutex<Option<OwnedReadHalf>>,
    graceful_close_timeout_ms: u64,
    max_pending_bytes: u64,
    pending_bytes: Mutex<u64>,
    closed: AtomicBool,
    closing: AtomicBool,
}

impl UnixByteConnection {
    pub fn from_stream(
        stream: UnixStream,
        graceful_close_timeout_ms: u64,
        max_pending_bytes: u64,
    ) -> Self {
        Self::new(stream, graceful_close_timeout_ms, max_pending_bytes)
    }

    fn new(stream: UnixStream, graceful_close_timeout_ms: u64, max_pending_bytes: u64) -> Self {
        let (reader, writer) = stream.into_split();
        Self {
            writer: tokio::sync::Mutex::new(Some(writer)),
            reader: Mutex::new(Some(reader)),
            graceful_close_timeout_ms,
            max_pending_bytes,
            pending_bytes: Mutex::new(0),
            closed: AtomicBool::new(false),
            closing: AtomicBool::new(false),
        }
    }

    fn spawn_read_loop(
        self: &Arc<Self>,
        handler: Arc<dyn crate::connection::ByteConnectionHandler>,
    ) {
        let Some(mut reader) = self.reader.lock().expect("connection mutex").take() else {
            return;
        };
        let connection = Arc::clone(self);
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buffer).await {
                    Ok(0) => {
                        connection.mark_closed();
                        handler.on_close();
                        return;
                    }
                    Ok(read) => handler.on_data(&buffer[..read]),
                    Err(error) => {
                        handler.on_error(io_error(error));
                        connection.mark_closed();
                        handler.on_close();
                        return;
                    }
                }
            }
        });
    }

    fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.closing.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl ByteConnection for UnixByteConnection {
    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    async fn send(&self, chunk: Vec<u8>) -> Result<(), ServerError> {
        if self.closed.load(Ordering::SeqCst) || self.closing.load(Ordering::SeqCst) {
            return Err(ServerError::other("Unix connection is closed"));
        }
        {
            let mut pending = self.pending_bytes.lock().expect("pending bytes mutex");
            if *pending + chunk.len() as u64 > self.max_pending_bytes {
                return Err(ServerError::other(
                    "Unix connection exceeded its pending byte limit",
                ));
            }
            *pending += chunk.len() as u64;
        }
        let result = {
            let mut writer = self.writer.lock().await;
            match writer.as_mut() {
                Some(writer) if !self.closed.load(Ordering::SeqCst) => {
                    writer.write_all(&chunk).await.map_err(io_error)
                }
                _ => Err(ServerError::other("Unix connection is closed")),
            }
        };
        *self.pending_bytes.lock().expect("pending bytes mutex") -= chunk.len() as u64;
        result
    }

    async fn close(&self, final_chunk: Option<Vec<u8>>) -> Result<(), ServerError> {
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.closing.store(true, Ordering::SeqCst);
        let timeout = std::time::Duration::from_millis(self.graceful_close_timeout_ms);
        let graceful = async {
            let mut writer = self.writer.lock().await;
            if let Some(writer) = writer.as_mut() {
                if let Some(final_chunk) = final_chunk {
                    let _ = writer.write_all(&final_chunk).await;
                }
                let _ = writer.shutdown().await;
            }
            *writer = None;
        };
        let _ = tokio::time::timeout(timeout, graceful).await;
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

fn owned_bind_path(path: &str) -> String {
    let digest = Sha256::digest(path.as_bytes());
    let suffix: String = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..8]
        .to_owned();
    let directory = Path::new(path).parent().unwrap_or(Path::new("."));
    directory
        .join(format!(".p-{suffix}"))
        .to_string_lossy()
        .into_owned()
}

fn sibling_path(path: &str, prefix: &str) -> String {
    let unique = uuid::Uuid::new_v4().to_string()[..6].to_owned();
    let directory = Path::new(path).parent().unwrap_or(Path::new("."));
    directory
        .join(format!("{prefix}{unique}"))
        .to_string_lossy()
        .into_owned()
}

async fn create_private_directory(path: &str) -> Result<(), ServerError> {
    let directory: PathBuf = Path::new(path)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(io_error)?;
    let permissions = std::fs::Permissions::from_mode(0o700);
    if let Err(error) = tokio::fs::set_permissions(&directory, permissions).await
        && error.kind() != std::io::ErrorKind::PermissionDenied
    {
        return Err(io_error(error));
    }
    Ok(())
}

async fn remove_stale_socket(path: &str) -> Result<(), ServerError> {
    let original = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };
    if !original.file_type().is_socket() {
        return Err(ServerError::other(format!(
            "Refusing to remove non-socket Unix listener path: {path}"
        )));
    }
    if is_socket_live(path).await? {
        return Err(ServerError::other(format!(
            "Unix listener is already running: {path}"
        )));
    }

    let preserved = sibling_path(path, ".s-");
    match tokio::fs::rename(path, &preserved).await {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    }
    let current = tokio::fs::symlink_metadata(&preserved)
        .await
        .map_err(io_error)?;
    if !current.file_type().is_socket()
        || current.dev() != original.dev()
        || current.ino() != original.ino()
    {
        match tokio::fs::symlink_metadata(path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::rename(&preserved, path)
                    .await
                    .map_err(io_error)?;
            }
            Err(error) => return Err(io_error(error)),
            Ok(_) => (),
        }
        return Err(ServerError::other(format!(
            "Unix listener path changed while checking for a stale socket: {path}"
        )));
    }
    remove_path(&preserved).await
}

async fn remove_path(path: &str) -> Result<(), ServerError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

/// Probes the socket: a successful connect (or a timeout) means it is live.
async fn is_socket_live(path: &str) -> Result<bool, ServerError> {
    let probe = UnixStream::connect(path);
    match tokio::time::timeout(
        std::time::Duration::from_millis(SOCKET_PROBE_TIMEOUT_MS),
        probe,
    )
    .await
    {
        Err(_) => Ok(true),
        Ok(Ok(_)) => Ok(true),
        Ok(Err(error)) => match error.kind() {
            std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::NotFound
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset => Ok(false),
            _ => Err(io_error(error)),
        },
    }
}

async fn set_socket_mode(path: &str, mode: u32) -> Result<(), ServerError> {
    match tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::ENOSYS) | Some(libc::ENOTSUP)
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(io_error(error)),
    }
}

fn io_error(error: std::io::Error) -> ServerError {
    ServerError::other(error.to_string())
}

pub fn create_unix_listener(options: UnixListenerOptions) -> Result<SharedListener, ServerError> {
    let resolved = resolve_unix_listener_options(options)?;
    Ok(Arc::new(UnixListener {
        options: resolved,
        state: Mutex::new(UnixListenerState {
            listener: None,
            accept_task: None,
            socket_identity: None,
            owned_bind_path: None,
            bound_path: None,
            connections: Vec::new(),
        }),
        closing: AtomicBool::new(false),
    }))
}

fn resolve_unix_listener_options(
    options: UnixListenerOptions,
) -> Result<ResolvedUnixListenerOptions, ServerError> {
    validate_unix_socket_path(&options.path, "PiServer Unix socket path")?;
    let mode = options.mode.unwrap_or(DEFAULT_SOCKET_MODE);
    if mode > 0o777 {
        return Err(ServerError::other(
            "PiServer Unix socket mode must be an integer between 0 and 0o777",
        ));
    }
    let max_frame_length = options.max_frame_length.unwrap_or(DEFAULT_MAX_FRAME_LENGTH);
    if max_frame_length == 0 || max_frame_length > MAX_UINT32 {
        return Err(ServerError::other(format!(
            "PiServer maxFrameLength must be an integer between 1 and {MAX_UINT32}"
        )));
    }
    let max_pending_bytes = options.max_pending_bytes.unwrap_or(max_frame_length * 4);
    if max_pending_bytes < max_frame_length + 4 {
        return Err(ServerError::other(
            "PiServer maxPendingBytes must be a safe integer at least maxFrameLength + 4",
        ));
    }
    let graceful_close_timeout_ms = options
        .graceful_close_timeout_ms
        .unwrap_or(DEFAULT_GRACEFUL_CLOSE_TIMEOUT_MS);
    if graceful_close_timeout_ms == 0 || graceful_close_timeout_ms > MAX_TIMER_DELAY_MS {
        return Err(ServerError::other(format!(
            "PiServer gracefulCloseTimeoutMs must be an integer between 1 and {MAX_TIMER_DELAY_MS}"
        )));
    }
    Ok(ResolvedUnixListenerOptions {
        path: options.path,
        mode,
        max_pending_bytes,
        graceful_close_timeout_ms,
        on_error: options.on_error,
    })
}
