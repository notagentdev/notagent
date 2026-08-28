mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notagent_client::{
    ByteTransportHandlers, ConnectionState, PiClient, PiClientOptions, PiError,
    UnixTransportOptions, create_unix_transport_factory,
};
use notagent_protocol::{
    ClientMessage, ClientMessageDecoder, CommandResult, HelloTag, ListResult, ListTag,
    ProtocolVersionTag, ServerHello, ServerMessage, ServerSnapshot, encode_server_message,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::Notify;

fn server_snapshot() -> ServerSnapshot {
    ServerSnapshot {
        server_id: "unix-server".to_owned(),
        protocol_version: ProtocolVersionTag,
        revision: 4,
        sessions: vec![],
        models: vec![],
    }
}

fn hello_frame() -> Vec<u8> {
    encode_server_message(
        &ServerMessage::Hello(ServerHello {
            kind: HelloTag,
            version: ProtocolVersionTag,
            connection_id: "unix-connection".to_owned(),
            snapshot: server_snapshot(),
        }),
        None,
    )
    .expect("encodes")
}

#[test]
fn rejects_invalid_unix_transport_options() {
    let error = create_unix_transport_factory(UnixTransportOptions::new(""))
        .err()
        .expect("rejects");
    assert!(
        error.message().contains("must not be empty"),
        "{}",
        error.message()
    );
    let error = create_unix_transport_factory(UnixTransportOptions::new(format!(
        "/tmp/{}",
        "x".repeat(512)
    )))
    .err()
    .expect("rejects");
    assert!(error.message().contains("too long"), "{}", error.message());
    let error = create_unix_transport_factory(UnixTransportOptions {
        path: "/tmp/notagent.sock".to_owned(),
        max_pending_bytes: Some(0),
    })
    .err()
    .expect("rejects");
    assert!(error.message().contains("positive"), "{}", error.message());
}

#[tokio::test]
async fn pi_client_exchanges_fragmented_framed_messages_over_a_real_unix_socket() {
    let directory = tempfile::tempdir().expect("temp dir");
    let socket_path = directory.path().join("notagent.sock");
    let listener = UnixListener::bind(&socket_path).expect("binds");
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accepts");
        let (mut reader, mut writer) = stream.into_split();
        let mut decoder = ClientMessageDecoder::new(None).expect("decoder");
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer).await.expect("reads");
            if read == 0 {
                return;
            }
            for message in decoder.push(&buffer[..read]).expect("decodes") {
                match message {
                    ClientMessage::Hello(_) => {
                        for byte in hello_frame() {
                            writer.write_all(&[byte]).await.expect("writes");
                        }
                    }
                    ClientMessage::Request(request) => {
                        let response = encode_server_message(
                            &ServerMessage::Response(notagent_protocol::ResponseEnvelope::Ok(
                                notagent_protocol::OkResponseEnvelope {
                                    kind: notagent_protocol::ResponseTag,
                                    id: request.id.clone(),
                                    ok: notagent_protocol::TrueTag,
                                    result: CommandResult::List(ListResult {
                                        command: ListTag,
                                        sessions: vec![],
                                    }),
                                },
                            )),
                            None,
                        )
                        .expect("encodes");
                        let split = response.len() / 2;
                        writer.write_all(&response[..split]).await.expect("writes");
                        writer.write_all(&response[split..]).await.expect("writes");
                    }
                }
            }
        }
    });

    let factory = create_unix_transport_factory(UnixTransportOptions::new(
        socket_path.to_string_lossy().into_owned(),
    ))
    .expect("factory");
    let client = PiClient::new(PiClientOptions::new(factory)).expect("valid options");
    assert_eq!(client.connect().await.expect("connects"), server_snapshot());
    let (first, second) = tokio::join!(client.list_sessions(), client.list_sessions());
    assert_eq!(first.expect("lists"), vec![]);
    assert_eq!(second.expect("lists"), vec![]);
    client.disconnect("Client disconnected");
}

#[tokio::test]
async fn bounds_pending_writes_preserves_order_and_reports_remote_end_once() {
    let directory = tempfile::tempdir().expect("temp dir");
    let socket_path = directory.path().join("notagent.sock");
    let listener = UnixListener::bind(&socket_path).expect("binds");
    let first = vec![1u8; 2 * 1024 * 1024];
    let second = vec![2u8; 2 * 1024 * 1024];
    let expected_length = first.len() + second.len();
    let resume = Arc::new(Notify::new());
    let server_ready = Arc::new(Notify::new());
    let received = Arc::new(Notify::new());
    let invalid_order = Arc::new(AtomicUsize::new(0));

    {
        let resume = Arc::clone(&resume);
        let server_ready = Arc::clone(&server_ready);
        let received = Arc::clone(&received);
        let invalid_order = Arc::clone(&invalid_order);
        let first_length = first.len();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accepts");
            let (mut reader, mut writer) = stream.into_split();
            server_ready.notify_one();
            resume.notified().await;
            let mut buffer = vec![0u8; 64 * 1024];
            let mut received_length = 0usize;
            while received_length < expected_length {
                let read = reader.read(&mut buffer).await.expect("reads");
                if read == 0 {
                    break;
                }
                for (index, byte) in buffer[..read].iter().enumerate() {
                    let expected = if received_length + index < first_length {
                        1
                    } else {
                        2
                    };
                    if *byte != expected {
                        invalid_order.fetch_add(1, Ordering::SeqCst);
                    }
                }
                received_length += read;
            }
            writer.write_all(&[9]).await.expect("writes");
            writer.shutdown().await.expect("shuts down");
            received.notify_one();
        });
    }

    let inbound = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let errors = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let close_count = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(Notify::new());
    let handlers = ByteTransportHandlers::new(
        {
            let inbound = Arc::clone(&inbound);
            Arc::new(move |chunk: &[u8]| inbound.lock().unwrap().extend_from_slice(chunk))
        },
        {
            let close_count = Arc::clone(&close_count);
            let closed = Arc::clone(&closed);
            Arc::new(move || {
                close_count.fetch_add(1, Ordering::SeqCst);
                closed.notify_one();
            })
        },
        {
            let errors = Arc::clone(&errors);
            Arc::new(move |error: PiError| errors.lock().unwrap().push(error.message()))
        },
    );
    let factory = create_unix_transport_factory(UnixTransportOptions {
        path: socket_path.to_string_lossy().into_owned(),
        max_pending_bytes: Some(expected_length as u64),
    })
    .expect("factory");
    let transport = factory(handlers).await.expect("connects");

    server_ready.notified().await;
    let first_write = transport.send(first);
    let second_write = transport.send(second);
    let error = transport.send(vec![3]).await.expect_err("rejects");
    assert!(
        error.message().contains("pending byte limit"),
        "{}",
        error.message()
    );
    resume.notify_one();
    let (first_result, second_result) = tokio::join!(first_write, second_write);
    first_result.expect("writes");
    second_result.expect("writes");
    received.notified().await;
    closed.notified().await;

    assert_eq!(invalid_order.load(Ordering::SeqCst), 0);
    assert_eq!(*inbound.lock().unwrap(), vec![9]);
    assert!(
        errors.lock().unwrap().is_empty(),
        "{:?}",
        errors.lock().unwrap()
    );
    assert_eq!(close_count.load(Ordering::SeqCst), 1);
    transport.close();
}

#[tokio::test]
async fn pi_client_rejects_a_truncated_final_frame_from_a_real_unix_socket() {
    let directory = tempfile::tempdir().expect("temp dir");
    let socket_path = directory.path().join("notagent.sock");
    let listener = UnixListener::bind(&socket_path).expect("binds");
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accepts");
        let (mut reader, mut writer) = stream.into_split();
        let mut decoder = ClientMessageDecoder::new(None).expect("decoder");
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer).await.expect("reads");
            if read == 0 {
                return;
            }
            for message in decoder.push(&buffer[..read]).expect("decodes") {
                match message {
                    ClientMessage::Hello(_) => {
                        writer.write_all(&hello_frame()).await.expect("writes")
                    }
                    ClientMessage::Request(_) => {
                        writer.write_all(&[0, 0, 0, 2, 1]).await.expect("writes");
                        writer.shutdown().await.expect("shuts down");
                        return;
                    }
                }
            }
        }
    });

    let factory = create_unix_transport_factory(UnixTransportOptions::new(
        socket_path.to_string_lossy().into_owned(),
    ))
    .expect("factory");
    let client = PiClient::new(PiClientOptions::new(factory)).expect("valid options");
    client.connect().await.expect("connects");
    let error = client.list_sessions().await.expect_err("rejects");
    assert_eq!(error.name(), "ProtocolValidationError");
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    client.disconnect("Client disconnected");
}

#[tokio::test]
async fn rejects_connection_errors() {
    let directory = tempfile::tempdir().expect("temp dir");
    let missing_path = directory.path().join("missing.sock");
    let factory = create_unix_transport_factory(UnixTransportOptions::new(
        missing_path.to_string_lossy().into_owned(),
    ))
    .expect("factory");
    let handlers = ByteTransportHandlers::new(
        Arc::new(|_: &[u8]| {}),
        Arc::new(|| {}),
        Arc::new(|_: PiError| {}),
    );
    let error = factory(handlers).await.err().expect("rejects");
    assert!(
        error.message().to_lowercase().contains("no such file"),
        "{}",
        error.message()
    );
}
