mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use notagent_client::{
    BoxFuture, ByteTransport, ByteTransportHandlers, ConnectionState, ConnectionStateChange,
    PiClient, PiClientOptions, PiError,
};
use notagent_protocol::{
    CborValue, ClientMessage, HelloTag, PROTOCOL_VERSION, ProtocolVersionTag, ServerHello,
    ServerHelloError, ServerMessage, ServerSnapshot, encode_cbor, encode_frame,
    encode_server_message,
};
use support::*;

fn hello_message(connection_id: &str, snapshot: ServerSnapshot) -> ServerMessage {
    ServerMessage::Hello(ServerHello {
        kind: HelloTag,
        version: ProtocolVersionTag,
        connection_id: connection_id.to_owned(),
        snapshot,
    })
}

#[tokio::test]
async fn sends_a_framed_version_before_accepting_a_fragmented_server_hello() {
    let server = MemoryByteServer::new();
    let received: Arc<Mutex<Vec<ClientMessage>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let server = server.clone();
        let received = Arc::clone(&received);
        server
            .clone()
            .on_message(Arc::new(move |message: &ClientMessage| {
                received.lock().unwrap().push(message.clone());
                if matches!(message, ClientMessage::Hello(_)) {
                    server.send_split(&hello_message("connection-1", base_server_snapshot()), 3);
                }
            }));
    }
    let client = create_client(&server);

    assert_eq!(
        client.connect().await.expect("connects"),
        base_server_snapshot()
    );
    assert_eq!(
        received.lock().unwrap()[0],
        ClientMessage::Hello(notagent_protocol::ClientHello {
            kind: HelloTag,
            version: PROTOCOL_VERSION
        })
    );
    assert_eq!(client.connection_state(), ConnectionState::Connected);
}

#[tokio::test]
async fn rejects_server_data_delivered_before_sending_the_client_hello() {
    let send_count = Arc::new(AtomicUsize::new(0));
    let close_count = Arc::new(AtomicUsize::new(0));

    struct CountingTransport {
        send_count: Arc<AtomicUsize>,
        close_count: Arc<AtomicUsize>,
    }
    impl ByteTransport for CountingTransport {
        fn send(&self, _chunk: Vec<u8>) -> BoxFuture<Result<(), PiError>> {
            self.send_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
        fn close(&self) {
            self.close_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    let factory = {
        let send_count = Arc::clone(&send_count);
        let close_count = Arc::clone(&close_count);
        Arc::new(move |handlers: ByteTransportHandlers| {
            handlers.on_data(
                &encode_server_message(
                    &hello_message("connection-1", base_server_snapshot()),
                    None,
                )
                .expect("encodes"),
            );
            let transport: Arc<dyn ByteTransport> = Arc::new(CountingTransport {
                send_count: Arc::clone(&send_count),
                close_count: Arc::clone(&close_count),
            });
            let future: BoxFuture<Result<Arc<dyn ByteTransport>, PiError>> =
                Box::pin(async move { Ok(transport) });
            future
        })
    };
    let client = PiClient::new(PiClientOptions::new(factory)).expect("valid options");

    let error = client.connect().await.expect_err("rejects");
    assert_eq!(error.name(), "ProtocolValidationError");
    assert_eq!(
        error.message(),
        "Received server data before the client hello was sent"
    );
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    assert_eq!(send_count.load(Ordering::SeqCst), 0);
    assert_eq!(close_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn isolates_subscriber_failures_from_handshake_and_transport_state() {
    let server = MemoryByteServer::new();
    server.answer_hello("connection-1", base_server_snapshot());
    let client = create_client(&server);
    let _unsubscribe = client
        .subscribe(Arc::new(|_: &ServerSnapshot| panic!("consumer failure")))
        .expect("subscribes");

    assert_eq!(
        client.connect().await.expect("connects"),
        base_server_snapshot()
    );
    assert_eq!(client.connection_state(), ConnectionState::Connected);
}

#[tokio::test]
async fn reports_subscriber_failures_without_changing_connection_state() {
    let server = MemoryByteServer::new();
    let listener_errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    server.answer_hello("connection-1", base_server_snapshot());
    let mut options = PiClientOptions::new(server.factory());
    options.on_listener_error = Some({
        let listener_errors = Arc::clone(&listener_errors);
        Arc::new(move |error: PiError| listener_errors.lock().unwrap().push(error.message()))
    });
    let client = PiClient::new(options).expect("valid options");
    let _unsubscribe = client
        .subscribe(Arc::new(|_: &ServerSnapshot| panic!("consumer failure")))
        .expect("subscribes");

    assert_eq!(
        client.connect().await.expect("connects"),
        base_server_snapshot()
    );
    assert_eq!(
        *listener_errors.lock().unwrap(),
        vec!["consumer failure".to_owned()]
    );
    assert_eq!(client.connection_state(), ConnectionState::Connected);
}

#[tokio::test]
async fn does_not_restore_a_connection_after_a_snapshot_listener_disconnects_during_handshake() {
    let server = MemoryByteServer::new();
    server.answer_hello("connection-1", base_server_snapshot());
    let client = create_client(&server);
    {
        let listener_client = client.clone();
        let _unsubscribe = client
            .subscribe(Arc::new(move |_: &ServerSnapshot| {
                listener_client.disconnect("Client disconnected")
            }))
            .expect("subscribes");
    }

    let error = client.connect().await.expect_err("rejects");
    assert_eq!(error.name(), "PiDisconnectedError");
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    assert_eq!(server.client_close_count(), 1);
}

#[tokio::test]
async fn does_not_restore_a_stale_connection_when_a_snapshot_listener_reconnects_during_handshake()
{
    let first = MemoryByteServer::new();
    let second = MemoryByteServer::new();
    let connection = Arc::new(AtomicUsize::new(0));
    for (index, server) in [&first, &second].into_iter().enumerate() {
        server.answer_hello(
            &format!("connection-{index}"),
            ServerSnapshot {
                revision: index as u64,
                ..base_server_snapshot()
            },
        );
    }
    let factory = {
        let first = first.clone();
        let second = second.clone();
        let connection = Arc::clone(&connection);
        Arc::new(move |handlers: ByteTransportHandlers| {
            let transport = if connection.fetch_add(1, Ordering::SeqCst) == 0 {
                first.connect(handlers)
            } else {
                second.connect(handlers)
            };
            let future: BoxFuture<Result<Arc<dyn ByteTransport>, PiError>> =
                Box::pin(async move { Ok(transport) });
            future
        })
    };
    let client = PiClient::new(PiClientOptions::new(factory)).expect("valid options");
    type Reconnect = Arc<
        Mutex<
            Option<
                Box<
                    dyn std::future::Future<Output = Result<ServerSnapshot, PiError>>
                        + Send
                        + Unpin,
                >,
            >,
        >,
    >;
    let reconnect: Reconnect = Arc::new(Mutex::new(None));
    let requested = Arc::new(Mutex::new(false));
    {
        let listener_client = client.clone();
        let reconnect = Arc::clone(&reconnect);
        let requested = Arc::clone(&requested);
        let _unsubscribe = client
            .subscribe(Arc::new(move |_: &ServerSnapshot| {
                let mut requested = requested.lock().unwrap();
                if *requested {
                    return;
                }
                *requested = true;
                listener_client.disconnect("Client disconnected");
                *reconnect.lock().unwrap() = Some(Box::new(Box::pin(listener_client.reconnect())));
            }))
            .expect("subscribes");
    }

    let error = client.connect().await.expect_err("rejects");
    assert_eq!(error.name(), "PiDisconnectedError");
    let reconnect = reconnect.lock().unwrap().take().expect("reconnect started");
    let snapshot = reconnect.await.expect("reconnects");
    assert_eq!(snapshot.revision, 1);
    assert_eq!(client.connection_state(), ConnectionState::Connected);
    assert_eq!(first.client_close_count(), 1);
}

#[tokio::test]
async fn rejects_a_typed_handshake_version_error() {
    let server = MemoryByteServer::new();
    {
        let server = server.clone();
        server
            .clone()
            .on_message(Arc::new(move |_: &ClientMessage| {
                server.send(&ServerMessage::HelloError(ServerHelloError {
                    kind: notagent_protocol::HelloErrorTag,
                    error: notagent_protocol::ProtocolError {
                        code: notagent_protocol::ProtocolErrorCode::Version,
                        message: "Unsupported protocol version".to_owned(),
                        details: None,
                    },
                }));
            }));
    }
    let client = create_client(&server);

    let error = client.connect().await.expect_err("rejects");
    assert_eq!(error.name(), "PiServerError");
    assert_eq!(
        error.code(),
        Some(notagent_protocol::ProtocolErrorCode::Version)
    );
    assert_eq!(error.message(), "Unsupported protocol version");
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    assert_eq!(server.client_close_count(), 1);
}

#[tokio::test]
async fn rejects_pending_requests_on_close_and_reconnects_through_a_fresh_factory_result() {
    let first = MemoryByteServer::new();
    let second = MemoryByteServer::new();
    let connection = Arc::new(AtomicUsize::new(0));
    for (index, server) in [&first, &second].into_iter().enumerate() {
        server.answer_hello(
            &format!("connection-{index}"),
            ServerSnapshot {
                revision: index as u64,
                ..base_server_snapshot()
            },
        );
    }
    let factory = {
        let first = first.clone();
        let second = second.clone();
        let connection = Arc::clone(&connection);
        Arc::new(move |handlers: ByteTransportHandlers| {
            let transport = if connection.fetch_add(1, Ordering::SeqCst) == 0 {
                first.connect(handlers)
            } else {
                second.connect(handlers)
            };
            let future: BoxFuture<Result<Arc<dyn ByteTransport>, PiError>> =
                Box::pin(async move { Ok(transport) });
            future
        })
    };
    let client = PiClient::new(PiClientOptions::new(factory)).expect("valid options");
    let states: Arc<Mutex<Vec<ConnectionState>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let states = Arc::clone(&states);
        let _unsubscribe = client
            .on_connection_state_change(Arc::new(move |change: &ConnectionStateChange| {
                states.lock().unwrap().push(change.state);
            }))
            .expect("subscribes");
    }
    client.connect().await.expect("connects");
    let pending = client.list_sessions();
    first.close();
    let error = pending.await.expect_err("rejects");
    assert_eq!(error.name(), "PiDisconnectedError");
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);

    assert_eq!(client.reconnect().await.expect("reconnects").revision, 1);
    assert_eq!(client.connection_state(), ConnectionState::Connected);
    assert_eq!(
        *states.lock().unwrap(),
        vec![
            ConnectionState::Connecting,
            ConnectionState::Connected,
            ConnectionState::Disconnected,
            ConnectionState::Connecting,
            ConnectionState::Connected,
        ]
    );
}

#[tokio::test]
async fn supports_synchronous_reconnect_from_a_disconnection_listener() {
    let first = MemoryByteServer::new();
    let second = MemoryByteServer::new();
    let connection = Arc::new(AtomicUsize::new(0));
    for (index, server) in [&first, &second].into_iter().enumerate() {
        server.answer_hello(
            &format!("connection-{index}"),
            ServerSnapshot {
                revision: index as u64,
                ..base_server_snapshot()
            },
        );
    }
    let factory = {
        let first = first.clone();
        let second = second.clone();
        let connection = Arc::clone(&connection);
        Arc::new(move |handlers: ByteTransportHandlers| {
            let transport = if connection.fetch_add(1, Ordering::SeqCst) == 0 {
                first.connect(handlers)
            } else {
                second.connect(handlers)
            };
            let future: BoxFuture<Result<Arc<dyn ByteTransport>, PiError>> =
                Box::pin(async move { Ok(transport) });
            future
        })
    };
    let client = PiClient::new(PiClientOptions::new(factory)).expect("valid options");
    client.connect().await.expect("connects");
    type Reconnect = Arc<
        Mutex<
            Option<
                Box<
                    dyn std::future::Future<Output = Result<ServerSnapshot, PiError>>
                        + Send
                        + Unpin,
                >,
            >,
        >,
    >;
    let reconnect: Reconnect = Arc::new(Mutex::new(None));
    {
        let listener_client = client.clone();
        let reconnect = Arc::clone(&reconnect);
        let _unsubscribe = client
            .on_connection_state_change(Arc::new(move |change: &ConnectionStateChange| {
                if change.state == ConnectionState::Disconnected {
                    *reconnect.lock().unwrap() =
                        Some(Box::new(Box::pin(listener_client.reconnect())));
                }
            }))
            .expect("subscribes");
    }

    first.close();
    let reconnect = reconnect.lock().unwrap().take().expect("reconnect started");
    assert_eq!(reconnect.await.expect("reconnects").revision, 1);
    assert_eq!(client.connection_state(), ConnectionState::Connected);
}

#[tokio::test]
async fn rejects_pending_requests_on_transport_errors() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let pending = client.list_sessions();
    server.error(PiError::Other("read failed".to_owned()));

    let error = pending.await.expect_err("rejects");
    assert_eq!(error.name(), "PiDisconnectedError");
    assert_eq!(error.message(), "read failed");
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
}

#[tokio::test]
async fn enforces_the_configured_frame_limit_for_outbound_and_inbound_messages() {
    let server = MemoryByteServer::new();
    server.answer_hello("connection-1", base_server_snapshot());
    let mut options = PiClientOptions::new(server.factory());
    options.max_frame_length = Some(512);
    let client = PiClient::new(options).expect("valid options");
    client.connect().await.expect("connects");
    let handle = attach_session(&client, &server, session_snapshot("session-1")).await;
    let sent_before = server.sent_by_client();
    let error = handle
        .prompt(&"x".repeat(1_000))
        .await
        .expect_err("rejects");
    assert_eq!(error.name(), "ProtocolValidationError");
    assert_eq!(server.sent_by_client(), sent_before);

    server.send_raw(&[0, 0, 2, 1]);
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
}

#[tokio::test]
async fn disconnects_on_invalid_protocol_data() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let payload = encode_cbor(
        &CborValue::map([
            ("type", CborValue::text("event")),
            (
                "event",
                CborValue::map([
                    ("type", CborValue::text("session_removed")),
                    ("sessionId", CborValue::Number(1.0)),
                ]),
            ),
        ]),
        None,
    )
    .expect("encodes");
    server.send_raw(&encode_frame(&payload).expect("frames"));
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
}

#[tokio::test]
async fn reports_truncated_framing_when_the_transport_closes() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let pending = client.list_sessions();
    server.send_raw(&[0, 0, 0, 2, 1]);
    server.close();

    let error = pending.await.expect_err("rejects");
    assert_eq!(error.name(), "ProtocolValidationError");
    assert!(
        error.message().to_lowercase().contains("truncated"),
        "{}",
        error.message()
    );
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
}

#[test]
fn rejects_frame_limits_outside_the_unsigned_32_bit_range() {
    let server = MemoryByteServer::new();
    let mut options = PiClientOptions::new(server.factory());
    options.max_frame_length = Some(0x1_0000_0000);
    let error = match PiClient::new(options) {
        Ok(_) => panic!("expected an invalid frame limit to be rejected"),
        Err(error) => error,
    };
    assert!(
        error.message().contains("maxFrameLength"),
        "{}",
        error.message()
    );
}
