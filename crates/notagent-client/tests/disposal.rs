//! Port von `packages/client/test/disposal.test.ts`.

mod support;

use notagent_client::{ConnectionState, PiClient, PiClientOptions};
use support::*;

#[tokio::test]
async fn connects_through_its_ownership_factory() {
    let server = MemoryByteServer::new();
    server.answer_hello("connection-1", base_server_snapshot());

    let client = PiClient::connect_with(PiClientOptions::new(server.factory()))
        .await
        .expect("connects");

    assert!(client.connected());
    client.dispose().await.expect("disposes");
}

#[tokio::test]
async fn disconnects_invalidates_child_handles_and_rejects_pending_requests() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let handle = attach_session(&client, &server, session_snapshot("session-1")).await;
    let pending = client.list_sessions();

    let first_disposal = client.dispose();
    let second_disposal = client.dispose();

    assert!(client.disposed());
    assert!(!client.connected());
    assert!(!handle.attached());
    let error = pending.await.expect_err("rejects");
    assert_eq!(error.name(), "PiClientDisposedError");
    let error = handle.prompt("after disposal").await.expect_err("rejects");
    assert_eq!(error.name(), "PiClientDisposedError");
    first_disposal.await.expect("disposes");
    // TS prüft Promise-Identität (`toBe`); in Rust teilen sich beide dasselbe
    // `Shared`-Versprechen und liefern dasselbe Ergebnis.
    second_disposal.await.expect("disposes");
}

#[tokio::test]
async fn supports_explicit_async_disposal() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;

    client.dispose().await.expect("disposes");

    assert!(client.disposed());
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
}
