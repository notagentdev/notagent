//! Session server core (PiServer, LiveSessionManager, Unix transport).
//!
//! 1:1 port of `packages/server` (see crates/notagent-server/PARITY.md).
//! Port of `packages/server/src/index.ts`.

pub mod connection;
pub mod errors;
pub mod listener;
pub mod protocol;
pub mod server;
mod sessions;
mod snapshots;
pub mod transports;
pub mod types;

pub use connection::{
    ByteConnection, ByteConnectionAcceptor, ByteConnectionHandler, ConnectionStage,
};
pub use errors::{
    INTERNAL_SERVER_ERROR_MESSAGE, NOT_IMPLEMENTED_MESSAGE, PiServerError,
    PiServerOperationErrorCode, ServerError,
};
pub use listener::{PiServerListener, SharedListener};
pub use server::PiServer;
pub use types::*;
