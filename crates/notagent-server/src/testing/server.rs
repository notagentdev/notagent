use std::sync::Arc;

use crate::errors::ServerError;
use crate::listener::SharedListener;
use crate::server::PiServer;
use crate::types::{ErrorObserver, PiServerOptions, PiServerService};

use super::service::TestServerService;

pub struct TestServerOptions {
    pub listeners: Vec<SharedListener>,
    pub max_frame_length: Option<u64>,
    pub handshake_timeout_ms: Option<u64>,
    pub server_id: Option<String>,
    pub on_error: Option<ErrorObserver>,
    pub service: Option<Arc<dyn PiServerService>>,
}

impl TestServerOptions {
    pub fn new(listeners: Vec<SharedListener>) -> Self {
        Self {
            listeners,
            max_frame_length: None,
            handshake_timeout_ms: None,
            server_id: None,
            on_error: None,
            service: None,
        }
    }
}

pub struct TestServer {
    pub server: PiServer,
    pub service: Arc<dyn PiServerService>,
}

/// Create an unstarted PiServer with deterministic defaults for transport
/// conformance tests.
pub fn create_test_server(options: TestServerOptions) -> Result<TestServer, ServerError> {
    let service = options
        .service
        .unwrap_or_else(|| TestServerService::new().as_service());
    let server = PiServer::new(
        Arc::clone(&service),
        PiServerOptions {
            listeners: options.listeners,
            max_frame_length: options.max_frame_length,
            handshake_timeout_ms: options.handshake_timeout_ms,
            server_id: options.server_id,
            on_error: options.on_error,
        },
    )?;
    Ok(TestServer { server, service })
}
