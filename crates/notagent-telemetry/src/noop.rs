use std::sync::{Arc, OnceLock};

use crate::{
    SpanAttributes, SpanOptions, SpanOutcome, SpanStatus, TelemetryContext, TelemetrySpan,
};

struct NoopTelemetrySpan;

impl TelemetryContext for NoopTelemetrySpan {
    fn begin_span(&self, _options: SpanOptions) -> Arc<dyn TelemetrySpan> {
        noop_span()
    }
}

impl TelemetrySpan for NoopTelemetrySpan {
    fn add_event(&self, _name: &str, _attributes: Option<SpanAttributes>) {}
    fn set_attributes(&self, _attributes: SpanAttributes) {}
    fn set_status(&self, _status: SpanStatus) {}
    fn settle(&self, _outcome: SpanOutcome) {}
}

fn noop_span() -> Arc<dyn TelemetrySpan> {
    static NOOP_SPAN: OnceLock<Arc<NoopTelemetrySpan>> = OnceLock::new();
    NOOP_SPAN
        .get_or_init(|| Arc::new(NoopTelemetrySpan))
        .clone()
}

pub fn noop_telemetry_context() -> Arc<dyn TelemetryContext> {
    static NOOP_CONTEXT: OnceLock<Arc<NoopTelemetrySpan>> = OnceLock::new();
    NOOP_CONTEXT
        .get_or_init(|| Arc::new(NoopTelemetrySpan))
        .clone()
}
