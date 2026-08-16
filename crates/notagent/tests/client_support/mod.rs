#![allow(dead_code)]

//! Port of `packages/coding-agent/test/client/support.ts` (111 LOC).
//!
//! The in-memory server speaks the real wire format, so the client layer is
//! exercised through the same encode/decode path as in production. The byte
//! server mirrors the one in `crates/notagent-client/tests/support/mod.rs`.

use std::sync::{Arc, Mutex};

use notagent::client::{RemoteSession, RemoteSessionOptions};
use notagent_client::{
    BoxFuture, ByteTransport, ByteTransportFactory, ByteTransportHandlers, PiClient,
    PiClientOptions, PiError,
};
use notagent_protocol::{
    ClientMessage, ClientMessageDecoder, CommandName, CommandResult, ModelRef, ProtocolVersionTag,
    RequestEnvelope, ServerHello, ServerMessage, ServerSnapshot, SessionPhase, SessionSnapshot,
    ThinkingLevel, TranscriptProgress, encode_server_message,
};
use tokio::sync::oneshot;

type MessageListener = Arc<dyn Fn(&ClientMessage) + Send + Sync>;

#[derive(Default)]
struct MemoryServerState {
    handlers: Option<ByteTransportHandlers>,
    decoder: Option<ClientMessageDecoder>,
    message_listeners: Vec<MessageListener>,
}

/// `MemoryServer`.
#[derive(Clone, Default)]
pub struct MemoryServer {
    state: Arc<Mutex<MemoryServerState>>,
}

impl MemoryServer {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryServerState> {
        self.state.lock().expect("memory server mutex")
    }

    fn connect(&self, handlers: ByteTransportHandlers) -> Arc<dyn ByteTransport> {
        let mut state = self.lock();
        state.handlers = Some(handlers);
        state.decoder = Some(ClientMessageDecoder::new(None).expect("decoder"));
        drop(state);
        Arc::new(MemoryTransport {
            server: self.clone(),
            closed: Arc::new(Mutex::new(false)),
        })
    }

    pub fn factory(&self) -> ByteTransportFactory {
        let server = self.clone();
        Arc::new(move |handlers: ByteTransportHandlers| {
            let transport = server.connect(handlers);
            let future: BoxFuture<Result<Arc<dyn ByteTransport>, PiError>> =
                Box::pin(async move { Ok(transport) });
            future
        })
    }

    pub fn on_message(&self, listener: MessageListener) {
        self.lock().message_listeners.push(listener);
    }

    pub fn send(&self, message: &ServerMessage) {
        let frame = encode_server_message(message, None).expect("encodes");
        let handlers = self.lock().handlers.clone();
        if let Some(handlers) = handlers {
            handlers.on_data(&frame);
        }
    }

    pub fn close(&self) {
        let handlers = self.lock().handlers.clone();
        if let Some(handlers) = handlers {
            handlers.on_close();
        }
    }

    fn deliver(&self, chunk: &[u8]) -> Result<(), PiError> {
        let messages = {
            let mut state = self.lock();
            let decoder = state.decoder.as_mut().expect("connected decoder");
            decoder
                .push(chunk)
                .map_err(|error| PiError::ProtocolValidation(error.message().to_owned()))?
        };
        for message in messages {
            // Never hold the lock while listeners run: they answer immediately.
            let listeners = self.lock().message_listeners.clone();
            for listener in listeners {
                listener(&message);
            }
        }
        Ok(())
    }
}

struct MemoryTransport {
    server: MemoryServer,
    closed: Arc<Mutex<bool>>,
}

impl ByteTransport for MemoryTransport {
    fn send(&self, chunk: Vec<u8>) -> BoxFuture<Result<(), PiError>> {
        if *self.closed.lock().expect("closed mutex") {
            return Box::pin(async { Err(PiError::Other("Transport is closed".to_owned())) });
        }
        let result = self.server.deliver(&chunk);
        Box::pin(async move { result })
    }

    fn close(&self) {
        *self.closed.lock().expect("closed mutex") = true;
    }
}

/// `serverSnapshot`.
pub fn server_snapshot() -> ServerSnapshot {
    ServerSnapshot {
        server_id: "server-1".to_owned(),
        protocol_version: ProtocolVersionTag,
        revision: 1,
        sessions: vec![],
        models: vec![],
    }
}

/// `sessionSnapshot` — the overrides of the TS helper are plain field writes.
pub fn session_snapshot(id: &str) -> SessionSnapshot {
    SessionSnapshot {
        id: id.to_owned(),
        name: None,
        cwd: "/workspace".to_owned(),
        created_at: 1,
        updated_at: 1,
        phase: SessionPhase::Idle,
        model: ModelRef::new("faux", "model"),
        thinking_level: ThinkingLevel::Off,
        attached: true,
        locked: true,
        revision: 1,
        transcript: vec![],
        queued_steer: vec![],
        queued_steer_count: 0,
    }
}

/// `connectClient`.
pub async fn connect_client(server: &MemoryServer) -> PiClient {
    let client = PiClient::new(PiClientOptions::new(server.factory())).expect("valid options");
    let answering = server.clone();
    server.on_message(Arc::new(move |message: &ClientMessage| {
        if matches!(message, ClientMessage::Hello(_)) {
            answering.send(&ServerMessage::Hello(ServerHello {
                kind: notagent_protocol::HelloTag,
                version: ProtocolVersionTag,
                connection_id: "connection-1".to_owned(),
                snapshot: server_snapshot(),
            }));
        }
    }));
    client.connect().await.expect("connects");
    client
}

/// `collectRequests`.
#[derive(Clone, Default)]
pub struct RequestLog {
    requests: Arc<Mutex<Vec<RequestEnvelope>>>,
}

impl RequestLog {
    pub fn all(&self) -> Vec<RequestEnvelope> {
        self.requests.lock().expect("request log mutex").clone()
    }

    pub fn commands(&self) -> Vec<&'static str> {
        self.all().iter().map(command_name).collect()
    }

    pub fn last(&self) -> Option<RequestEnvelope> {
        self.all().last().cloned()
    }

    pub fn find(&self, command: &str) -> Option<RequestEnvelope> {
        self.all()
            .into_iter()
            .find(|request| command_name(request) == command)
    }

    pub fn find_session(&self, command: &str, session_id: &str) -> Option<RequestEnvelope> {
        self.all().into_iter().find(|request| {
            command_name(request) == command
                && request_session_id(request).as_deref() == Some(session_id)
        })
    }

    pub fn is_empty(&self) -> bool {
        self.requests.lock().expect("request log mutex").is_empty()
    }
}

pub fn collect_requests(server: &MemoryServer) -> RequestLog {
    let log = RequestLog::default();
    let requests = Arc::clone(&log.requests);
    server.on_message(Arc::new(move |message: &ClientMessage| {
        if let ClientMessage::Request(request) = message {
            requests
                .lock()
                .expect("request log mutex")
                .push(request.clone());
        }
    }));
    log
}

/// `nextRequest` from the lifecycle suite.
pub fn next_request(
    server: &MemoryServer,
    command: &'static str,
) -> impl std::future::Future<Output = RequestEnvelope> + use<> {
    let (sender, receiver) = oneshot::channel::<RequestEnvelope>();
    let sender = Arc::new(Mutex::new(Some(sender)));
    server.on_message(Arc::new(move |message: &ClientMessage| {
        let ClientMessage::Request(request) = message else {
            return;
        };
        if command_name(request) != command {
            return;
        }
        if let Some(sender) = sender.lock().expect("next request mutex").take() {
            let _ = sender.send(request.clone());
        }
    }));
    async move { receiver.await.expect("request arrives") }
}

pub fn command_name(request: &RequestEnvelope) -> &'static str {
    match request.request.name() {
        CommandName::List => "list",
        CommandName::Create => "create",
        CommandName::Attach => "attach",
        CommandName::Detach => "detach",
        CommandName::Prompt => "prompt",
        CommandName::Steer => "steer",
        CommandName::Abort => "abort",
        CommandName::SetModel => "setModel",
        CommandName::SetThinking => "setThinking",
    }
}

pub fn request_session_id(request: &RequestEnvelope) -> Option<String> {
    use notagent_protocol::Command;
    match &request.request {
        Command::List(_) => None,
        Command::Create(_) => None,
        Command::Attach(command) => Some(command.session_id.clone()),
        Command::Detach(command) => Some(command.session_id.clone()),
        Command::Prompt(command) => Some(command.session_id.clone()),
        Command::Steer(command) => Some(command.session_id.clone()),
        Command::Abort(command) => Some(command.session_id.clone()),
        Command::SetModel(command) => Some(command.session_id.clone()),
        Command::SetThinking(command) => Some(command.session_id.clone()),
    }
}

pub fn request_text(request: &RequestEnvelope) -> Option<String> {
    use notagent_protocol::Command;
    match &request.request {
        Command::Prompt(command) => Some(command.text.clone()),
        Command::Steer(command) => Some(command.text.clone()),
        _ => None,
    }
}

pub fn ok_response(id: &str, result: CommandResult) -> ServerMessage {
    ServerMessage::Response(notagent_protocol::ResponseEnvelope::Ok(
        notagent_protocol::OkResponseEnvelope {
            kind: notagent_protocol::ResponseTag,
            id: id.to_owned(),
            ok: notagent_protocol::TrueTag,
            result,
        },
    ))
}

pub fn error_response(id: &str, message: &str) -> ServerMessage {
    ServerMessage::Response(notagent_protocol::ResponseEnvelope::Error(
        notagent_protocol::ErrorResponseEnvelope {
            kind: notagent_protocol::ResponseTag,
            id: id.to_owned(),
            ok: notagent_protocol::FalseTag,
            error: notagent_protocol::ProtocolError {
                code: notagent_protocol::ProtocolErrorCode::InvalidRequest,
                message: message.to_owned(),
                details: None,
            },
        },
    ))
}

pub fn attach_result(session: SessionSnapshot) -> CommandResult {
    CommandResult::Attach(notagent_protocol::AttachResult {
        command: notagent_protocol::AttachTag,
        session,
    })
}

pub fn create_result(session: SessionSnapshot) -> CommandResult {
    CommandResult::Create(notagent_protocol::CreateResult {
        command: notagent_protocol::CreateTag,
        session,
    })
}

pub fn prompt_result(session: SessionSnapshot) -> CommandResult {
    CommandResult::Prompt(notagent_protocol::PromptResult {
        command: notagent_protocol::PromptTag,
        session,
    })
}

pub fn steer_result(session: SessionSnapshot) -> CommandResult {
    CommandResult::Steer(notagent_protocol::SteerResult {
        command: notagent_protocol::SteerTag,
        session,
    })
}

pub fn abort_result(session: SessionSnapshot) -> CommandResult {
    CommandResult::Abort(notagent_protocol::AbortResult {
        command: notagent_protocol::AbortTag,
        session,
    })
}

pub fn detach_result(session_id: &str) -> CommandResult {
    CommandResult::Detach(notagent_protocol::DetachResult {
        command: notagent_protocol::DetachTag,
        session_id: session_id.to_owned(),
    })
}

pub fn snapshot_event(snapshot: SessionSnapshot) -> ServerMessage {
    ServerMessage::Event(notagent_protocol::EventEnvelope {
        kind: notagent_protocol::EventTag,
        event: notagent_protocol::ServerEvent::SessionSnapshot(
            notagent_protocol::SessionSnapshotEvent {
                kind: notagent_protocol::SessionSnapshotTag,
                snapshot,
            },
        ),
    })
}

pub fn progress_event(session_id: &str, progress: TranscriptProgress) -> ServerMessage {
    ServerMessage::Event(notagent_protocol::EventEnvelope {
        kind: notagent_protocol::EventTag,
        event: notagent_protocol::ServerEvent::SessionProgress(
            notagent_protocol::SessionProgressEvent {
                kind: notagent_protocol::SessionProgressTag,
                session_id: session_id.to_owned(),
                progress,
            },
        ),
    })
}

pub fn session_removed_event(session_id: &str) -> ServerMessage {
    ServerMessage::Event(notagent_protocol::EventEnvelope {
        kind: notagent_protocol::EventTag,
        event: notagent_protocol::ServerEvent::SessionRemoved(
            notagent_protocol::SessionRemovedEvent {
                kind: notagent_protocol::SessionRemovedTag,
                session_id: session_id.to_owned(),
            },
        ),
    })
}

/// `openRemoteSession`.
pub async fn open_remote_session(
    client: &PiClient,
    server: &MemoryServer,
    snapshot: SessionSnapshot,
    options: RemoteSessionOptions,
) -> RemoteSession {
    let requests = collect_requests(server);
    let opening = RemoteSession::open_session(client.clone(), &snapshot.id, options);
    let request = requests.find("attach").expect("Missing attach request");
    server.send(&ok_response(&request.id, attach_result(snapshot)));
    opening.await.expect("opens")
}
