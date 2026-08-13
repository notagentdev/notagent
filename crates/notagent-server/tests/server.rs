//! Port of `packages/server/test/server.test.ts`.

mod support;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use notagent_protocol::{ServerMessage, ServerMessageDecoder};
use notagent_server::connection::ByteConnection;
use notagent_server::errors::ServerError;
use notagent_server::testing::TestServerService;
use notagent_server::transports::unix::{UnixServerOptions, create_unix_server};
use notagent_server::{PiServer, PiServerOptions};
use support::*;

// The TS case "requires explicit listeners" has no Rust equivalent: the
// listener list is a required field of `PiServerOptions`.

#[test]
fn rejects_unix_socket_paths_that_cannot_fit_in_sockaddr_un() {
    let service = TestServerService::new();
    let error = create_unix_server(
        service.as_service(),
        UnixServerOptions::new(format!("/tmp/{}", "x".repeat(512))),
    )
    .err()
    .expect("rejects");
    assert!(error.to_string().contains("too long"), "{error}");
}

#[tokio::test]
async fn rejects_an_overlong_derived_private_unix_bind_path() {
    let max_length = if cfg!(target_os = "linux") { 107 } else { 103 };
    let path = format!("/tmp/{}/s", "x".repeat(max_length - "/tmp//s".len()));
    let service = TestServerService::new();
    let server =
        create_unix_server(service.as_service(), UnixServerOptions::new(path)).expect("server");
    let error = server.start().await.expect_err("fails");
    assert!(
        error.to_string().contains("private Unix bind path"),
        "{error}"
    );
    assert!(error.to_string().contains("too long"), "{error}");
}

#[tokio::test]
async fn rejects_concurrent_start_calls_without_leaking_the_unix_listener() {
    let directory = temp_directory();
    let path = socket_path(&directory);
    let service = TestServerService::new();
    let server = Arc::new(
        create_unix_server(service.as_service(), UnixServerOptions::new(path.clone()))
            .expect("server"),
    );
    let starting_server = Arc::clone(&server);
    let starting = tokio::spawn(async move { starting_server.start().await });
    tokio::task::yield_now().await;
    let error = server.start().await.expect_err("rejects the second start");
    assert!(error.to_string().contains("starting"), "{error}");
    starting.await.expect("task").expect("starts");
    server.close().await.expect("closes");
    assert!(server.addresses().is_empty());
    assert!(!std::path::Path::new(&path).exists());
}

#[tokio::test]
async fn handshake_timeout_cleanup_does_not_wait_for_a_blocked_output_queue() {
    struct BlockedConnection {
        closed: AtomicBool,
        final_chunk: Mutex<Option<Vec<u8>>>,
    }

    #[async_trait]
    impl ByteConnection for BlockedConnection {
        fn closed(&self) -> bool {
            self.closed.load(Ordering::SeqCst)
        }

        async fn send(&self, _chunk: Vec<u8>) -> Result<(), ServerError> {
            // Never resolves, like the TS `new Promise(() => {})`.
            std::future::pending::<()>().await;
            Ok(())
        }

        async fn close(&self, final_chunk: Option<Vec<u8>>) -> Result<(), ServerError> {
            *self.final_chunk.lock().expect("connection mutex") = final_chunk;
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    let service = TestServerService::new();
    let core = PiServer::new(
        service.as_service(),
        PiServerOptions {
            listeners: vec![],
            max_frame_length: Some(1024),
            handshake_timeout_ms: Some(10),
            server_id: None,
            on_error: None,
        },
    )
    .expect("server");
    let connection = Arc::new(BlockedConnection {
        closed: AtomicBool::new(false),
        final_chunk: Mutex::new(None),
    });
    let _handler = core.accept(Arc::clone(&connection) as Arc<dyn ByteConnection>);

    for _ in 0..200 {
        if connection.closed() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        connection.closed(),
        "connection was not closed by the handshake timeout"
    );
    let final_chunk = connection
        .final_chunk
        .lock()
        .expect("connection mutex")
        .clone()
        .expect("final chunk");
    let messages = ServerMessageDecoder::new(None)
        .expect("decoder")
        .push(&final_chunk)
        .expect("decodes");
    assert_eq!(messages.len(), 1);
    let ServerMessage::HelloError(error) = &messages[0] else {
        panic!("expected hello_error")
    };
    assert_eq!(
        error.error.code,
        notagent_protocol::ProtocolErrorCode::InvalidRequest
    );
    core.close().await.expect("closes");
}

#[test]
fn rejects_timeout_values_above_the_maximum_timer_delay() {
    let service = TestServerService::new();
    let path = "/tmp/notagent-server-timeout-test.sock";
    let error = create_unix_server(
        service.as_service(),
        UnixServerOptions {
            handshake_timeout_ms: Some(2_147_483_648),
            ..UnixServerOptions::new(path)
        },
    )
    .err()
    .expect("rejects");
    assert!(error.to_string().contains("handshakeTimeoutMs"), "{error}");
    let error = create_unix_server(
        service.as_service(),
        UnixServerOptions {
            graceful_close_timeout_ms: Some(2_147_483_648),
            ..UnixServerOptions::new(path)
        },
    )
    .err()
    .expect("rejects");
    assert!(
        error.to_string().contains("gracefulCloseTimeoutMs"),
        "{error}"
    );
}

#[test]
fn rejects_pending_byte_limits_smaller_than_one_maximum_frame() {
    let service = TestServerService::new();
    let error = create_unix_server(
        service.as_service(),
        UnixServerOptions {
            max_frame_length: Some(128),
            max_pending_bytes: Some(131),
            ..UnixServerOptions::new("/tmp/notagent-server-pending-test.sock")
        },
    )
    .err()
    .expect("rejects");
    assert!(error.to_string().contains("maxPendingBytes"), "{error}");
}
