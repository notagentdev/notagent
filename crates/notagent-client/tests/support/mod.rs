#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use notagent_client::{
    BoxFuture, ByteTransport, ByteTransportFactory, ByteTransportHandlers, PiClient,
    PiClientOptions, PiError, SessionHandle,
};
use notagent_protocol::{
    ClientMessage, ClientMessageDecoder, ModelRef, ProtocolVersionTag, RequestEnvelope,
    ServerHello, ServerMessage, ServerSnapshot, SessionPhase, SessionSnapshot, ThinkingLevel,
    encode_server_message,
};

type MessageListener = Arc<dyn Fn(&ClientMessage) + Send + Sync>;

#[derive(Default)]
struct MemoryServerState {
    handlers: Option<ByteTransportHandlers>,
    decoder: Option<ClientMessageDecoder>,
    message_listeners: Vec<MessageListener>,
    pub sent_by_client: Vec<Vec<u8>>,
    pub client_close_count: usize,
}

#[derive(Clone, Default)]
pub struct MemoryByteServer {
    state: Arc<Mutex<MemoryServerState>>,
}

impl MemoryByteServer {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryServerState> {
        self.state.lock().expect("memory server mutex")
    }

    pub fn connect(&self, handlers: ByteTransportHandlers) -> Arc<dyn ByteTransport> {
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

    /// Answers the client hello automatically.
    pub fn answer_hello(&self, connection_id: &str, snapshot: ServerSnapshot) {
        let server = self.clone();
        let connection_id = connection_id.to_owned();
        self.on_message(Arc::new(move |message: &ClientMessage| {
            if matches!(message, ClientMessage::Hello(_)) {
                server.send(&ServerMessage::Hello(ServerHello {
                    kind: notagent_protocol::HelloTag,
                    version: ProtocolVersionTag,
                    connection_id: connection_id.clone(),
                    snapshot: snapshot.clone(),
                }));
            }
        }));
    }

    pub fn send(&self, message: &ServerMessage) {
        self.send_raw(&encode_server_message(message, None).expect("encodes"));
    }

    pub fn send_split(&self, message: &ServerMessage, split_at: usize) {
        let frame = encode_server_message(message, None).expect("encodes");
        self.send_raw(&frame[..split_at]);
        self.send_raw(&frame[split_at..]);
    }

    pub fn send_together(&self, messages: &[ServerMessage]) {
        let mut chunk = Vec::new();
        for message in messages {
            chunk.extend(encode_server_message(message, None).expect("encodes"));
        }
        self.send_raw(&chunk);
    }

    pub fn send_raw(&self, chunk: &[u8]) {
        let handlers = self.lock().handlers.clone();
        if let Some(handlers) = handlers {
            handlers.on_data(chunk);
        }
    }

    pub fn close(&self) {
        let handlers = self.lock().handlers.clone();
        if let Some(handlers) = handlers {
            handlers.on_close();
        }
    }

    pub fn error(&self, error: PiError) {
        let handlers = self.lock().handlers.clone();
        if let Some(handlers) = handlers {
            handlers.on_error(error);
        }
    }

    pub fn sent_by_client(&self) -> usize {
        self.lock().sent_by_client.len()
    }

    pub fn client_close_count(&self) -> usize {
        self.lock().client_close_count
    }

    fn deliver(&self, chunk: &[u8]) -> Result<(), PiError> {
        let messages = {
            let mut state = self.lock();
            state.sent_by_client.push(chunk.to_vec());
            let decoder = state.decoder.as_mut().expect("connected decoder");
            decoder
                .push(chunk)
                .map_err(|error| PiError::ProtocolValidation(error.message().to_owned()))?
        };
        for message in messages {
            // Never call listeners while holding the lock: they send server
            // messages themselves.
            let listeners = self.lock().message_listeners.clone();
            for listener in listeners {
                listener(&message);
            }
        }
        Ok(())
    }
}

struct MemoryTransport {
    server: MemoryByteServer,
    closed: Arc<Mutex<bool>>,
}

impl ByteTransport for MemoryTransport {
    fn send(&self, chunk: Vec<u8>) -> BoxFuture<Result<(), PiError>> {
        // before the caller awaits the promise.
        if *self.closed.lock().expect("closed mutex") {
            return Box::pin(async { Err(PiError::Other("Transport is closed".to_owned())) });
        }
        let result = self.server.deliver(&chunk);
        Box::pin(async move { result })
    }

    fn close(&self) {
        let mut closed = self.closed.lock().expect("closed mutex");
        if *closed {
            return;
        }
        *closed = true;
        drop(closed);
        self.server.lock().client_close_count += 1;
    }
}

pub fn base_server_snapshot() -> ServerSnapshot {
    ServerSnapshot {
        server_id: "server-1".to_owned(),
        protocol_version: ProtocolVersionTag,
        revision: 1,
        sessions: vec![],
        models: vec![],
    }
}

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

pub fn create_client(server: &MemoryByteServer) -> PiClient {
    PiClient::new(PiClientOptions::new(server.factory())).expect("valid options")
}

pub async fn connect_client(server: &MemoryByteServer) -> PiClient {
    let client = create_client(server);
    server.answer_hello("connection-1", base_server_snapshot());
    client.connect().await.expect("connects");
    client
}

#[derive(Clone, Default)]
pub struct RequestLog {
    requests: Arc<Mutex<Vec<RequestEnvelope>>>,
}

impl RequestLog {
    pub fn all(&self) -> Vec<RequestEnvelope> {
        self.requests.lock().expect("request log mutex").clone()
    }

    pub fn commands(&self) -> Vec<String> {
        self.all()
            .iter()
            .map(|request| command_name(request).to_owned())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.requests.lock().expect("request log mutex").len()
    }

    pub fn last(&self) -> Option<RequestEnvelope> {
        self.requests
            .lock()
            .expect("request log mutex")
            .last()
            .cloned()
    }

    pub fn find(&self, command: &str) -> Option<RequestEnvelope> {
        self.all()
            .into_iter()
            .find(|request| command_name(request) == command)
    }
}

pub fn collect_requests(server: &MemoryByteServer) -> RequestLog {
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

pub fn command_name(request: &RequestEnvelope) -> &'static str {
    match request.request.name() {
        notagent_protocol::CommandName::List => "list",
        notagent_protocol::CommandName::Create => "create",
        notagent_protocol::CommandName::Attach => "attach",
        notagent_protocol::CommandName::Detach => "detach",
        notagent_protocol::CommandName::Prompt => "prompt",
        notagent_protocol::CommandName::Steer => "steer",
        notagent_protocol::CommandName::Abort => "abort",
        notagent_protocol::CommandName::SetModel => "set_model",
        notagent_protocol::CommandName::SetThinking => "set_thinking",
    }
}

pub fn ok_response(id: &str, result: notagent_protocol::CommandResult) -> ServerMessage {
    ServerMessage::Response(notagent_protocol::ResponseEnvelope::Ok(
        notagent_protocol::OkResponseEnvelope {
            kind: notagent_protocol::ResponseTag,
            id: id.to_owned(),
            ok: notagent_protocol::TrueTag,
            result,
        },
    ))
}

pub fn error_response(
    id: &str,
    code: notagent_protocol::ProtocolErrorCode,
    message: &str,
) -> ServerMessage {
    ServerMessage::Response(notagent_protocol::ResponseEnvelope::Error(
        notagent_protocol::ErrorResponseEnvelope {
            kind: notagent_protocol::ResponseTag,
            id: id.to_owned(),
            ok: notagent_protocol::FalseTag,
            error: notagent_protocol::ProtocolError {
                code,
                message: message.to_owned(),
                details: None,
            },
        },
    ))
}

pub fn attach_result(session: SessionSnapshot) -> notagent_protocol::CommandResult {
    notagent_protocol::CommandResult::Attach(notagent_protocol::AttachResult {
        command: notagent_protocol::AttachTag,
        session,
    })
}

pub fn detach_result(session_id: &str) -> notagent_protocol::CommandResult {
    notagent_protocol::CommandResult::Detach(notagent_protocol::DetachResult {
        command: notagent_protocol::DetachTag,
        session_id: session_id.to_owned(),
    })
}

pub fn session_event(snapshot: SessionSnapshot) -> ServerMessage {
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

pub async fn attach_session(
    client: &PiClient,
    server: &MemoryByteServer,
    snapshot: SessionSnapshot,
) -> SessionHandle {
    let requests = collect_requests(server);
    let attaching = client.attach_session(&snapshot.id);
    let request = requests.find("attach").expect("Missing attach request");
    server.send(&ok_response(&request.id, attach_result(snapshot)));
    attaching.await.expect("attaches")
}
