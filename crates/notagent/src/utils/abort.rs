//! Port of `packages/coding-agent/src/utils/abort.ts`.

use std::future::Future;

use tokio_util::sync::CancellationToken;

/// The rejection value of an aborted operation (`AbortError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("The operation was aborted")]
pub struct AbortError;

/// Normalize an optional public signal without imposing a deadline.
pub fn operation_signal(signal: Option<CancellationToken>) -> CancellationToken {
    signal.unwrap_or_default()
}

/// Stop waiting on abort while observing the abandoned operation through
/// settlement.
///
/// Dropping a Rust future cancels it, so callers that must keep the abandoned
/// operation running pass a handle to work owned elsewhere (a spawned task or a
/// shared future), exactly as the TS version races an already-started promise.
pub async fn race_with_abort_signal<T>(
    operation: impl Future<Output = T>,
    signal: Option<&CancellationToken>,
) -> Result<T, AbortError> {
    let Some(signal) = signal else { return Ok(operation.await) };
    if signal.is_cancelled() {
        return Err(AbortError);
    }
    tokio::select! {
        biased;
        value = operation => Ok(value),
        () = signal.cancelled() => Err(AbortError),
    }
}

/// `signal?.throwIfAborted()`.
pub fn throw_if_aborted(signal: Option<&CancellationToken>) -> Result<(), AbortError> {
    match signal {
        Some(signal) if signal.is_cancelled() => Err(AbortError),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_signal_the_operation_decides() {
        assert_eq!(race_with_abort_signal(async { 7 }, None).await, Ok(7));
    }

    #[tokio::test]
    async fn an_already_aborted_signal_rejects_immediately() {
        let signal = CancellationToken::new();
        signal.cancel();
        let result = race_with_abort_signal(async { 7 }, Some(&signal)).await;
        assert_eq!(result, Err(AbortError));
    }

    #[tokio::test]
    async fn aborting_stops_the_wait() {
        let signal = CancellationToken::new();
        let pending = signal.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            pending.cancel();
        });
        let result = race_with_abort_signal(std::future::pending::<u8>(), Some(&signal)).await;
        assert_eq!(result, Err(AbortError));
    }

    #[tokio::test]
    async fn a_settled_operation_wins_over_a_later_abort() {
        let signal = CancellationToken::new();
        assert_eq!(race_with_abort_signal(async { 1 }, Some(&signal)).await, Ok(1));
        signal.cancel();
        assert!(throw_if_aborted(Some(&signal)).is_err());
        assert!(throw_if_aborted(None).is_ok());
    }

    #[test]
    fn operation_signal_returns_a_live_token() {
        assert!(!operation_signal(None).is_cancelled());
        let signal = CancellationToken::new();
        signal.cancel();
        assert!(operation_signal(Some(signal)).is_cancelled());
    }
}
