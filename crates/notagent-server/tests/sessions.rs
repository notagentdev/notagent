//! Port of `packages/server/test/sessions.test.ts`.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notagent_protocol::Command;
use notagent_protocol::{
    AbortCommand, AbortTag, AssistantDeltaKind, AssistantDeltaProgress, AssistantDeltaTag,
    AttachCommand, AttachTag, CommandResult, CreateCommand, CreateTag, DetachCommand, DetachTag,
    ListCommand, ListTag, ModelRef, PromptCommand, PromptTag, ResponseEnvelope, ServerEvent,
    ServerMessage, SessionMetadata, SessionPhase, SessionSnapshot, SetModelCommand, SetModelTag,
    SetThinkingCommand, SetThinkingTag, SteerCommand, SteerTag, ThinkingLevel, TranscriptItem,
    TranscriptProgress,
};
use notagent_server::errors::ServerError;
use notagent_server::testing::{Deferred, ProtocolTestClient, TestServerService};
use notagent_server::transports::unix::UnixServerOptions;
use support::*;

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

fn ok(response: &ResponseEnvelope) -> &CommandResult {
    match response {
        ResponseEnvelope::Ok(ok) => &ok.result,
        ResponseEnvelope::Error(error) => panic!("expected ok, got {:?}", error.error),
    }
}

fn session_of(result: &CommandResult) -> &SessionSnapshot {
    match result {
        CommandResult::Create(result) => &result.session,
        CommandResult::Attach(result) => &result.session,
        CommandResult::Prompt(result) => &result.session,
        CommandResult::Steer(result) => &result.session,
        CommandResult::Abort(result) => &result.session,
        CommandResult::SetModel(result) => &result.session,
        CommandResult::SetThinking(result) => &result.session,
        other => panic!("expected a session result, got {other:?}"),
    }
}

fn sessions_of(result: &CommandResult) -> &Vec<SessionMetadata> {
    match result {
        CommandResult::List(result) => &result.sessions,
        other => panic!("expected a list result, got {other:?}"),
    }
}

async fn attach(client: &Arc<ProtocolTestClient>, session_id: &str) -> SessionSnapshot {
    let response = client.request(attach_command(session_id)).await;
    session_of(ok(&response)).clone()
}

fn snapshot_event_matching(
    predicate: impl Fn(&SessionSnapshot) -> bool + Send + Sync + 'static,
) -> Arc<dyn Fn(&ServerMessage) -> bool + Send + Sync> {
    Arc::new(move |message: &ServerMessage| match message {
        ServerMessage::Event(event) => match &event.event {
            ServerEvent::SessionSnapshot(snapshot) => predicate(&snapshot.snapshot),
            _ => false,
        },
        _ => false,
    })
}

#[tokio::test]
async fn serializes_server_snapshot_revisions() {
    let mut service = HookedService::new(TestServerService::new());
    let first_started = Arc::new(Deferred::<()>::new());
    let second_started = Arc::new(Deferred::<()>::new());
    let first_release = Arc::new(Deferred::<()>::new());
    let second_release = Arc::new(Deferred::<()>::new());
    let controlled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started_count = Arc::new(AtomicUsize::new(0));
    {
        let first_started = Arc::clone(&first_started);
        let second_started = Arc::clone(&second_started);
        let first_release = Arc::clone(&first_release);
        let second_release = Arc::clone(&second_release);
        let controlled = Arc::clone(&controlled);
        let started_count = Arc::clone(&started_count);
        service.on_list_models = Some(Arc::new(move || {
            let (first_started, second_started) =
                (Arc::clone(&first_started), Arc::clone(&second_started));
            let (first_release, second_release) =
                (Arc::clone(&first_release), Arc::clone(&second_release));
            let controlled = Arc::clone(&controlled);
            let started_count = Arc::clone(&started_count);
            Box::pin(async move {
                if !controlled.load(Ordering::SeqCst) {
                    return Ok(());
                }
                let count = started_count.fetch_add(1, Ordering::SeqCst) + 1;
                if count == 1 {
                    first_started.resolve(());
                    first_release.promise().await;
                } else if count == 2 {
                    second_started.resolve(());
                    second_release.promise().await;
                }
                Ok(())
            })
        }));
    }
    let harness = start_hooked_server(service, UnixServerOptions::default()).await;
    let client = connect(&harness.server).await;
    client.hello().await;
    controlled.store(true, Ordering::SeqCst);
    let message_index = client.messages().len();

    let first_client = Arc::clone(&client);
    let first_create = tokio::spawn(async move {
        first_client
            .request(Command::Create(CreateCommand {
                command: CreateTag,
                cwd: None,
                name: Some("first".to_owned()),
                model: None,
                thinking_level: None,
            }))
            .await
    });
    first_started.promise().await;
    let second_client = Arc::clone(&client);
    let second_create = tokio::spawn(async move {
        second_client
            .request(Command::Create(CreateCommand {
                command: CreateTag,
                cwd: None,
                name: Some("second".to_owned()),
                model: None,
                thinking_level: None,
            }))
            .await
    });
    tokio::task::yield_now().await;
    assert_eq!(started_count.load(Ordering::SeqCst), 1);

    first_release.resolve(());
    second_started.promise().await;
    second_release.resolve(());
    first_create.await.expect("task");
    second_create.await.expect("task");
    client
        .next_from(
            message_index,
            Arc::new(|message: &ServerMessage| match message {
                ServerMessage::Event(event) => match &event.event {
                    ServerEvent::ServerSnapshot(snapshot) => snapshot.snapshot.revision == 2,
                    _ => false,
                },
                _ => false,
            }),
        )
        .await
        .expect("second broadcast");

    let revisions: Vec<u64> = client
        .messages()
        .iter()
        .skip(message_index)
        .filter_map(|message| match message {
            ServerMessage::Event(event) => match &event.event {
                ServerEvent::ServerSnapshot(snapshot) => Some(snapshot.snapshot.revision),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(revisions, vec![1, 2]);
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn creates_server_assigned_durable_ids_and_supports_list_attach_and_detach() {
    let harness = start_default_server().await;
    let client = connect(&harness.server).await;
    client.hello().await;
    let created = client
        .request(Command::Create(CreateCommand {
            command: CreateTag,
            cwd: Some("/work".to_owned()),
            name: Some("Created".to_owned()),
            model: None,
            thinking_level: None,
        }))
        .await;
    let session = session_of(ok(&created)).clone();
    let created_id = session.id.clone();
    assert_eq!(Some(created_id.clone()), harness.service.last_created_id());
    assert_eq!(session.cwd, "/work");
    assert_eq!(session.name.as_deref(), Some("Created"));
    assert!(session.attached);
    assert!(session.locked);

    let listed = client
        .request(Command::List(ListCommand { command: ListTag }))
        .await;
    assert_eq!(
        sessions_of(ok(&listed)),
        &vec![SessionMetadata {
            id: created_id.clone(),
            created_at: 1,
            updated_at: Some(1),
            parent_session_id: None,
            session_name: Some("Created".to_owned()),
            cwd: Some("/work".to_owned()),
        }]
    );

    let detached = client.request(detach_command(&created_id)).await;
    assert!(matches!(ok(&detached), CommandResult::Detach(_)));
    assert_eq!(
        harness.service.latest_runtime(&created_id).dispose_count(),
        1
    );
    let detached_again = client.request(detach_command(&created_id)).await;
    assert!(matches!(ok(&detached_again), CommandResult::Detach(_)));

    let attached = attach(&client, &created_id).await;
    assert_eq!(attached.id, created_id);
    assert_eq!(harness.service.runtime_count(&created_id), 2);
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn preserves_backend_metadata_while_refreshing_live_session_metadata() {
    let inner = TestServerService::new();
    inner.seed_with(
        "session-1",
        "Live name",
        "/tmp/notagent-server-conformance",
        None,
        None,
    );
    let mut service = HookedService::new(inner);
    service.on_list_sessions = Some(Arc::new(|sessions: Vec<SessionMetadata>| {
        Box::pin(async move {
            Ok(sessions
                .into_iter()
                .map(|metadata| SessionMetadata {
                    parent_session_id: Some("parent-1".to_owned()),
                    session_name: Some("stale name".to_owned()),
                    ..metadata
                })
                .collect())
        })
    }));
    let harness = start_hooked_server(service, UnixServerOptions::default()).await;
    let client = connect(&harness.server).await;
    client.hello().await;
    attach(&client, "session-1").await;

    let listed = client
        .request(Command::List(ListCommand { command: ListTag }))
        .await;
    assert_eq!(
        sessions_of(ok(&listed)),
        &vec![SessionMetadata {
            id: "session-1".to_owned(),
            created_at: 1,
            updated_at: Some(1),
            parent_session_id: Some("parent-1".to_owned()),
            session_name: Some("Live name".to_owned()),
            cwd: Some("/tmp/notagent-server-conformance".to_owned()),
        }]
    );
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn keeps_multiple_attachments_on_one_connection_independent() {
    let service = TestServerService::new();
    service.seed("first");
    service.seed("second");
    let harness = start_server(service.clone(), UnixServerOptions::default()).await;
    let client = connect(&harness.server).await;
    client.hello().await;
    attach(&client, "first").await;
    attach(&client, "second").await;

    client.request(detach_command("first")).await;
    assert_eq!(service.latest_runtime("first").dispose_count(), 1);
    assert_eq!(service.latest_runtime("second").dispose_count(), 0);
    let response = client
        .request(Command::SetThinking(SetThinkingCommand {
            command: SetThinkingTag,
            session_id: "second".to_owned(),
            thinking_level: ThinkingLevel::Medium,
        }))
        .await;
    let session = session_of(ok(&response));
    assert_eq!(session.id, "second");
    assert_eq!(session.thinking_level, ThinkingLevel::Medium);
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn broadcasts_snapshots_and_progress_only_to_attached_clients() {
    let service = TestServerService::new();
    service.seed("session-1");
    let harness = start_server(service.clone(), UnixServerOptions::default()).await;
    let attached_client = connect(&harness.server).await;
    let unattached_client = connect(&harness.server).await;
    attached_client.hello().await;
    unattached_client.hello().await;
    attach(&attached_client, "session-1").await;
    let runtime = service.latest_runtime("session-1");
    let progress = TranscriptProgress::AssistantDelta(AssistantDeltaProgress {
        kind: AssistantDeltaTag,
        message_id: "assistant-1".to_owned(),
        content_index: 0,
        delta_kind: AssistantDeltaKind::Text,
        delta: "hello".to_owned(),
    });
    runtime.emit_progress(progress.clone());
    let progress_message = attached_client
        .next(Arc::new(|message: &ServerMessage| {
            matches!(message, ServerMessage::Event(event) if matches!(event.event, ServerEvent::SessionProgress(_)))
        }))
        .await
        .expect("progress");
    let ServerMessage::Event(event) = progress_message else {
        panic!("expected event")
    };
    let ServerEvent::SessionProgress(session_progress) = event.event else {
        panic!("expected progress")
    };
    assert_eq!(session_progress.session_id, "session-1");
    assert_eq!(session_progress.progress, progress);
    assert!(!unattached_client.messages().iter().any(|message| matches!(
        message,
        ServerMessage::Event(event) if matches!(event.event, ServerEvent::SessionProgress(_))
    )));

    let message_count = attached_client.messages().len();
    let expected_revision = runtime.stored_snapshot().revision;
    runtime.emit_snapshot();
    let snapshot_message = attached_client
        .next_from(
            message_count,
            snapshot_event_matching(move |snapshot| snapshot.revision == expected_revision),
        )
        .await
        .expect("snapshot");
    let ServerMessage::Event(event) = snapshot_message else {
        panic!("expected event")
    };
    let ServerEvent::SessionSnapshot(snapshot) = event.event else {
        panic!("expected snapshot")
    };
    assert_eq!(snapshot.snapshot.id, "session-1");
    assert!(snapshot.snapshot.attached);
    assert!(snapshot.snapshot.locked);
    assert!(!unattached_client.messages().iter().any(|message| matches!(
        message,
        ServerMessage::Event(event) if matches!(event.event, ServerEvent::SessionSnapshot(_))
    )));
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn allows_every_attached_client_to_control_a_singleton_live_runtime() {
    let service = TestServerService::new();
    service.seed("session-1");
    let harness = start_server(service.clone(), UnixServerOptions::default()).await;
    let first = connect(&harness.server).await;
    let second = connect(&harness.server).await;
    first.hello().await;
    second.hello().await;
    attach(&first, "session-1").await;
    let second_list = second
        .request(Command::List(ListCommand { command: ListTag }))
        .await;
    assert_eq!(
        sessions_of(ok(&second_list)),
        &vec![SessionMetadata {
            id: "session-1".to_owned(),
            created_at: 1,
            updated_at: Some(1),
            parent_session_id: None,
            session_name: Some("Session session-1".to_owned()),
            cwd: Some("/tmp/notagent-server-conformance".to_owned()),
        }]
    );
    attach(&second, "session-1").await;
    assert_eq!(service.runtime_count("session-1"), 1);

    let model_response = second
        .request(Command::SetModel(SetModelCommand {
            command: SetModelTag,
            session_id: "session-1".to_owned(),
            model: ModelRef::new("test", "large"),
        }))
        .await;
    assert_eq!(session_of(ok(&model_response)).model.id, "large");
    first
        .next(snapshot_event_matching(|snapshot| {
            snapshot.model.id == "large"
        }))
        .await
        .expect("snapshot");
    let thinking_response = first
        .request(Command::SetThinking(SetThinkingCommand {
            command: SetThinkingTag,
            session_id: "session-1".to_owned(),
            thinking_level: ThinkingLevel::High,
        }))
        .await;
    assert_eq!(
        session_of(ok(&thinking_response)).thinking_level,
        ThinkingLevel::High
    );
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn does_not_queue_prompts_and_processes_steer_and_abort_while_pending() {
    let service = TestServerService::new();
    service.seed("session-1");
    let harness = start_server(service.clone(), UnixServerOptions::default()).await;
    let client = connect(&harness.server).await;
    client.hello().await;
    attach(&client, "session-1").await;

    let prompt_client = Arc::clone(&client);
    let prompt = tokio::spawn(async move {
        prompt_client
            .request(Command::Prompt(PromptCommand {
                command: PromptTag,
                session_id: "session-1".to_owned(),
                text: "first".to_owned(),
            }))
            .await
    });
    client
        .next(snapshot_event_matching(|snapshot| {
            snapshot.phase == SessionPhase::Turn
        }))
        .await
        .expect("turn");
    let busy = client
        .request(Command::Prompt(PromptCommand {
            command: PromptTag,
            session_id: "session-1".to_owned(),
            text: "second".to_owned(),
        }))
        .await;
    let ResponseEnvelope::Error(error) = &busy else {
        panic!("expected busy error")
    };
    assert_eq!(error.error.code, notagent_protocol::ProtocolErrorCode::Busy);

    let steer = client
        .request(Command::Steer(SteerCommand {
            command: SteerTag,
            session_id: "session-1".to_owned(),
            text: "adjust".to_owned(),
        }))
        .await;
    assert!(matches!(ok(&steer), CommandResult::Steer(_)));
    assert_eq!(
        service
            .latest_runtime("session-1")
            .steers()
            .iter()
            .map(|steer| steer.text.clone())
            .collect::<Vec<_>>(),
        vec!["adjust".to_owned()]
    );
    let abort = client
        .request(Command::Abort(AbortCommand {
            command: AbortTag,
            session_id: "session-1".to_owned(),
        }))
        .await;
    assert!(matches!(ok(&abort), CommandResult::Abort(_)));
    let prompt = prompt.await.expect("task");
    assert_eq!(session_of(ok(&prompt)).phase, SessionPhase::Idle);
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn returns_operation_attachment_state_relative_to_the_requesting_connection() {
    let service = TestServerService::new();
    service.seed("session-1");
    let harness = start_server(service.clone(), UnixServerOptions::default()).await;
    let first = connect(&harness.server).await;
    let second = connect(&harness.server).await;
    first.hello().await;
    second.hello().await;
    attach(&first, "session-1").await;
    attach(&second, "session-1").await;

    let prompt_client = Arc::clone(&first);
    let prompt = tokio::spawn(async move {
        prompt_client
            .request(Command::Prompt(PromptCommand {
                command: PromptTag,
                session_id: "session-1".to_owned(),
                text: "hello".to_owned(),
            }))
            .await
    });
    first
        .next(snapshot_event_matching(|snapshot| {
            snapshot.phase == SessionPhase::Turn
        }))
        .await
        .expect("turn");
    first.request(detach_command("session-1")).await;
    service.latest_runtime("session-1").finish_prompt();

    let prompt = prompt.await.expect("task");
    let session = session_of(ok(&prompt));
    assert_eq!(session.id, "session-1");
    assert!(!session.attached);
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn keeps_busy_work_alive_after_disconnect_and_disposes_when_idle() {
    let service = TestServerService::new();
    service.seed("session-1");
    let harness = start_server(service.clone(), UnixServerOptions::default()).await;
    let client = connect(&harness.server).await;
    client.hello().await;
    attach(&client, "session-1").await;
    let prompt_client = Arc::clone(&client);
    let prompt = tokio::spawn(async move {
        prompt_client
            .request(Command::Prompt(PromptCommand {
                command: PromptTag,
                session_id: "session-1".to_owned(),
                text: "survive".to_owned(),
            }))
            .await
    });
    client
        .next(snapshot_event_matching(|snapshot| {
            snapshot.phase == SessionPhase::Turn
        }))
        .await
        .expect("turn");
    let runtime = service.latest_runtime("session-1");
    client.close().await.expect("closes");
    prompt.abort();
    assert_eq!(runtime.dispose_count(), 0);
    runtime.finish_prompt();
    runtime.disposed.promise().await;
    assert_eq!(runtime.dispose_count(), 1);

    let reconnect = connect(&harness.server).await;
    reconnect.hello().await;
    let snapshot = attach(&reconnect, "session-1").await;
    assert_eq!(snapshot.transcript.len(), 2);
    let TranscriptItem::Assistant(assistant) = &snapshot.transcript[1] else {
        panic!("expected assistant")
    };
    let notagent_protocol::AssistantTranscriptItem::Complete(complete) = assistant else {
        panic!("expected a complete assistant item")
    };
    let notagent_protocol::AssistantContent::Text(text) = &complete.content[0] else {
        panic!("expected text")
    };
    assert_eq!(text.text, "reply:survive");
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn restores_persisted_sessions_lazily_after_a_server_restart() {
    let service = TestServerService::new();
    service.seed("session-1");
    let first_harness = start_server(service.clone(), UnixServerOptions::default()).await;
    let first_client = connect(&first_harness.server).await;
    first_client.hello().await;
    attach(&first_client, "session-1").await;
    first_client
        .request(Command::SetThinking(SetThinkingCommand {
            command: SetThinkingTag,
            session_id: "session-1".to_owned(),
            thinking_level: ThinkingLevel::High,
        }))
        .await;
    first_client.close().await.expect("closes");
    first_harness.server.close().await.expect("closes");

    let second_harness = start_server(service.clone(), UnixServerOptions::default()).await;
    assert_eq!(service.runtime_count("session-1"), 1);
    let second_client = connect(&second_harness.server).await;
    second_client.hello().await;
    let restored = attach(&second_client, "session-1").await;
    assert_eq!(restored.thinking_level, ThinkingLevel::High);
    assert_eq!(service.runtime_count("session-1"), 2);
    second_harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn rejects_and_disposes_a_service_runtime_with_the_wrong_server_assigned_id() {
    let mut service = HookedService::new(TestServerService::new());
    service.create_id_override = Some("wrong-id".to_owned());
    let harness = start_hooked_server(service, UnixServerOptions::default()).await;
    let client = connect(&harness.server).await;
    client.hello().await;
    let response = client
        .request(Command::Create(CreateCommand {
            command: CreateTag,
            cwd: None,
            name: None,
            model: None,
            thinking_level: None,
        }))
        .await;
    let ResponseEnvelope::Error(error) = &response else {
        panic!("expected error")
    };
    assert_eq!(
        error.error.code,
        notagent_protocol::ProtocolErrorCode::InvalidRequest
    );
    assert_eq!(
        harness
            .service
            .inner
            .latest_runtime("wrong-id")
            .dispose_count(),
        1
    );
    harness.server.close().await.expect("closes");
}

#[tokio::test]
async fn maps_service_lock_errors_and_rejects_control_from_unattached_clients() {
    let service = TestServerService::new();
    service.seed("locked");
    service.lock_session("locked");
    let harness = start_server(service.clone(), UnixServerOptions::default()).await;
    let client = connect(&harness.server).await;
    client.hello().await;
    let locked = client.request(attach_command("locked")).await;
    let ResponseEnvelope::Error(error) = &locked else {
        panic!("expected error")
    };
    assert_eq!(
        error.error.code,
        notagent_protocol::ProtocolErrorCode::SessionLocked
    );
    let unattached = client
        .request(Command::Abort(AbortCommand {
            command: AbortTag,
            session_id: "locked".to_owned(),
        }))
        .await;
    let ResponseEnvelope::Error(error) = &unattached else {
        panic!("expected error")
    };
    assert_eq!(
        error.error.code,
        notagent_protocol::ProtocolErrorCode::InvalidRequest
    );
    harness.server.close().await.expect("closes");
}

/// Referenced to keep the unused-import guard honest.
#[allow(dead_code)]
fn unused(error: ServerError) -> String {
    error.to_string()
}
