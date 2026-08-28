mod client_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use client_support::{
    MemoryServer, attach_result, collect_requests, command_name, connect_client, create_result,
    detach_result, error_response, ok_response, request_session_id, session_snapshot,
};
use notagent::client::{
    CreateRemoteSessionOptions, RemoteSession, RemoteSessionError, RemoteSessionLifecycle,
    RemoteSessionOptions,
};
use notagent_client::PiError;
use notagent_protocol::ClientMessage;

/// `respondToAttach`.
fn respond_to_attach(server: &MemoryServer, session_id: &str) {
    let answering = server.clone();
    let session_id = session_id.to_owned();
    server.on_message(Arc::new(move |message: &ClientMessage| {
        let ClientMessage::Request(request) = message else {
            return;
        };
        if command_name(request) != "attach" {
            return;
        }
        answering.send(&ok_response(
            &request.id,
            attach_result(session_snapshot(&session_id)),
        ));
    }));
}

fn respond_to_detach(server: &MemoryServer) {
    let answering = server.clone();
    server.on_message(Arc::new(move |message: &ClientMessage| {
        let ClientMessage::Request(request) = message else {
            return;
        };
        if command_name(request) != "detach" {
            return;
        }
        let session_id = request_session_id(request).unwrap_or_default();
        answering.send(&ok_response(&request.id, detach_result(&session_id)));
    }));
}

#[tokio::test]
async fn factory_opens_a_session_and_disposal_awaits_detach_without_disconnecting_the_client() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    respond_to_attach(&server, "session-1");
    let remote_session =
        RemoteSession::open_session(client.clone(), "session-1", RemoteSessionOptions::default())
            .await
            .expect("opens");
    let requests = collect_requests(&server);

    let first_disposal = remote_session.dispose();
    // second call detaches nothing more and settles with the same result.
    let second_disposal = remote_session.dispose();

    tokio::task::yield_now().await;
    assert_eq!(
        requests.commands(),
        vec!["detach"],
        "disposal detaches exactly once"
    );
    let detach_request = requests.last().expect("Missing detach request");
    assert_eq!(
        request_session_id(&detach_request).as_deref(),
        Some("session-1")
    );
    server.send(&ok_response(&detach_request.id, detach_result("session-1")));

    first_disposal.await.expect("disposes");
    second_disposal.await.expect("disposes");
    assert!(remote_session.disposed());
    assert!(client.connected());
    assert_eq!(requests.commands(), vec!["detach"]);
}

#[tokio::test]
async fn rejects_an_exclusive_coordinator_while_a_direct_shared_lease_is_active() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    respond_to_attach(&server, "session-1");
    respond_to_detach(&server);
    let direct_handle = client.attach_session("session-1").await.expect("attaches");
    let requests = collect_requests(&server);

    let error =
        RemoteSession::open_session(client.clone(), "session-1", RemoteSessionOptions::default())
            .await
            .expect_err("refuses");
    assert!(
        matches!(
            error,
            RemoteSessionError::Client(PiError::SessionOwnership { .. })
        ),
        "expected a session ownership error, got {error:?}"
    );

    assert!(requests.is_empty());
    assert!(direct_handle.active());
    direct_handle.detach().await.expect("detaches");
    assert_eq!(requests.commands(), vec!["detach"]);
}

#[tokio::test]
async fn factory_creates_a_session() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    let answering = server.clone();
    server.on_message(Arc::new(move |message: &ClientMessage| {
        let ClientMessage::Request(request) = message else {
            return;
        };
        if command_name(request) != "create" {
            return;
        }
        answering.send(&ok_response(
            &request.id,
            create_result(session_snapshot("session-1")),
        ));
    }));

    let remote_session = RemoteSession::create_session(
        client.clone(),
        CreateRemoteSessionOptions {
            cwd: "/workspace".to_owned(),
            ..Default::default()
        },
        RemoteSessionOptions::default(),
    )
    .await
    .expect("creates");

    assert_eq!(remote_session.id().as_deref(), Some("session-1"));
    assert_eq!(
        remote_session.state().lifecycle,
        RemoteSessionLifecycle::Ready
    );
}

#[tokio::test]
async fn disposal_reports_cleanup_failure_without_retaining_exclusive_ownership() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    respond_to_attach(&server, "session-1");
    let remote_session =
        RemoteSession::open_session(client.clone(), "session-1", RemoteSessionOptions::default())
            .await
            .expect("opens");

    let detach_count = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&detach_count);
    let answering = server.clone();
    server.on_message(Arc::new(move |message: &ClientMessage| {
        let ClientMessage::Request(request) = message else {
            return;
        };
        if command_name(request) != "detach" {
            return;
        }
        if counted.fetch_add(1, Ordering::SeqCst) == 0 {
            answering.send(&error_response(&request.id, "no"));
        } else {
            answering.send(&ok_response(&request.id, detach_result("session-1")));
        }
    }));

    assert_eq!(
        remote_session
            .dispose()
            .await
            .expect_err("reports the failure")
            .to_string(),
        "no"
    );
    assert!(remote_session.disposed());
    assert!(client.connected());

    let replacement =
        RemoteSession::open_session(client.clone(), "session-1", RemoteSessionOptions::default())
            .await
            .expect("reopens");
    assert_eq!(replacement.state().lifecycle, RemoteSessionLifecycle::Ready);
    assert_eq!(detach_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn multiple_sessions_borrow_one_client_independently() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    let answering = server.clone();
    server.on_message(Arc::new(move |message: &ClientMessage| {
        let ClientMessage::Request(request) = message else {
            return;
        };
        let session_id = request_session_id(request).unwrap_or_default();
        match command_name(request) {
            "attach" => answering.send(&ok_response(
                &request.id,
                attach_result(session_snapshot(&session_id)),
            )),
            "detach" => {
                answering.send(&ok_response(&request.id, detach_result(&session_id)));
            }
            _ => {}
        }
    }));

    let first =
        RemoteSession::open_session(client.clone(), "session-1", RemoteSessionOptions::default())
            .await
            .expect("opens");
    let second =
        RemoteSession::open_session(client.clone(), "session-2", RemoteSessionOptions::default())
            .await
            .expect("opens");
    first.dispose().await.expect("disposes");

    assert!(first.disposed());
    assert!(!second.disposed());
    assert!(client.connected());
    second.dispose().await.expect("disposes");
}

#[tokio::test]
async fn session_disposal_treats_a_client_first_disposal_as_released() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    respond_to_attach(&server, "session-1");
    let remote_session =
        RemoteSession::open_session(client.clone(), "session-1", RemoteSessionOptions::default())
            .await
            .expect("opens");
    let requests = collect_requests(&server);

    client.dispose().await.expect("disposes the client");
    remote_session.dispose().await.expect("disposes");

    assert!(remote_session.disposed());
    assert!(requests.is_empty());
}
