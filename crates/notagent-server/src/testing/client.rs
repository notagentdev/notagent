use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use notagent_protocol::{
    ClientHello, ClientMessage, Command, HelloTag, PROTOCOL_VERSION, RequestEnvelope, RequestTag,
    ResponseEnvelope, ServerMessage, ServerMessageDecoder, encode_client_message,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;

use crate::errors::ServerError;

use super::service::Deferred;

#[async_trait]
pub trait WireChannel: Send + Sync {
    async fn send(&self, chunk: Vec<u8>) -> Result<(), ServerError>;
    async fn send_fragmented(&self, chunk: Vec<u8>, split_at: usize) -> Result<(), ServerError>;
    async fn close(&self) -> Result<(), ServerError>;
}

type Predicate = Arc<dyn Fn(&ServerMessage) -> bool + Send + Sync>;

struct MessageWaiter {
    predicate: Predicate,
    resolver: Arc<Deferred<Option<ServerMessage>>>,
}

pub struct ProtocolTestClient {
    channel: Arc<dyn WireChannel>,
    messages: Mutex<Vec<ServerMessage>>,
    decoder: Mutex<ServerMessageDecoder>,
    waiters: Mutex<Vec<MessageWaiter>>,
    closed_deferred: Deferred<()>,
    request_sequence: AtomicU64,
    closed: AtomicBool,
}

impl ProtocolTestClient {
    pub fn new(channel: Arc<dyn WireChannel>) -> Self {
        Self {
            channel,
            messages: Mutex::new(Vec::new()),
            decoder: Mutex::new(ServerMessageDecoder::new(None).expect("decoder")),
            waiters: Mutex::new(Vec::new()),
            closed_deferred: Deferred::new(),
            request_sequence: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        }
    }

    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn messages(&self) -> Vec<ServerMessage> {
        self.messages.lock().expect("client mutex").clone()
    }

    pub async fn hello(&self) -> ServerMessage {
        self.hello_with_version(PROTOCOL_VERSION).await
    }

    pub async fn hello_with_version(&self, version: u64) -> ServerMessage {
        let response = self.next(Arc::new(|message: &ServerMessage| {
            matches!(
                message,
                ServerMessage::Hello(_) | ServerMessage::HelloError(_)
            )
        }));
        self.send_message(ClientMessage::Hello(ClientHello {
            kind: HelloTag,
            version,
        }))
        .await
        .expect("sends hello");
        response.await.expect("handshake response")
    }

    pub async fn request(&self, command: Command) -> ResponseEnvelope {
        let id = format!(
            "request-{}",
            self.request_sequence.fetch_add(1, Ordering::SeqCst) + 1
        );
        self.request_with_id(command, &id).await
    }

    pub async fn request_with_id(&self, command: Command, id: &str) -> ResponseEnvelope {
        let expected = id.to_owned();
        let response = self.next(Arc::new(move |message: &ServerMessage| match message {
            ServerMessage::Response(ResponseEnvelope::Ok(ok)) => ok.id == expected,
            ServerMessage::Response(ResponseEnvelope::Error(error)) => error.id == expected,
            _ => false,
        }));
        self.send_message(ClientMessage::Request(RequestEnvelope {
            kind: RequestTag,
            id: id.to_owned(),
            request: command,
        }))
        .await
        .expect("sends request");
        match response.await.expect("response") {
            ServerMessage::Response(response) => response,
            other => panic!("expected a response, got {other:?}"),
        }
    }

    pub async fn send_message(&self, message: ClientMessage) -> Result<(), ServerError> {
        let frame = encode_client_message(&message, None)
            .map_err(|error| ServerError::ProtocolValidation(error.message().to_owned()))?;
        self.channel.send(frame).await
    }

    pub async fn send_bytes(&self, chunk: Vec<u8>) -> Result<(), ServerError> {
        self.channel.send(chunk).await
    }

    pub async fn send_fragmented_message(
        &self,
        message: ClientMessage,
        split_at: usize,
    ) -> Result<(), ServerError> {
        let frame = encode_client_message(&message, None)
            .map_err(|error| ServerError::ProtocolValidation(error.message().to_owned()))?;
        self.channel.send_fragmented(frame, split_at).await
    }

    /// Resolves with the first matching message; `None` once the wire closed first.
    pub fn next(
        &self,
        predicate: Predicate,
    ) -> futures::future::BoxFuture<'static, Option<ServerMessage>> {
        self.next_from(0, predicate)
    }

    pub fn next_from(
        &self,
        index: usize,
        predicate: Predicate,
    ) -> futures::future::BoxFuture<'static, Option<ServerMessage>> {
        {
            let messages = self.messages.lock().expect("client mutex");
            if let Some(existing) = messages
                .iter()
                .skip(index)
                .find(|message| predicate(message))
            {
                let existing = existing.clone();
                return Box::pin(async move { Some(existing) });
            }
        }
        if self.closed() {
            return Box::pin(async { None });
        }
        let resolver = Arc::new(Deferred::<Option<ServerMessage>>::new());
        self.waiters
            .lock()
            .expect("client mutex")
            .push(MessageWaiter {
                predicate,
                resolver: Arc::clone(&resolver),
            });
        Box::pin(async move { resolver.promise().await })
    }

    pub async fn wait_for_close(&self) {
        if self.closed() {
            return;
        }
        self.closed_deferred.promise().await;
    }

    pub async fn close(&self) -> Result<(), ServerError> {
        self.channel.close().await
    }

    pub fn receive(&self, chunk: &[u8]) {
        let messages = { self.decoder.lock().expect("client mutex").push(chunk) };
        let messages = match messages {
            Ok(messages) => messages,
            Err(_) => {
                self.fail();
                return;
            }
        };
        for message in messages {
            self.messages
                .lock()
                .expect("client mutex")
                .push(message.clone());
            let resolved: Vec<Arc<Deferred<Option<ServerMessage>>>> = {
                let mut waiters = self.waiters.lock().expect("client mutex");
                let mut resolved = Vec::new();
                waiters.retain(|waiter| {
                    if (waiter.predicate)(&message) {
                        resolved.push(Arc::clone(&waiter.resolver));
                        false
                    } else {
                        true
                    }
                });
                resolved
            };
            for resolver in resolved {
                resolver.resolve(Some(message.clone()));
            }
        }
    }

    pub fn mark_closed(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.closed_deferred.resolve(());
        self.fail();
    }

    fn fail(&self) {
        let waiters: Vec<MessageWaiter> =
            std::mem::take(&mut self.waiters.lock().expect("client mutex"));
        for waiter in waiters {
            waiter.resolver.resolve(None);
        }
    }
}

struct UnixWireChannel {
    writer: tokio::sync::Mutex<Option<OwnedWriteHalf>>,
}

#[async_trait]
impl WireChannel for UnixWireChannel {
    async fn send(&self, chunk: Vec<u8>) -> Result<(), ServerError> {
        let mut writer = self.writer.lock().await;
        let writer = writer
            .as_mut()
            .ok_or_else(|| ServerError::other("Wire channel is closed"))?;
        writer
            .write_all(&chunk)
            .await
            .map_err(|error| ServerError::other(error.to_string()))
    }

    async fn send_fragmented(&self, chunk: Vec<u8>, split_at: usize) -> Result<(), ServerError> {
        self.send(chunk[..split_at].to_vec()).await?;
        self.send(chunk[split_at..].to_vec()).await
    }

    async fn close(&self) -> Result<(), ServerError> {
        let mut writer = self.writer.lock().await;
        if let Some(writer) = writer.as_mut() {
            let _ = writer.shutdown().await;
        }
        *writer = None;
        Ok(())
    }
}

pub async fn connect_unix_test_client(path: &str) -> Result<Arc<ProtocolTestClient>, ServerError> {
    let stream = UnixStream::connect(path)
        .await
        .map_err(|error| ServerError::other(error.to_string()))?;
    let (mut reader, writer) = stream.into_split();
    let client = Arc::new(ProtocolTestClient::new(Arc::new(UnixWireChannel {
        writer: tokio::sync::Mutex::new(Some(writer)),
    })));
    let read_client = Arc::clone(&client);
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => {
                    read_client.mark_closed();
                    return;
                }
                Ok(read) => read_client.receive(&buffer[..read]),
            }
        }
    });
    Ok(client)
}
