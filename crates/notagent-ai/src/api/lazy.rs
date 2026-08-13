//! Streams returned synchronously while their setup runs behind them.
//!
//! 1:1 port of `packages/ai/src/api/lazy.ts` (98 LOC). `lazyApi` wraps a dynamically
//! imported module so bundlers can split it out; Rust links statically, so only the
//! stream-side behaviour (`lazyStream`) is needed and `lazyApi` is excluded
//! (deviation class 4, distribution mechanics — see PARITY.md).

use std::future::Future;

use crate::types::{
    AssistantMessage, AssistantMessageEvent, ErrorReason, Model, StopReason, Usage,
};
use crate::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};

/// `createSetupErrorMessage(model, error)`
pub fn create_setup_error_message(
    model: &Model,
    error: &dyn std::fmt::Display,
    timestamp: i64,
) -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        deferred: None,
        error_message: Some(error.to_string()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp,
    }
}

/// Forwards every event of `source` into `target` and ends it with the source result.
pub async fn forward_stream(
    target: &AssistantMessageEventStream,
    source: AssistantMessageEventStream,
) {
    while let Some(event) = source.next().await {
        target.push(event);
    }
    target.end(Some(source.result().await));
}

/// `lazyStream(model, setup)` — returns a stream synchronously while `setup` runs.
///
/// Setup failures terminate the stream with an error event, so the stream contract
/// ("nothing is thrown after the call") holds.
pub fn lazy_stream<F, Fut>(model: Model, setup: F) -> AssistantMessageEventStream
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<AssistantMessageEventStream, String>> + Send,
{
    let outer = create_assistant_message_event_stream();
    let stream = outer.clone();
    tokio::spawn(async move {
        match setup().await {
            Ok(inner) => forward_stream(&stream, inner).await,
            Err(error) => {
                let message =
                    create_setup_error_message(&model, &error, crate::auth::resolve::now_ms());
                stream.push(AssistantMessageEvent::Error {
                    reason: ErrorReason::Error,
                    error: message.clone(),
                });
                stream.end(Some(message));
            }
        }
    });
    outer
}
