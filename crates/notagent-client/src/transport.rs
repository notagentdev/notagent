//! Port von `packages/client/src/transport.ts`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::errors::PiError;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

pub trait ByteTransport: Send + Sync {
    /// Sends one byte chunk. Calls must be delivered in invocation order.
    fn send(&self, chunk: Vec<u8>) -> BoxFuture<Result<(), PiError>>;
    /// Closes the transport. Implementations must make repeated calls harmless.
    fn close(&self);
}

pub type DataHandler = Arc<dyn Fn(&[u8]) + Send + Sync>;
pub type CloseHandler = Arc<dyn Fn() + Send + Sync>;
pub type ErrorHandler = Arc<dyn Fn(PiError) + Send + Sync>;

/// Delivers inbound bytes und die beiden terminalen Ereignisse.
#[derive(Clone)]
pub struct ByteTransportHandlers {
    on_data: DataHandler,
    on_close: CloseHandler,
    on_error: ErrorHandler,
}

impl ByteTransportHandlers {
    pub fn new(on_data: DataHandler, on_close: CloseHandler, on_error: ErrorHandler) -> Self {
        Self {
            on_data,
            on_close,
            on_error,
        }
    }

    /// Delivers an arbitrary inbound byte chunk.
    pub fn on_data(&self, chunk: &[u8]) {
        (self.on_data)(chunk);
    }

    /// Reports an orderly terminal close.
    pub fn on_close(&self) {
        (self.on_close)();
    }

    /// Reports a terminal transport failure.
    pub fn on_error(&self, error: PiError) {
        (self.on_error)(error);
    }
}

/// Creates a fresh connected, authenticated transport. Exactly one terminal
/// handler is expected.
pub type ByteTransportFactory = Arc<
    dyn Fn(ByteTransportHandlers) -> BoxFuture<Result<Arc<dyn ByteTransport>, PiError>>
        + Send
        + Sync,
>;
