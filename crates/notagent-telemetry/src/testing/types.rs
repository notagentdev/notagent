//! Fixture-Verträge der Konformanzsuite.
//!
//! 1:1-Port von `packages/telemetry/src/testing/types.ts` (18 LOC).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::{RecordedTelemetrySpan, TelemetryContext};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// `TelemetryAdapterFixture` — frische Adapter-Instanz plus normalisierter Leser.
///
/// Abweichung Klasse 1: `AsyncDisposable` entfällt; das Aufräumen erledigt `Drop`.
pub trait TelemetryAdapterFixture: Send + Sync {
    fn context(&self) -> Arc<dyn TelemetryContext>;
    fn get_spans(&self) -> BoxFuture<'_, Vec<RecordedTelemetrySpan>>;
}

/// `TelemetryAdapterFixtureFactory` — erzeugt je Konformanzfall eine isolierte Fixture.
pub type TelemetryAdapterFixtureFactory =
    Arc<dyn Fn() -> BoxFuture<'static, Arc<dyn TelemetryAdapterFixture>> + Send + Sync>;
