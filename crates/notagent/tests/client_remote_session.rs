//! Port of `packages/coding-agent/test/client/remote-session.test.ts` (196 LOC).

mod client_support;

use std::sync::{Arc, Mutex};

use client_support::{
    MemoryServer, abort_result, attach_result, collect_requests, connect_client, ok_response,
    open_remote_session, progress_event, prompt_result, request_session_id, request_text,
    session_removed_event, session_snapshot, snapshot_event, steer_result,
};
use notagent::client::{RemoteSessionLifecycle, RemoteSessionOperation, RemoteSessionOptions};
use notagent_protocol::{
    AssistantContent, AssistantDeltaKind, AssistantDeltaProgress, AssistantDeltaTag, AssistantTag,
    AssistantTranscriptItem, ModelRef, SessionPhase, StreamingAssistantTranscriptItem,
    StreamingTag, TextContent, TranscriptItem, TranscriptProgress,
};

fn streaming_assistant(text: &str) -> TranscriptItem {
    TranscriptItem::Assistant(AssistantTranscriptItem::Streaming(
        StreamingAssistantTranscriptItem {
            id: "assistant-1".to_owned(),
            role: AssistantTag,
            content: vec![AssistantContent::Text(TextContent::new(text))],
            model: ModelRef::new("faux", "model"),
            response_model: None,
            usage: None,
            timestamp: 1,
            status: StreamingTag,
        },
    ))
}

fn first_assistant_text(item: Option<&TranscriptItem>) -> Option<String> {
    match item? {
        TranscriptItem::Assistant(AssistantTranscriptItem::Streaming(inner)) => {
            match inner.content.first()? {
                AssistantContent::Text(text) => Some(text.text.clone()),
                _ => None,
            }
        }
        _ => None,
    }
}

#[tokio::test]
async fn projects_progress_for_subscribers_without_changing_the_authoritative_snapshot() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    let mut snapshot = session_snapshot("session-1");
    snapshot.phase = SessionPhase::Turn;
    snapshot.transcript = vec![streaming_assistant("hello")];
    let remote_session =
        open_remote_session(&client, &server, snapshot, RemoteSessionOptions::default()).await;

    let views = Arc::new(Mutex::new(Vec::<String>::new()));
    let collected = Arc::clone(&views);
    let _unsubscribe = remote_session
        .subscribe(Arc::new(move |state| {
            if let Some(text) = first_assistant_text(state.transcript.first()) {
                collected.lock().expect("views mutex").push(text);
            }
        }))
        .expect("subscribes");

    server.send(&progress_event(
        "session-1",
        TranscriptProgress::AssistantDelta(AssistantDeltaProgress {
            kind: AssistantDeltaTag,
            message_id: "assistant-1".to_owned(),
            content_index: 0,
            delta_kind: AssistantDeltaKind::Text,
            delta: " world".to_owned(),
        }),
    ));

    assert_eq!(
        *views.lock().expect("views mutex"),
        vec!["hello".to_owned(), "hello world".to_owned()]
    );
    let snapshot = remote_session.snapshot().expect("a snapshot");
    assert_eq!(
        first_assistant_text(snapshot.transcript.first()),
        Some("hello".to_owned())
    );
}

#[tokio::test]
async fn becomes_unbound_and_can_reopen_after_its_session_is_removed() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    let remote_session = open_remote_session(
        &client,
        &server,
        session_snapshot("session-1"),
        RemoteSessionOptions::default(),
    )
    .await;

    server.send(&session_removed_event("session-1"));

    assert_eq!(remote_session.id(), None);
    assert_eq!(remote_session.snapshot(), None);
    assert!(remote_session.state().transcript.is_empty());
    assert_eq!(
        remote_session.state().lifecycle,
        RemoteSessionLifecycle::Unbound
    );

    let requests = collect_requests(&server);
    let reopening = remote_session.open("session-1");
    let request = requests.last().expect("Missing attach request");
    assert_eq!(client_support::command_name(&request), "attach");
    assert_eq!(request_session_id(&request).as_deref(), Some("session-1"));
    let mut reopened = session_snapshot("session-1");
    reopened.revision = 2;
    server.send(&ok_response(&request.id, attach_result(reopened)));
    reopening.await.expect("reopens");
    assert_eq!(
        remote_session.state().lifecycle,
        RemoteSessionLifecycle::Ready
    );
}

#[tokio::test]
async fn exposes_the_active_operation_while_prompting() {
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

    let lifecycles = Arc::new(Mutex::new(Vec::<String>::new()));
    let collected = Arc::clone(&lifecycles);
    let _unsubscribe = remote_session
        .subscribe(Arc::new(move |state| {
            let label = match state.lifecycle {
                RemoteSessionLifecycle::Busy { operation } => {
                    format!("busy:{}", operation.as_str())
                }
                RemoteSessionLifecycle::Ready => "ready".to_owned(),
                RemoteSessionLifecycle::Unbound => "unbound".to_owned(),
                RemoteSessionLifecycle::Disposed => "disposed".to_owned(),
            };
            collected.lock().expect("lifecycles mutex").push(label);
        }))
        .expect("subscribes");

    let prompting = remote_session.submit("  first prompt  ");
    let request = requests.last().expect("Missing prompt request");
    assert_eq!(client_support::command_name(&request), "prompt");
    assert_eq!(request_session_id(&request).as_deref(), Some("session-1"));
    assert_eq!(request_text(&request).as_deref(), Some("first prompt"));
    assert_eq!(
        remote_session.operation(),
        Some(RemoteSessionOperation::Submit)
    );
    let mut answered = session_snapshot("session-1");
    answered.revision = 2;
    answered.phase = SessionPhase::Turn;
    server.send(&ok_response(&request.id, prompt_result(answered)));
    prompting.await.expect("prompts");

    assert_eq!(
        *lifecycles.lock().expect("lifecycles mutex"),
        vec![
            "ready".to_owned(),
            "busy:submit".to_owned(),
            "busy:submit".to_owned(),
            "ready".to_owned()
        ]
    );
    assert_eq!(
        remote_session.state().lifecycle,
        RemoteSessionLifecycle::Ready
    );
}

#[tokio::test]
async fn steers_when_the_server_session_is_in_a_turn() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    let mut snapshot = session_snapshot("session-1");
    snapshot.phase = SessionPhase::Turn;
    let remote_session =
        open_remote_session(&client, &server, snapshot, RemoteSessionOptions::default()).await;
    let requests = collect_requests(&server);

    let steering = remote_session.submit("adjust");
    let request = requests.last().expect("Missing steer request");
    assert_eq!(client_support::command_name(&request), "steer");
    assert_eq!(request_session_id(&request).as_deref(), Some("session-1"));
    assert_eq!(request_text(&request).as_deref(), Some("adjust"));
    let mut answered = session_snapshot("session-1");
    answered.revision = 2;
    answered.phase = SessionPhase::Turn;
    server.send(&ok_response(&request.id, steer_result(answered)));
    steering.await.expect("steers");
}

#[tokio::test]
async fn aborts_while_a_prompt_response_is_pending() {
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

    let prompting = remote_session.submit("hello");
    let prompt_request = requests.last().expect("Missing prompt request");
    let mut turned = session_snapshot("session-1");
    turned.revision = 2;
    turned.phase = SessionPhase::Turn;
    server.send(&snapshot_event(turned));

    let aborting = remote_session.abort();
    let abort_request = requests.last().expect("Missing abort request");
    assert_eq!(client_support::command_name(&abort_request), "abort");
    assert_eq!(
        request_session_id(&abort_request).as_deref(),
        Some("session-1")
    );
    assert_eq!(
        remote_session.operation(),
        Some(RemoteSessionOperation::Abort)
    );
    let mut prompted = session_snapshot("session-1");
    prompted.revision = 3;
    prompted.phase = SessionPhase::Turn;
    server.send(&ok_response(&prompt_request.id, prompt_result(prompted)));
    prompting.await.expect("prompts");
    assert_eq!(
        remote_session.operation(),
        Some(RemoteSessionOperation::Abort)
    );
    let mut aborted = session_snapshot("session-1");
    aborted.revision = 4;
    server.send(&ok_response(&abort_request.id, abort_result(aborted)));
    aborting.await.expect("aborts");
    assert_eq!(
        remote_session.state().lifecycle,
        RemoteSessionLifecycle::Ready
    );
}

#[tokio::test]
async fn rejects_conflicting_operations_while_locally_busy() {
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

    let prompting = remote_session.submit("hello");
    let thinking = remote_session
        .set_thinking(notagent_protocol::ThinkingLevel::High)
        .await
        .expect_err("refuses");
    assert_eq!(thinking.to_string(), "Remote session is busy with submit");
    let opening = remote_session.open("session-2").await.expect_err("refuses");
    assert_eq!(opening.to_string(), "Remote session is busy with submit");

    let request = requests.last().expect("Missing prompt request");
    let mut answered = session_snapshot("session-1");
    answered.revision = 2;
    answered.phase = SessionPhase::Turn;
    server.send(&ok_response(&request.id, prompt_result(answered)));
    prompting.await.expect("prompts");
}

#[tokio::test]
async fn reports_subscriber_failures_without_interrupting_other_subscribers() {
    let server = MemoryServer::new();
    let client = connect_client(&server).await;
    let listener_errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let collected = Arc::clone(&listener_errors);
    let options = RemoteSessionOptions {
        on_listener_error: Some(Arc::new(move |error| {
            collected
                .lock()
                .expect("listener errors mutex")
                .push(error.to_string());
        })),
    };
    let remote_session =
        open_remote_session(&client, &server, session_snapshot("session-1"), options).await;

    // The panic below is the port of a throwing listener; keep it off stderr.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let _failing = remote_session
        .subscribe(Arc::new(|_| panic!("render failed")))
        .expect("subscribes");
    std::panic::set_hook(previous_hook);

    let notified = Arc::new(Mutex::new(false));
    let flag = Arc::clone(&notified);
    let _working = remote_session
        .subscribe(Arc::new(move |_| {
            *flag.lock().expect("notified mutex") = true;
        }))
        .expect("subscribes");

    assert_eq!(
        *listener_errors.lock().expect("listener errors mutex"),
        vec!["render failed".to_owned()]
    );
    assert!(*notified.lock().expect("notified mutex"));
}
