use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::{RecordedTelemetrySpan, TelemetryContext};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// `TelemetryAdapterFixture` — frische Adapter-Instanz plus normalisierter Leser.
pub trait TelemetryAdapterFixture: Send + Sync {
    fn context(&self) -> Arc<dyn TelemetryContext>;
    fn get_spans(&self) -> BoxFuture<'_, Vec<RecordedTelemetrySpan>>;
}

pub type TelemetryAdapterFixtureFactory =
    Arc<dyn Fn() -> BoxFuture<'static, Arc<dyn TelemetryAdapterFixture>> + Send + Sync>;
