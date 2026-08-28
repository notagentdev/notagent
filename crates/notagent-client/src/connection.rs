use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, Weak};

use notagent_protocol::{
    DEFAULT_MAX_FRAME_LENGTH, EventEnvelope, PROTOCOL_VERSION, ResponseEnvelope, ServerMessage,
    ServerMessageDecoder, ServerSnapshot, encode_client_message,
};

use crate::errors::{PiError, to_disconnected_error};
use crate::promise::{Resolver, SharedPromise, create_promise_resolvers};
use crate::state::panic_message;
use crate::transport::{ByteTransport, ByteTransportFactory, ByteTransportHandlers};
use crate::types::{ConnectionState, ConnectionStateChange};

const MAX_UINT32: u64 = 0xffff_ffff;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ServerNonHandshakeMessage {
    Response(ResponseEnvelope),
    Event(EventEnvelope),
}

pub(crate) trait ConnectionCallbacks: Send + Sync {
    fn on_handshake(&self, snapshot: ServerSnapshot);
    fn on_message(&self, message: ServerNonHandshakeMessage);
    fn on_state_change(&self, change: ConnectionStateChange);
}

enum Lifecycle {
    Disconnected,
    Connecting {
        id: u64,
        decoder: ServerMessageDecoder,
        transport: Option<Arc<dyn ByteTransport>>,
        handshake: Arc<Resolver<ServerSnapshot>>,
    },
    Connected {
        id: u64,
        decoder: ServerMessageDecoder,
        transport: Arc<dyn ByteTransport>,
        handshake: Option<Arc<Resolver<ServerSnapshot>>>,
    },
}

impl Lifecycle {
    fn state(&self) -> ConnectionState {
        match self {
            Self::Disconnected => ConnectionState::Disconnected,
            Self::Connecting { .. } => ConnectionState::Connecting,
            Self::Connected { .. } => ConnectionState::Connected,
        }
    }

    fn id(&self) -> Option<u64> {
        match self {
            Self::Disconnected => None,
            Self::Connecting { id, .. } | Self::Connected { id, .. } => Some(*id),
        }
    }

    fn transport(&self) -> Option<Arc<dyn ByteTransport>> {
        match self {
            Self::Disconnected => None,
            Self::Connecting { transport, .. } => transport.clone(),
            Self::Connected { transport, .. } => Some(Arc::clone(transport)),
        }
    }
}

struct Inner {
    lifecycle: Lifecycle,
    /// Replaces the object identity comparison `this.#lifecycle !== connected`
    epoch: u64,
    sequence: u64,
}

pub(crate) struct Connection {
    transport_factory: ByteTransportFactory,
    max_frame_length: u64,
    callbacks: Weak<dyn ConnectionCallbacks>,
    inner: Mutex<Inner>,
}

impl Connection {
    pub(crate) fn new(
        transport_factory: ByteTransportFactory,
        max_frame_length: Option<u64>,
        callbacks: Weak<dyn ConnectionCallbacks>,
    ) -> Result<Self, PiError> {
        let max_frame_length = max_frame_length.unwrap_or(DEFAULT_MAX_FRAME_LENGTH);
        if max_frame_length == 0 || max_frame_length > MAX_UINT32 {
            return Err(PiError::Other(format!(
                "PiClient maxFrameLength must be an integer between 1 and {MAX_UINT32}"
            )));
        }
        Ok(Self {
            transport_factory,
            max_frame_length,
            callbacks,
            inner: Mutex::new(Inner {
                lifecycle: Lifecycle::Disconnected,
                epoch: 0,
                sequence: 0,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("connection mutex")
    }

    fn set_lifecycle(inner: &mut Inner, lifecycle: Lifecycle) -> u64 {
        inner.lifecycle = lifecycle;
        inner.epoch += 1;
        inner.epoch
    }

    pub(crate) fn state(&self) -> ConnectionState {
        self.lock().lifecycle.state()
    }

    pub(crate) fn max_frame_length(&self) -> u64 {
        self.max_frame_length
    }

    pub(crate) fn connect(self: &Arc<Self>) -> SharedPromise<ServerSnapshot> {
        let (handshake, promise) = create_promise_resolvers::<ServerSnapshot>();
        let id = {
            let mut inner = self.lock();
            if !matches!(inner.lifecycle, Lifecycle::Disconnected) {
                let state = match inner.lifecycle.state() {
                    ConnectionState::Connecting => "connecting",
                    ConnectionState::Connected => "connected",
                    ConnectionState::Disconnected => "disconnected",
                };
                drop(inner);
                handshake.reject(PiError::Disconnected(format!(
                    "PiClient is already {state}"
                )));
                return promise;
            }
            inner.sequence += 1;
            let id = inner.sequence;
            let decoder = ServerMessageDecoder::new(Some(
                notagent_protocol::FrameDecoderOptions::with_max_frame_length(
                    self.max_frame_length,
                ),
            ))
            .expect("validated frame length");
            Self::set_lifecycle(
                &mut inner,
                Lifecycle::Connecting {
                    id,
                    decoder,
                    transport: None,
                    handshake: Arc::clone(&handshake),
                },
            );
            id
        };
        self.notify_state_change(ConnectionStateChange {
            state: ConnectionState::Connecting,
            error: None,
        });

        let handlers = {
            let data_connection = Arc::downgrade(self);
            let close_connection = Arc::downgrade(self);
            let error_connection = Arc::downgrade(self);
            ByteTransportHandlers::new(
                Arc::new(move |chunk: &[u8]| {
                    if let Some(connection) = data_connection.upgrade() {
                        connection.handle_data(id, chunk);
                    }
                }),
                Arc::new(move || {
                    if let Some(connection) = close_connection.upgrade()
                        && connection.is_current(id)
                    {
                        connection.handle_close();
                    }
                }),
                Arc::new(move |error: PiError| {
                    if let Some(connection) = error_connection.upgrade()
                        && connection.is_current(id)
                    {
                        connection.fail_and_close(to_disconnected_error(error));
                    }
                }),
            )
        };
        let connection = Arc::clone(self);
        tokio::spawn(async move {
            connection.open_transport(id, handlers).await;
        });
        promise
    }

    pub(crate) fn disconnect(&self, reason: PiError) {
        if matches!(self.lock().lifecycle, Lifecycle::Disconnected) {
            return;
        }
        self.fail_and_close(reason);
    }

    pub(crate) fn fail(&self, error: PiError) {
        self.fail_and_close(error);
    }

    pub(crate) fn send(self: &Arc<Self>, frame: Vec<u8>) -> Result<(), PiError> {
        let transport = {
            let inner = self.lock();
            match &inner.lifecycle {
                Lifecycle::Connected { transport, .. } => Arc::clone(transport),
                _ => return Err(PiError::disconnected()),
            }
        };
        let sending = transport.send(frame);
        let connection = Arc::clone(self);
        let sent_transport = Arc::clone(&transport);
        tokio::spawn(async move {
            if let Err(error) = sending.await {
                let current = connection.lock().lifecycle.transport();
                let matches_transport =
                    current.is_some_and(|current| Arc::ptr_eq(&current, &sent_transport));
                if matches_transport {
                    connection.fail_and_close(to_disconnected_error(error));
                }
            }
        });
        Ok(())
    }

    async fn open_transport(self: Arc<Self>, id: u64, handlers: ByteTransportHandlers) {
        let transport = match (self.transport_factory)(handlers).await {
            Ok(transport) => transport,
            Err(error) => {
                if self.is_current(id) {
                    self.fail(to_disconnected_error(error));
                }
                return;
            }
        };
        {
            let mut inner = self.lock();
            match &mut inner.lifecycle {
                Lifecycle::Connecting {
                    id: current,
                    transport: slot,
                    ..
                } if *current == id => {
                    *slot = Some(Arc::clone(&transport));
                    inner.epoch += 1;
                }
                _ => {
                    drop(inner);
                    transport.close();
                    return;
                }
            }
        }
        let hello = encode_client_message(
            &notagent_protocol::ClientMessage::Hello(notagent_protocol::ClientHello {
                kind: notagent_protocol::HelloTag,
                version: PROTOCOL_VERSION,
            }),
            Some(
                notagent_protocol::FrameDecoderOptions::with_max_frame_length(
                    self.max_frame_length,
                ),
            ),
        )
        .expect("client hello encodes");
        if let Err(error) = transport.send(hello).await
            && self.is_current(id)
        {
            self.fail_and_close(to_disconnected_error(error));
        }
    }

    fn handle_data(self: &Arc<Self>, id: u64, chunk: &[u8]) {
        let messages = {
            let mut inner = self.lock();
            match &mut inner.lifecycle {
                Lifecycle::Disconnected => return,
                Lifecycle::Connecting { id: current, .. }
                | Lifecycle::Connected { id: current, .. }
                    if *current != id =>
                {
                    return;
                }
                Lifecycle::Connecting {
                    transport: None, ..
                } => {
                    drop(inner);
                    self.fail_and_close(PiError::ProtocolValidation(
                        "Received server data before the client hello was sent".to_owned(),
                    ));
                    return;
                }
                Lifecycle::Connecting { decoder, .. } | Lifecycle::Connected { decoder, .. } => {
                    match decoder.push(chunk) {
                        Ok(messages) => messages,
                        Err(error) => {
                            drop(inner);
                            self.fail_and_close(PiError::ProtocolValidation(
                                error.message().to_owned(),
                            ));
                            return;
                        }
                    }
                }
            }
        };
        for message in messages {
            if matches!(self.lock().lifecycle, Lifecycle::Disconnected) {
                return;
            }
            self.handle_message(message);
        }
    }

    fn handle_message(self: &Arc<Self>, message: ServerMessage) {
        let inner = self.lock();
        match &inner.lifecycle {
            Lifecycle::Connecting { .. } => {
                drop(inner);
                self.handle_handshake_message(message);
            }
            Lifecycle::Connected { .. } => {
                drop(inner);
                match message {
                    ServerMessage::Hello(_) | ServerMessage::HelloError(_) => {
                        self.fail_and_close(PiError::ProtocolValidation(
                            "Unexpected handshake message".to_owned(),
                        ));
                    }
                    ServerMessage::Response(response) => {
                        self.with_callbacks(|callbacks| {
                            callbacks
                                .on_message(ServerNonHandshakeMessage::Response(response.clone()));
                        });
                    }
                    ServerMessage::Event(event) => {
                        self.with_callbacks(|callbacks| {
                            callbacks.on_message(ServerNonHandshakeMessage::Event(event.clone()));
                        });
                    }
                }
            }
            Lifecycle::Disconnected => (),
        }
    }

    fn handle_handshake_message(self: &Arc<Self>, message: ServerMessage) {
        let hello = match message {
            ServerMessage::HelloError(error) => {
                self.fail_and_close(PiError::server(error.error));
                return;
            }
            ServerMessage::Hello(hello) => hello,
            _ => {
                self.fail_and_close(PiError::ProtocolValidation(
                    "Expected server hello as first message".to_owned(),
                ));
                return;
            }
        };
        let (epoch, handshake, snapshot) = {
            let mut inner = self.lock();
            let Lifecycle::Connecting {
                id,
                decoder,
                transport,
                handshake,
            } = std::mem::replace(&mut inner.lifecycle, Lifecycle::Disconnected)
            else {
                return;
            };
            let Some(transport) = transport else {
                Self::set_lifecycle(
                    &mut inner,
                    Lifecycle::Connecting {
                        id,
                        decoder,
                        transport: None,
                        handshake,
                    },
                );
                drop(inner);
                self.fail_and_close(PiError::ProtocolValidation(
                    "Received server hello before the client hello was sent".to_owned(),
                ));
                return;
            };
            let epoch = Self::set_lifecycle(
                &mut inner,
                Lifecycle::Connected {
                    id,
                    decoder,
                    transport,
                    handshake: Some(Arc::clone(&handshake)),
                },
            );
            (epoch, handshake, hello.snapshot)
        };

        let handshake_snapshot = snapshot.clone();
        let failure = self.with_callbacks_catching(move |callbacks| {
            callbacks.on_handshake(handshake_snapshot.clone())
        });
        if let Some(message) = failure {
            if self.lock().epoch == epoch {
                self.fail_and_close(PiError::Other(message));
            }
            return;
        }
        if self.lock().epoch != epoch {
            return;
        }
        self.notify_state_change(ConnectionStateChange {
            state: ConnectionState::Connected,
            error: None,
        });
        if self.lock().epoch != epoch {
            return;
        }
        {
            let mut inner = self.lock();
            if let Lifecycle::Connected {
                id,
                decoder,
                transport,
                ..
            } = std::mem::replace(&mut inner.lifecycle, Lifecycle::Disconnected)
            {
                Self::set_lifecycle(
                    &mut inner,
                    Lifecycle::Connected {
                        id,
                        decoder,
                        transport,
                        handshake: None,
                    },
                );
            }
        }
        handshake.resolve(snapshot);
    }

    fn handle_close(self: &Arc<Self>) {
        let mut error = PiError::Disconnected("Byte transport closed".to_owned());
        {
            let mut inner = self.lock();
            match &mut inner.lifecycle {
                Lifecycle::Disconnected => return,
                Lifecycle::Connecting { decoder, .. } | Lifecycle::Connected { decoder, .. } => {
                    if let Err(decoder_error) = decoder.end() {
                        error = PiError::ProtocolValidation(decoder_error.message().to_owned());
                    }
                }
            }
        }
        self.do_fail(error);
    }

    fn fail_and_close(&self, error: PiError) {
        let transport = self.lock().lifecycle.transport();
        self.do_fail(error);
        if let Some(transport) = transport {
            transport.close();
        }
    }

    fn do_fail(&self, error: PiError) {
        let handshake = {
            let mut inner = self.lock();
            match std::mem::replace(&mut inner.lifecycle, Lifecycle::Disconnected) {
                Lifecycle::Disconnected => return,
                Lifecycle::Connecting { handshake, .. } => {
                    inner.epoch += 1;
                    Some(handshake)
                }
                Lifecycle::Connected { handshake, .. } => {
                    inner.epoch += 1;
                    handshake
                }
            }
        };
        if let Some(handshake) = handshake {
            handshake.reject(error.clone());
        }
        self.notify_state_change(ConnectionStateChange {
            state: ConnectionState::Disconnected,
            error: Some(error),
        });
    }

    fn notify_state_change(&self, change: ConnectionStateChange) {
        self.with_callbacks(|callbacks| callbacks.on_state_change(change.clone()));
    }

    fn with_callbacks(&self, action: impl Fn(&dyn ConnectionCallbacks)) {
        if let Some(callbacks) = self.callbacks.upgrade() {
            action(callbacks.as_ref());
        }
    }

    fn with_callbacks_catching(&self, action: impl Fn(&dyn ConnectionCallbacks)) -> Option<String> {
        let callbacks = self.callbacks.upgrade()?;
        catch_unwind(AssertUnwindSafe(|| action(callbacks.as_ref())))
            .err()
            .map(|payload| panic_message(&payload))
    }

    fn is_current(&self, id: u64) -> bool {
        self.lock().lifecycle.id() == Some(id)
    }
}
