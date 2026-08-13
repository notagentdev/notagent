//! Cancellation helpers.
//!
//! 1:1 port of `packages/ai/src/utils/abort.ts` (50 LOC) and `abort-signals.ts` (41 LOC).
//! Substitution class 3 of the master plan: `AbortController`/`AbortSignal` become
//! `tokio_util::sync::CancellationToken`.

use std::future::Future;

use tokio_util::sync::CancellationToken;

/// The abort reason; `AbortError` in TS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("The operation was aborted")]
pub struct Aborted;

/// `operationSignal(signal?)` — an operation-local token when the caller passes none.
pub fn operation_signal(signal: Option<CancellationToken>) -> CancellationToken {
    signal.unwrap_or_default()
}

/// `raceWithAbortSignal(operation, signal)` — stops waiting once the signal fires.
///
/// The abandoned future is dropped instead of being observed further; Rust futures do
/// not produce unhandled rejections, so the TS `void operation.catch(() => {})` guard
/// has no counterpart.
pub async fn race_with_abort_signal<T>(
    operation: impl Future<Output = T>,
    signal: &CancellationToken,
) -> Result<T, Aborted> {
    if signal.is_cancelled() {
        return Err(Aborted);
    }
    tokio::select! {
        value = operation => Ok(value),
        _ = signal.cancelled() => Err(Aborted),
    }
}

/// `CombinedAbortSignal { signal?, cleanup }`
pub struct CombinedAbortSignal {
    pub signal: Option<CancellationToken>,
    forwarder: Option<tokio::task::JoinHandle<()>>,
}

impl CombinedAbortSignal {
    /// `cleanup()` — removes the forwarding listeners.
    pub fn cleanup(&mut self) {
        if let Some(forwarder) = self.forwarder.take() {
            forwarder.abort();
        }
    }
}

impl Drop for CombinedAbortSignal {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// `combineAbortSignals(signals)` — no token, the single token, or a derived token.
pub fn combine_abort_signals(signals: &[Option<CancellationToken>]) -> CombinedAbortSignal {
    let active: Vec<CancellationToken> = signals.iter().flatten().cloned().collect();
    if active.is_empty() {
        return CombinedAbortSignal {
            signal: None,
            forwarder: None,
        };
    }
    if active.len() == 1 {
        return CombinedAbortSignal {
            signal: Some(active[0].clone()),
            forwarder: None,
        };
    }

    let combined = CancellationToken::new();
    // An already cancelled source cancels immediately, as in TS.
    if let Some(cancelled) = active.iter().find(|signal| signal.is_cancelled()) {
        let _ = cancelled;
        combined.cancel();
        return CombinedAbortSignal {
            signal: Some(combined),
            forwarder: None,
        };
    }

    let target = combined.clone();
    let forwarder = tokio::spawn(async move {
        let mut futures: Vec<_> = active
            .iter()
            .map(|signal| Box::pin(signal.cancelled()))
            .collect();
        futures::future::select_all(&mut futures).await;
        target.cancel();
    });
    CombinedAbortSignal {
        signal: Some(combined),
        forwarder: Some(forwarder),
    }
}
