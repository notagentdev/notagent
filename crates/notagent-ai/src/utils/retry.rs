//! Retry policy and classification of transient assistant errors.
//!
//! 1:1 port of `packages/ai/src/utils/retry.ts` (228 LOC).

use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use tokio_util::sync::CancellationToken;

use crate::types::{AssistantMessage, StopReason};

/// Subscription/account limits returned as 429s — deterministic, never retried.
const NON_RETRYABLE_PROVIDER_LIMIT_PATTERNS: [&str; 8] = [
    "GoUsageLimitError",
    "FreeUsageLimitError",
    "Monthly usage limit reached",
    "available balance",
    "insufficient_quota",
    "out of budget",
    "quota exceeded",
    "billing",
];

/// Transient provider, transport and stream failures (`retry.ts:27-88`).
const RETRYABLE_PROVIDER_PATTERNS: [&str; 38] = [
    "overloaded",
    "rate.?limit",
    "too many requests",
    "429",
    "500",
    "502",
    "503",
    "504",
    "524",
    "service.?unavailable",
    "server.?error",
    "internal.?error",
    "provider.?returned.?error",
    "exceeded request buffer limit while retrying upstream",
    "network.?error",
    "connection.?error",
    "connection.?refused",
    "connection.?lost",
    "other side closed",
    "fetch failed",
    "getaddrinfo",
    "ENOTFOUND",
    "EAI_AGAIN",
    "upstream.?connect",
    "reset before headers",
    "socket hang up",
    "socket connection was closed",
    "timed? out",
    "timeout",
    "terminated",
    "websocket.?closed",
    "websocket.?error",
    "ended without",
    "stream ended before message_stop",
    "stream ended before a terminal response event",
    "http2 request did not get a response",
    "retry delay",
    "you can retry your request",
];

/// The remaining patterns of the same list; split only to stay within array literals.
const RETRYABLE_PROVIDER_PATTERNS_TAIL: [&str; 2] =
    ["try your request again", "please retry your request"];

/// gRPC based providers (e.g. NVIDIA NIM).
const RETRYABLE_PROVIDER_PATTERNS_GRPC: [&str; 1] = ["ResourceExhausted"];

fn build_pattern(patterns: &[&str]) -> Regex {
    Regex::new(&format!("(?i){}", patterns.join("|"))).expect("provider error pattern compiles")
}

fn non_retryable_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| build_pattern(&NON_RETRYABLE_PROVIDER_LIMIT_PATTERNS))
}

fn retryable_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        let mut patterns = RETRYABLE_PROVIDER_PATTERNS.to_vec();
        patterns.extend_from_slice(&RETRYABLE_PROVIDER_PATTERNS_TAIL);
        patterns.extend_from_slice(&RETRYABLE_PROVIDER_PATTERNS_GRPC);
        build_pattern(&patterns)
    })
}

/// `RetryPolicy { enabled, maxRetries, baseDelayMs }`
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicy {
    pub enabled: bool,
    /// Max retry attempts (0 = no retries); the initial call never counts as a retry.
    pub max_retries: u32,
    /// Per-attempt delay is `base_delay_ms * 2^(attempt-1)` before jitter.
    pub base_delay_ms: u64,
}

/// `onRetryScheduled(attempt, maxAttempts, delayMs, errorMessage)`
pub type OnRetryScheduled = std::sync::Arc<dyn Fn(u32, u32, u64, String) + Send + Sync>;
/// `onRetryAttemptStart()`
pub type OnRetryAttemptStart = std::sync::Arc<dyn Fn() + Send + Sync>;
/// `onRetryFinished(success, attempt, finalError?)`
pub type OnRetryFinished = std::sync::Arc<dyn Fn(bool, u32, Option<String>) + Send + Sync>;

/// Callbacks emitted by [`retry_assistant_call`] around each retry.
#[derive(Clone, Default)]
pub struct RetryCallbacks {
    /// Before the backoff sleep of each retry attempt (1-indexed).
    pub on_retry_scheduled: Option<OnRetryScheduled>,
    /// After the backoff sleep, immediately before the retried call starts.
    pub on_retry_attempt_start: Option<OnRetryAttemptStart>,
    /// Once when the loop ends.
    pub on_retry_finished: Option<OnRetryFinished>,
}

/// Runs a single assistant-producing call with bounded retry on transient errors.
pub async fn retry_assistant_call<F, Fut>(
    mut produce: F,
    policy: Option<RetryPolicy>,
    signal: Option<CancellationToken>,
    callbacks: Option<&RetryCallbacks>,
) -> AssistantMessage
where
    F: FnMut() -> Fut,
    Fut: Future<Output = AssistantMessage>,
{
    let max_attempts = match policy {
        Some(policy) if policy.enabled => policy.max_retries,
        _ => 0,
    };

    let mut attempt = 0u32;
    let mut last_retry: Option<(u32, String)> = None;
    loop {
        let response = produce().await;

        // Abort: terminal but not successful. Never retry an aborted message.
        if response.stop_reason == StopReason::Aborted {
            if let Some((attempt, _)) = &last_retry
                && let Some(callback) =
                    callbacks.and_then(|callbacks| callbacks.on_retry_finished.as_ref())
            {
                callback(false, *attempt, None);
            }
            return response;
        }

        // Success: non-error, non-abort responses return as-is.
        if response.stop_reason != StopReason::Error {
            if let Some((attempt, _)) = &last_retry
                && let Some(callback) =
                    callbacks.and_then(|callbacks| callbacks.on_retry_finished.as_ref())
            {
                callback(true, *attempt, None);
            }
            return response;
        }

        // Non-retryable, or budget exhausted: return the final error message.
        if attempt >= max_attempts || !is_retryable_assistant_error(&response) {
            if let Some((attempt, _)) = &last_retry
                && let Some(callback) =
                    callbacks.and_then(|callbacks| callbacks.on_retry_finished.as_ref())
            {
                callback(false, *attempt, response.error_message.clone());
            }
            return response;
        }

        attempt += 1;
        let error_message = response
            .error_message
            .clone()
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| "Unknown error".to_string());
        last_retry = Some((attempt, error_message.clone()));
        let policy = policy.expect("max_attempts > 0 implies a policy");
        let delay_ms = policy.base_delay_ms * 2u64.pow(attempt - 1);
        if let Some(callback) =
            callbacks.and_then(|callbacks| callbacks.on_retry_scheduled.as_ref())
        {
            callback(attempt, max_attempts, delay_ms, error_message.clone());
        }

        // Aborts during the backoff are normalized to the aborted AssistantMessage shape.
        let aborted = sleep_with_cancellation(delay_ms, signal.as_ref()).await;
        if aborted {
            if let Some(callback) =
                callbacks.and_then(|callbacks| callbacks.on_retry_finished.as_ref())
            {
                callback(false, attempt, Some(error_message));
            }
            return AssistantMessage {
                stop_reason: StopReason::Aborted,
                error_message: None,
                ..response
            };
        }
        if let Some(callback) =
            callbacks.and_then(|callbacks| callbacks.on_retry_attempt_start.as_ref())
        {
            callback();
        }
    }
}

/// Returns true when the sleep was cut short by cancellation.
async fn sleep_with_cancellation(delay_ms: u64, signal: Option<&CancellationToken>) -> bool {
    let Some(signal) = signal else {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        return false;
    };
    if signal.is_cancelled() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => false,
        _ = signal.cancelled() => true,
    }
}

/// `isRetryableAssistantError(message)`
pub fn is_retryable_assistant_error(message: &AssistantMessage) -> bool {
    if message.stop_reason != StopReason::Error {
        return false;
    }
    let Some(error_message) = message
        .error_message
        .as_ref()
        .filter(|message| !message.is_empty())
    else {
        return false;
    };
    if non_retryable_pattern().is_match(error_message) {
        return false;
    }
    retryable_pattern().is_match(error_message)
}
