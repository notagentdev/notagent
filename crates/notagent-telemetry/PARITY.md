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
| `src/index.ts` | 357 | `lib.rs` | portiert (Kern, Task 1) | Klasse 1: `startSpan(options, callback)` ist in Rust nicht objekt-sicher — objekt-sicheres Primitiv `begin_span` plus generischer Wrapper (Task 2) mit identischer Settlement-Semantik. Klasse 1: Die TS-Typinferenz-Maschinerie (`InferStartAttributes`, `TypedSpanStarter`, …) entfällt laut Faktenbericht §5; die Schema-Datenstrukturen bleiben. Klasse 1: `SpanAttributes` ohne `undefined`-Werte (fehlender Schlüssel statt `undefined`) |
| `src/memory.ts` | 219 | `memory.rs` | offen (Task 2) | |
| `src/noop.ts` | 20 | `noop.rs` | offen (Task 2) | |
| `src/testing/conformance.ts` | 315 | `tests/conformance.rs` | offen (Task 2) | |

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| Typ-Ebene aus `src/index.ts` (`AttributeDefinitionValue`, `Infer*`, `SchemaTelemetrySpan`, `TypedSpanStarter`, `createTypedSpanStarter`) | Faktenbericht §5 Port-Hinweis 4: „nur Datenstrukturen + Noop; TS-Typmaschinerie entfällt". Die reine Compile-Zeit-Prüfung hat kein Laufzeitverhalten |
