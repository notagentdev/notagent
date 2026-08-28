use std::sync::Arc;

use async_trait::async_trait;
use notagent_protocol::{
    ModelMetadata, ModelRef, SessionMetadata, SessionPhase, SessionSnapshot, ThinkingLevel,
    TranscriptProgress,
};

use crate::errors::{PiServerError, ServerError};
use crate::listener::SharedListener;

pub type ErrorObserver = Arc<dyn Fn(ServerError) + Send + Sync>;

pub struct PiServerOptions {
    pub listeners: Vec<SharedListener>,
    pub max_frame_length: Option<u64>,
    pub handshake_timeout_ms: Option<u64>,
    pub server_id: Option<String>,
    pub on_error: Option<ErrorObserver>,
}

impl PiServerOptions {
    pub fn new(listeners: Vec<SharedListener>) -> Self {
        Self {
            listeners,
            max_frame_length: None,
            handshake_timeout_ms: None,
            server_id: None,
            on_error: None,
        }
    }
}

/// `Omit<Extract<Command, { command: "prompt" }>, "command" | "sessionId">`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptInput {
    pub text: String,
}

/// `Omit<Extract<Command, { command: "steer" }>, "command" | "sessionId">`
pub type SteerInput = PromptInput;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CreateSessionOptions {
    /// A collision-resistant ID assigned by PiServer. The service must persist this exact ID.
    pub id: String,
    pub cwd: Option<String>,
    pub name: Option<String>,
    pub model: Option<ModelRef>,
    pub thinking_level: Option<ThinkingLevel>,
}

// (CONVENTIONS.md §9); runtime events are constructed and matched directly.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum PiSessionRuntimeEvent {
    Snapshot,
    Progress(TranscriptProgress),
    Error(PiServerError),
}

pub type RuntimeEventListener = Arc<dyn Fn(&PiSessionRuntimeEvent) + Send + Sync>;
pub type Unsubscribe = Box<dyn FnOnce() + Send + Sync>;

/// One acquired durable session. Conflicting operations must reject rather than queue.
#[async_trait]
pub trait PiSessionRuntime: Send + Sync {
    async fn snapshot(&self) -> Result<SessionSnapshot, ServerError>;
    fn get_phase(&self) -> SessionPhase;
    async fn prompt(&self, input: PromptInput) -> Result<(), ServerError>;
    async fn steer(&self, input: SteerInput) -> Result<(), ServerError>;
    async fn abort(&self) -> Result<(), ServerError>;
    async fn set_model(&self, model: ModelRef) -> Result<(), ServerError>;
    async fn set_thinking(&self, thinking_level: ThinkingLevel) -> Result<(), ServerError>;
    fn subscribe(&self, listener: RuntimeEventListener) -> Unsubscribe;
    async fn dispose(&self) -> Result<(), ServerError>;
}

/// Service boundary for durable sessions and exclusively acquired runtimes.
#[async_trait]
pub trait PiServerService: Send + Sync {
    async fn list_sessions(&self) -> Result<Vec<SessionMetadata>, ServerError>;
    async fn list_models(&self) -> Result<Vec<ModelMetadata>, ServerError>;
    async fn create_session(
        &self,
        options: CreateSessionOptions,
    ) -> Result<Arc<dyn PiSessionRuntime>, ServerError>;
    async fn open_session(
        &self,
        session_id: &str,
    ) -> Result<Arc<dyn PiSessionRuntime>, ServerError>;
}

pub type SessionRuntime = dyn PiSessionRuntime;
pub type SessionRuntimeEvent = PiSessionRuntimeEvent;
