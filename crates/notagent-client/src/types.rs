use std::sync::Arc;

use notagent_protocol::{ModelRef, ThinkingLevel};

use crate::errors::PiError;
use crate::transport::ByteTransportFactory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionStateChange {
    pub state: ConnectionState,
    pub error: Option<PiError>,
}

pub type Unsubscribe = Box<dyn FnOnce() + Send + Sync>;

/// Reports subscriber failures without allowing them to corrupt client state.
pub type ListenerErrorHandler = Arc<dyn Fn(PiError) + Send + Sync>;

#[derive(Clone)]
pub struct PiClientOptions {
    pub transport_factory: ByteTransportFactory,
    pub max_frame_length: Option<u64>,
    pub on_listener_error: Option<ListenerErrorHandler>,
}

impl PiClientOptions {
    pub fn new(transport_factory: ByteTransportFactory) -> Self {
        Self {
            transport_factory,
            max_frame_length: None,
            on_listener_error: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateSessionOptions {
    pub cwd: Option<String>,
    pub name: Option<String>,
    pub model: Option<ModelRef>,
    pub thinking_level: Option<ThinkingLevel>,
}
