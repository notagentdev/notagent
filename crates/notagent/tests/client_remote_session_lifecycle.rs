//! Port of `packages/coding-agent/test/client/remote-session-lifecycle.test.ts` (190 LOC).

mod client_support;

use std::sync::{Arc, Mutex};

use client_support::{
    MemoryServer, attach_result, collect_requests, command_name, connect_client, detach_result,
    next_request, ok_response, open_remote_session, request_session_id, session_snapshot,
    snapshot_event,
};
use notagent::client::{RemoteSessionLifecycle, RemoteSessionOptions};
use notagent_protocol::SessionPhase;

#[tokio::test]
async fn opens_a_replacement_before_detaching_the_current_session() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    let remote_session = open_remote_session(
        &client,
        &server,
        session_snapshot("session-1"),
        RemoteSessionOptions::default(),
    )
    .await;
    let requests = collect_requests(&server);

    let opening = remote_session.open("session-2");
    let attach_request = requests.last().expect("Missing attach request");
    let detach_request = next_request(&server, "detach");
    server.send(&ok_response(
        &attach_request.id,
        attach_result(session_snapshot("session-2")),
    ));
    let detach_request = detach_request.await;
    assert_eq!(command_name(&detach_request), "detach");
    assert_eq!(
        request_session_id(&detach_request).as_deref(),
        Some("session-1")
    );
    server.send(&ok_response(&detach_request.id, detach_result("session-1")));
    opening.await.expect("opens");

    assert_eq!(remote_session.id().as_deref(), Some("session-2"));
}

#[tokio::test]
async fn rejects_another_mutation_while_replacement_attachment_is_pending() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    let remote_session = open_remote_session(
        &client,
        &server,
        session_snapshot("session-1"),
        RemoteSessionOptions::default(),
    )
    .await;
    let requests = collect_requests(&server);

    let opening = remote_session.open("session-2");
    assert_eq!(
        remote_session
            .submit("race")
            .await
            .expect_err("refuses")
            .to_string(),
        "Remote session is busy with open"
    );
    assert_eq!(
        remote_session
            .create(notagent::client::CreateRemoteSessionOptions {
                cwd: "/other".to_owned(),
                ..Default::default()
            })
            .await
            .expect_err("refuses")
            .to_string(),
        "Remote session is busy with open"
    );
    let pending: Vec<(&'static str, Option<String>)> = requests
        .all()
        .iter()
        .map(|request| (command_name(request), request_session_id(request)))
        .collect();
    assert_eq!(pending, vec![("attach", Some("session-2".to_owned()))]);

    let attach_request = requests
        .all()
        .first()
        .cloned()
        .expect("Missing attach request");
    let detach_request = next_request(&server, "detach");
    server.send(&ok_response(
        &attach_request.id,
        attach_result(session_snapshot("session-2")),
    ));
    let detach_request = detach_request.await;
    server.send(&ok_response(&detach_request.id, detach_result("session-1")));
    opening.await.expect("opens");
}

#[tokio::test]
async fn rolls_back_a_replacement_when_the_current_server_session_becomes_active() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    let remote_session = open_remote_session(
        &client,
        &server,
        session_snapshot("session-1"),
        RemoteSessionOptions::default(),
    )
    .await;
    let requests = collect_requests(&server);

    let opening = remote_session.open("session-2");
    let attach_request = requests.last().expect("Missing attach request");
    let detach_request = next_request(&server, "detach");
    let mut turned = session_snapshot("session-1");
    turned.phase = SessionPhase::Turn;
    turned.revision = 2;
    server.send(&snapshot_event(turned));
    server.send(&ok_response(
        &attach_request.id,
        attach_result(session_snapshot("session-2")),
    ));
    let detach_request = detach_request.await;
    assert_eq!(
        request_session_id(&detach_request).as_deref(),
        Some("session-2")
    );
    server.send(&ok_response(&detach_request.id, detach_result("session-2")));

    assert_eq!(
        opening.await.expect_err("rolls back").to_string(),
        "Cannot open a session while session is turn"
    );
    assert_eq!(remote_session.id().as_deref(), Some("session-1"));
    assert_eq!(
        remote_session.state().lifecycle,
        RemoteSessionLifecycle::Ready
    );
}

#[tokio::test]
async fn dispose_awaits_attachment_cleanup_started_by_reconnect() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    let remote_session = open_remote_session(
        &client,
        &server,
        session_snapshot("session-1"),
        RemoteSessionOptions::default(),
    )
    .await;
    client.disconnect("test reconnect");
    let attach_request = next_request(&server, "attach");

    let reconnecting = remote_session.reconnect();
    let attach_request = attach_request.await;
    let disposing = remote_session.dispose();
    let disposal_settled = Arc::new(Mutex::new(false));
    let settled = Arc::clone(&disposal_settled);
    // `dispose()` hands out the same cleanup again, as the ownership suite asserts.
    let watching = remote_session.dispose();
    let watcher = tokio::spawn(async move {
        let _ = watching.await;
        *settled.lock().expect("settled mutex") = true;
    });

    assert_eq!(
        reconnecting.await.expect_err("gives up").to_string(),
        "Remote session is disposed"
    );
    let detach_request = next_request(&server, "detach");
    let mut reattached = session_snapshot("session-1");
    reattached.revision = 2;
    server.send(&ok_response(&attach_request.id, attach_result(reattached)));
    let detach_request = detach_request.await;
    assert!(!*disposal_settled.lock().expect("settled mutex"));
    server.send(&ok_response(&detach_request.id, detach_result("session-1")));

    disposing.await.expect("disposes");
    watcher.await.expect("watcher");
    assert!(*disposal_settled.lock().expect("settled mutex"));
    assert!(client.connected());
}

#[tokio::test]
async fn dispose_immediately_preempts_pending_work_and_awaits_attachment_cleanup() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    let remote_session = open_remote_session(
        &client,
        &server,
        session_snapshot("session-1"),
        RemoteSessionOptions::default(),
    )
    .await;
    let states = Arc::new(Mutex::new(Vec::<String>::new()));
    let collected = Arc::clone(&states);
    let _unsubscribe = remote_session
        .subscribe(Arc::new(move |state| {
            let label = match state.lifecycle {
                RemoteSessionLifecycle::Busy { .. } => "busy",
                RemoteSessionLifecycle::Ready => "ready",
                RemoteSessionLifecycle::Unbound => "unbound",
                RemoteSessionLifecycle::Disposed => "disposed",
            };
            collected
                .lock()
                .expect("states mutex")
                .push(label.to_owned());
        }))
        .expect("subscribes");
    let requests = collect_requests(&server);

    let opening = remote_session.open("session-2");
    let attach_request = requests.find("attach").expect("Missing attach request");
    let disposing = remote_session.dispose();
    let current_detach_request = requests
        .find_session("detach", "session-1")
        .expect("Missing current detach request");

    assert!(client.connected());
    assert_eq!(
        remote_session.state().lifecycle,
        RemoteSessionLifecycle::Disposed
    );
    opening.await.expect_err("gives up");
    let replacement_detach_request = next_request(&server, "detach");
    server.send(&ok_response(
        &attach_request.id,
        attach_result(session_snapshot("session-2")),
    ));
    server.send(&ok_response(
        &current_detach_request.id,
        detach_result("session-1"),
    ));
    let replacement_detach_request = replacement_detach_request.await;
    assert_eq!(
        request_session_id(&replacement_detach_request).as_deref(),
        Some("session-2")
    );
    server.send(&ok_response(
        &replacement_detach_request.id,
        detach_result("session-2"),
    ));
    disposing.await.expect("disposes");

    assert!(
        states
            .lock()
            .expect("states mutex")
            .contains(&"disposed".to_owned())
    );
    let refused = remote_session
        .subscribe(Arc::new(|_| {}))
        .err()
        .expect("refuses");
    assert_eq!(refused.to_string(), "Remote session is disposed");
}
