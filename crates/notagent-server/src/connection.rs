use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::future::Shared;
use notagent_protocol::ClientMessageDecoder;
use tokio::task::JoinHandle;

use crate::errors::ServerError;

/// An established, authorized ordered byte connection.
#[async_trait]
pub trait ByteConnection: Send + Sync {
    fn closed(&self) -> bool;
    async fn send(&self, chunk: Vec<u8>) -> Result<(), ServerError>;
    async fn close(&self, final_chunk: Option<Vec<u8>>) -> Result<(), ServerError>;
}

pub trait ByteConnectionHandler: Send + Sync {
    fn on_data(&self, chunk: &[u8]);
    fn on_close(&self);
    fn on_error(&self, error: ServerError);
}

pub type ByteConnectionAcceptor =
    Arc<dyn Fn(Arc<dyn ByteConnection>) -> Arc<dyn ByteConnectionHandler> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStage {
    AwaitingHello,
    Handshaking,
    Ready,
    Closing,
    Closed,
}

pub(crate) type HandshakePromise = Shared<futures::future::BoxFuture<'static, ()>>;

pub(crate) struct ConnectionData {
    pub decoder: ClientMessageDecoder,
    pub session_ids: HashSet<String>,
    pub stage: ConnectionStage,
    pub disconnected: bool,
    pub handshake_complete: bool,
    pub handshake: Option<HandshakePromise>,
    pub handshake_timeout: Option<JoinHandle<()>>,
}

pub(crate) struct ConnectionState {
    pub id: String,
    pub connection: Arc<dyn ByteConnection>,
    pub(crate) data: Mutex<ConnectionData>,
}

impl ConnectionState {
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, ConnectionData> {
        self.data.lock().expect("connection state mutex")
    }

    pub(crate) fn stage(&self) -> ConnectionStage {
        self.lock().stage
    }

    pub(crate) fn set_stage(&self, stage: ConnectionStage) {
        self.lock().stage = stage;
    }

    pub(crate) fn disconnected(&self) -> bool {
        self.lock().disconnected
    }

    pub(crate) fn is_terminal(&self) -> bool {
        let data = self.lock();
        data.disconnected
            || data.stage == ConnectionStage::Closing
            || data.stage == ConnectionStage::Closed
    }

    pub(crate) fn clear_handshake_timeout(&self) {
        if let Some(handle) = self.lock().handshake_timeout.take() {
            handle.abort();
        }
    }
}
