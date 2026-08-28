use std::sync::Arc;

use async_trait::async_trait;

use crate::connection::ByteConnectionAcceptor;
use crate::errors::ServerError;

/// Supplies established byte connections after any required transport authentication.
#[async_trait]
pub trait PiServerListener: Send + Sync {
    /// Human-readable bound address after startup, when the transport has one.
    fn address(&self) -> Option<String> {
        None
    }
    /// Starts listening and passes authorized connections to accept.
    async fn start(&self, accept: ByteConnectionAcceptor) -> Result<(), ServerError>;
    async fn close(&self) -> Result<(), ServerError>;
}

pub type SharedListener = Arc<dyn PiServerListener>;
