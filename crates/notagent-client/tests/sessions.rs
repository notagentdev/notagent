//! Port von `packages/client/test/sessions.test.ts`.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use notagent_client::{AcquireSessionOptions, SessionLeaseMode};
use notagent_protocol::{ClientMessage, Command, ProtocolErrorCode, SessionSnapshot};
use support::*;

fn shared() -> AcquireSessionOptions {
    AcquireSessionOptions {
        mode: SessionLeaseMode::Shared,
    }
}

fn exclusive() -> AcquireSessionOptions {
    AcquireSessionOptions {
        mode: SessionLeaseMode::Exclusive,
    }
}

/// Antwortet automatisch auf attach/detach.
fn auto_respond(server: &MemoryByteServer, log: Option<Arc<Mutex<Vec<String>>>>) {
    let responder = server.clone();
    server.on_message(Arc::new(move |message: &ClientMessage| {
        let ClientMessage::Request(request) = message else {
            return;
        };
        if let Some(log) = &log {
            log.lock().unwrap().push(command_name(request).to_owned());
        }
        match &request.request {
            Command::Attach(attach) => {
                responder.send(&ok_response(
                    &request.id,
                    attach_result(session_snapshot(&attach.session_id)),
                ));
            }
            Command::Detach(detach) => {
                responder.send(&ok_response(&request.id, detach_result(&detach.session_id)));
            }
            _ => (),
        }
    }));
}

#[tokio::test]
async fn keeps_multiple_session_handles_independent_and_enforces_detach() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    auto_respond(&server, None);

    let first = client.attach_session("session-1").await.expect("attaches");
    let second = client.attach_session("session-2").await.expect("attaches");
    assert!(first.attached());
    assert!(second.attached());
    first.detach().await.expect("detaches");
    assert!(!first.attached());
    assert!(second.attached());
    let error = first.abort().await.expect_err("rejects");
    assert_eq!(error.name(), "PiSessionDetachedError");
}

#[tokio::test]
async fn detaches_a_shared_session_only_after_its_final_lease_is_released() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    auto_respond(&server, Some(Arc::clone(&requests)));

    let first = client.attach_session("session-1").await.expect("attaches");
    let second = client.attach_session("session-1").await.expect("attaches");
    assert_eq!(*requests.lock().unwrap(), vec!["attach".to_owned()]);

    first.detach().await.expect("detaches");
    assert!(!first.attached());
    assert!(second.attached());
    assert_eq!(*requests.lock().unwrap(), vec!["attach".to_owned()]);

    second.detach().await.expect("detaches");
    assert!(!second.attached());
    assert_eq!(
        *requests.lock().unwrap(),
        vec!["attach".to_owned(), "detach".to_owned()]
    );
}

#[tokio::test]
async fn enforces_exclusive_and_shared_lease_modes() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    auto_respond(&server, None);

    let shared_lease = client
        .acquire_session("session-1", shared())
        .await
        .expect("acquires");
    let error = client
        .acquire_session("session-1", exclusive())
        .await
        .expect_err("rejects");
    assert_eq!(error.name(), "PiSessionOwnershipError");
    shared_lease.dispose().await.expect("disposes");

    let exclusive_lease = client
        .acquire_session("session-1", exclusive())
        .await
        .expect("acquires");
    let error = client
        .acquire_session("session-1", shared())
        .await
        .expect_err("rejects");
    assert_eq!(error.name(), "PiSessionOwnershipError");
    exclusive_lease.dispose().await.expect("disposes");
}

#[tokio::test]
async fn invalidated_leases_dispose_without_protocol_cleanup() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    {
        let responder = server.clone();
        server.on_message(Arc::new(move |message: &ClientMessage| {
            let ClientMessage::Request(request) = message else {
                return;
            };
            let Command::Attach(attach) = &request.request else {
                return;
            };
            responder.send(&ok_response(
                &request.id,
                attach_result(session_snapshot(&attach.session_id)),
            ));
        }));
    }
    let lease = client
        .acquire_session("session-1", exclusive())
        .await
        .expect("acquires");

    client.disconnect("Client disconnected");

    lease.dispose().await.expect("disposes");
    assert!(!lease.active());
}

#[tokio::test]
async fn rejects_commands_while_releasing_and_restores_an_explicit_detach_after_failure() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let requests = collect_requests(&server);
    let acquiring = client.acquire_session("session-1", exclusive());
    let attach_request = requests.last().expect("Missing attach request");
    server.send(&ok_response(
        &attach_request.id,
        attach_result(session_snapshot("session-1")),
    ));
    let lease = acquiring.await.expect("acquires");

    let first_detach = lease.detach();
    let failed_detach_request = requests.last().expect("Missing detach request");
    let error = lease.abort().await.expect_err("rejects");
    assert_eq!(error.name(), "PiSessionDetachedError");
    server.send(&error_response(
        &failed_detach_request.id,
        ProtocolErrorCode::InvalidRequest,
        "retry",
    ));
    let error = first_detach.await.expect_err("rejects");
    assert_eq!(error.message(), "retry");
    assert!(lease.active());

    let second_detach = lease.detach();
    let successful_detach_request = requests.last().expect("Missing retry detach request");
    server.send(&ok_response(
        &successful_detach_request.id,
        detach_result("session-1"),
    ));
    second_detach.await.expect("detaches");
    assert!(!lease.active());
}

#[tokio::test]
async fn serializes_reacquisition_behind_final_lease_detachment() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let requests = collect_requests(&server);

    let first_attachment = client.attach_session("session-1");
    let first_attach_request = requests.last().expect("Missing first attach request");
    server.send(&ok_response(
        &first_attach_request.id,
        attach_result(session_snapshot("session-1")),
    ));
    let first = first_attachment.await.expect("attaches");
    let detaching = first.detach();
    let detach_request = requests.last().expect("Missing detach request");
    let reacquiring = tokio::spawn(client.attach_session("session-1"));
    tokio::task::yield_now().await;
    assert_eq!(
        requests.commands(),
        vec!["attach".to_owned(), "detach".to_owned()]
    );

    server.send(&ok_response(&detach_request.id, detach_result("session-1")));
    detaching.await.expect("detaches");
    tokio::task::yield_now().await;
    let second_attach_request = requests.last().expect("Missing second attach request");
    assert_eq!(command_name(&second_attach_request), "attach");
    assert_ne!(second_attach_request.id, first_attach_request.id);
    server.send(&ok_response(
        &second_attach_request.id,
        attach_result(SessionSnapshot {
            revision: 2,
            ..session_snapshot("session-1")
        }),
    ));

    let handle = reacquiring.await.expect("task").expect("attaches");
    assert!(handle.attached());
}

#[tokio::test]
async fn accepts_a_lower_revision_after_detaching_and_reacquiring_the_same_session() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let attach_count = Arc::new(AtomicUsize::new(0));
    {
        let responder = server.clone();
        let attach_count = Arc::clone(&attach_count);
        server.on_message(Arc::new(move |message: &ClientMessage| {
            let ClientMessage::Request(request) = message else {
                return;
            };
            match &request.request {
                Command::Attach(attach) => {
                    let revision = if attach_count.fetch_add(1, Ordering::SeqCst) == 0 {
                        10
                    } else {
                        0
                    };
                    responder.send(&ok_response(
                        &request.id,
                        attach_result(SessionSnapshot {
                            revision,
                            ..session_snapshot(&attach.session_id)
                        }),
                    ));
                }
                Command::Detach(detach) => {
                    responder.send(&ok_response(&request.id, detach_result(&detach.session_id)));
                }
                _ => (),
            }
        }));
    }

    let first = client.attach_session("session-1").await.expect("attaches");
    assert_eq!(first.snapshot().map(|snapshot| snapshot.revision), Some(10));
    first.detach().await.expect("detaches");
    let reopened = client.attach_session("session-1").await.expect("attaches");
    assert_eq!(
        reopened.snapshot().map(|snapshot| snapshot.revision),
        Some(0)
    );
}
