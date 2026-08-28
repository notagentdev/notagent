mod support;

use notagent_protocol::{CommandResult, ListResult, ListTag, ProtocolErrorCode};
use support::*;

#[tokio::test]
async fn correlates_coalesced_out_of_order_responses() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let requests = collect_requests(&server);
    let listed = client.list_sessions();
    let attached = client.attach_session("session-1");
    assert_eq!(requests.len(), 2);

    let attach_request = requests.find("attach").expect("Missing requests");
    let list_request = requests.find("list").expect("Missing requests");
    server.send_together(&[
        ok_response(
            &attach_request.id,
            attach_result(session_snapshot("session-1")),
        ),
        ok_response(
            &list_request.id,
            CommandResult::List(ListResult {
                command: ListTag,
                sessions: vec![],
            }),
        ),
    ]);

    assert_eq!(listed.await.expect("lists"), vec![]);
    let handle = attached.await.expect("attaches");
    assert_eq!(handle.id(), "session-1");
    assert!(handle.attached());
}

#[tokio::test]
async fn rejects_a_mismatched_response_instead_of_leaving_its_request_pending() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let requests = collect_requests(&server);
    let listed = client.list_sessions();
    assert_eq!(requests.commands(), vec!["list".to_owned()]);
    server.send(&ok_response(
        &requests.all()[0].id,
        attach_result(session_snapshot("session-1")),
    ));

    let error = listed.await.expect_err("rejects");
    assert_eq!(error.name(), "ProtocolValidationError");
    assert_eq!(
        error.message(),
        "Response command attach does not match list"
    );
    assert_eq!(
        client.connection_state(),
        notagent_client::ConnectionState::Disconnected
    );
}

#[tokio::test]
async fn surfaces_typed_request_errors() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let requests = collect_requests(&server);
    let attaching = client.attach_session("locked");
    server.send(&error_response(
        &requests.all()[0].id,
        ProtocolErrorCode::SessionLocked,
        "Already attached",
    ));

    let error = attaching.await.expect_err("rejects");
    assert_eq!(error.name(), "PiServerError");
    assert_eq!(error.code(), Some(ProtocolErrorCode::SessionLocked));
}
