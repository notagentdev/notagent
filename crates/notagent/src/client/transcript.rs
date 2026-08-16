//! 1:1 port of `packages/coding-agent/src/client/transcript.ts` (101 LOC).
//!
//! The snapshot the server sends stays authoritative; streamed progress is kept
//! beside it and only merged when the transcript is read (`select_transcript`).

use std::collections::{HashMap, HashSet};

use notagent_protocol::{
    AssistantContent, AssistantDeltaKind, AssistantDeltaProgress, AssistantTranscriptItem,
    FinishedTranscriptItem, JsonValue, SessionSnapshot, TranscriptItem, TranscriptProgress,
    UpdatedTranscriptItem,
};

/// `TranscriptState`.
///
/// Deviation class 1: the TS `ReadonlyMap`/`readonly` markers are compile-time
/// only; the Rust functions take the state by value and hand back a new one, so
/// the same "never mutate in place" contract holds.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptState {
    pub snapshot: SessionSnapshot,
    pub progress_items: HashMap<String, TranscriptItem>,
    pub progress_order: Vec<String>,
    pub tool_call_buffers: HashMap<String, String>,
}

/// `isJsonValue`.
///
/// Rust cannot build a `serde_json::Value` that carries a non-plain object or a
/// non-finite number, so only the finiteness guard has anything left to check —
/// it stays because `serde_json` does parse `1e999` into an infinite `f64`.
fn is_json_value(value: &JsonValue) -> bool {
    match value {
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::String(_) => true,
        JsonValue::Number(number) => number.as_f64().is_some_and(f64::is_finite),
        JsonValue::Array(items) => items.iter().all(is_json_value),
        JsonValue::Object(entries) => entries.values().all(is_json_value),
    }
}

/// `parsePartialToolInput` — tool arguments are incomplete while streaming, so
/// the raw prefix is preserved until it forms valid JSON.
fn parse_partial_tool_input(value: &str) -> JsonValue {
    if let Ok(parsed) = serde_json::from_str::<JsonValue>(value)
        && is_json_value(&parsed)
    {
        return parsed;
    }
    JsonValue::String(value.to_owned())
}

/// `createTranscriptState`.
pub fn create_transcript_state(snapshot: SessionSnapshot) -> TranscriptState {
    TranscriptState {
        snapshot,
        progress_items: HashMap::new(),
        progress_order: Vec::new(),
        tool_call_buffers: HashMap::new(),
    }
}

/// `applyTranscriptSnapshot` — a stale revision of the same session is ignored.
pub fn apply_transcript_snapshot(
    state: TranscriptState,
    snapshot: SessionSnapshot,
) -> TranscriptState {
    if state.snapshot.id == snapshot.id && snapshot.revision < state.snapshot.revision {
        return state;
    }
    create_transcript_state(snapshot)
}

/// `applyTranscriptProgress`.
pub fn apply_transcript_progress(
    state: TranscriptState,
    progress: &TranscriptProgress,
) -> TranscriptState {
    match progress {
        TranscriptProgress::ItemStarted(started) => set_progress_item(state, started.item.clone()),
        TranscriptProgress::ItemUpdated(updated) => {
            set_progress_item(state, updated_to_item(&updated.item))
        }
        TranscriptProgress::ItemFinished(finished) => {
            let item = finished_to_item(&finished.item);
            let prefix = format!("{}:", item_id(&item));
            let mut state = state;
            state
                .tool_call_buffers
                .retain(|key, _| !key.starts_with(&prefix));
            set_progress_item(state, item)
        }
        TranscriptProgress::AssistantDelta(delta) => apply_assistant_delta(state, delta),
    }
}

fn apply_assistant_delta(
    state: TranscriptState,
    progress: &AssistantDeltaProgress,
) -> TranscriptState {
    let item = state
        .progress_items
        .get(&progress.message_id)
        .or_else(|| {
            state
                .snapshot
                .transcript
                .iter()
                .find(|item| item_id(item) == progress.message_id)
        })
        .cloned();
    let Some(TranscriptItem::Assistant(assistant)) = item else {
        return state;
    };

    let mut state = state;
    let mut assistant = assistant;
    let content = assistant_content_mut(&mut assistant);
    let index = usize::try_from(progress.content_index).unwrap_or(usize::MAX);
    if let Some(part) = content.get_mut(index) {
        match (progress.delta_kind, part) {
            (AssistantDeltaKind::Text, AssistantContent::Text(text)) => {
                text.text.push_str(&progress.delta);
            }
            (AssistantDeltaKind::Thinking, AssistantContent::Thinking(thinking)) => {
                thinking.thinking.push_str(&progress.delta);
            }
            (AssistantDeltaKind::ToolCall, AssistantContent::ToolCall(tool_call)) => {
                let key = format!("{}:{}", progress.message_id, progress.content_index);
                let existing = state
                    .tool_call_buffers
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| match &tool_call.input {
                        JsonValue::String(raw) => raw.clone(),
                        _ => String::new(),
                    });
                let buffer = existing + &progress.delta;
                tool_call.input = parse_partial_tool_input(&buffer);
                state.tool_call_buffers.insert(key, buffer);
            }
            _ => {}
        }
    }

    set_progress_item(state, TranscriptItem::Assistant(assistant))
}

/// `selectTranscript` — the snapshot order wins, then progress-only items, then
/// the queued steering messages the server accepted.
pub fn select_transcript(state: &TranscriptState) -> Vec<TranscriptItem> {
    let mut transcript: Vec<TranscriptItem> = state
        .snapshot
        .transcript
        .iter()
        .map(|item| {
            state
                .progress_items
                .get(item_id(item))
                .cloned()
                .unwrap_or_else(|| item.clone())
        })
        .collect();
    let mut ids: HashSet<String> = transcript
        .iter()
        .map(|item| item_id(item).to_owned())
        .collect();

    for id in &state.progress_order {
        if ids.contains(id) {
            continue;
        }
        if let Some(item) = state.progress_items.get(id) {
            transcript.push(item.clone());
            ids.insert(id.clone());
        }
    }
    for item in &state.snapshot.queued_steer {
        if ids.contains(&item.id) {
            continue;
        }
        transcript.push(TranscriptItem::User(item.clone()));
        ids.insert(item.id.clone());
    }
    transcript
}

fn set_progress_item(mut state: TranscriptState, item: TranscriptItem) -> TranscriptState {
    let id = item_id(&item).to_owned();
    if !state.progress_items.contains_key(&id) {
        state.progress_order.push(id.clone());
    }
    state.progress_items.insert(id, item);
    state
}

/// The untagged protocol enums carry the id in every variant; TS reads
/// `item.id` straight off the union.
pub fn item_id(item: &TranscriptItem) -> &str {
    match item {
        TranscriptItem::User(user) => &user.id,
        TranscriptItem::Assistant(assistant) => match assistant {
            AssistantTranscriptItem::Streaming(inner) => &inner.id,
            AssistantTranscriptItem::Complete(inner) => &inner.id,
            AssistantTranscriptItem::Error(inner) => &inner.id,
            AssistantTranscriptItem::Aborted(inner) => &inner.id,
        },
        TranscriptItem::Tool(tool) => match tool {
            notagent_protocol::ToolTranscriptItem::Running(inner) => &inner.id,
            notagent_protocol::ToolTranscriptItem::Complete(inner) => &inner.id,
            notagent_protocol::ToolTranscriptItem::Error(inner) => &inner.id,
        },
    }
}

fn assistant_content_mut(item: &mut AssistantTranscriptItem) -> &mut Vec<AssistantContent> {
    match item {
        AssistantTranscriptItem::Streaming(inner) => &mut inner.content,
        AssistantTranscriptItem::Complete(inner) => &mut inner.content,
        AssistantTranscriptItem::Error(inner) => &mut inner.content,
        AssistantTranscriptItem::Aborted(inner) => &mut inner.content,
    }
}

fn updated_to_item(item: &UpdatedTranscriptItem) -> TranscriptItem {
    match item {
        UpdatedTranscriptItem::Assistant(assistant) => TranscriptItem::Assistant(assistant.clone()),
        UpdatedTranscriptItem::Tool(tool) => TranscriptItem::Tool(tool.clone()),
    }
}

fn finished_to_item(item: &FinishedTranscriptItem) -> TranscriptItem {
    match item {
        FinishedTranscriptItem::CompleteAssistant(inner) => {
            TranscriptItem::Assistant(AssistantTranscriptItem::Complete(inner.clone()))
        }
        FinishedTranscriptItem::ErrorAssistant(inner) => {
            TranscriptItem::Assistant(AssistantTranscriptItem::Error(inner.clone()))
        }
        FinishedTranscriptItem::AbortedAssistant(inner) => {
            TranscriptItem::Assistant(AssistantTranscriptItem::Aborted(inner.clone()))
        }
        FinishedTranscriptItem::CompleteTool(inner) => TranscriptItem::Tool(
            notagent_protocol::ToolTranscriptItem::Complete(inner.clone()),
        ),
        FinishedTranscriptItem::ErrorTool(inner) => {
            TranscriptItem::Tool(notagent_protocol::ToolTranscriptItem::Error(inner.clone()))
        }
    }
}
