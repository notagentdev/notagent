//! Ports of the utility test suites of `packages/ai/test`:
//! `text.test.ts` (33), `overflow.test.ts` (177), `node-http-proxy.test.ts` (76),
//! `context-estimate.test.ts` (81) and `retry.test.ts` (223).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notagent_ai::types::*;
use notagent_ai::utils::estimate::{calculate_context_tokens, estimate_context_tokens};
use notagent_ai::utils::node_http_proxy::{
    UNSUPPORTED_PROXY_PROTOCOL_MESSAGE, resolve_http_proxy_url_for_target,
};
use notagent_ai::utils::overflow::{is_context_overflow, is_recoverable_length};
use notagent_ai::utils::retry::{
    RetryCallbacks, RetryPolicy, is_retryable_assistant_error, retry_assistant_call,
};
use notagent_ai::utils::text::content_text_with;

/// Stand-in for `fauxAssistantMessage` until the faux provider lands in task 11.
fn assistant_message(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: if text.is_empty() {
            vec![]
        } else {
            vec![AssistantContent::Text(TextContent::new(text))]
        },
        api: "openai-completions".to_string(),
        provider: "faux".to_string(),
        model: "faux".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

fn error_message(message: &str) -> AssistantMessage {
    AssistantMessage {
        stop_reason: StopReason::Error,
        error_message: Some(message.to_string()),
        ..assistant_message("")
    }
}

// ---------------------------------------------------------------------------
// text.test.ts
// ---------------------------------------------------------------------------

#[test]
fn content_text_extracts_assistant_text_blocks() {
    let content = vec![
        AssistantContent::Thinking(ThinkingContent {
            thinking: "reasoning".to_string(),
            ..Default::default()
        }),
        AssistantContent::Text(TextContent::new("first")),
        AssistantContent::ToolCall(ToolCall {
            id: "1".to_string(),
            name: "read".to_string(),
            ..Default::default()
        }),
        AssistantContent::Text(TextContent::new("second")),
    ];
    assert_eq!(content_text_with(&content, "\n"), "first\nsecond");
    assert_eq!(content_text_with(&content, ""), "firstsecond");
}

#[test]
fn content_text_passes_string_content_through() {
    assert_eq!(content_text_with("hello", "\n"), "hello");
}

#[test]
fn content_text_extracts_text_from_tool_result_content() {
    let content = vec![
        TextOrImageContent::Text(TextContent::new("first")),
        TextOrImageContent::Image(ImageContent {
            data: "...".to_string(),
            mime_type: "image/png".to_string(),
        }),
        TextOrImageContent::Text(TextContent::new("second")),
    ];
    assert_eq!(content_text_with(&content, ""), "firstsecond");
}

// ---------------------------------------------------------------------------
// overflow.test.ts
// ---------------------------------------------------------------------------

fn length_stop_message(
    input: u64,
    cache_read: u64,
    output: u64,
    cache_write: u64,
) -> AssistantMessage {
    AssistantMessage {
        usage: Usage {
            input,
            output,
            cache_read,
            cache_write,
            cache_write1h: None,
            reasoning: None,
            total_tokens: Some(input + cache_read + cache_write + output),
            cost: UsageCost::default(),
        },
        stop_reason: StopReason::Length,
        ..assistant_message("")
    }
}

#[test]
fn detects_provider_specific_overflow_errors() {
    let cases: &[(&str, u64)] = &[
        (
            "400 `prompt too long; exceeded max context length by 100918 tokens`",
            32768,
        ),
        (
            "400 The input (516368 tokens) is longer than the model's context length (262144 tokens).",
            262144,
        ),
        (
            "Error: 503 litellm.ServiceUnavailableError: litellm.MidStreamFallbackError: litellm.APIConnectionError: APIConnectionError: OpenAIException - Requested token count exceeds the model's maximum context length of 131072 tokens.",
            131072,
        ),
        (
            "Error: 400 Input length (265330) exceeds model's maximum context length (262144).",
            262144,
        ),
        (
            "Provider returned error: Input length 131393 exceeds the maximum allowed input length of 131040 tokens.",
            131072,
        ),
        (
            "400 Prompt has 256468 tokens, but the configured context size is 256000 tokens",
            256000,
        ),
        (
            "Prompt has 5,958,968 tokens, but the configured context size is 256,000 tokens",
            256000,
        ),
    ];
    for (message, context_window) in cases {
        assert!(
            is_context_overflow(&error_message(message), Some(*context_window)),
            "{message}"
        );
    }
}

#[test]
fn does_not_treat_non_overflow_errors_as_overflow() {
    let cases: &[(&str, u64)] = &[
        ("500 `model runner crashed unexpectedly`", 32768),
        (
            "Throttling error: Too many tokens, please wait before trying again.",
            200000,
        ),
        (
            "Service unavailable: The service is temporarily unavailable.",
            200000,
        ),
        (
            "Rate limit exceeded, please retry after 30 seconds.",
            200000,
        ),
        ("Too many requests. Please slow down.", 200000),
    ];
    for (message, context_window) in cases {
        assert!(
            !is_context_overflow(&error_message(message), Some(*context_window)),
            "{message}"
        );
    }
}

#[test]
fn detects_xiaomi_style_length_stop_overflow() {
    let message = length_stop_message(58, 1_048_512, 0, 0);
    assert!(is_context_overflow(&message, Some(1_048_576)));
}

#[test]
fn recoverable_length_stops() {
    assert!(is_recoverable_length(
        &length_stop_message(3, 253_584, 16, 25_554),
        128_000
    ));
    assert!(!is_recoverable_length(
        &length_stop_message(4062, 0, 1024, 0),
        1024
    ));
    assert!(is_recoverable_length(
        &length_stop_message(100, 0, 0, 0),
        128_000
    ));
    assert!(!is_context_overflow(
        &length_stop_message(1000, 0, 4096, 0),
        Some(200_000)
    ));
    assert!(!is_context_overflow(
        &length_stop_message(100, 0, 0, 0),
        Some(200_000)
    ));
}

// ---------------------------------------------------------------------------
// node-http-proxy.test.ts
// ---------------------------------------------------------------------------

fn proxy_env(pairs: &[(&str, &str)]) -> ProviderEnv {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<BTreeMap<_, _>>()
}

#[test]
fn respects_no_proxy_exclusions() {
    let env = proxy_env(&[
        ("HTTPS_PROXY", "http://proxy.example:8080"),
        ("NO_PROXY", "bedrock-runtime.us-east-1.amazonaws.com"),
    ]);
    assert_eq!(
        resolve_http_proxy_url_for_target(
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            Some(&env)
        ),
        Ok(None)
    );
}

#[test]
fn no_proxy_matches_case_insensitively_like_the_whatwg_parser() {
    // The WHATWG `URL` in TS lowercases scheme and host; the Rust parser must
    // do the same or `HTTPS://EXAMPLE.COM` slips past `no_proxy=example.com`.
    let env = proxy_env(&[
        ("HTTPS_PROXY", "http://proxy.example:8080"),
        ("NO_PROXY", "bedrock-runtime.us-east-1.amazonaws.com"),
    ]);
    assert_eq!(
        resolve_http_proxy_url_for_target(
            "HTTPS://BEDROCK-RUNTIME.US-EAST-1.AMAZONAWS.COM",
            Some(&env)
        ),
        Ok(None)
    );
}

#[test]
fn resolves_http_and_https_proxy_urls() {
    let env = proxy_env(&[("HTTPS_PROXY", "http://proxy.example:8080")]);
    assert_eq!(
        resolve_http_proxy_url_for_target(
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            Some(&env)
        ),
        Ok(Some("http://proxy.example:8080/".to_string()))
    );
}

#[test]
fn prefers_scoped_proxy_env_aliases() {
    // The TS test puts `https_proxy` into the process env and `HTTPS_PROXY` into the
    // scoped env; the scoped value wins.
    let env = proxy_env(&[("HTTPS_PROXY", "http://scoped-proxy.example:8080")]);
    assert_eq!(
        resolve_http_proxy_url_for_target(
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            Some(&env)
        ),
        Ok(Some("http://scoped-proxy.example:8080/".to_string()))
    );
}

#[test]
fn rejects_socks_and_pac_proxy_urls() {
    let env = proxy_env(&[("HTTPS_PROXY", "socks5://proxy.example:1080")]);
    let error = resolve_http_proxy_url_for_target(
        "https://bedrock-runtime.us-east-1.amazonaws.com",
        Some(&env),
    )
    .expect_err("must reject");
    assert!(
        error.starts_with(UNSUPPORTED_PROXY_PROTOCOL_MESSAGE),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// context-estimate.test.ts
// ---------------------------------------------------------------------------

fn usage_with_total(total_tokens: u64) -> Usage {
    Usage {
        input: total_tokens,
        total_tokens: Some(total_tokens),
        ..Usage::default()
    }
}

fn assistant_with_usage(timestamp: i64, total_tokens: u64) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![AssistantContent::Text(TextContent::new("kept"))],
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        model: "test-model".to_string(),
        usage: usage_with_total(total_tokens),
        stop_reason: StopReason::Stop,
        timestamp,
        ..assistant_message("")
    })
}

fn user_message(text: &str, timestamp: i64) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp,
    })
}

#[test]
fn ignores_stale_assistant_usage_after_a_newer_message_was_inserted() {
    let context = Context {
        system_prompt: Some("system".to_string()),
        messages: vec![
            user_message("summary", 200),
            assistant_with_usage(100, 9_500),
            user_message(&"x".repeat(4_000), 300),
        ],
        tools: None,
    };
    let estimate = estimate_context_tokens(&context);
    assert_eq!(estimate.tokens, 1_005);
    assert_eq!(estimate.usage_tokens, 0);
    assert_eq!(estimate.trailing_tokens, 1_005);
    assert_eq!(estimate.last_usage_index, None);
}

#[test]
fn uses_assistant_usage_again_after_a_response_to_the_inserted_context() {
    let context = Context {
        system_prompt: None,
        messages: vec![
            user_message("summary", 200),
            assistant_with_usage(100, 9_500),
            user_message("new prompt", 300),
            assistant_with_usage(400, 2_000),
            user_message("tail", 500),
        ],
        tools: None,
    };
    let estimate = estimate_context_tokens(&context);
    assert_eq!(estimate.tokens, 2_001);
    assert_eq!(estimate.usage_tokens, 2_000);
    assert_eq!(estimate.trailing_tokens, 1);
    assert_eq!(estimate.last_usage_index, Some(3));
}

#[test]
fn calculate_context_tokens_falls_back_to_the_sum() {
    let usage = Usage {
        input: 3,
        output: 4,
        cache_read: 5,
        cache_write: 6,
        total_tokens: None,
        ..Usage::default()
    };
    assert_eq!(calculate_context_tokens(&usage), 18);
    assert_eq!(
        calculate_context_tokens(&Usage {
            total_tokens: Some(0),
            ..usage
        }),
        18
    );
    assert_eq!(
        calculate_context_tokens(&Usage {
            total_tokens: Some(42),
            ..usage
        }),
        42
    );
}

// ---------------------------------------------------------------------------
// retry.test.ts
// ---------------------------------------------------------------------------

#[test]
fn classifies_provider_error_messages() {
    let retryable = [
        "An error occurred while processing your request. You can retry your request, or contact us through our help center at help.openai.com if the error persists. Please include the request ID req_******** in your message.",
        "{\"message\":\"The system encountered an unexpected error during processing. Try your request again.\"}",
        "ResourceExhausted: Worker local total request limit reached (288/48)",
        "The socket connection was closed unexpectedly. For more information, pass `verbose: true` in the second argument to fetch()",
        "Error: exceeded request buffer limit while retrying upstream",
        "The pending stream has been canceled (caused by: getaddrinfo ENOTFOUND bedrock-runtime.us-east-1.amazonaws.com)",
        "connect ENOTFOUND api.example.com",
        "EAI_AGAIN api.example.com",
        "getaddrinfo failed for api.example.com",
        "OpenAI Responses stream ended before a terminal response event",
        "overloaded_error",
        "524 status code (no body)",
    ];
    for message in retryable {
        assert!(
            is_retryable_assistant_error(&error_message(message)),
            "{message}"
        );
    }

    assert!(!is_retryable_assistant_error(&error_message(
        "429 quota exceeded"
    )));
    assert!(!is_retryable_assistant_error(&assistant_message(
        "not an error"
    )));
}

const DISABLED: RetryPolicy = RetryPolicy {
    enabled: false,
    max_retries: 3,
    base_delay_ms: 0,
};
const ENABLED: RetryPolicy = RetryPolicy {
    enabled: true,
    max_retries: 3,
    base_delay_ms: 0,
};

#[tokio::test]
async fn returns_a_successful_response_without_retrying() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let result = retry_assistant_call(
        || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { assistant_message("ok") }
        },
        Some(ENABLED),
        None,
        None,
    )
    .await;
    assert_eq!(
        result.content,
        vec![AssistantContent::Text(TextContent::new("ok"))]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn does_not_retry_an_aborted_message() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let scheduled = Arc::new(AtomicUsize::new(0));
    let scheduled_counter = Arc::clone(&scheduled);
    let callbacks = RetryCallbacks {
        on_retry_scheduled: Some(Arc::new(move |_, _, _, _| {
            scheduled_counter.fetch_add(1, Ordering::SeqCst);
        })),
        ..Default::default()
    };
    let result = retry_assistant_call(
        || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                AssistantMessage {
                    stop_reason: StopReason::Aborted,
                    ..assistant_message("")
                }
            }
        },
        Some(ENABLED),
        None,
        Some(&callbacks),
    )
    .await;
    assert_eq!(result.stop_reason, StopReason::Aborted);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(scheduled.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn does_not_retry_a_non_retryable_error() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let finished = Arc::new(AtomicUsize::new(0));
    let finished_counter = Arc::clone(&finished);
    let callbacks = RetryCallbacks {
        on_retry_finished: Some(Arc::new(move |_, _, _| {
            finished_counter.fetch_add(1, Ordering::SeqCst);
        })),
        ..Default::default()
    };
    let result = retry_assistant_call(
        || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { error_message("insufficient_quota") }
        },
        Some(ENABLED),
        None,
        Some(&callbacks),
    )
    .await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(finished.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn retries_a_transient_error_up_to_max_retries() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let finished = Arc::new(std::sync::Mutex::new(Vec::new()));
    let finished_sink = Arc::clone(&finished);
    let scheduled = Arc::new(AtomicUsize::new(0));
    let scheduled_counter = Arc::clone(&scheduled);
    let callbacks = RetryCallbacks {
        on_retry_scheduled: Some(Arc::new(move |_, _, _, _| {
            scheduled_counter.fetch_add(1, Ordering::SeqCst);
        })),
        on_retry_finished: Some(Arc::new(move |success, attempt, message| {
            finished_sink
                .lock()
                .unwrap()
                .push((success, attempt, message));
        })),
        ..Default::default()
    };
    let result = retry_assistant_call(
        || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { error_message("terminated") }
        },
        Some(ENABLED),
        None,
        Some(&callbacks),
    )
    .await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "1 initial call plus 3 retries"
    );
    assert_eq!(scheduled.load(Ordering::SeqCst), 3);
    assert_eq!(
        finished.lock().unwrap().as_slice(),
        [(false, 3, Some("terminated".to_string()))]
    );
}

#[tokio::test]
async fn stops_retrying_once_a_call_succeeds() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let finished = Arc::new(std::sync::Mutex::new(Vec::new()));
    let finished_sink = Arc::clone(&finished);
    let callbacks = RetryCallbacks {
        on_retry_finished: Some(Arc::new(move |success, attempt, message| {
            finished_sink
                .lock()
                .unwrap()
                .push((success, attempt, message));
        })),
        ..Default::default()
    };
    let result = retry_assistant_call(
        || {
            let call = counter.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if call < 3 {
                    error_message("terminated")
                } else {
                    assistant_message("recovered")
                }
            }
        },
        Some(ENABLED),
        None,
        Some(&callbacks),
    )
    .await;
    assert_eq!(
        result.content,
        vec![AssistantContent::Text(TextContent::new("recovered"))]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(finished.lock().unwrap().as_slice(), [(true, 2, None)]);
}

#[tokio::test]
async fn reports_an_aborted_retried_call_as_unsuccessful() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let finished = Arc::new(std::sync::Mutex::new(Vec::new()));
    let finished_sink = Arc::clone(&finished);
    let callbacks = RetryCallbacks {
        on_retry_finished: Some(Arc::new(move |success, attempt, message| {
            finished_sink
                .lock()
                .unwrap()
                .push((success, attempt, message));
        })),
        ..Default::default()
    };
    let result = retry_assistant_call(
        || {
            let call = counter.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if call == 1 {
                    error_message("terminated")
                } else {
                    AssistantMessage {
                        stop_reason: StopReason::Aborted,
                        ..assistant_message("")
                    }
                }
            }
        },
        Some(ENABLED),
        None,
        Some(&callbacks),
    )
    .await;
    assert_eq!(result.stop_reason, StopReason::Aborted);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(finished.lock().unwrap().as_slice(), [(false, 1, None)]);
}

#[tokio::test]
async fn does_not_retry_when_the_policy_is_disabled() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let result = retry_assistant_call(
        || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { error_message("terminated") }
        },
        Some(DISABLED),
        None,
        None,
    )
    .await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn emits_attempt_start_after_backoff_before_each_retried_call() {
    let events = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let produce_events = Arc::clone(&events);
    let scheduled_events = Arc::clone(&events);
    let attempt_events = Arc::clone(&events);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);

    let callbacks = RetryCallbacks {
        on_retry_scheduled: Some(Arc::new(move |attempt, _, _, _| {
            scheduled_events
                .lock()
                .unwrap()
                .push(format!("retry:{attempt}"));
        })),
        on_retry_attempt_start: Some(Arc::new(move || {
            attempt_events
                .lock()
                .unwrap()
                .push("attempt-start".to_string());
        })),
        ..Default::default()
    };

    let result = retry_assistant_call(
        || {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            produce_events
                .lock()
                .unwrap()
                .push(format!("produce:{call}"));
            async move {
                if call + 1 < 3 {
                    error_message("terminated")
                } else {
                    assistant_message("recovered")
                }
            }
        },
        Some(ENABLED),
        None,
        Some(&callbacks),
    )
    .await;

    assert_eq!(
        result.content,
        vec![AssistantContent::Text(TextContent::new("recovered"))]
    );
    assert_eq!(
        events.lock().unwrap().as_slice(),
        [
            "produce:0",
            "retry:1",
            "attempt-start",
            "produce:1",
            "retry:2",
            "attempt-start",
            "produce:2"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn aborts_the_backoff_sleep_and_returns_an_aborted_message() {
    let signal = tokio_util::sync::CancellationToken::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let finished = Arc::new(std::sync::Mutex::new(Vec::new()));
    let finished_sink = Arc::clone(&finished);
    let callbacks = RetryCallbacks {
        on_retry_finished: Some(Arc::new(move |success, attempt, message| {
            finished_sink
                .lock()
                .unwrap()
                .push((success, attempt, message));
        })),
        ..Default::default()
    };
    let policy = RetryPolicy {
        enabled: true,
        max_retries: 5,
        base_delay_ms: 10_000,
    };

    let aborting = signal.clone();
    let abort_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        aborting.cancel();
    });

    let result = retry_assistant_call(
        || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { error_message("terminated") }
        },
        Some(policy),
        Some(signal),
        Some(&callbacks),
    )
    .await;
    abort_task.await.unwrap();

    assert_eq!(result.stop_reason, StopReason::Aborted);
    assert_eq!(result.error_message, None);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        finished.lock().unwrap().as_slice(),
        [(false, 1, Some("terminated".to_string()))]
    );
}

/// Port of `packages/ai/src/utils/typebox-helpers.ts` (24) — `index.ts` exports it, so the
/// shape it produces is part of the public surface.
#[test]
fn string_enum_builds_a_plain_json_schema() {
    use notagent_ai::utils::typebox_helpers::string_enum;
    use serde_json::json;

    assert_eq!(
        string_enum(["add", "subtract"], None, None),
        json!({ "type": "string", "enum": ["add", "subtract"] })
    );
    assert_eq!(
        string_enum(
            ["add", "subtract"],
            Some("The operation to perform"),
            Some("add")
        ),
        json!({
            "type": "string",
            "enum": ["add", "subtract"],
            "description": "The operation to perform",
            "default": "add",
        })
    );
}
