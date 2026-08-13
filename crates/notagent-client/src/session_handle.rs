//! Port von `packages/client/src/session-handle.ts`.

use std::future::Future;
use std::sync::Arc;

use notagent_protocol::{
    Command, CommandResult, ModelRef, ServerEvent, SessionSnapshot, ThinkingLevel,
};

use crate::errors::PiError;
use crate::promise::SharedPromise;
use crate::state::Listener;
use crate::types::Unsubscribe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLeaseMode {
    Shared,
    Exclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquireSessionOptions {
    pub mode: SessionLeaseMode,
}

type SnapshotResult = Result<SessionSnapshot, PiError>;

#[derive(Clone)]
pub(crate) struct SessionHandleCallbacks {
    pub(crate) is_attached: Arc<dyn Fn() -> bool + Send + Sync>,
    pub(crate) get_snapshot: Arc<dyn Fn() -> Option<SessionSnapshot> + Send + Sync>,
    #[allow(clippy::type_complexity)]
    pub(crate) subscribe:
        Arc<dyn Fn(Listener<SessionSnapshot>) -> Result<Unsubscribe, PiError> + Send + Sync>,
    #[allow(clippy::type_complexity)]
    pub(crate) on_event:
        Arc<dyn Fn(Listener<ServerEvent>) -> Result<Unsubscribe, PiError> + Send + Sync>,
    pub(crate) detach: Arc<dyn Fn() -> SharedPromise<()> + Send + Sync>,
    pub(crate) dispose: Arc<dyn Fn() -> SharedPromise<()> + Send + Sync>,
    #[allow(clippy::type_complexity)]
    pub(crate) request: Arc<dyn Fn(Command) -> SharedPromise<CommandResult> + Send + Sync>,
}

/// `SessionLease` / `PiSessionHandle`.
///
/// Abweichung Klasse 1: `subscribe`/`onEvent` geben `Result` zurück statt zu
/// werfen; `AsyncDisposable` entfällt (Rust kennt kein `Symbol.asyncDispose`),
/// `dispose()` ist der explizite Ersatz.
#[derive(Clone)]
pub struct SessionHandle {
    id: String,
    callbacks: SessionHandleCallbacks,
}

impl std::fmt::Debug for SessionHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionHandle")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl SessionHandle {
    pub(crate) fn new(id: String, callbacks: SessionHandleCallbacks) -> Self {
        Self { id, callbacks }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn attached(&self) -> bool {
        (self.callbacks.is_attached)()
    }

    pub fn active(&self) -> bool {
        self.attached()
    }

    pub fn snapshot(&self) -> Option<SessionSnapshot> {
        (self.callbacks.get_snapshot)()
    }

    pub fn subscribe(&self, listener: Listener<SessionSnapshot>) -> Result<Unsubscribe, PiError> {
        (self.callbacks.subscribe)(listener)
    }

    pub fn on_event(&self, listener: Listener<ServerEvent>) -> Result<Unsubscribe, PiError> {
        (self.callbacks.on_event)(listener)
    }

    pub fn detach(&self) -> impl Future<Output = Result<(), PiError>> + Send + use<> {
        (self.callbacks.detach)()
    }

    pub fn dispose(&self) -> impl Future<Output = Result<(), PiError>> + Send + use<> {
        (self.callbacks.dispose)()
    }

    pub fn prompt(&self, text: &str) -> impl Future<Output = SnapshotResult> + Send + use<> {
        self.session_request(Command::Prompt(notagent_protocol::PromptCommand {
            command: notagent_protocol::PromptTag,
            session_id: self.id.clone(),
            text: text.to_owned(),
        }))
    }

    pub fn steer(&self, text: &str) -> impl Future<Output = SnapshotResult> + Send + use<> {
        self.session_request(Command::Steer(notagent_protocol::SteerCommand {
            command: notagent_protocol::SteerTag,
            session_id: self.id.clone(),
            text: text.to_owned(),
        }))
    }

    pub fn abort(&self) -> impl Future<Output = SnapshotResult> + Send + use<> {
        self.session_request(Command::Abort(notagent_protocol::AbortCommand {
            command: notagent_protocol::AbortTag,
            session_id: self.id.clone(),
        }))
    }

    pub fn set_model(
        &self,
        model: ModelRef,
    ) -> impl Future<Output = SnapshotResult> + Send + use<> {
        self.session_request(Command::SetModel(notagent_protocol::SetModelCommand {
            command: notagent_protocol::SetModelTag,
            session_id: self.id.clone(),
            model,
        }))
    }

    pub fn set_thinking(
        &self,
        thinking_level: ThinkingLevel,
    ) -> impl Future<Output = SnapshotResult> + Send + use<> {
        self.session_request(Command::SetThinking(
            notagent_protocol::SetThinkingCommand {
                command: notagent_protocol::SetThinkingTag,
                session_id: self.id.clone(),
                thinking_level,
            },
        ))
    }

    fn session_request(
        &self,
        command: Command,
    ) -> impl Future<Output = SnapshotResult> + Send + use<> {
        let response = (self.callbacks.request)(command);
        async move { session_from_result(response.await?) }
    }
}

fn session_from_result(result: CommandResult) -> SnapshotResult {
    match result {
        CommandResult::Create(result) => Ok(result.session),
        CommandResult::Attach(result) => Ok(result.session),
        CommandResult::Prompt(result) => Ok(result.session),
        CommandResult::Steer(result) => Ok(result.session),
        CommandResult::Abort(result) => Ok(result.session),
        CommandResult::SetModel(result) => Ok(result.session),
        CommandResult::SetThinking(result) => Ok(result.session),
        CommandResult::List(_) | CommandResult::Detach(_) => Err(PiError::ProtocolValidation(
            "Response has no session snapshot".to_owned(),
        )),
    }
}
