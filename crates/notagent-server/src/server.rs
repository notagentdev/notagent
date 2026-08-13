//! Port of `packages/server/src/server.ts`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use futures::FutureExt;
use futures::future::BoxFuture;
use notagent_protocol::{
    ClientHello, ClientMessage, ClientMessageDecoder, DEFAULT_MAX_FRAME_LENGTH,
    ErrorResponseEnvelope, EventEnvelope, FalseTag, FrameDecoderOptions, HelloErrorTag, HelloTag,
    OkResponseEnvelope, PROTOCOL_VERSION, ProtocolError, ProtocolErrorCode, ProtocolVersionTag,
    RequestEnvelope, ResponseEnvelope, ResponseTag, ServerHello, ServerHelloError, ServerMessage,
    ServerSnapshot, TrueTag, encode_server_message, is_supported_protocol_version,
};

use crate::connection::{
    ByteConnection, ByteConnectionHandler, ConnectionData, ConnectionStage, ConnectionState,
    HandshakePromise,
};
use crate::errors::{INTERNAL_SERVER_ERROR_MESSAGE, NOT_IMPLEMENTED_MESSAGE, ServerError};
use crate::listener::SharedListener;
use crate::sessions::LiveSessionManager;
use crate::snapshots::ServerSnapshotPublisher;
use crate::types::{ErrorObserver, PiServerOptions, PiServerService};

const DEFAULT_HANDSHAKE_TIMEOUT_MS: u64 = 5_000;
const MAX_UINT32: u64 = 0xffff_ffff;
const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;

pub(crate) struct PiServerInner {
    pub(crate) id: String,
    pub(crate) service: Arc<dyn PiServerService>,
    listeners: Vec<SharedListener>,
    max_frame_length: u64,
    handshake_timeout_ms: u64,
    on_error: Option<ErrorObserver>,
    connections: Mutex<Vec<Arc<ConnectionState>>>,
    sessions: Arc<LiveSessionManager>,
    snapshots: ServerSnapshotPublisher,
    closing: AtomicBool,
    started: AtomicBool,
}

pub struct PiServer {
    inner: Arc<PiServerInner>,
}

impl PiServer {
    pub fn new(
        service: Arc<dyn PiServerService>,
        options: PiServerOptions,
    ) -> Result<Self, ServerError> {
        let (max_frame_length, handshake_timeout_ms) = resolve_options(&options)?;
        let id = options
            .server_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let inner = Arc::new_cyclic(|weak: &Weak<PiServerInner>| PiServerInner {
            id: id.clone(),
            service,
            listeners: options.listeners,
            max_frame_length,
            handshake_timeout_ms,
            on_error: options.on_error,
            connections: Mutex::new(Vec::new()),
            sessions: Arc::new(LiveSessionManager::new(weak.clone())),
            snapshots: ServerSnapshotPublisher::new(id),
            closing: AtomicBool::new(false),
            started: AtomicBool::new(false),
        });
        Ok(Self { inner })
    }

    pub fn id(&self) -> &str {
        &self.inner.id
    }

    pub fn addresses(&self) -> Vec<String> {
        self.inner
            .listeners
            .iter()
            .filter_map(|listener| listener.address())
            .collect()
    }

    pub async fn start(&self) -> Result<(), ServerError> {
        self.inner.start().await
    }

    pub async fn close(&self) -> Result<(), ServerError> {
        self.inner.close().await
    }
}

impl PiServerInner {
    pub(crate) fn is_closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    async fn start(self: &Arc<Self>) -> Result<(), ServerError> {
        if self.started.load(Ordering::SeqCst) {
            return Err(ServerError::other("PiServer is already started"));
        }
        if self.is_closing() {
            return Err(ServerError::other("PiServer is closing or closed"));
        }
        let mut started: Vec<SharedListener> = Vec::new();
        for listener in &self.listeners {
            let server = Arc::clone(self);
            let acceptor: crate::connection::ByteConnectionAcceptor =
                Arc::new(move |connection| server.accept(connection));
            match listener.start(acceptor).await {
                Ok(()) => started.push(Arc::clone(listener)),
                Err(error) => {
                    self.closing.store(true, Ordering::SeqCst);
                    for listener in started {
                        let _ = listener.close().await;
                    }
                    self.close_server_state().await;
                    return Err(error);
                }
            }
        }
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn accept(
        self: &Arc<Self>,
        connection: Arc<dyn ByteConnection>,
    ) -> Arc<dyn ByteConnectionHandler> {
        if self.is_closing() {
            let closing = Arc::clone(self);
            let closing_connection = Arc::clone(&connection);
            tokio::spawn(async move {
                closing
                    .close_byte_connection(&closing_connection, None)
                    .await;
            });
            return Arc::new(ClosingHandler {
                server: Arc::clone(self),
            });
        }

        let state = Arc::new(ConnectionState {
            id: uuid::Uuid::new_v4().to_string(),
            connection,
            data: Mutex::new(ConnectionData {
                decoder: ClientMessageDecoder::new(Some(
                    FrameDecoderOptions::with_max_frame_length(self.max_frame_length),
                ))
                .expect("validated frame length"),
                session_ids: HashSet::new(),
                stage: ConnectionStage::AwaitingHello,
                disconnected: false,
                handshake_complete: false,
                handshake: None,
                handshake_timeout: None,
            }),
        });
        let timeout_server = Arc::clone(self);
        let timeout_state = Arc::clone(&state);
        let timeout_ms = self.handshake_timeout_ms;
        let handshake_timeout = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)).await;
            timeout_server
                .fail_protocol(
                    &timeout_state,
                    ProtocolError {
                        code: ProtocolErrorCode::InvalidRequest,
                        message: "Handshake timeout".to_owned(),
                        details: None,
                    },
                )
                .await;
        });
        state.lock().handshake_timeout = Some(handshake_timeout);
        self.connections
            .lock()
            .expect("connections mutex")
            .push(Arc::clone(&state));

        Arc::new(ServerConnectionHandler {
            server: Arc::clone(self),
            state,
        })
    }

    async fn close(self: &Arc<Self>) -> Result<(), ServerError> {
        self.closing.store(true, Ordering::SeqCst);
        let mut result = Ok(());
        for listener in &self.listeners {
            if let Err(error) = listener.close().await {
                result = Err(error);
            }
        }
        self.close_server_state().await;
        self.started.store(false, Ordering::SeqCst);
        result
    }

    fn receive(self: &Arc<Self>, state: &Arc<ConnectionState>, chunk: &[u8]) {
        if state.is_terminal() {
            return;
        }
        let messages = {
            let mut data = state.lock();
            match data.decoder.push(chunk) {
                Ok(messages) => messages,
                Err(error) => {
                    drop(data);
                    let server = Arc::clone(self);
                    let state = Arc::clone(state);
                    let protocol_error = server.to_protocol_error(
                        &ServerError::ProtocolValidation(error.message().to_owned()),
                    );
                    tokio::spawn(async move {
                        server.fail_protocol(&state, protocol_error).await;
                    });
                    return;
                }
            }
        };
        for message in messages {
            if state.is_terminal() {
                return;
            }
            self.dispatch_message(state, message);
        }
    }

    fn dispatch_message(self: &Arc<Self>, state: &Arc<ConnectionState>, message: ClientMessage) {
        let stage = state.stage();
        if stage == ConnectionStage::AwaitingHello {
            let ClientMessage::Hello(hello) = message else {
                self.spawn_fail_protocol(
                    state,
                    ProtocolErrorCode::InvalidRequest,
                    "The first client message must be hello",
                );
                return;
            };
            state.set_stage(ConnectionStage::Handshaking);
            let server = Arc::clone(self);
            let handshake_state = Arc::clone(state);
            let handshake: HandshakePromise = (Box::pin(async move {
                server.finish_handshake(&handshake_state, hello).await;
            }) as BoxFuture<'static, ()>)
                .shared();
            state.lock().handshake = Some(handshake.clone());
            tokio::spawn(handshake);
            return;
        }

        let envelope = match message {
            ClientMessage::Hello(_) => {
                self.spawn_fail_protocol(
                    state,
                    ProtocolErrorCode::InvalidRequest,
                    "hello may only be sent as the first message",
                );
                return;
            }
            ClientMessage::Request(envelope) => envelope,
        };

        if stage == ConnectionStage::Ready {
            let server = Arc::clone(self);
            let state = Arc::clone(state);
            tokio::spawn(async move {
                server.handle_request(&state, envelope).await;
            });
            return;
        }
        if stage != ConnectionStage::Handshaking {
            return;
        }
        let Some(handshake) = state.lock().handshake.clone() else {
            return;
        };
        let server = Arc::clone(self);
        let state = Arc::clone(state);
        tokio::spawn(async move {
            handshake.await;
            if state.stage() == ConnectionStage::Ready && !state.disconnected() {
                server.handle_request(&state, envelope).await;
            }
        });
    }

    async fn finish_handshake(self: &Arc<Self>, state: &Arc<ConnectionState>, hello: ClientHello) {
        if !is_supported_protocol_version(hello.version) {
            self.fail_protocol(
                state,
                ProtocolError {
                    code: ProtocolErrorCode::Version,
                    message: format!(
                        "Unsupported protocol version {}; expected {PROTOCOL_VERSION}",
                        hello.version
                    ),
                    details: None,
                },
            )
            .await;
            return;
        }

        let snapshot = match self.server_snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let protocol_error = self.to_protocol_error(&error);
                self.fail_protocol(state, protocol_error).await;
                return;
            }
        };
        if self.is_closing()
            || state.disconnected()
            || state.stage() != ConnectionStage::Handshaking
            || state.connection.closed()
        {
            return;
        }
        let sent = self
            .send_message(
                state,
                ServerMessage::Hello(ServerHello {
                    kind: HelloTag,
                    version: ProtocolVersionTag,
                    connection_id: state.id.clone(),
                    snapshot: snapshot.clone(),
                }),
            )
            .await;
        if sent && !state.disconnected() && state.stage() == ConnectionStage::Handshaking {
            {
                let mut data = state.lock();
                data.handshake_complete = true;
                data.stage = ConnectionStage::Ready;
            }
            state.clear_handshake_timeout();
            if snapshot.revision != self.snapshots.current_revision()
                && let Ok(current) = self.server_snapshot().await
            {
                self.send_message(
                    state,
                    ServerMessage::Event(ServerSnapshotPublisher::envelope(current)),
                )
                .await;
            }
        }
    }

    async fn handle_request(
        self: &Arc<Self>,
        state: &Arc<ConnectionState>,
        envelope: RequestEnvelope,
    ) {
        match self.sessions.execute_command(state, envelope.request).await {
            Ok(result) => {
                self.send_message(
                    state,
                    ServerMessage::Response(ResponseEnvelope::Ok(OkResponseEnvelope {
                        kind: ResponseTag,
                        id: envelope.id,
                        ok: TrueTag,
                        result,
                    })),
                )
                .await;
            }
            Err(error) => {
                let protocol_error = self.to_protocol_error(&error);
                self.send_message(
                    state,
                    ServerMessage::Response(ResponseEnvelope::Error(ErrorResponseEnvelope {
                        kind: ResponseTag,
                        id: envelope.id,
                        ok: FalseTag,
                        error: protocol_error,
                    })),
                )
                .await;
            }
        }
    }

    fn transport_closed(self: &Arc<Self>, state: &Arc<ConnectionState>) {
        {
            let mut data = state.lock();
            if !data.disconnected
                && data.stage != ConnectionStage::Closing
                && let Err(error) = data.decoder.end()
            {
                let message = error.message().to_owned();
                drop(data);
                self.report_error(ServerError::ProtocolValidation(message));
            }
        }
        let server = Arc::clone(self);
        let state = Arc::clone(state);
        tokio::spawn(async move {
            server.disconnect(&state).await;
        });
    }

    pub(crate) async fn disconnect(self: &Arc<Self>, state: &Arc<ConnectionState>) {
        let handshake_complete = {
            let mut data = state.lock();
            if data.disconnected {
                return;
            }
            let handshake_complete = data.handshake_complete;
            data.disconnected = true;
            data.stage = ConnectionStage::Closed;
            handshake_complete
        };
        state.clear_handshake_timeout();
        self.connections
            .lock()
            .expect("connections mutex")
            .retain(|candidate| !Arc::ptr_eq(candidate, state));
        self.sessions.disconnect(state).await;
        if !self.is_closing() && handshake_complete {
            self.broadcast_server_snapshot();
        }
    }

    pub(crate) async fn send_event(
        self: &Arc<Self>,
        state: &Arc<ConnectionState>,
        envelope: EventEnvelope,
    ) -> bool {
        self.send_message(state, ServerMessage::Event(envelope))
            .await
    }

    async fn send_message(
        self: &Arc<Self>,
        state: &Arc<ConnectionState>,
        message: ServerMessage,
    ) -> bool {
        if state.disconnected() || state.connection.closed() {
            return false;
        }
        let frame = match encode_server_message(
            &message,
            Some(FrameDecoderOptions::with_max_frame_length(
                self.max_frame_length,
            )),
        ) {
            Ok(frame) => frame,
            Err(error) => {
                self.report_error(ServerError::ProtocolValidation(error.message().to_owned()));
                self.close_byte_connection(&state.connection, None).await;
                self.disconnect(state).await;
                return false;
            }
        };
        match state.connection.send(frame).await {
            Ok(()) => true,
            Err(error) => {
                self.report_error(error);
                self.close_byte_connection(&state.connection, None).await;
                self.disconnect(state).await;
                false
            }
        }
    }

    fn spawn_fail_protocol(
        self: &Arc<Self>,
        state: &Arc<ConnectionState>,
        code: ProtocolErrorCode,
        message: &str,
    ) {
        let server = Arc::clone(self);
        let state = Arc::clone(state);
        let error = ProtocolError {
            code,
            message: message.to_owned(),
            details: None,
        };
        tokio::spawn(async move {
            server.fail_protocol(&state, error).await;
        });
    }

    async fn fail_protocol(self: &Arc<Self>, state: &Arc<ConnectionState>, error: ProtocolError) {
        {
            let mut data = state.lock();
            if data.disconnected
                || data.stage == ConnectionStage::Closing
                || data.stage == ConnectionStage::Closed
            {
                return;
            }
            data.stage = ConnectionStage::Closing;
        }
        state.clear_handshake_timeout();
        let message = ServerMessage::HelloError(ServerHelloError {
            kind: HelloErrorTag,
            error,
        });
        let final_frame = match encode_server_message(
            &message,
            Some(FrameDecoderOptions::with_max_frame_length(
                self.max_frame_length,
            )),
        ) {
            Ok(frame) => Some(frame),
            Err(encode_error) => {
                self.report_error(ServerError::ProtocolValidation(
                    encode_error.message().to_owned(),
                ));
                None
            }
        };
        self.close_byte_connection(&state.connection, final_frame)
            .await;
        self.disconnect(state).await;
    }

    async fn close_server_state(self: &Arc<Self>) {
        let connections: Vec<Arc<ConnectionState>> =
            self.connections.lock().expect("connections mutex").clone();
        for connection in &connections {
            connection.set_stage(ConnectionStage::Closing);
            connection.clear_handshake_timeout();
        }
        for connection in &connections {
            self.close_byte_connection(&connection.connection, None)
                .await;
        }
        for connection in &connections {
            self.disconnect(connection).await;
        }
        self.sessions.close().await;
        self.connections.lock().expect("connections mutex").clear();
    }

    pub(crate) async fn close_byte_connection(
        self: &Arc<Self>,
        connection: &Arc<dyn ByteConnection>,
        final_chunk: Option<Vec<u8>>,
    ) {
        if let Err(error) = connection.close(final_chunk).await {
            self.report_error(error);
        }
    }

    async fn server_snapshot(self: &Arc<Self>) -> Result<ServerSnapshot, ServerError> {
        let sessions = self.sessions.list_metadata().await?;
        let models = self.service.list_models().await?;
        Ok(self.snapshots.build(sessions, models))
    }

    pub(crate) fn broadcast_server_snapshot(self: &Arc<Self>) {
        let server = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = server.perform_broadcast().await {
                server.report_error(error);
            }
        });
    }

    async fn perform_broadcast(self: &Arc<Self>) -> Result<(), ServerError> {
        let _guard = self.snapshots.broadcast_lock().await;
        let ready = ServerSnapshotPublisher::ready_connections(
            self.connections.lock().expect("connections mutex").clone(),
        );
        if ready.is_empty() || self.is_closing() {
            return Ok(());
        }
        let revision = self.snapshots.next_revision();
        let models = self.service.list_models().await?;
        let sessions = self.sessions.list_metadata().await?;
        let snapshot = ServerSnapshot {
            revision,
            ..self.snapshots.build(sessions, models)
        };
        let envelope = ServerSnapshotPublisher::envelope(snapshot);
        for connection in ready {
            self.send_event(&connection, envelope.clone()).await;
        }
        Ok(())
    }

    fn to_protocol_error(&self, error: &ServerError) -> ProtocolError {
        match error {
            ServerError::Internal { cause } => {
                self.report_error(ServerError::other(cause.clone()));
                ProtocolError {
                    code: ProtocolErrorCode::InternalError,
                    message: INTERNAL_SERVER_ERROR_MESSAGE.to_owned(),
                    details: None,
                }
            }
            ServerError::Server(server_error) => {
                if server_error.code == crate::errors::PiServerOperationErrorCode::NotImplemented {
                    return ProtocolError {
                        code: ProtocolErrorCode::NotImplemented,
                        message: NOT_IMPLEMENTED_MESSAGE.to_owned(),
                        details: None,
                    };
                }
                ProtocolError {
                    code: server_error.code.to_protocol(),
                    message: server_error.message.clone(),
                    details: server_error.details.clone(),
                }
            }
            ServerError::ProtocolValidation(message) => ProtocolError {
                code: ProtocolErrorCode::InvalidRequest,
                message: message.clone(),
                details: None,
            },
            ServerError::Other(_) => {
                self.report_error(error.clone());
                ProtocolError {
                    code: ProtocolErrorCode::InternalError,
                    message: INTERNAL_SERVER_ERROR_MESSAGE.to_owned(),
                    details: None,
                }
            }
        }
    }

    pub(crate) fn report_error(&self, error: ServerError) {
        let Some(observer) = &self.on_error else {
            return;
        };
        // Error observers cannot affect server state.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| observer(error)));
    }
}

struct ServerConnectionHandler {
    server: Arc<PiServerInner>,
    state: Arc<ConnectionState>,
}

impl ByteConnectionHandler for ServerConnectionHandler {
    fn on_data(&self, chunk: &[u8]) {
        self.server.receive(&self.state, chunk);
    }

    fn on_close(&self) {
        self.server.transport_closed(&self.state);
    }

    fn on_error(&self, error: ServerError) {
        self.server.report_error(error);
        let server = Arc::clone(&self.server);
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            server.close_byte_connection(&state.connection, None).await;
            server.disconnect(&state).await;
        });
    }
}

struct ClosingHandler {
    server: Arc<PiServerInner>,
}

impl ByteConnectionHandler for ClosingHandler {
    fn on_data(&self, _chunk: &[u8]) {}
    fn on_close(&self) {}
    fn on_error(&self, error: ServerError) {
        self.server.report_error(error);
    }
}

fn resolve_options(options: &PiServerOptions) -> Result<(u64, u64), ServerError> {
    if options.server_id.as_deref() == Some("") {
        return Err(ServerError::other("PiServer serverId must not be empty"));
    }
    let max_frame_length = options.max_frame_length.unwrap_or(DEFAULT_MAX_FRAME_LENGTH);
    if max_frame_length == 0 || max_frame_length > MAX_UINT32 {
        return Err(ServerError::other(format!(
            "PiServer maxFrameLength must be an integer between 1 and {MAX_UINT32}"
        )));
    }
    let handshake_timeout_ms = options
        .handshake_timeout_ms
        .unwrap_or(DEFAULT_HANDSHAKE_TIMEOUT_MS);
    if handshake_timeout_ms == 0 || handshake_timeout_ms > MAX_TIMER_DELAY_MS {
        return Err(ServerError::other(format!(
            "PiServer handshakeTimeoutMs must be an integer between 1 and {MAX_TIMER_DELAY_MS}"
        )));
    }
    Ok((max_frame_length, handshake_timeout_ms))
}
