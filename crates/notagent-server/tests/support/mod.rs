#![allow(dead_code)]
//! Shared harness for the ported server test suites.

use std::sync::{Arc, Mutex};

use notagent_server::PiServer;
use notagent_server::errors::ServerError;
use notagent_server::testing::{ProtocolTestClient, TestServerService, connect_unix_test_client};
use notagent_server::transports::unix::{UnixServerOptions, create_unix_server};

pub struct Harness {
    pub directory: tempfile::TempDir,
    pub server: PiServer,
    pub service: TestServerService,
}

pub fn socket_path(directory: &tempfile::TempDir) -> String {
    directory
        .path()
        .join("server.sock")
        .to_string_lossy()
        .into_owned()
}

pub fn temp_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("pss-")
        .tempdir()
        .expect("temp dir")
}

pub fn unix_options(path: &str) -> UnixServerOptions {
    UnixServerOptions::new(path)
}

pub async fn start_server(service: TestServerService, mut options: UnixServerOptions) -> Harness {
    let directory = temp_directory();
    options.path = socket_path(&directory);
    let server = create_unix_server(service.as_service(), options).expect("server");
    server.start().await.expect("starts");
    Harness {
        directory,
        server,
        service,
    }
}

pub async fn start_default_server() -> Harness {
    let service = TestServerService::new();
    start_server(service, UnixServerOptions::default()).await
}

pub async fn connect(server: &PiServer) -> Arc<ProtocolTestClient> {
    let address = server.addresses().first().expect("address").clone();
    connect_unix_test_client(&address).await.expect("connects")
}

/// Collects errors reported through `PiServerOptions::on_error`.
#[derive(Clone, Default)]
pub struct ErrorLog {
    errors: Arc<Mutex<Vec<ServerError>>>,
}

impl ErrorLog {
    pub fn observer(&self) -> notagent_server::types::ErrorObserver {
        let errors = Arc::clone(&self.errors);
        Arc::new(move |error: ServerError| errors.lock().expect("error log mutex").push(error))
    }

    pub fn all(&self) -> Vec<ServerError> {
        self.errors.lock().expect("error log mutex").clone()
    }

    pub fn messages(&self) -> Vec<String> {
        self.all()
            .iter()
            .map(std::string::ToString::to_string)
            .collect()
    }
}

use async_trait::async_trait;
use futures::future::BoxFuture;
use notagent_protocol::{ModelMetadata, SessionMetadata};
use notagent_server::types::{CreateSessionOptions, PiServerService, PiSessionRuntime};

type ListSessionsHook = Arc<
    dyn Fn(Vec<SessionMetadata>) -> BoxFuture<'static, Result<Vec<SessionMetadata>, ServerError>>
        + Send
        + Sync,
>;
type ListModelsHook = Arc<dyn Fn() -> BoxFuture<'static, Result<(), ServerError>> + Send + Sync>;

/// Deviation class 1: TS extends `TestServerService` per test case; Rust
/// delegates to it and overrides through optional hooks.
#[derive(Clone, Default)]
pub struct HookedService {
    pub inner: TestServerService,
    pub on_list_sessions: Option<ListSessionsHook>,
    pub on_list_models: Option<ListModelsHook>,
    pub create_id_override: Option<String>,
}

impl HookedService {
    pub fn new(inner: TestServerService) -> Self {
        Self {
            inner,
            on_list_sessions: None,
            on_list_models: None,
            create_id_override: None,
        }
    }

    pub fn as_service(&self) -> Arc<dyn PiServerService> {
        Arc::new(self.clone())
    }
}

#[async_trait]
impl PiServerService for HookedService {
    async fn list_sessions(&self) -> Result<Vec<SessionMetadata>, ServerError> {
        let sessions = self.inner.list_sessions().await?;
        match &self.on_list_sessions {
            Some(hook) => hook(sessions).await,
            None => Ok(sessions),
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelMetadata>, ServerError> {
        if let Some(hook) = &self.on_list_models {
            hook().await?;
        }
        self.inner.list_models().await
    }

    async fn create_session(
        &self,
        options: CreateSessionOptions,
    ) -> Result<Arc<dyn PiSessionRuntime>, ServerError> {
        let options = match &self.create_id_override {
            Some(id) => CreateSessionOptions {
                id: id.clone(),
                ..options
            },
            None => options,
        };
        self.inner.create_session(options).await
    }

    async fn open_session(
        &self,
        session_id: &str,
    ) -> Result<Arc<dyn PiSessionRuntime>, ServerError> {
        self.inner.open_session(session_id).await
    }
}

pub async fn start_hooked_server(
    service: HookedService,
    mut options: UnixServerOptions,
) -> HookedHarness {
    let directory = temp_directory();
    options.path = socket_path(&directory);
    let server = create_unix_server(service.as_service(), options).expect("server");
    server.start().await.expect("starts");
    HookedHarness {
        directory,
        server,
        service,
    }
}

pub struct HookedHarness {
    pub directory: tempfile::TempDir,
    pub server: PiServer,
    pub service: HookedService,
}
