use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

const CANCEL_MESSAGE: &str = "Login cancelled";
const TIMEOUT_MESSAGE: &str = "Device flow timed out";
const SLOW_DOWN_TIMEOUT_MESSAGE: &str = "Device flow timed out after one or more slow_down responses. This is often caused by clock drift in WSL or VM environments. Please sync or restart the VM clock and try again.";
const MINIMUM_INTERVAL_MS: u64 = 1000;
/// RFC 8628 §3.2: without an `interval` the client must use 5 seconds.
const DEFAULT_POLL_INTERVAL_SECONDS: f64 = 5.0;
/// RFC 8628 §3.5: `slow_down` increases the interval by 5 seconds.
const SLOW_DOWN_INTERVAL_INCREMENT_MS: u64 = 5000;

/// `OAuthDeviceCodePollResult<T>`
pub enum DeviceCodePollResult<T> {
    Pending,
    SlowDown { interval_seconds: Option<f64> },
    Failed { message: String },
    Complete { value: T },
}

/// Failure of [`poll_device_code_flow`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DeviceCodeError(pub String);

/// `OAuthDeviceCodePollOptions<T>`
pub struct DeviceCodePollOptions {
    pub interval_seconds: Option<f64>,
    pub expires_in_seconds: Option<f64>,
    pub wait_before_first_poll: bool,
    pub signal: CancellationToken,
}

/// Sleeps unless the signal fires; the signal turns into the cancel message.
async fn abortable_sleep(ms: u64, signal: &CancellationToken) -> Result<(), DeviceCodeError> {
    if signal.is_cancelled() {
        return Err(DeviceCodeError(CANCEL_MESSAGE.to_string()));
    }
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(ms)) => Ok(()),
        _ = signal.cancelled() => Err(DeviceCodeError(CANCEL_MESSAGE.to_string())),
    }
}

/// `pollOAuthDeviceCodeFlow(options)`
pub async fn poll_device_code_flow<T, F, Fut>(
    options: DeviceCodePollOptions,
    mut poll: F,
) -> Result<T, DeviceCodeError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = DeviceCodePollResult<T>>,
{
    let now_ms = || crate::auth::resolve::now_ms() as f64;
    let deadline = match options.expires_in_seconds {
        Some(expires_in_seconds) => now_ms() + expires_in_seconds * 1000.0,
        None => f64::INFINITY,
    };
    let mut interval_ms = ((options
        .interval_seconds
        .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS)
        * 1000.0)
        .floor() as u64)
        .max(MINIMUM_INTERVAL_MS);

    let mut slow_down_responses = 0u32;
    if options.wait_before_first_poll {
        let remaining_ms = deadline - now_ms();
        if remaining_ms > 0.0 {
            abortable_sleep(interval_ms.min(remaining_ms as u64), &options.signal).await?;
        }
    }

    while now_ms() < deadline {
        if options.signal.is_cancelled() {
            return Err(DeviceCodeError(CANCEL_MESSAGE.to_string()));
        }

        match poll().await {
            DeviceCodePollResult::Complete { value } => return Ok(value),
            DeviceCodePollResult::Failed { message } => return Err(DeviceCodeError(message)),
            DeviceCodePollResult::SlowDown { interval_seconds } => {
                slow_down_responses += 1;
                // A server-provided interval wins: trusting only the client-side counter
                // risks polling early forever under WSL/VM clock drift.
                interval_ms = match interval_seconds
                    .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
                {
                    Some(seconds) => ((seconds * 1000.0).floor() as u64).max(MINIMUM_INTERVAL_MS),
                    None => {
                        (interval_ms + SLOW_DOWN_INTERVAL_INCREMENT_MS).max(MINIMUM_INTERVAL_MS)
                    }
                };
            }
            DeviceCodePollResult::Pending => {}
        }

        let remaining_ms = deadline - now_ms();
        if remaining_ms <= 0.0 {
            break;
        }
        abortable_sleep(interval_ms.min(remaining_ms as u64), &options.signal).await?;
    }

    Err(DeviceCodeError(
        if slow_down_responses > 0 {
            SLOW_DOWN_TIMEOUT_MESSAGE
        } else {
            TIMEOUT_MESSAGE
        }
        .to_string(),
    ))
}
