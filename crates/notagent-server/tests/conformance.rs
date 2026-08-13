//! Port of `packages/server/test/conformance.test.ts`.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notagent_protocol::{
    AssistantDeltaKind, AssistantDeltaProgress, AssistantDeltaTag, AttachCommand, AttachTag,
    ClientHello, ClientMessage, Command, DetachCommand, DetachTag, HelloTag, ListCommand, ListTag,
    PROTOCOL_VERSION, ResponseEnvelope, ServerEvent, ServerMessage, SetThinkingCommand,
    SetThinkingTag, ThinkingLevel, TranscriptProgress, encode_client_message, encode_frame,
};
use notagent_server::PiServerError;
use notagent_server::errors::ServerError;
use notagent_server::testing::{Deferred, TestServerService};
use notagent_server::transports::unix::UnixServerOptions;
use support::*;

fn list_command() -> Command {
    Command::List(ListCommand { command: ListTag })
}

fn attach_command(session_id: &str) -> Command {
    Command::Attach(AttachCommand {
        command: AttachTag,
        session_id: session_id.to_owned(),
    })
}

fn detach_command(session_id: &str) -> Command {
    Command::Detach(DetachCommand {
        command: DetachTag,
        session_id: session_id.to_owned(),
    })
}

fn is_hello(message: &ServerMessage) -> bool {
    matches!(message, ServerMessage::Hello(_))
}

fn is_hello_error(message: &ServerMessage) -> bool {
    matches!(message, ServerMessage::HelloError(_))
}

#[tokio::test]
async fn accepts_a_transport_fragmented_framed_cbor_hello() {
    let harness = start_default_server().await;
    let client = connect(&harness.server).await;
    let response = client.next(Arc::new(is_hello));
    client
        .send_fragmented_message(
            ClientMessage::Hello(ClientHello {
                kind: HelloTag,
                version: PROTOCOL_VERSION,
            }),
            2,
        )
        .await
        .expect("sends");
    assert!(is_hello(&response.await.expect("hello")));
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn enforces_version_and_exactly_one_first_message_hello() {
    let harness = start_default_server().await;

    let bad_version = connect(&harness.server).await;
    let response = bad_version.hello_with_version(PROTOCOL_VERSION + 1).await;
    let ServerMessage::HelloError(error) = &response else {
        panic!("expected hello_error, got {response:?}")
    };
    assert_eq!(
        error.error.code,
        notagent_protocol::ProtocolErrorCode::Version
    );
    bad_version.wait_for_close().await;

    let request_first = connect(&harness.server).await;
    let first_error = request_first.next(Arc::new(is_hello_error));
    request_first
        .send_message(ClientMessage::Request(notagent_protocol::RequestEnvelope {
            kind: notagent_protocol::RequestTag,
            id: "too-early".to_owned(),
            request: list_command(),
        }))
        .await
        .expect("sends");
    let ServerMessage::HelloError(error) = first_error.await.expect("hello_error") else {
        panic!("expected error")
    };
    assert_eq!(
        error.error.code,
        notagent_protocol::ProtocolErrorCode::InvalidRequest
    );
    request_first.wait_for_close().await;

    let duplicate = connect(&harness.server).await;
    assert!(is_hello(&duplicate.hello().await));
    let duplicate_error = duplicate.next(Arc::new(is_hello_error));
    duplicate
        .send_message(ClientMessage::Hello(ClientHello {
            kind: HelloTag,
            version: PROTOCOL_VERSION,
        }))
        .await
        .expect("sends");
    let ServerMessage::HelloError(error) = duplicate_error.await.expect("hello_error") else {
        panic!("expected error")
    };
    assert_eq!(
        error.error.code,
        notagent_protocol::ProtocolErrorCode::InvalidRequest
    );
    duplicate.wait_for_close().await;
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn closes_connections_that_do_not_complete_hello_before_the_timeout() {
    let harness = start_server(
        TestServerService::new(),
        UnixServerOptions {
            handshake_timeout_ms: Some(20),
            ..UnixServerOptions::default()
        },
    )
    .await;
    let client = connect(&harness.server).await;
    client.wait_for_close().await;
    assert!(client.messages().iter().any(is_hello_error));
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn keeps_the_handshake_timeout_active_until_the_server_hello_is_sent() {
    let service = TestServerService::new();
    let delay = service.delay_next_list();
    let harness = start_server(
        service,
        UnixServerOptions {
            handshake_timeout_ms: Some(20),
            ..UnixServerOptions::default()
        },
    )
    .await;
    let client = connect(&harness.server).await;
    client
        .send_message(ClientMessage::Hello(ClientHello {
            kind: HelloTag,
            version: PROTOCOL_VERSION,
        }))
        .await
        .expect("sends");
    delay.entered.promise().await;
    client.wait_for_close().await;
    delay.release.resolve(());
    assert!(client.messages().iter().any(is_hello_error));
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn bounds_and_closes_malformed_or_oversized_frames() {
    let malformed_harness = start_default_server().await;
    let malformed = connect(&malformed_harness.server).await;
    let malformed_error = malformed.next(Arc::new(is_hello_error));
    malformed
        .send_bytes(encode_frame(&[0xff]).expect("frames"))
        .await
        .expect("sends");
    let ServerMessage::HelloError(error) = malformed_error.await.expect("hello_error") else {
        panic!("expected error")
    };
    assert_eq!(
        error.error.code,
        notagent_protocol::ProtocolErrorCode::InvalidRequest
    );
    malformed.wait_for_close().await;

    let bounded_harness = start_server(
        TestServerService::new(),
        UnixServerOptions {
            max_frame_length: Some(128),
            ..UnixServerOptions::default()
        },
    )
    .await;
    let oversized = connect(&bounded_harness.server).await;
    let mut frame = vec![0u8; 4 + 129];
    frame[3] = 129;
    oversized.send_bytes(frame).await.expect("sends");
    oversized.wait_for_close().await;
    assert!(!oversized.messages().iter().any(is_hello));

    let outbound_harness = start_server(
        TestServerService::new(),
        UnixServerOptions {
            max_frame_length: Some(128),
            ..UnixServerOptions::default()
        },
    )
    .await;
    let outbound = connect(&outbound_harness.server).await;
    outbound
        .send_message(ClientMessage::Hello(ClientHello {
            kind: HelloTag,
            version: PROTOCOL_VERSION,
        }))
        .await
        .expect("sends");
    outbound.wait_for_close().await;
    assert!(outbound.messages().is_empty());

    for harness in [malformed_harness, bounded_harness, outbound_harness] {
        harness.server.close().await.expect("closes");
    }
}

#[tokio::test]
async fn catches_up_a_handshaking_client_after_a_concurrent_server_change() {
    let inner = TestServerService::new();
    inner.seed("shared");
    let mut service = HookedService::new(inner);
    let entered = Arc::new(Deferred::<()>::new());
    let release = Arc::new(Deferred::<()>::new());
    let race = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        let race = Arc::clone(&race);
        service.on_list_sessions = Some(Arc::new(move |sessions| {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            let race = Arc::clone(&race);
            Box::pin(async move {
                if !race.load(Ordering::SeqCst) {
                    return Ok(sessions);
                }
                entered.resolve(());
                release.promise().await;
                Ok(sessions)
            })
        }));
    }
    let harness = start_hooked_server(service, UnixServerOptions::default()).await;
    let controller = connect(&harness.server).await;
    controller.hello().await;
    race.store(true, Ordering::SeqCst);
    let joining = connect(&harness.server).await;
    let joining_client = Arc::clone(&joining);
    let hello = tokio::spawn(async move { joining_client.hello().await });
    entered.promise().await;
    controller.request(attach_command("shared")).await;
    release.resolve(());
    let handshake = hello.await.expect("task");
    let ServerMessage::Hello(handshake) = handshake else {
        panic!("expected server hello")
    };
    let base_revision = handshake.snapshot.revision;
    let catchup = joining
        .next(Arc::new(move |message: &ServerMessage| match message {
            ServerMessage::Event(event) => match &event.event {
                ServerEvent::ServerSnapshot(snapshot) => snapshot.snapshot.revision > base_revision,
                _ => false,
            },
            _ => false,
        }))
        .await
        .expect("catchup");
    let ServerMessage::Event(event) = catchup else {
        panic!("expected event")
    };
    let ServerEvent::ServerSnapshot(snapshot) = event.event else {
        panic!("expected server snapshot")
    };
    assert_eq!(snapshot.snapshot.sessions.len(), 1);
    assert_eq!(snapshot.snapshot.sessions[0].id, "shared");
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn shares_request_event_attachment_and_disconnect_behavior() {
    let service = TestServerService::new();
    service.seed("first");
    service.seed("second");
    let harness = start_server(service.clone(), UnixServerOptions::default()).await;
    let client = connect(&harness.server).await;
    let ServerMessage::Hello(hello) = client.hello().await else {
        panic!("expected hello")
    };
    assert_eq!(
        hello
            .snapshot
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>(),
        vec!["first".to_owned(), "second".to_owned()]
    );

    let listed = client.request(list_command()).await;
    let ResponseEnvelope::Ok(ok) = &listed else {
        panic!("expected ok")
    };
    let notagent_protocol::CommandResult::List(list) = &ok.result else {
        panic!("expected list")
    };
    assert_eq!(
        list.sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );

    for id in ["first", "second"] {
        let attached = client.request(attach_command(id)).await;
        let ResponseEnvelope::Ok(ok) = &attached else {
            panic!("expected ok")
        };
        let notagent_protocol::CommandResult::Attach(attach) = &ok.result else {
            panic!("expected attach")
        };
        assert_eq!(attach.session.id, id);
        assert!(attach.session.attached);
    }

    let progress = TranscriptProgress::AssistantDelta(AssistantDeltaProgress {
        kind: AssistantDeltaTag,
        message_id: "assistant-1".to_owned(),
        content_index: 0,
        delta_kind: AssistantDeltaKind::Text,
        delta: "hello".to_owned(),
    });
    let progress_event = client.next(Arc::new(|message: &ServerMessage| {
        matches!(message, ServerMessage::Event(event) if matches!(event.event, ServerEvent::SessionProgress(_)))
    }));
    service
        .latest_runtime("first")
        .emit_progress(progress.clone());
    let ServerMessage::Event(event) = progress_event.await.expect("progress") else {
        panic!("expected event")
    };
    let ServerEvent::SessionProgress(session_progress) = event.event else {
        panic!("expected progress")
    };
    assert_eq!(session_progress.session_id, "first");
    assert_eq!(session_progress.progress, progress);

    let detached = client.request(detach_command("first")).await;
    assert!(matches!(detached, ResponseEnvelope::Ok(_)));
    assert_eq!(service.latest_runtime("first").dispose_count(), 1);

    let thinking = client
        .request(Command::SetThinking(SetThinkingCommand {
            command: SetThinkingTag,
            session_id: "second".to_owned(),
            thinking_level: ThinkingLevel::High,
        }))
        .await;
    let ResponseEnvelope::Ok(ok) = &thinking else {
        panic!("expected ok")
    };
    let notagent_protocol::CommandResult::SetThinking(result) = &ok.result else {
        panic!("expected set_thinking")
    };
    assert_eq!(result.session.thinking_level, ThinkingLevel::High);

    let second_runtime = service.latest_runtime("second");
    client.close().await.expect("closes");
    second_runtime.disposed.promise().await;
    assert_eq!(second_runtime.dispose_count(), 1);
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn disconnects_attached_clients_when_a_runtime_reports_a_terminal_error() {
    let service = TestServerService::new();
    service.seed("terminal");
    let errors = ErrorLog::default();
    let harness = start_server(
        service.clone(),
        UnixServerOptions {
            on_error: Some(errors.observer()),
            ..UnixServerOptions::default()
        },
    )
    .await;
    let client = connect(&harness.server).await;
    client.hello().await;
    client.request(attach_command("terminal")).await;
    let runtime = service.latest_runtime("terminal");

    runtime.set_phase(notagent_protocol::SessionPhase::Turn);
    runtime.emit_error(PiServerError::locked("lock ownership lost"));
    client.wait_for_close().await;
    runtime.disposed.promise().await;
    assert_eq!(runtime.dispose_count(), 1);
    assert!(!service.locked().contains("terminal"));
    assert!(
        errors
            .messages()
            .iter()
            .any(|message| message == "lock ownership lost"),
        "{:?}",
        errors.messages()
    );

    let next_client = connect(&harness.server).await;
    next_client.hello().await;
    let attached = next_client.request(attach_command("terminal")).await;
    assert!(matches!(attached, ResponseEnvelope::Ok(_)));
    assert_eq!(service.runtime_count("terminal"), 2);
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn does_not_expose_unexpected_service_errors_to_clients() {
    let inner = TestServerService::new();
    let mut service = HookedService::new(inner);
    let list_count = Arc::new(AtomicUsize::new(0));
    {
        let list_count = Arc::clone(&list_count);
        service.on_list_sessions = Some(Arc::new(move |sessions| {
            let count = list_count.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                if count > 1 {
                    return Err(ServerError::other("private service detail"));
                }
                Ok(sessions)
            })
        }));
    }
    let errors = ErrorLog::default();
    let harness = start_hooked_server(
        service,
        UnixServerOptions {
            on_error: Some(errors.observer()),
            ..UnixServerOptions::default()
        },
    )
    .await;
    let client = connect(&harness.server).await;
    client.hello().await;
    let response = client.request(list_command()).await;
    let ResponseEnvelope::Error(error) = &response else {
        panic!("expected error")
    };
    assert_eq!(
        error.error.code,
        notagent_protocol::ProtocolErrorCode::InternalError
    );
    assert_eq!(error.error.message, "Internal server error");
    assert!(
        errors
            .messages()
            .iter()
            .any(|message| message == "private service detail")
    );
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn keeps_not_implemented_stable() {
    let mut service = HookedService::new(TestServerService::new());
    let list_count = Arc::new(AtomicUsize::new(0));
    {
        let list_count = Arc::clone(&list_count);
        service.on_list_sessions = Some(Arc::new(move |sessions| {
            let count = list_count.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                if count > 1 {
                    return Err(PiServerError::not_implemented().into());
                }
                Ok(sessions)
            })
        }));
    }
    let harness = start_hooked_server(service, UnixServerOptions::default()).await;
    let client = connect(&harness.server).await;
    client.hello().await;
    let response = client.request(list_command()).await;
    let ResponseEnvelope::Error(error) = &response else {
        panic!("expected error")
    };
    assert_eq!(
        error.error.code,
        notagent_protocol::ProtocolErrorCode::NotImplemented
    );
    assert_eq!(error.error.message, "Operation is not implemented");
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn reports_wrapped_internal_causes_without_exposing_them() {
    let mut service = HookedService::new(TestServerService::new());
    let list_count = Arc::new(AtomicUsize::new(0));
    {
        let list_count = Arc::clone(&list_count);
        service.on_list_sessions = Some(Arc::new(move |sessions| {
            let count = list_count.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                if count > 1 {
                    return Err(ServerError::internal("private storage detail"));
                }
                Ok(sessions)
            })
        }));
    }
    let errors = ErrorLog::default();
    let harness = start_hooked_server(
        service,
        UnixServerOptions {
            on_error: Some(errors.observer()),
            ..UnixServerOptions::default()
        },
    )
    .await;
    let client = connect(&harness.server).await;
    client.hello().await;
    let response = client.request(list_command()).await;
    let ResponseEnvelope::Error(error) = &response else {
        panic!("expected error")
    };
    assert_eq!(
        error.error.code,
        notagent_protocol::ProtocolErrorCode::InternalError
    );
    assert_eq!(error.error.message, "Internal server error");
    let serialized = serde_json::to_string(&response).expect("serializes");
    assert!(!serialized.contains("private"), "{serialized}");
    assert!(
        errors
            .messages()
            .iter()
            .any(|message| message == "private storage detail")
    );
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn can_respond_out_of_request_order_after_the_handshake() {
    let service = TestServerService::new();
    service.seed("first");
    let harness = start_server(service.clone(), UnixServerOptions::default()).await;
    let client = connect(&harness.server).await;
    client.hello().await;

    let delay = service.delay_next_list();
    let slow_client = Arc::clone(&client);
    let slow =
        tokio::spawn(async move { slow_client.request_with_id(list_command(), "slow").await });
    delay.entered.promise().await;
    let fast = client
        .request_with_id(attach_command("first"), "fast")
        .await;
    assert!(matches!(fast, ResponseEnvelope::Ok(_)));
    assert!(!client.messages().iter().any(|message| matches!(
        message,
        ServerMessage::Response(ResponseEnvelope::Ok(ok)) if ok.id == "slow"
    )));

    delay.release.resolve(());
    let slow = slow.await.expect("task");
    assert!(matches!(slow, ResponseEnvelope::Ok(_)));
    let response_ids: Vec<String> = client
        .messages()
        .iter()
        .filter_map(|message| match message {
            ServerMessage::Response(ResponseEnvelope::Ok(ok))
                if ok.id == "slow" || ok.id == "fast" =>
            {
                Some(ok.id.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(response_ids, vec!["fast".to_owned(), "slow".to_owned()]);
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn gracefully_closes_connections_sessions_and_listener_resources() {
    let service = TestServerService::new();
    service.seed("first");
    let harness = start_server(service.clone(), UnixServerOptions::default()).await;
    let socket_path = harness.server.addresses().first().cloned();
    let client = connect(&harness.server).await;
    client.hello().await;
    client.request(attach_command("first")).await;
    let runtime = service.latest_runtime("first");

    harness.server.close().await.expect("closes");
    client.wait_for_close().await;
    assert_eq!(runtime.dispose_count(), 1);
    assert!(harness.server.addresses().is_empty());
    if let Some(socket_path) = socket_path {
        assert!(
            !std::path::Path::new(&socket_path).exists(),
            "socket still exists: {socket_path}"
        );
    }
    harness.server.close().await.expect("closes twice");
}

#[tokio::test]
async fn unix_socket_decodes_multiple_framed_requests_from_one_raw_chunk() {
    let harness = start_default_server().await;
    let client = connect(&harness.server).await;
    client.hello().await;
    let mut combined = encode_client_message(
        &ClientMessage::Request(notagent_protocol::RequestEnvelope {
            kind: notagent_protocol::RequestTag,
            id: "first".to_owned(),
            request: list_command(),
        }),
        None,
    )
    .expect("encodes");
    combined.extend(
        encode_client_message(
            &ClientMessage::Request(notagent_protocol::RequestEnvelope {
                kind: notagent_protocol::RequestTag,
                id: "second".to_owned(),
                request: list_command(),
            }),
            None,
        )
        .expect("encodes"),
    );
    let first_response = client.next(Arc::new(|message: &ServerMessage| {
        matches!(message, ServerMessage::Response(ResponseEnvelope::Ok(ok)) if ok.id == "first")
    }));
    let second_response = client.next(Arc::new(|message: &ServerMessage| {
        matches!(message, ServerMessage::Response(ResponseEnvelope::Ok(ok)) if ok.id == "second")
    }));
    client.send_bytes(combined).await.expect("sends");
    assert!(first_response.await.is_some());
    assert!(second_response.await.is_some());
    harness.server.close().await.expect("closes");
}
