//! Konformanz- und Verhaltenstests des In-Memory-Adapters.
//!
//! Port von `packages/telemetry/test/conformance.test.ts` (46 LOC) und
//! `packages/telemetry/test/telemetry.test.ts` (197 LOC).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use notagent_telemetry::testing::conformance::create_telemetry_adapter_conformance;
use notagent_telemetry::testing::types::{BoxFuture, TelemetryAdapterFixture};
use notagent_telemetry::*;

struct InMemoryFixture {
    context: InMemoryTelemetryContext,
}

impl TelemetryAdapterFixture for InMemoryFixture {
    fn context(&self) -> Arc<dyn TelemetryContext> {
        Arc::new(self.context.clone())
    }

    fn get_spans(&self) -> BoxFuture<'_, Vec<RecordedTelemetrySpan>> {
        let spans = self.context.get_spans();
        Box::pin(async move { spans })
    }
}

fn attributes(pairs: &[(&str, AttributeValue)]) -> SpanAttributes {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect::<BTreeMap<_, _>>()
}

#[tokio::test]
async fn in_memory_context_passes_adapter_conformance() {
    let cases = create_telemetry_adapter_conformance(Arc::new(|| {
        Box::pin(async {
            Arc::new(InMemoryFixture {
                context: InMemoryTelemetryContext::new(),
            }) as Arc<dyn TelemetryAdapterFixture>
        })
    }));
    assert_eq!(
        cases.len(),
        6,
        "Alle portierbaren Konformanzfälle müssen registriert sein"
    );
    for case in cases {
        case.run().await;
    }
}

#[tokio::test]
async fn returns_detached_snapshots_without_exposing_recording_state() {
    let context = InMemoryTelemetryContext::new();
    let recorded = context.clone();
    let (open_settled, open_end_sequence) = start_span(
        &context,
        SpanOptions::with_attributes(
            "snapshot",
            attributes(&[(
                "tags",
                AttributeValue::StringArray(vec!["initial".to_string()]),
            )]),
        ),
        |span| async move {
            span.add_event(
                "event",
                Some(attributes(&[("value", AttributeValue::Number(1.0))])),
            );
            let open = recorded.get_spans();
            (open[0].settled, open[0].end_sequence)
        },
    )
    .await;

    assert!(!open_settled);
    assert_eq!(open_end_sequence, None);

    let mut first = context.get_spans();
    assert!(first[0].settled);
    assert_eq!(first[0].end_sequence, Some(1));

    // Der Schnappschuss ist losgelöst: Änderungen daran erreichen die Aufzeichnung nicht.
    first[0].attributes.insert(
        "tags".to_string(),
        AttributeValue::StringArray(vec!["mutated".to_string()]),
    );
    first[0].events[0]
        .attributes
        .insert("value".to_string(), AttributeValue::Number(2.0));

    let second = context.get_spans();
    assert_eq!(
        second[0].attributes,
        attributes(&[(
            "tags",
            AttributeValue::StringArray(vec!["initial".to_string()])
        )])
    );
    assert_eq!(
        second[0].events[0].attributes,
        attributes(&[("value", AttributeValue::Number(1.0))])
    );
}

#[tokio::test]
async fn noop_context_admits_callbacks_and_reuses_one_inert_span() {
    let admitted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&admitted);
    let context = noop_telemetry_context();
    let result = start_span(
        context.as_ref(),
        SpanOptions::new("first"),
        |span| async move {
            flag.store(true, Ordering::SeqCst);
            let child = start_span(
                span.as_ref(),
                SpanOptions::new("child"),
                |child| async move { child },
            )
            .await;
            // TS: `expect(child).toBe(span)` — derselbe inerte Span.
            assert!(Arc::ptr_eq(&child, &span));
            42
        },
    )
    .await;

    assert!(admitted.load(Ordering::SeqCst));
    assert_eq!(result, 42);
}

#[tokio::test]
async fn noop_context_preserves_rejection_values() {
    #[derive(Debug, PartialEq, Eq, thiserror::Error)]
    #[error("{0}")]
    struct Failure(&'static str);

    let context = noop_telemetry_context();
    let result: Result<(), Failure> =
        try_start_span(context.as_ref(), SpanOptions::new("sync"), |_span| async {
            Err(Failure("sync"))
        })
        .await;
    assert_eq!(result.unwrap_err(), Failure("sync"));
}

#[tokio::test]
async fn noop_context_does_not_retain_payloads() {
    let context = noop_telemetry_context();
    start_span(
        context.as_ref(),
        SpanOptions::with_attributes(
            "operation",
            attributes(&[("secret", AttributeValue::from("prompt"))]),
        ),
        |span| async move {
            span.add_event(
                "event",
                Some(attributes(&[("secret", AttributeValue::from("content"))])),
            );
            span.set_attributes(attributes(&[("secret", AttributeValue::from("content"))]));
            span.set_status(SpanStatus::Ok);
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_children_record_independent_parentage() {
    // Ergänzung zum Konformanzfall: prüft dieselbe Aussage unter echter Nebenläufigkeit
    // des generischen Wrappers (TS nutzt dafür ein Promise-Gate).
    let context = InMemoryTelemetryContext::new();
    let (release, wait) = tokio::sync::oneshot::channel::<()>();

    start_span(&context, SpanOptions::new("parent"), |parent| async move {
        let first_parent = Arc::clone(&parent);
        let first = start_span(
            first_parent.as_ref(),
            SpanOptions::new("first-child"),
            |_span| async move {
                wait.await.expect("Gate");
            },
        );
        let second = start_span(
            parent.as_ref(),
            SpanOptions::new("second-child"),
            |_span| async move { "done" },
        );
        let (second_result, _) = tokio::join!(
            async {
                let second_value = second.await;
                release.send(()).expect("Gate freigeben");
                second_value
            },
            first
        );
        assert_eq!(second_result, "done");
    })
    .await;

    let spans = context.get_spans();
    let parent = spans
        .iter()
        .find(|span| span.name == "parent")
        .expect("parent");
    let first = spans
        .iter()
        .find(|span| span.name == "first-child")
        .expect("first-child");
    let second = spans
        .iter()
        .find(|span| span.name == "second-child")
        .expect("second-child");
    assert_eq!(parent.parent_id, None);
    assert_eq!(first.parent_id, Some(parent.id));
    assert_eq!(second.parent_id, Some(parent.id));
    assert!(second.end_sequence.unwrap() < first.end_sequence.unwrap());
    assert!(first.end_sequence.unwrap() < parent.end_sequence.unwrap());
}

#[tokio::test]
async fn schema_definitions_stay_serializable() {
    // TS: `expect(() => JSON.stringify(schema)).not.toThrow()` plus Feldtreue.
    let json = serde_json::json!({
        "version": 1,
        "spans": {
            "operation": {
                "description": "Test operation",
                "parents": {"kind": "any"},
                "startAttributes": {
                    "kind": {"type": "string", "required": true, "values": ["read", "write"], "description": "Kind"}
                },
                "endAttributes": {},
                "events": {
                    "result": {
                        "description": "Result",
                        "attributes": {
                            "outcome": {"type": "string", "required": true, "values": ["ok", "error"],
                                        "description": "Outcome"}
                        }
                    }
                },
                "status": {"default": "ok", "errorWhen": "The operation fails"}
            }
        }
    });
    let schema: TelemetrySchemaDefinition = serde_json::from_value(json.clone()).expect("Schema");
    assert_eq!(schema.version, 1);
    assert_eq!(serde_json::to_value(&schema).expect("Serialisierung"), json);
}
