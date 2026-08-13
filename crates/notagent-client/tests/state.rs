//! Port von `packages/client/test/state.test.ts`.

mod support;

use std::sync::{Arc, Mutex};

use notagent_protocol::{
    AssistantDeltaKind, AssistantDeltaProgress, EventEnvelope, EventTag, ServerEvent,
    ServerMessage, ServerSnapshot, ServerSnapshotEvent, ServerSnapshotTag, SessionMetadata,
    SessionPhase, SessionProgressEvent, SessionProgressTag, SessionSnapshot, ThinkingLevel,
    TranscriptProgress,
};
use support::*;

fn with_revision(id: &str, revision: u64) -> SessionSnapshot {
    SessionSnapshot {
        revision,
        ..session_snapshot(id)
    }
}

#[tokio::test]
async fn reduces_only_authoritative_snapshots_and_supports_unsubscribe() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let requests = collect_requests(&server);
    let initial = SessionSnapshot {
        revision: 1,
        phase: SessionPhase::Idle,
        ..session_snapshot("session-1")
    };
    let handle = attach_session(&client, &server, initial.clone()).await;

    let observed: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let progress_types: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let unsubscribe = {
        let observed = Arc::clone(&observed);
        handle
            .subscribe(Arc::new(move |snapshot: &SessionSnapshot| {
                observed.lock().unwrap().push(snapshot.revision);
            }))
            .expect("subscribes")
    };
    let unsubscribe_events = {
        let progress_types = Arc::clone(&progress_types);
        handle
            .on_event(Arc::new(move |event: &ServerEvent| {
                let name = match event {
                    ServerEvent::ServerSnapshot(_) => "server_snapshot",
                    ServerEvent::SessionSnapshot(_) => "session_snapshot",
                    ServerEvent::SessionProgress(_) => "session_progress",
                    ServerEvent::SessionRemoved(_) => "session_removed",
                };
                progress_types.lock().unwrap().push(name.to_owned());
            }))
            .expect("subscribes")
    };

    server.send(&ServerMessage::Event(EventEnvelope {
        kind: EventTag,
        event: ServerEvent::SessionProgress(SessionProgressEvent {
            kind: SessionProgressTag,
            session_id: "session-1".to_owned(),
            progress: TranscriptProgress::AssistantDelta(AssistantDeltaProgress {
                kind: notagent_protocol::AssistantDeltaTag,
                message_id: "assistant-1".to_owned(),
                content_index: 0,
                delta_kind: AssistantDeltaKind::Text,
                delta: "hi".to_owned(),
            }),
        }),
    }));
    assert_eq!(
        *progress_types.lock().unwrap(),
        vec!["session_progress".to_owned()]
    );
    assert_eq!(handle.snapshot(), Some(initial.clone()));

    let prompting = handle.prompt("hello");
    assert_eq!(handle.snapshot(), Some(initial));
    let prompt_request = requests.find("prompt").expect("Missing prompt request");
    let updated = SessionSnapshot {
        revision: 2,
        phase: SessionPhase::Turn,
        ..session_snapshot("session-1")
    };
    server.send(&ok_response(
        &prompt_request.id,
        notagent_protocol::CommandResult::Prompt(notagent_protocol::PromptResult {
            command: notagent_protocol::PromptTag,
            session: updated.clone(),
        }),
    ));
    assert_eq!(prompting.await.expect("prompts"), updated);
    assert_eq!(handle.snapshot(), Some(updated));
    assert_eq!(*observed.lock().unwrap(), vec![2]);

    unsubscribe();
    unsubscribe_events();
    server.send(&session_event(with_revision("session-1", 3)));
    assert_eq!(*observed.lock().unwrap(), vec![2]);
}

#[tokio::test]
async fn keeps_session_leases_attached_across_server_metadata_snapshots() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let handle = attach_session(&client, &server, session_snapshot("session-1")).await;

    server.send(&ServerMessage::Event(EventEnvelope {
        kind: EventTag,
        event: ServerEvent::ServerSnapshot(ServerSnapshotEvent {
            kind: ServerSnapshotTag,
            snapshot: ServerSnapshot {
                revision: 2,
                sessions: vec![SessionMetadata {
                    id: "session-1".to_owned(),
                    created_at: 1,
                    updated_at: None,
                    parent_session_id: None,
                    session_name: Some("Named session".to_owned()),
                    cwd: None,
                }],
                ..base_server_snapshot()
            },
        }),
    }));

    assert!(handle.attached());
}

#[tokio::test]
async fn does_not_let_a_delayed_command_response_replace_a_newer_event_snapshot() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let initial = SessionSnapshot {
        thinking_level: ThinkingLevel::Off,
        ..with_revision("session-1", 1)
    };
    let handle = attach_session(&client, &server, initial).await;
    let requests = collect_requests(&server);

    let changing = handle.set_thinking(ThinkingLevel::High);
    let request = requests
        .find("set_thinking")
        .expect("Missing set_thinking request");
    server.send(&session_event(SessionSnapshot {
        thinking_level: ThinkingLevel::High,
        ..with_revision("session-1", 3)
    }));
    server.send(&ok_response(
        &request.id,
        notagent_protocol::CommandResult::SetThinking(notagent_protocol::SetThinkingResult {
            command: notagent_protocol::SetThinkingTag,
            session: SessionSnapshot {
                thinking_level: ThinkingLevel::Medium,
                ..with_revision("session-1", 2)
            },
        }),
    ));

    changing.await.expect("changes thinking");
    let snapshot = handle.snapshot().expect("snapshot");
    assert_eq!(snapshot.revision, 3);
    assert_eq!(snapshot.thinking_level, ThinkingLevel::High);
}

#[tokio::test]
async fn does_not_let_an_attach_response_replace_a_newer_snapshot_from_the_reacquired_runtime() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    server.send(&session_event(SessionSnapshot {
        attached: false,
        ..with_revision("session-1", 10)
    }));
    {
        let server = server.clone();
        server.clone().on_message(Arc::new(
            move |message: &notagent_protocol::ClientMessage| {
                let notagent_protocol::ClientMessage::Request(request) = message else {
                    return;
                };
                if command_name(request) != "attach" {
                    return;
                }
                server.send(&session_event(SessionSnapshot {
                    thinking_level: ThinkingLevel::High,
                    ..with_revision("session-1", 3)
                }));
                server.send(&ok_response(
                    &request.id,
                    attach_result(SessionSnapshot {
                        thinking_level: ThinkingLevel::Medium,
                        ..with_revision("session-1", 2)
                    }),
                ));
            },
        ));
    }

    let handle = client.attach_session("session-1").await.expect("attaches");
    let snapshot = handle.snapshot().expect("snapshot");
    assert_eq!(snapshot.revision, 3);
    assert_eq!(snapshot.thinking_level, ThinkingLevel::High);
}
