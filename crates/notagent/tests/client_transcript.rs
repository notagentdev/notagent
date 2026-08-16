//! Port of `packages/coding-agent/test/client/transcript.test.ts` (220 LOC).
//!
//! Same eight cases in source order, same expectations.

use notagent::client::{
    apply_transcript_progress, apply_transcript_snapshot, create_transcript_state,
    select_transcript,
};
use notagent_protocol::{
    AssistantContent, AssistantDeltaKind, AssistantDeltaProgress, AssistantDeltaTag, AssistantTag,
    AssistantTranscriptItem, ItemStartedProgress, ItemStartedTag, ItemUpdatedProgress,
    ItemUpdatedTag, JsonValue, ModelRef, RunningToolTranscriptItem, SessionPhase, SessionSnapshot,
    StreamingAssistantTranscriptItem, StreamingTag, TextContent, ThinkingLevel, ToolCallContent,
    ToolCallTag, ToolTag, ToolTranscriptItem, TranscriptItem, TranscriptProgress,
    UpdatedTranscriptItem, UserContent, UserTag, UserTranscriptItem,
};

fn model() -> ModelRef {
    ModelRef::new("faux", "faux-1")
}

fn streaming_assistant(content: Vec<AssistantContent>) -> TranscriptItem {
    TranscriptItem::Assistant(AssistantTranscriptItem::Streaming(
        StreamingAssistantTranscriptItem {
            id: "assistant-1".to_owned(),
            role: AssistantTag,
            content,
            model: model(),
            response_model: None,
            usage: None,
            timestamp: 1,
            status: StreamingTag,
        },
    ))
}

/// The `snapshot(revision, text)` helper of the TS suite.
fn snapshot(revision: u64, text: &str) -> SessionSnapshot {
    SessionSnapshot {
        id: "session-1".to_owned(),
        name: None,
        cwd: "/workspace".to_owned(),
        created_at: 1,
        updated_at: revision + 1,
        phase: SessionPhase::Turn,
        model: model(),
        thinking_level: ThinkingLevel::Off,
        attached: true,
        locked: true,
        revision,
        transcript: vec![streaming_assistant(vec![AssistantContent::Text(
            TextContent::new(text),
        )])],
        queued_steer: vec![],
        queued_steer_count: 0,
    }
}

fn tool_call(input: JsonValue) -> AssistantContent {
    AssistantContent::ToolCall(ToolCallContent {
        kind: ToolCallTag,
        tool_call_id: "call-1".to_owned(),
        tool_name: "bash".to_owned(),
        input,
    })
}

fn delta(kind: AssistantDeltaKind, text: &str) -> TranscriptProgress {
    TranscriptProgress::AssistantDelta(AssistantDeltaProgress {
        kind: AssistantDeltaTag,
        message_id: "assistant-1".to_owned(),
        content_index: 0,
        delta_kind: kind,
        delta: text.to_owned(),
    })
}

fn assistant_content(item: &TranscriptItem) -> &[AssistantContent] {
    match item {
        TranscriptItem::Assistant(AssistantTranscriptItem::Streaming(inner)) => &inner.content,
        other => panic!("expected a streaming assistant item, got {other:?}"),
    }
}

fn text_of(item: &TranscriptItem) -> &str {
    match &assistant_content(item)[0] {
        AssistantContent::Text(text) => &text.text,
        other => panic!("expected text content, got {other:?}"),
    }
}

fn tool_input(item: &TranscriptItem) -> &JsonValue {
    match &assistant_content(item)[0] {
        AssistantContent::ToolCall(call) => &call.input,
        other => panic!("expected tool call content, got {other:?}"),
    }
}

fn running_tool(content: Vec<notagent_protocol::ToolContent>) -> TranscriptItem {
    TranscriptItem::Tool(ToolTranscriptItem::Running(RunningToolTranscriptItem {
        id: "tool-call-1".to_owned(),
        role: ToolTag,
        tool_call_id: "call-1".to_owned(),
        tool_name: "bash".to_owned(),
        input: serde_json::json!({ "command": "printf hi" }),
        content,
        details: None,
        usage: None,
        timestamp: 2,
        status: notagent_protocol::RunningTag,
        is_error: notagent_protocol::FalseTag,
    }))
}

#[test]
fn projects_progress_without_mutating_the_authoritative_snapshot() {
    let mut state = create_transcript_state(snapshot(1, "saved"));
    state = apply_transcript_progress(state, &delta(AssistantDeltaKind::Text, " response"));

    assert_eq!(text_of(&state.snapshot.transcript[0]), "saved");
    assert_eq!(text_of(&select_transcript(&state)[0]), "saved response");
}

#[test]
fn applies_streamed_tool_call_argument_deltas() {
    let mut base = snapshot(1, "saved");
    base.transcript = vec![streaming_assistant(vec![tool_call(JsonValue::Null)])];
    let mut state = create_transcript_state(base);

    state = apply_transcript_progress(state, &delta(AssistantDeltaKind::ToolCall, "{\"command\":"));
    assert_eq!(
        tool_input(&select_transcript(&state)[0]),
        &JsonValue::String("{\"command\":".to_owned())
    );

    state = apply_transcript_progress(
        state,
        &TranscriptProgress::ItemUpdated(ItemUpdatedProgress {
            kind: ItemUpdatedTag,
            item: UpdatedTranscriptItem::Assistant(AssistantTranscriptItem::Streaming(
                StreamingAssistantTranscriptItem {
                    id: "assistant-1".to_owned(),
                    role: AssistantTag,
                    content: vec![tool_call(JsonValue::Null)],
                    model: model(),
                    response_model: None,
                    usage: None,
                    timestamp: 1,
                    status: StreamingTag,
                },
            )),
        }),
    );
    state = apply_transcript_progress(state, &delta(AssistantDeltaKind::ToolCall, "\"pwd\"}"));

    assert_eq!(
        tool_input(&select_transcript(&state)[0]),
        &serde_json::json!({ "command": "pwd" })
    );
}

#[test]
fn appends_tool_call_deltas_to_a_partial_input_restored_from_a_snapshot() {
    let mut base = snapshot(1, "saved");
    base.transcript = vec![streaming_assistant(vec![tool_call(JsonValue::String(
        "{\"command\":".to_owned(),
    ))])];
    let mut state = create_transcript_state(base);

    state = apply_transcript_progress(state, &delta(AssistantDeltaKind::ToolCall, "\"pwd\"}"));

    assert_eq!(
        tool_input(&select_transcript(&state)[0]),
        &serde_json::json!({ "command": "pwd" })
    );
}

#[test]
fn appends_transient_tool_progress_and_replaces_it_by_id() {
    let mut state = create_transcript_state(snapshot(1, "saved"));
    state = apply_transcript_progress(
        state,
        &TranscriptProgress::ItemStarted(ItemStartedProgress {
            kind: ItemStartedTag,
            item: running_tool(vec![]),
        }),
    );

    let transcript = select_transcript(&state);
    let last = transcript.last().expect("an item");
    match last {
        TranscriptItem::Tool(ToolTranscriptItem::Running(tool)) => {
            assert_eq!(tool.id, "tool-call-1");
            assert!(tool.content.is_empty());
        }
        other => panic!("expected a running tool item, got {other:?}"),
    }

    state = apply_transcript_progress(
        state,
        &TranscriptProgress::ItemUpdated(ItemUpdatedProgress {
            kind: ItemUpdatedTag,
            item: UpdatedTranscriptItem::Tool(
                match running_tool(vec![notagent_protocol::ToolContent::Text(
                    TextContent::new("hi"),
                )]) {
                    TranscriptItem::Tool(tool) => tool,
                    other => panic!("expected a tool item, got {other:?}"),
                },
            ),
        }),
    );

    let transcript = select_transcript(&state);
    assert_eq!(transcript.len(), 2);
    match &transcript[1] {
        TranscriptItem::Tool(ToolTranscriptItem::Running(tool)) => {
            assert_eq!(
                tool.content,
                vec![notagent_protocol::ToolContent::Text(TextContent::new("hi"))]
            );
        }
        other => panic!("expected a running tool item, got {other:?}"),
    }
}

#[test]
fn resets_revision_history_when_the_same_session_runtime_is_reacquired() {
    let _state = create_transcript_state(snapshot(50, "old runtime"));
    let state = create_transcript_state(snapshot(0, "new runtime"));

    assert_eq!(state.snapshot.revision, 0);
    assert_eq!(text_of(&select_transcript(&state)[0]), "new runtime");
}

#[test]
fn accepts_a_lower_revision_when_switching_to_a_different_session() {
    let state = create_transcript_state(snapshot(50, "old session"));
    let mut next = snapshot(0, "new session");
    next.id = "session-2".to_owned();
    let state = apply_transcript_snapshot(state, next);

    assert_eq!(state.snapshot.id, "session-2");
    assert_eq!(text_of(&select_transcript(&state)[0]), "new session");
}

#[test]
fn renders_accepted_steering_messages_from_authoritative_queued_state() {
    let mut base = snapshot(2, "saved");
    base.queued_steer_count = 1;
    base.queued_steer = vec![UserTranscriptItem {
        id: "user-steer".to_owned(),
        role: UserTag,
        content: vec![UserContent::Text(TextContent::new("adjust the approach"))],
        timestamp: 2,
    }];
    let state = create_transcript_state(base);

    let transcript = select_transcript(&state);
    match transcript.last().expect("an item") {
        TranscriptItem::User(user) => {
            assert_eq!(
                user.content,
                vec![UserContent::Text(TextContent::new("adjust the approach"))]
            );
        }
        other => panic!("expected a user item, got {other:?}"),
    }
}

#[test]
fn a_newer_snapshot_is_authoritative_and_stale_snapshots_are_ignored() {
    let mut state = create_transcript_state(snapshot(3, "new"));
    state = apply_transcript_progress(state, &delta(AssistantDeltaKind::Text, " transient"));
    state = apply_transcript_snapshot(state, snapshot(4, "authoritative"));
    state = apply_transcript_snapshot(state, snapshot(2, "stale"));

    assert_eq!(state.snapshot.revision, 4);
    assert_eq!(text_of(&select_transcript(&state)[0]), "authoritative");
}
