//! Runner-unabhängige Fälle für den Callback-Vertrag eines Telemetrie-Adapters.
//!
//! 1:1-Port von `packages/telemetry/src/testing/conformance.ts` (315 LOC).
//!
//! Nicht portierbar sind die drei Fälle, die mit JS-`Proxy`-Objekten arbeiten, deren
//! Lesezugriffe werfen („ignores failed attribute calls atomically", „suppresses
//! unreadable telemetry payload failures", „ignores failed status calls atomically").
//! Rust-Werte können beim Lesen nicht fehlschlagen: `SpanAttributes` ist eine fertige
//! Map, `set_attributes` bekommt sie als Ganzes. Die Fälle sind damit gegenstandslos
//! (Abweichung Klasse 1, siehe PARITY.md).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::testing::types::{BoxFuture, TelemetryAdapterFixture, TelemetryAdapterFixtureFactory};
use crate::{
    AttributeValue, RecordedTelemetrySpan, SpanAttributes, SpanOptions, SpanOutcome, SpanStatus,
    SpanStatusError, start_span, try_start_span,
};

type CaseBody =
    Arc<dyn Fn(Arc<dyn TelemetryAdapterFixture>) -> BoxFuture<'static, ()> + Send + Sync>;

/// `TelemetryAdapterConformanceCase { group, name, run() }`
#[derive(Clone)]
pub struct TelemetryAdapterConformanceCase {
    pub group: &'static str,
    pub name: &'static str,
    factory: TelemetryAdapterFixtureFactory,
    body: CaseBody,
}

impl TelemetryAdapterConformanceCase {
    pub async fn run(&self) {
        let fixture = (self.factory)().await;
        (self.body)(fixture).await;
    }
}

fn find_span(spans: &[RecordedTelemetrySpan], name: &str) -> RecordedTelemetrySpan {
    spans
        .iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| panic!("Expected recorded span {name}"))
        .clone()
}

fn attributes(pairs: &[(&str, AttributeValue)]) -> SpanAttributes {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect::<BTreeMap<_, _>>()
}

fn case(
    factory: &TelemetryAdapterFixtureFactory,
    group: &'static str,
    name: &'static str,
    body: impl Fn(Arc<dyn TelemetryAdapterFixture>) -> BoxFuture<'static, ()> + Send + Sync + 'static,
) -> TelemetryAdapterConformanceCase {
    TelemetryAdapterConformanceCase {
        group,
        name,
        factory: Arc::clone(factory),
        body: Arc::new(body),
    }
}

/// Fehlerwert der Konformanzfälle (TS wirft beliebige Werte; hier genügt ein Typ,
/// dessen Identität geprüft werden kann).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConformanceError(pub String);

/// `createTelemetryAdapterConformance(factory)`
pub fn create_telemetry_adapter_conformance(
    factory: TelemetryAdapterFixtureFactory,
) -> Vec<TelemetryAdapterConformanceCase> {
    vec![
        case(
            &factory,
            "callback lifecycle",
            "admits once and preserves the result",
            |fixture| {
                Box::pin(async move {
                    let calls = Arc::new(AtomicUsize::new(0));
                    let counter = Arc::clone(&calls);
                    let expected = 42;
                    let result = start_span(
                        fixture.context().as_ref(),
                        SpanOptions::new("success"),
                        |_span| async move {
                            counter.fetch_add(1, Ordering::SeqCst);
                            expected
                        },
                    )
                    .await;

                    assert_eq!(calls.load(Ordering::SeqCst), 1);
                    assert_eq!(result, expected);
                    let spans = fixture.get_spans().await;
                    assert_eq!(find_span(&spans, "success").status, SpanStatus::Ok);
                    assert!(find_span(&spans, "success").settled);
                })
            },
        ),
        case(
            &factory,
            "callback lifecycle",
            "preserves rejection values",
            |fixture| {
                Box::pin(async move {
                    let sync_error = ConformanceError("sync".to_string());
                    let returned: Result<(), ConformanceError> = try_start_span(
                        fixture.context().as_ref(),
                        SpanOptions::new("sync-error"),
                        |_span| {
                            let error = sync_error.clone();
                            async move { Err(error) }
                        },
                    )
                    .await;
                    assert_eq!(returned.unwrap_err(), sync_error);

                    let async_error = ConformanceError("async".to_string());
                    let returned: Result<(), ConformanceError> = try_start_span(
                        fixture.context().as_ref(),
                        SpanOptions::new("async-error"),
                        |_span| {
                            let error = async_error.clone();
                            async move { Err(error) }
                        },
                    )
                    .await;
                    assert_eq!(returned.unwrap_err(), async_error);

                    let spans = fixture.get_spans().await;
                    for name in ["sync-error", "async-error"] {
                        assert!(matches!(
                            find_span(&spans, name).status,
                            SpanStatus::Error { .. }
                        ));
                    }
                })
            },
        ),
        case(
            &factory,
            "status",
            "uses last explicit status without automatic overwrite",
            |fixture| {
                Box::pin(async move {
                    let context = fixture.context();
                    start_span(
                        context.as_ref(),
                        SpanOptions::new("last-status"),
                        |span| async move {
                            span.set_status(SpanStatus::error_with("Expected", "first"));
                            span.set_status(SpanStatus::Ok);
                        },
                    )
                    .await;

                    let thrown = ConformanceError("after explicit status".to_string());
                    let returned: Result<(), ConformanceError> = try_start_span(
                        context.as_ref(),
                        SpanOptions::new("explicit-before-throw"),
                        |span| {
                            let error = thrown.clone();
                            async move {
                                span.set_status(SpanStatus::Ok);
                                Err(error)
                            }
                        },
                    )
                    .await;
                    assert_eq!(returned.unwrap_err(), thrown);

                    let rejected = ConformanceError("after async explicit status".to_string());
                    let returned: Result<(), ConformanceError> = try_start_span(
                        context.as_ref(),
                        SpanOptions::new("explicit-before-rejection"),
                        |span| {
                            let error = rejected.clone();
                            async move {
                                span.set_status(SpanStatus::error_with(
                                    "Expected",
                                    "async failure",
                                ));
                                Err(error)
                            }
                        },
                    )
                    .await;
                    assert_eq!(returned.unwrap_err(), rejected);

                    start_span(
                        context.as_ref(),
                        SpanOptions::new("expected-failure"),
                        |span| async move {
                            span.set_status(SpanStatus::error_with("Expected", "returned failure"));
                        },
                    )
                    .await;

                    let spans = fixture.get_spans().await;
                    assert_eq!(find_span(&spans, "last-status").status, SpanStatus::Ok);
                    assert_eq!(
                        find_span(&spans, "explicit-before-throw").status,
                        SpanStatus::Ok
                    );
                    assert_eq!(
                        find_span(&spans, "explicit-before-rejection").status,
                        SpanStatus::Error {
                            error: Some(SpanStatusError {
                                name: "Expected".to_string(),
                                message: "async failure".to_string()
                            })
                        }
                    );
                    assert_eq!(
                        find_span(&spans, "expected-failure").status,
                        SpanStatus::Error {
                            error: Some(SpanStatusError {
                                name: "Expected".to_string(),
                                message: "returned failure".to_string()
                            })
                        }
                    );
                })
            },
        ),
        case(
            &factory,
            "recording",
            "merges attributes and records ordered events",
            |fixture| {
                Box::pin(async move {
                    start_span(
                        fixture.context().as_ref(),
                        SpanOptions::with_attributes(
                            "recording",
                            attributes(&[
                                ("start", AttributeValue::from("value")),
                                ("overwrite", AttributeValue::from("start")),
                            ]),
                        ),
                        |span| async move {
                            span.set_attributes(attributes(&[
                                ("count", AttributeValue::Number(1.0)),
                                ("overwrite", AttributeValue::from("middle")),
                            ]));
                            // TS setzt hier zusätzlich `count: undefined`; ein fehlender Schlüssel
                            // ist das Rust-Äquivalent und lässt den Wert ebenso unangetastet.
                            span.set_attributes(attributes(&[(
                                "overwrite",
                                AttributeValue::from("end"),
                            )]));
                            span.add_event(
                                "first",
                                Some(attributes(&[("index", AttributeValue::Number(1.0))])),
                            );
                            span.add_event(
                                "second",
                                Some(attributes(&[("index", AttributeValue::Number(2.0))])),
                            );
                        },
                    )
                    .await;

                    let spans = fixture.get_spans().await;
                    let span = find_span(&spans, "recording");
                    assert_eq!(
                        span.attributes,
                        attributes(&[
                            ("start", AttributeValue::from("value")),
                            ("overwrite", AttributeValue::from("end")),
                            ("count", AttributeValue::Number(1.0)),
                        ])
                    );
                    assert_eq!(span.events.len(), 2);
                    assert_eq!(span.events[0].name, "first");
                    assert_eq!(span.events[1].name, "second");
                    assert_eq!(
                        span.events[0].attributes,
                        attributes(&[("index", AttributeValue::Number(1.0))])
                    );
                    assert_eq!(
                        span.events[1].attributes,
                        attributes(&[("index", AttributeValue::Number(2.0))])
                    );
                })
            },
        ),
        case(
            &factory,
            "recording",
            "makes calls after settlement inert",
            |fixture| {
                Box::pin(async move {
                    let captured = Arc::new(std::sync::Mutex::new(None));
                    let sink = Arc::clone(&captured);
                    start_span(
                        fixture.context().as_ref(),
                        SpanOptions::with_attributes(
                            "settled",
                            attributes(&[("value", AttributeValue::from("initial"))]),
                        ),
                        |span| async move {
                            *sink.lock().expect("Mutex") = Some(span);
                        },
                    )
                    .await;
                    let span = captured
                        .lock()
                        .expect("Mutex")
                        .clone()
                        .expect("Expected callback span");

                    span.set_attributes(attributes(&[("value", AttributeValue::from("late"))]));
                    span.add_event(
                        "late",
                        Some(attributes(&[("value", AttributeValue::Boolean(true))])),
                    );
                    span.set_status(SpanStatus::error());

                    let mut child_admitted = false;
                    let child_result =
                        start_span(span.as_ref(), SpanOptions::new("late-child"), |_child| {
                            child_admitted = true;
                            async move { 7 }
                        })
                        .await;
                    assert!(child_admitted);
                    assert_eq!(child_result, 7);

                    let spans = fixture.get_spans().await;
                    assert_eq!(spans.len(), 1);
                    assert_eq!(
                        spans[0].attributes,
                        attributes(&[("value", AttributeValue::from("initial"))])
                    );
                    assert!(spans[0].events.is_empty());
                    assert_eq!(spans[0].status, SpanStatus::Ok);
                })
            },
        ),
        case(
            &factory,
            "parentage",
            "records nested and concurrent child relationships",
            |fixture| {
                Box::pin(async move {
                    // TS hält das erste Kind über ein Promise offen, während das zweite bereits
                    // abschließt. Ohne Kombinator-Bibliothek wird derselbe beobachtbare Ablauf
                    // über das objekt-sichere Primitiv nachgestellt: Startreihenfolge erst, zweites
                    // Kind schließt vor dem ersten, das Elternteil zuletzt.
                    start_span(
                        fixture.context().as_ref(),
                        SpanOptions::new("parent"),
                        |parent| async move {
                            let first = parent.begin_span(SpanOptions::new("first-child"));
                            let second = parent.begin_span(SpanOptions::new("second-child"));
                            second.settle(SpanOutcome::Completed);
                            first.settle(SpanOutcome::Completed);
                        },
                    )
                    .await;

                    let spans = fixture.get_spans().await;
                    let parent = find_span(&spans, "parent");
                    let first = find_span(&spans, "first-child");
                    let second = find_span(&spans, "second-child");
                    assert_eq!(parent.parent_id, None);
                    assert_eq!(first.parent_id, Some(parent.id));
                    assert_eq!(second.parent_id, Some(parent.id));
                    let (first_end, second_end, parent_end) = (
                        first.end_sequence.expect("endSequence"),
                        second.end_sequence.expect("endSequence"),
                        parent.end_sequence.expect("endSequence"),
                    );
                    assert!(second_end < first_end);
                    assert!(first_end < parent_end);
                })
            },
        ),
    ]
}
