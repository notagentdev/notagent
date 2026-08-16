# Parity-Ledger: notagent-telemetry

TS-Quelle: `/Users/dev/projects/notagent-main/packages/telemetry` (935 LOC in src/) — Workstream B.

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

## Lektüre-Protokoll

| Datum | TS-/Referenz-Datei | LOC | gelesen von (Task) |
|---|---|---|---|
| 2026-08-13 | `packages/telemetry/src/index.ts` | 357 | 1/2 |
| 2026-08-13 | `packages/telemetry/src/memory.ts` | 219 | 2 |
| 2026-08-13 | `packages/telemetry/src/noop.ts` | 20 | 2 |
| 2026-08-13 | `packages/telemetry/src/testing/conformance.ts` | 315 | 2 |
| 2026-08-13 | `packages/telemetry/src/testing/types.ts` | 18 | 2 |
| 2026-08-13 | `packages/telemetry/src/testing/index.ts` | 6 | 2 |
| 2026-08-13 | `packages/telemetry/test/telemetry.test.ts` | 197 | 2 |
| 2026-08-13 | `packages/telemetry/test/conformance.test.ts` | 46 | 2 |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| `src/index.ts` | 357 | `lib.rs` | verifiziert (Task 2) | Klasse 1: `startSpan(options, callback)` ist in Rust nicht objekt-sicher — objekt-sicheres Primitiv `begin_span` plus generischer Wrapper (Task 2) mit identischer Settlement-Semantik. Klasse 1: Die TS-Typinferenz-Maschinerie (`InferStartAttributes`, `TypedSpanStarter`, …) entfällt laut Faktenbericht §5; die Schema-Datenstrukturen bleiben. Klasse 1: `SpanAttributes` ohne `undefined`-Werte (fehlender Schlüssel statt `undefined`) |
| `src/memory.ts` | 219 | `memory.rs` | verifiziert (Task 2) | Klasse 1: Zustand hinter `Arc<Mutex<..>>` statt Closure-Variablen; `getSpans()` liefert ebenso losgelöste Kopien. Die JS-`try/catch`-Blöcke gegen unlesbare Payloads entfallen (Rust-Werte können beim Lesen nicht werfen) |
| `src/noop.ts` | 20 | `noop.rs` | verifiziert (Task 2) | Klasse 1: `Object.freeze` entfällt; der geteilte inerte Span ist ein `OnceLock`-Singleton |
| `src/testing/conformance.ts` | 315 | `testing/conformance.rs` | verifiziert (Task 2) | Klasse 1: 6 der 9 Fälle portiert; drei Fälle prüfen ausschließlich JS-`Proxy`-Objekte, deren Lesezugriffe werfen — in Rust gegenstandslos (siehe Ausschlüsse). Der Nebenläufigkeitsfall nutzt im Suite-Code das objekt-sichere Primitiv; die echte Nebenläufigkeit des Wrappers prüft `tests/conformance.rs::concurrent_children_record_independent_parentage` |
| `src/testing/types.ts` | 18 | `testing/types.rs` | verifiziert (Task 2) | Klasse 1: `AsyncDisposable` entfällt; Aufräumen über `Drop` |
| `src/testing/index.ts` | 6 | `testing.rs` | verifiziert (Task 2) | |
| `test/telemetry.test.ts` | 197 | `tests/conformance.rs` | Tests portiert (Task 2) | Klasse 1: Die `expectTypeOf`/`@ts-expect-error`-Blöcke prüfen ausschließlich TS-Typinferenz und haben kein Laufzeitverhalten |
| `test/conformance.test.ts` | 46 | `tests/conformance.rs` | Tests portiert (Task 2) | |

## Datei-Abdeckung

`packages/telemetry` besteht aus sechs Quell- und zwei Testdateien; alle sind oben im
Ledger geführt und portiert. Es gibt keine weiteren Dateien im Paket.

| TS-Datei | LOC | Rust | Status |
|---|---|---|---|
| `src/index.ts` | 357 | `lib.rs` | verifiziert (Task 2) |
| `src/memory.ts` | 219 | `memory.rs` | verifiziert (Task 2) |
| `src/noop.ts` | 20 | `noop.rs` | verifiziert (Task 2) |
| `src/testing/conformance.ts` | 315 | `testing/conformance.rs` | verifiziert (Task 2) |
| `src/testing/types.ts` | 18 | `testing/types.rs` | verifiziert (Task 2) |
| `src/testing/index.ts` | 6 | `testing.rs` | verifiziert (Task 2) |
| `test/telemetry.test.ts` | 197 | `tests/conformance.rs` | portiert (Task 2) |
| `test/conformance.test.ts` | 46 | `tests/conformance.rs` | portiert (Task 2) |

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| Typ-Ebene aus `src/index.ts` (`AttributeDefinitionValue`, `Infer*`, `SchemaTelemetrySpan`, `TypedSpanStarter`, `createTypedSpanStarter`) | Faktenbericht §5 Port-Hinweis 4: „nur Datenstrukturen + Noop; TS-Typmaschinerie entfällt". Die reine Compile-Zeit-Prüfung hat kein Laufzeitverhalten |
| Konformanzfälle „ignores failed attribute calls atomically", „suppresses unreadable telemetry payload failures", „ignores failed status calls atomically" | Alle drei konstruieren JS-`Proxy`-Objekte, deren `get`/`ownKeys`-Traps werfen, und prüfen, dass der Adapter das schluckt. In Rust ist `SpanAttributes` eine fertige Map und `SpanOptions` ein Struct — Lesen kann nicht fehlschlagen, ein Teilerfolg beim Setzen ist unmöglich (Abweichung Klasse 1) |
