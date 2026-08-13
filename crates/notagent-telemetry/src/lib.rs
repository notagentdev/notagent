//! Vendor-neutrale Telemetrie-Contracts (Spans, Events, Attribute).
//!
//! 1:1-Port von `packages/telemetry/src/index.ts` (357 LOC). Kein Exporter, kein
//! globaler Current-Span: Parent-Kontexte werden explizit weitergereicht.
//!
//! Siehe `crates/notagent-telemetry/PARITY.md`.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub mod memory;
pub mod noop;
pub mod testing;

pub use memory::{InMemoryTelemetryContext, RecordedTelemetryEvent, RecordedTelemetrySpan};
pub use noop::noop_telemetry_context;

/// `AttributeValue = string | number | boolean | readonly string[] | readonly number[] | readonly boolean[]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValue {
    String(String),
    Number(f64),
    Boolean(bool),
    StringArray(Vec<String>),
    NumberArray(Vec<f64>),
    BooleanArray(Vec<bool>),
}

impl From<&str> for AttributeValue {
    fn from(value: &str) -> Self {
        AttributeValue::String(value.to_string())
    }
}

impl From<String> for AttributeValue {
    fn from(value: String) -> Self {
        AttributeValue::String(value)
    }
}

impl From<f64> for AttributeValue {
    fn from(value: f64) -> Self {
        AttributeValue::Number(value)
    }
}

impl From<u64> for AttributeValue {
    fn from(value: u64) -> Self {
        AttributeValue::Number(value as f64)
    }
}

impl From<i64> for AttributeValue {
    fn from(value: i64) -> Self {
        AttributeValue::Number(value as f64)
    }
}

impl From<bool> for AttributeValue {
    fn from(value: bool) -> Self {
        AttributeValue::Boolean(value)
    }
}

/// `SpanAttributes { [name: string]: AttributeValue | undefined }`.
///
/// Abweichung Klasse 1: `undefined`-Werte existieren in Rust nicht — in TS werden sie
/// beim Kopieren/Mergen übersprungen, hier fehlt der Schlüssel schlicht. Beobachtbares
/// Verhalten identisch (siehe `memory.rs::copy_attributes`).
pub type SpanAttributes = BTreeMap<String, AttributeValue>;

/// `SpanOptions { name, attributes? }`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpanOptions {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<SpanAttributes>,
}

impl SpanOptions {
    pub fn new(name: impl Into<String>) -> Self {
        SpanOptions {
            name: name.into(),
            attributes: None,
        }
    }

    pub fn with_attributes(name: impl Into<String>, attributes: SpanAttributes) -> Self {
        SpanOptions {
            name: name.into(),
            attributes: Some(attributes),
        }
    }
}

/// `{ name, message }` innerhalb von `SpanStatus`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanStatusError {
    pub name: String,
    pub message: String,
}

/// `SpanStatus = { status: "ok" } | { status: "error"; error?: { name, message } }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum SpanStatus {
    Ok,
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<SpanStatusError>,
    },
}

impl SpanStatus {
    pub fn error() -> Self {
        SpanStatus::Error { error: None }
    }

    pub fn error_with(name: impl Into<String>, message: impl Into<String>) -> Self {
        SpanStatus::Error {
            error: Some(SpanStatusError {
                name: name.into(),
                message: message.into(),
            }),
        }
    }
}

/// Ergebnis eines Span-Callbacks; ersetzt die TS-Unterscheidung „Callback wirft" (error)
/// gegenüber „Callback liefert einen Fehlerwert zurück" (ok).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanOutcome {
    /// Callback ist normal zurückgekehrt (`Ok`) — Status bleibt beim Default bzw. beim
    /// zuletzt explizit gesetzten Wert.
    Completed,
    /// Callback ist fehlgeschlagen (TS: throw/reject) — ohne expliziten Status wird
    /// automatisch ein Error-Status gesetzt.
    Failed(Option<SpanStatusError>),
}

/// `TelemetryContext { startSpan(options, callback) }`.
///
/// Abweichung Klasse 1 (Sprachidiomatik): TS bindet die Span-Lebensdauer an einen
/// generischen Callback. Ein generischer Callback macht das Trait in Rust nicht
/// objekt-sicher, deshalb ist `begin_span` das objekt-sichere Primitiv und
/// [`start_span`] der generische Wrapper, der die Callback-Semantik (Settlement bei
/// Rückkehr, Auto-Error bei Fehler) exakt nachbildet.
pub trait TelemetryContext: Send + Sync {
    /// Startet einen Span mit diesem Kontext als explizitem Parent.
    fn begin_span(&self, options: SpanOptions) -> Arc<dyn TelemetrySpan>;
}

/// `TelemetrySpan extends TelemetryContext`.
pub trait TelemetrySpan: TelemetryContext {
    fn add_event(&self, name: &str, attributes: Option<SpanAttributes>);
    fn set_attributes(&self, attributes: SpanAttributes);
    fn set_status(&self, status: SpanStatus);
    /// Schließt den Span ab. Aufrufe nach dem Settlement sind wirkungslos.
    fn settle(&self, outcome: SpanOutcome);
}

/// `context.startSpan(options, callback)` für Callbacks, die nicht fehlschlagen.
///
/// Der Span wird abgeschlossen, sobald der Callback zurückkehrt — wie in TS, wo das
/// zurückgegebene Promise das Settlement auslöst.
pub async fn start_span<C, F, Fut, T>(context: &C, options: SpanOptions, callback: F) -> T
where
    C: TelemetryContext + ?Sized,
    F: FnOnce(Arc<dyn TelemetrySpan>) -> Fut,
    Fut: Future<Output = T>,
{
    let span = context.begin_span(options);
    let result = callback(Arc::clone(&span)).await;
    span.settle(SpanOutcome::Completed);
    result
}

/// Wie [`start_span`], aber für fehlbare Callbacks: `Err` entspricht dem geworfenen
/// Fehler in TS und setzt ohne expliziten Status automatisch einen Error-Status.
pub async fn try_start_span<C, F, Fut, T, E>(
    context: &C,
    options: SpanOptions,
    callback: F,
) -> Result<T, E>
where
    C: TelemetryContext + ?Sized,
    F: FnOnce(Arc<dyn TelemetrySpan>) -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let span = context.begin_span(options);
    match callback(Arc::clone(&span)).await {
        Ok(value) => {
            span.settle(SpanOutcome::Completed);
            Ok(value)
        }
        Err(error) => {
            span.settle(SpanOutcome::Failed(Some(memory::automatic_error_status(
                &error,
            ))));
            Err(error)
        }
    }
}

// ---------------------------------------------------------------------------
// Schema-Definitionen (reine Datenstrukturen; die TS-Typinferenz-Maschinerie
// entfällt laut Master-Plan, Faktenbericht §5: „TS-Typmaschinerie entfällt").
// ---------------------------------------------------------------------------

/// `TelemetryAttributeType = "string" | "number" | "boolean" | "string[]" | "number[]" | "boolean[]"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TelemetryAttributeType {
    #[serde(rename = "string")]
    String,
    #[serde(rename = "number")]
    Number,
    #[serde(rename = "boolean")]
    Boolean,
    #[serde(rename = "string[]")]
    StringArray,
    #[serde(rename = "number[]")]
    NumberArray,
    #[serde(rename = "boolean[]")]
    BooleanArray,
}

/// `cardinality?: "low" | "high"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryCardinality {
    Low,
    High,
}

/// Typabhängiger Teil von `TelemetryAttributeDefinition`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TelemetryAttributeValues {
    #[serde(rename = "string")]
    String {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        examples: Option<Vec<String>>,
    },
    #[serde(rename = "number")]
    Number {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Vec<f64>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        examples: Option<Vec<f64>>,
    },
    #[serde(rename = "boolean")]
    Boolean {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Vec<bool>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        examples: Option<Vec<bool>>,
    },
    #[serde(rename = "string[]")]
    StringArray {
        #[serde(
            rename = "elementValues",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        element_values: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        examples: Option<Vec<Vec<String>>>,
    },
    #[serde(rename = "number[]")]
    NumberArray {
        #[serde(
            rename = "elementValues",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        element_values: Option<Vec<f64>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        examples: Option<Vec<Vec<f64>>>,
    },
    #[serde(rename = "boolean[]")]
    BooleanArray {
        #[serde(
            rename = "elementValues",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        element_values: Option<Vec<bool>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        examples: Option<Vec<Vec<bool>>>,
    },
}

/// `TelemetryAttributeDefinition = TelemetryAttributeMetadata & (typabhängiger Teil)`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryAttributeDefinition {
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cardinality: Option<TelemetryCardinality>,
    #[serde(flatten)]
    pub values: TelemetryAttributeValues,
}

/// `TelemetryStartAttributeDefinition = TelemetryAttributeDefinition & { required: boolean }`
/// (identisch zu `TelemetryEventAttributeDefinition`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryRequiredAttributeDefinition {
    pub required: bool,
    #[serde(flatten)]
    pub definition: TelemetryAttributeDefinition,
}

pub type TelemetryStartAttributeDefinition = TelemetryRequiredAttributeDefinition;
pub type TelemetryEventAttributeDefinition = TelemetryRequiredAttributeDefinition;

/// `TelemetryEventDefinition { description, attributes }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryEventDefinition {
    pub description: String,
    pub attributes: BTreeMap<String, TelemetryEventAttributeDefinition>,
}

/// `TelemetryParentDefinition = { kind: "any" } | { kind: "root_or_external" } | { kind: "spans"; spans }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TelemetryParentDefinition {
    Any,
    RootOrExternal,
    Spans { spans: Vec<String> },
}

/// `status: { default: "ok"; errorWhen: string }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetrySpanStatusDefinition {
    /// In TS als Literaltyp `"ok"` festgelegt.
    pub default: String,
    #[serde(rename = "errorWhen")]
    pub error_when: String,
}

/// `TelemetrySpanDefinition`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetrySpanDefinition {
    pub description: String,
    pub parents: TelemetryParentDefinition,
    #[serde(rename = "startAttributes")]
    pub start_attributes: BTreeMap<String, TelemetryStartAttributeDefinition>,
    #[serde(rename = "endAttributes")]
    pub end_attributes: BTreeMap<String, TelemetryAttributeDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<BTreeMap<String, TelemetryEventDefinition>>,
    pub status: TelemetrySpanStatusDefinition,
}

/// `TelemetrySchemaDefinition { version, spans }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetrySchemaDefinition {
    pub version: u32,
    pub spans: BTreeMap<String, TelemetrySpanDefinition>,
}

/// `defineTelemetrySchema` — in TS ein typisierter Identitätshelfer.
pub fn define_telemetry_schema(schema: TelemetrySchemaDefinition) -> TelemetrySchemaDefinition {
    schema
}
