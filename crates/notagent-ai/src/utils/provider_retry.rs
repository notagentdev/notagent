//! HTTP-level retry that mirrors the pinned OpenAI/Anthropic SDK policy.
//!
//! 1:1 port of `packages/ai/src/utils/provider-retry.ts` (125 LOC). The SDKs' own retry
//! timers ignore the abort signal, which is why the TS code calls them with
//! `maxRetries: 0` and wraps the request here; the Rust port keeps that structure.

use std::future::Future;
use std::time::Duration;

use chrono::DateTime;
use rand::RngExt;
use tokio_util::sync::CancellationToken;

const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// `ProviderRetryOptions`
#[derive(Debug, Clone, Default)]
pub struct ProviderRetryOptions {
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
    pub signal: Option<CancellationToken>,
}

/// The SDK error shape the TS code probes for (`status` plus `headers`).
pub trait ProviderErrorInfo {
    fn status(&self) -> Option<u16>;
    fn header(&self, name: &str) -> Option<String>;
    fn message(&self) -> String;
}

/// Error of [`retry_provider_request`].
#[derive(Debug, thiserror::Error)]
pub enum ProviderRetryError<E> {
    /// The request failed and was not retried (or the budget ran out).
    #[error(transparent)]
    Request(E),
    /// The server asked for a longer delay than `max_retry_delay_ms` allows.
    #[error("{0}")]
    RetryDelayTooLong(String),
    /// `AbortError` — the request or its backoff was aborted.
    #[error("Request aborted")]
    Aborted,
}

/// `isRetryableProviderError(error)`
fn is_retryable_provider_error(error: &impl ProviderErrorInfo) -> bool {
    match error.header("x-should-retry").as_deref() {
        Some("true") => return true,
        Some("false") => return false,
        _ => {}
    }
    match error.status() {
        None => true,
        Some(status) => status == 408 || status == 409 || status == 429 || status >= 500,
    }
}

fn validate_server_retry_delay_ms(
    delay_ms: f64,
    max_retry_delay_ms: Option<u64>,
    provider_error_message: &str,
) -> Result<f64, String> {
    let max_delay_ms = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS);
    if max_delay_ms > 0 && delay_ms > max_delay_ms as f64 {
        return Err(format!(
            "Server requested {}s retry delay (max: {}s). {provider_error_message}",
            (delay_ms / 1000.0).ceil(),
            (max_delay_ms as f64 / 1000.0).ceil()
        ));
    }
    Ok(delay_ms)
}

/// `getRetryDelayMs(error, retryIndex, maxRetryDelayMs)`
fn retry_delay_ms(
    error: &impl ProviderErrorInfo,
    retry_index: u32,
    max_retry_delay_ms: Option<u64>,
) -> Result<f64, String> {
    if let Some(retry_after_ms) = error.header("retry-after-ms")
        && let Ok(value) = retry_after_ms.trim().parse::<f64>()
        && !value.is_nan()
    {
        return validate_server_retry_delay_ms(value, max_retry_delay_ms, &error.message());
    }

    if let Some(retry_after) = error.header("retry-after") {
        let delay_ms = match retry_after.trim().parse::<f64>() {
            Ok(seconds) if !seconds.is_nan() => Some(seconds * 1000.0),
            // `Date.parse(retryAfter) - Date.now()` for the HTTP-date form.
            _ => DateTime::parse_from_rfc2822(retry_after.trim())
                .ok()
                .map(|target| {
                    (target.timestamp_millis() - chrono::Utc::now().timestamp_millis()) as f64
                }),
        };
        // Deviation from the TS original (user decision 2026-08-16, v0.1.4):
        // an unparseable header produced NaN there, which slipped through the
        // `delayMs > max` validation and slept 0ms — a retry with no backoff
        // at all. Falling through to the exponential delay keeps the pacing.
        if let Some(delay_ms) = delay_ms {
            return validate_server_retry_delay_ms(delay_ms, max_retry_delay_ms, &error.message());
        }
    }

    let exponential_delay = (0.5f64 * 2f64.powi(retry_index as i32)).min(8.0) * 1000.0;
    Ok(exponential_delay * (1.0 - rand::rng().random::<f64>() * 0.25))
}

/// Sleeps unless cancelled; returns true when the signal fired.
async fn abortable_sleep(ms: f64, signal: Option<&CancellationToken>) -> bool {
    let duration = Duration::from_millis(ms.max(0.0) as u64);
    let Some(signal) = signal else {
        tokio::time::sleep(duration).await;
        return false;
    };
    if signal.is_cancelled() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        _ = signal.cancelled() => true,
    }
}

/// `retryProviderRequest(request, options)`
pub async fn retry_provider_request<T, E, F, Fut>(
    mut request: F,
    options: ProviderRetryOptions,
) -> Result<T, ProviderRetryError<E>>
where
    E: ProviderErrorInfo,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let max_retries = options.max_retries.unwrap_or(0);
    let mut retries_remaining = max_retries;

    loop {
        match request().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if options
                    .signal
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
                {
                    return Err(ProviderRetryError::Aborted);
                }
                if retries_remaining == 0 || !is_retryable_provider_error(&error) {
                    return Err(ProviderRetryError::Request(error));
                }

                let retry_index = max_retries - retries_remaining;
                retries_remaining -= 1;
                let delay_ms = match retry_delay_ms(&error, retry_index, options.max_retry_delay_ms)
                {
                    Ok(delay_ms) => delay_ms,
                    Err(message) => return Err(ProviderRetryError::RetryDelayTooLong(message)),
                };
                if abortable_sleep(delay_ms, options.signal.as_ref()).await {
                    return Err(ProviderRetryError::Aborted);
                }
            }
        }
    }
}
