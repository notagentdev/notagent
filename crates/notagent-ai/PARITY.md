# Parity-Ledger: notagent-ai

TS-Quelle: `/Users/dev/projects/notagent-main/packages/ai` (22 744 LOC in src/) — Workstream B.

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

## Lektüre-Protokoll

| Datum | TS-/Referenz-Datei | LOC | gelesen von (Task) |
|---|---|---|---|
| 2026-08-13 | `packages/ai/src/types.ts` | 830 | 1 |
| 2026-08-13 | `packages/ai/src/index.ts` | 47 | 1 |
| 2026-08-13 | `packages/ai/src/utils/diagnostics.ts` | 45 | 1 |
| 2026-08-13 | `packages/ai/src/utils/event-stream.ts` | 88 | 1 |
| 2026-08-13 | `packages/ai/src/utils/uuid.ts` | 48 | 1 |
| 2026-08-13 | `packages/ai/src/utils/json-parse.ts` | 124 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/retry.ts` | 228 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/provider-retry.ts` | 125 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/overflow.ts` | 180 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/estimate.ts` | 143 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/validation.ts` | 350 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/text.ts` | 12 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/hash.ts` | 13 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/headers.ts` | 18 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/sanitize-unicode.ts` | 25 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/abort.ts` | 50 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/abort-signals.ts` | 41 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/error-body.ts` | 149 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/deferred-tools.ts` | 39 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/provider-env.ts` | 52 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/typebox-helpers.ts` | 24 | 3 (Lektüre vorgezogen) |
| 2026-08-13 | `packages/ai/src/utils/node-http-proxy.ts` | 112 | 3 (Lektüre vorgezogen) |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| `src/types.ts` | 830 | `types.rs` | verifiziert (Task 1) | Klasse 1: `Api`/`ProviderId`/`ImagesApi`/`ImagesProviderId` sind `String` (TS `KnownX \| (string & {})` ist zur Laufzeit ein String); bekannte Werte als `KNOWN_*`-Konstanten. Klasse 1: `ApiOptionsMap`/`ApiStreamOptions` sind reine Typ-Ebene und entfallen — die Dispatch-Form ist `StreamOptions`/`SimpleStreamOptions`, konkrete Optionstypen liegen bei den API-Modulen. Klasse 1: `Model.compat` wird api-abhängig deserialisiert (`ModelCompat`), da Rust keine bedingten Typen kennt; APIs ohne Zuordnung behalten den Rohwert (`ModelCompat::Other`). Klasse 1: Content-Blöcke tragen `extra: Map` (JS-Objektoffenheit) — erhält Scratch-Felder abgebrochener Streams (`partialJson`) verlustfrei, bug-compat. Klasse 1: `Usage.total_tokens` ist optional (historische TS-Session-Dateien enthalten das Feld nicht; `estimate.ts` behandelt `undefined`/`0` gleich). Klasse 3: `signal: AbortSignal` → `CancellationToken`; `fetch` → `FetchFn`-Trait; TypeBox-`TSchema` → `serde_json::Value`. Klasse 1: `js_number` bildet `JSON.stringify`-Zahlformatierung nach (`0` statt `0.0`). |
| `src/utils/diagnostics.ts` | 45 | `utils/diagnostics.rs` | portiert (Typen, Task 1) | Funktionen (`formatThrownValue`, `extractDiagnosticError`, …) folgen in Task 3 |
| `src/utils/event-stream.ts` | 88 | `utils/event_stream.rs` | portiert (Task 1, in Task 3 zu verifizieren) | Klasse 1: Waiter-Liste als `tokio::sync::Notify`; `result()` klont je Aufruf statt dieselbe Referenz zu liefern |
| `src/utils/uuid.ts` | 48 | `utils/uuid.rs` | portiert (Task 1, Tests in Task 3) | Klasse 1: Modulzustand als `Mutex` statt Modulvariablen |
| `src/index.ts` | 47 | `lib.rs` | teilweise (Task 1) | vollständige Oberfläche wird in Task 13 gezogen |
| — | — | `utils/js_number.rs` | verifiziert (Task 1) | Klasse 1 (Hilfsmodul ohne TS-Pendant): ECMAScript-`Number::toString` für byte-identische JSON-Zahlen |
| — | — | `utils/fetch.rs` | portiert (Task 1) | Klasse 3: Ersatz für `FetchFunction`; die konkrete Form kann Task 8 (SSE) nachschärfen |

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| `src/compat.ts` (298) | Master-Plan, Scope-Tabelle: im Code als Legacy markiert („wird mit ModelManager-Migration gelöscht") |
| `src/legacy-api-aliases.ts` (108) | wie oben |
| `src/api/*.lazy.ts` (9 Wrapper) | Master-Plan Task 10: Bundler-Wrapper; Rust linkt statisch |
| `scripts/generate-models.ts` | WS-B-Plan Task 5: der JSON-Snapshot ist die Quelle; Aktualisierung bleibt im TS-Repo |
