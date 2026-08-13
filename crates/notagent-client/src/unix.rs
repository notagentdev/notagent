//! Port of `packages/client/src/unix.ts`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use notagent_protocol::DEFAULT_MAX_FRAME_LENGTH;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::errors::PiError;
use crate::transport::{BoxFuture, ByteTransport, ByteTransportFactory, ByteTransportHandlers};

const MAX_UNIX_SOCKET_PATH_BYTES: usize = if cfg!(target_os = "linux") { 107 } else { 103 };

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnixTransportOptions {
    pub path: String,
    pub max_pending_bytes: Option<u64>,
}

impl UnixTransportOptions {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            max_pending_bytes: None,
        }
    }
}

/// Creates fresh Unix-domain socket transports for PiClient connection attempts.
pub fn create_unix_transport_factory(
    options: UnixTransportOptions,
) -> Result<ByteTransportFactory, PiError> {
    if options.path.is_empty() {
        return Err(PiError::Other(
            "Unix transport path must not be empty".to_owned(),
        ));
    }
    if options.path.len() > MAX_UNIX_SOCKET_PATH_BYTES {
        return Err(PiError::Other(format!(
            "Unix transport path is too long; maximum is {MAX_UNIX_SOCKET_PATH_BYTES} UTF-8 bytes"
        )));
    }
    let max_pending_bytes = options
        .max_pending_bytes
        .unwrap_or(DEFAULT_MAX_FRAME_LENGTH * 4);
    if max_pending_bytes == 0 {
        return Err(PiError::Other(
            "Unix transport maxPendingBytes must be a positive safe integer".to_owned(),
        ));
    }
    if cfg!(target_os = "windows") {
        return Err(PiError::Other(
            "Unix transport is not supported on Windows".to_owned(),
        ));
    }
    let path = options.path;
    Ok(Arc::new(move |handlers: ByteTransportHandlers| {
        let path = path.clone();
        let future: BoxFuture<Result<Arc<dyn ByteTransport>, PiError>> =
            Box::pin(async move { connect_unix_socket(path, max_pending_bytes, handlers).await });
        future
    }))
}

async fn connect_unix_socket(
    path: String,
    max_pending_bytes: u64,
    handlers: ByteTransportHandlers,
) -> Result<Arc<dyn ByteTransport>, PiError> {
    let stream = UnixStream::connect(&path)
        .await
        .map_err(|error| PiError::Other(error.to_string()))?;
    let (reader, writer) = stream.into_split();
    let terminal = Arc::new(AtomicBool::new(false));
    tokio::spawn(read_loop(reader, handlers, Arc::clone(&terminal)));
    Ok(Arc::new(UnixByteTransport {
        writer: Arc::new(tokio::sync::Mutex::new(writer)),
        pending_bytes: Arc::new(std::sync::Mutex::new(0)),
        max_pending_bytes,
        closed: Arc::new(AtomicBool::new(false)),
        terminal,
    }))
}

async fn read_loop(
    mut reader: OwnedReadHalf,
    handlers: ByteTransportHandlers,
    terminal: Arc<AtomicBool>,
) {
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                if !terminal.swap(true, Ordering::SeqCst) {
                    handlers.on_close();
                }
                return;
            }
            Ok(read) => {
                if terminal.load(Ordering::SeqCst) {
                    return;
                }
                handlers.on_data(&buffer[..read]);
            }
            Err(error) => {
                if !terminal.swap(true, Ordering::SeqCst) {
                    handlers.on_error(PiError::Other(error.to_string()));
                }
                return;
            }
        }
    }
}

struct UnixByteTransport {
    writer: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
    pending_bytes: Arc<std::sync::Mutex<u64>>,
    max_pending_bytes: u64,
    closed: Arc<AtomicBool>,
    terminal: Arc<AtomicBool>,
}

impl ByteTransport for UnixByteTransport {
    fn send(&self, chunk: Vec<u8>) -> BoxFuture<Result<(), PiError>> {
        if self.closed.load(Ordering::SeqCst) {
            return Box::pin(async { Err(PiError::Other("Unix transport is closed".to_owned())) });
        }
        {
            let mut pending = self.pending_bytes.lock().expect("pending bytes mutex");
            if *pending + chunk.len() as u64 > self.max_pending_bytes {
                return Box::pin(async {
                    Err(PiError::Other(
                        "Unix transport exceeded its pending byte limit".to_owned(),
                    ))
                });
            }
            *pending += chunk.len() as u64;
        }
        let writer = Arc::clone(&self.writer);
        let pending_bytes = Arc::clone(&self.pending_bytes);
        let closed = Arc::clone(&self.closed);
        Box::pin(async move {
            // The fair tokio mutex serializes writes in call order (TS: the
            // `writeTail` promise chain).
            let result = {
                let mut writer = writer.lock().await;
                if closed.load(Ordering::SeqCst) {
                    Err(PiError::Other("Unix transport is closed".to_owned()))
                } else {
                    writer
                        .write_all(&chunk)
                        .await
                        .map_err(|error| PiError::Other(error.to_string()))
                }
            };
            *pending_bytes.lock().expect("pending bytes mutex") -= chunk.len() as u64;
            result
        })
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.terminal.store(true, Ordering::SeqCst);
        let writer = Arc::clone(&self.writer);
        tokio::spawn(async move {
            let _ = writer.lock().await.shutdown().await;
        });
    }
}
