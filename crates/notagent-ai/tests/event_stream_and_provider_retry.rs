//! Ports of `packages/ai/test/provider-retry.test.ts` (81 LOC) plus the behaviour of
//! `EventStream` from `packages/ai/src/utils/event-stream.ts`, which the TS suites cover
//! indirectly through every provider stream test.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use notagent_ai::types::*;
use notagent_ai::utils::event_stream::{EventStream, create_assistant_message_event_stream};
use notagent_ai::utils::provider_retry::{
    ProviderErrorInfo, ProviderRetryError, ProviderRetryOptions, retry_provider_request,
};

/// `providerError(status, headers)`
#[derive(Debug, Clone)]
struct TestProviderError {
    status: Option<u16>,
    headers: Vec<(String, String)>,
}

impl TestProviderError {
    fn new(status: Option<u16>, headers: &[(&str, &str)]) -> Self {
        TestProviderError {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
        }
    }
}

impl ProviderErrorInfo for TestProviderError {
    fn status(&self) -> Option<u16> {
        self.status
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    fn message(&self) -> String {
        format!(
            "Provider error: {}",
            self.status
                .map(|status| status.to_string())
                .unwrap_or_default()
        )
    }
}

#[tokio::test(start_paused = true)]
async fn retries_retryable_provider_errors() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let result = retry_provider_request(
        || {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            async move {
                if call == 0 {
                    Err(TestProviderError::new(
                        Some(429),
                        &[("retry-after-ms", "1000")],
                    ))
                } else {
                    Ok("ok")
                }
            }
        },
        ProviderRetryOptions {
            max_retries: Some(1),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(result.unwrap(), "ok");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn does_not_retry_errors_marked_as_non_retryable() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let result: Result<&str, _> = retry_provider_request(
        || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                Err(TestProviderError::new(
                    Some(429),
                    &[("x-should-retry", "false")],
                ))
            }
        },
        ProviderRetryOptions {
            max_retries: Some(2),
            ..Default::default()
        },
    )
    .await;

    assert!(matches!(result, Err(ProviderRetryError::Request(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_a_provider_requested_retry_delay_above_the_limit() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let result: Result<&str, _> = retry_provider_request(
        || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                Err(TestProviderError::new(
                    Some(429),
                    &[("retry-after", "277403")],
                ))
            }
        },
        ProviderRetryOptions {
            max_retries: Some(1),
            max_retry_delay_ms: Some(1000),
            ..Default::default()
        },
    )
    .await;

    match result {
        Err(ProviderRetryError::RetryDelayTooLong(message)) => {
            assert!(
                message.starts_with("Server requested 277403s retry delay (max: 1s)"),
                "{message}"
            );
        }
        other => panic!("expected a delay error, got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn allows_disabling_the_retry_delay_cap() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let result = retry_provider_request(
        || {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            async move {
                if call == 0 {
                    Err(TestProviderError::new(Some(429), &[("retry-after", "2")]))
                } else {
                    Ok("ok")
                }
            }
        },
        ProviderRetryOptions {
            max_retries: Some(1),
            max_retry_delay_ms: Some(0),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(result.unwrap(), "ok");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn aborts_a_provider_requested_retry_delay() {
    let signal = tokio_util::sync::CancellationToken::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let aborting = signal.clone();
    let abort_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        aborting.cancel();
    });

    let result: Result<&str, _> = retry_provider_request(
        || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                Err(TestProviderError::new(
                    Some(429),
                    &[("retry-after", "277403")],
                ))
            }
        },
        ProviderRetryOptions {
            max_retries: Some(2),
            max_retry_delay_ms: Some(0),
            signal: Some(signal),
        },
    )
    .await;
    abort_task.await.unwrap();

    assert!(matches!(result, Err(ProviderRetryError::Aborted)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// EventStream
// ---------------------------------------------------------------------------

fn pending_message() -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        api: "faux".to_string(),
        provider: "faux".to_string(),
        model: "faux".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Pending,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

#[tokio::test]
async fn event_stream_queues_events_until_a_consumer_arrives() {
    let stream: EventStream<u32, u32> = EventStream::new(|event| *event == 99, |event| *event);
    stream.push(1);
    stream.push(2);
    assert_eq!(stream.next().await, Some(1));
    assert_eq!(stream.next().await, Some(2));

    stream.push(99);
    assert_eq!(stream.next().await, Some(99));
    assert_eq!(stream.result().await, 99);
    // Pushes after the completing event are ignored.
    stream.push(3);
    assert_eq!(stream.next().await, None);
}

#[tokio::test]
async fn event_stream_wakes_a_waiting_consumer() {
    let stream: EventStream<u32, u32> = EventStream::new(|event| *event == 0, |event| *event);
    let consumer = stream.clone();
    let waiting = tokio::spawn(async move { consumer.next().await });
    tokio::task::yield_now().await;
    stream.push(7);
    assert_eq!(waiting.await.unwrap(), Some(7));
}

#[tokio::test]
async fn event_stream_end_terminates_iteration() {
    let stream: EventStream<u32, u32> = EventStream::new(|event| *event == 0, |event| *event);
    let consumer = stream.clone();
    let waiting = tokio::spawn(async move { consumer.next().await });
    tokio::task::yield_now().await;
    stream.end(None);
    assert_eq!(waiting.await.unwrap(), None);
    assert!(stream.is_done());
}

#[tokio::test]
async fn assistant_message_event_stream_completes_on_done_and_error() {
    let stream = create_assistant_message_event_stream();
    let done_message = AssistantMessage {
        stop_reason: StopReason::Stop,
        ..pending_message()
    };
    stream.push(AssistantMessageEvent::Start {
        partial: pending_message(),
    });
    stream.push(AssistantMessageEvent::Done {
        reason: DoneReason::Stop,
        message: done_message.clone(),
    });
    assert_eq!(stream.result().await, done_message);

    let stream = create_assistant_message_event_stream();
    let error = AssistantMessage {
        stop_reason: StopReason::Error,
        error_message: Some("boom".to_string()),
        ..pending_message()
    };
    stream.push(AssistantMessageEvent::Error {
        reason: ErrorReason::Error,
        error: error.clone(),
    });
    assert_eq!(stream.result().await, error);
}
