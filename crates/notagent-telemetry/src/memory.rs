//! Backend-neutrale Referenzimplementierung, die Spans im Prozessspeicher aufzeichnet.
//!
//! 1:1-Port von `packages/telemetry/src/memory.ts` (219 LOC).

use std::sync::{Arc, Mutex};

use crate::noop::noop_telemetry_context;
use crate::{
    AttributeValue, SpanAttributes, SpanOptions, SpanOutcome, SpanStatus, SpanStatusError,
    TelemetryContext, TelemetrySpan,
};

/// `RecordedTelemetryEvent`
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedTelemetryEvent {
    pub name: String,
    pub attributes: SpanAttributes,
}

/// `RecordedTelemetrySpan` — losgelöster Schnappschuss.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedTelemetrySpan {
    pub id: u64,
    pub parent_id: Option<u64>,
    pub name: String,
    pub attributes: SpanAttributes,
    pub events: Vec<RecordedTelemetryEvent>,
    pub status: SpanStatus,
    pub settled: bool,
    pub end_sequence: Option<u64>,
}

struct MutableSpan {
    id: u64,
    parent_id: Option<u64>,
    name: String,
    attributes: SpanAttributes,
    events: Vec<RecordedTelemetryEvent>,
    status: SpanStatus,
    explicit_status: bool,
    settled: bool,
    end_sequence: Option<u64>,
}

struct State {
    spans: Vec<MutableSpan>,
    next_span_id: u64,
    next_end_sequence: u64,
}

impl State {
    fn find(&mut self, id: u64) -> Option<&mut MutableSpan> {
        self.spans.iter_mut().find(|span| span.id == id)
    }

    fn is_settled(&self, id: u64) -> bool {
        self.spans
            .iter()
            .find(|span| span.id == id)
            .is_some_and(|span| span.settled)
    }
}

/// `copyAttributes` — defensive Kopie; `undefined`-Werte gibt es in Rust nicht.
fn copy_attributes(attributes: Option<&SpanAttributes>) -> SpanAttributes {
    attributes.cloned().unwrap_or_default()
}

/// `mergeAttributes`
fn merge_attributes(current: &SpanAttributes, attributes: SpanAttributes) -> SpanAttributes {
    let mut merged = current.clone();
    for (name, value) in attributes {
        merged.insert(name, value);
    }
    merged
}

fn create_span(state: &mut State, parent_id: Option<u64>, options: SpanOptions) -> u64 {
    let id = state.next_span_id;
    state.next_span_id += 1;
    state.spans.push(MutableSpan {
        id,
        parent_id,
        name: options.name,
        attributes: copy_attributes(options.attributes.as_ref()),
        events: Vec::new(),
        status: SpanStatus::Ok,
        explicit_status: false,
        settled: false,
        end_sequence: None,
    });
    id
}

/// `settleSpan` — setzt bei Fehlern ohne expliziten Status automatisch Error.
fn settle_span(state: &mut State, id: u64, outcome: SpanOutcome) {
    let next_end_sequence = state.next_end_sequence;
    let Some(span) = state.find(id) else { return };
    if span.settled {
        return;
    }
    if let SpanOutcome::Failed(error) = outcome
        && !span.explicit_status
    {
        span.status = SpanStatus::Error { error };
    }
    span.settled = true;
    span.end_sequence = Some(next_end_sequence);
    state.next_end_sequence += 1;
}

struct InMemorySpan {
    state: Arc<Mutex<State>>,
    id: u64,
}

impl TelemetryContext for InMemorySpan {
    fn begin_span(&self, options: SpanOptions) -> Arc<dyn TelemetrySpan> {
        let mut state = self.state.lock().expect("Telemetrie-Zustand vergiftet");
        // TS: Kinder eines abgeschlossenen Spans werden nicht aufgezeichnet.
        if state.is_settled(self.id) {
            drop(state);
            return noop_telemetry_context().begin_span(options);
        }
        let id = create_span(&mut state, Some(self.id), options);
        drop(state);
        Arc::new(InMemorySpan {
            state: Arc::clone(&self.state),
            id,
        })
    }
}

impl TelemetrySpan for InMemorySpan {
    fn add_event(&self, name: &str, attributes: Option<SpanAttributes>) {
        let mut state = self.state.lock().expect("Telemetrie-Zustand vergiftet");
        let attributes = copy_attributes(attributes.as_ref());
        if let Some(span) = state.find(self.id)
            && !span.settled
        {
            span.events.push(RecordedTelemetryEvent {
                name: name.to_string(),
                attributes,
            });
        }
    }

    fn set_attributes(&self, attributes: SpanAttributes) {
        let mut state = self.state.lock().expect("Telemetrie-Zustand vergiftet");
        if let Some(span) = state.find(self.id)
            && !span.settled
        {
            span.attributes = merge_attributes(&span.attributes, attributes);
        }
    }

    fn set_status(&self, status: SpanStatus) {
        let mut state = self.state.lock().expect("Telemetrie-Zustand vergiftet");
        if let Some(span) = state.find(self.id)
            && !span.settled
        {
            span.status = status;
            span.explicit_status = true;
        }
    }

    fn settle(&self, outcome: SpanOutcome) {
        let mut state = self.state.lock().expect("Telemetrie-Zustand vergiftet");
        settle_span(&mut state, self.id, outcome);
    }
}

/// `InMemoryTelemetryContext` — frische Instanz je Test bzw. Aufzeichnungsbereich.
#[derive(Clone, Default)]
pub struct InMemoryTelemetryContext {
    state: Arc<Mutex<State>>,
}

impl Default for State {
    fn default() -> Self {
        State {
            spans: Vec::new(),
            next_span_id: 1,
            next_end_sequence: 1,
        }
    }
}

impl InMemoryTelemetryContext {
    pub fn new() -> Self {
        InMemoryTelemetryContext::default()
    }

    /// `getSpans()` — losgelöste Schnappschüsse in Startreihenfolge.
    pub fn get_spans(&self) -> Vec<RecordedTelemetrySpan> {
        let state = self.state.lock().expect("Telemetrie-Zustand vergiftet");
        state
            .spans
            .iter()
            .map(|span| RecordedTelemetrySpan {
                id: span.id,
                parent_id: span.parent_id,
                name: span.name.clone(),
                attributes: span.attributes.clone(),
                events: span.events.clone(),
                status: span.status.clone(),
                settled: span.settled,
                end_sequence: span.end_sequence,
            })
            .collect()
    }
}

impl TelemetryContext for InMemoryTelemetryContext {
    fn begin_span(&self, options: SpanOptions) -> Arc<dyn TelemetrySpan> {
        let mut state = self.state.lock().expect("Telemetrie-Zustand vergiftet");
        let id = create_span(&mut state, None, options);
        drop(state);
        Arc::new(InMemorySpan {
            state: Arc::clone(&self.state),
            id,
        })
    }
}

/// Hilfsfunktion für Attributwerte in Tests und Aufrufern.
pub fn attribute(value: impl Into<AttributeValue>) -> AttributeValue {
    value.into()
}

/// TS `automaticErrorStatus` für Rust-Fehlerwerte: `Error.name` ist bei einem
/// einfachen `new Error(...)` „Error", die Meldung ist `error.message`.
pub fn automatic_error_status(error: &dyn std::fmt::Display) -> SpanStatusError {
    SpanStatusError {
        name: "Error".to_string(),
        message: error.to_string(),
    }
}
