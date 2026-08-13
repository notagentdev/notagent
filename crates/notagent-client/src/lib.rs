//! Transport-neutral protocol client with a lease system.
//!
//! 1:1 port of `packages/client` (see crates/notagent-client/PARITY.md).
//! Port of `packages/client/src/index.ts`.

mod client;
mod connection;
mod errors;
mod promise;
mod session_handle;
mod state;
mod transport;
mod types;
mod unix;

pub use client::PiClient;
pub use errors::PiError;
pub use session_handle::{AcquireSessionOptions, SessionHandle, SessionLeaseMode};
pub use state::Listener;
pub use transport::{BoxFuture, ByteTransport, ByteTransportFactory, ByteTransportHandlers};
pub use types::{
    ConnectionState, ConnectionStateChange, CreateSessionOptions, ListenerErrorHandler,
    PiClientOptions, Unsubscribe,
};
pub use unix::{UnixTransportOptions, create_unix_transport_factory};
