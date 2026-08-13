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
| 2026-08-13 | `packages/ai/src/utils/json-parse.ts` | 124 | 3 |
| 2026-08-13 | `packages/ai/src/utils/retry.ts` | 228 | 3 |
| 2026-08-13 | `packages/ai/src/utils/provider-retry.ts` | 125 | 3 |
| 2026-08-13 | `packages/ai/src/utils/overflow.ts` | 180 | 3 |
| 2026-08-13 | `packages/ai/src/utils/estimate.ts` | 143 | 3 |
| 2026-08-13 | `packages/ai/src/utils/validation.ts` | 350 | 3 |
| 2026-08-13 | `packages/ai/src/utils/text.ts` | 12 | 3 |
| 2026-08-13 | `packages/ai/src/utils/hash.ts` | 13 | 3 |
| 2026-08-13 | `packages/ai/src/utils/headers.ts` | 18 | 3 |
| 2026-08-13 | `packages/ai/src/utils/sanitize-unicode.ts` | 25 | 3 |
| 2026-08-13 | `packages/ai/src/utils/abort.ts` | 50 | 3 |
| 2026-08-13 | `packages/ai/src/utils/abort-signals.ts` | 41 | 3 |
| 2026-08-13 | `packages/ai/src/utils/error-body.ts` | 149 | 3 |
| 2026-08-13 | `packages/ai/src/utils/deferred-tools.ts` | 39 | 3 |
| 2026-08-13 | `packages/ai/src/utils/provider-env.ts` | 52 | 3 |
| 2026-08-13 | `packages/ai/src/utils/typebox-helpers.ts` | 24 | 3 |
| 2026-08-13 | `packages/ai/src/utils/node-http-proxy.ts` | 112 | 3 |
| 2026-08-13 | `packages/ai/test/validation.test.ts` | 210 | 3 |
| 2026-08-13 | `packages/ai/test/retry.test.ts` | 223 | 3 |
| 2026-08-13 | `packages/ai/test/provider-retry.test.ts` | 81 | 3 |
| 2026-08-13 | `packages/ai/test/overflow.test.ts` | 177 | 3 |
| 2026-08-13 | `packages/ai/test/context-estimate.test.ts` | 81 | 3 |
| 2026-08-13 | `packages/ai/test/text.test.ts` | 33 | 3 |
| 2026-08-13 | `packages/ai/test/uuid.test.ts` | 50 | 3 |
| 2026-08-13 | `packages/ai/test/node-http-proxy.test.ts` | 76 | 3 |
| 2026-08-13 | `node_modules/partial-json/dist/{index,options}.js` (Referenz) | 220 + 60 | 3 |
| 2026-08-13 | `node_modules/typebox/build/value/convert/**` (Referenz) | 285 | 3 |
| 2026-08-13 | `packages/ai/src/models.ts` | 944 | 4 |
| 2026-08-13 | `packages/ai/src/models-store.ts` | 45 | 4 |
| 2026-08-13 | `packages/ai/src/auth/types.ts` | 240 | 6 |
| 2026-08-13 | `packages/ai/src/auth/credential-store.ts` | 67 | 6 |
| 2026-08-13 | `packages/ai/src/auth/resolve.ts` | 205 | 6 |
| 2026-08-13 | `packages/ai/src/auth/context.ts` | 45 | 6 |
| 2026-08-13 | `packages/ai/src/auth/helpers.ts` | 59 | 6 |
| 2026-08-13 | `packages/ai/src/env-api-keys.ts` | 188 | 6 |
| 2026-08-13 | `packages/ai/test/env-api-keys.test.ts` | 116 | 6 |
| 2026-08-13 | `packages/ai/test/oauth-auth.test.ts` | 166 | 6 |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| `src/types.ts` | 830 | `types.rs` | verifiziert (Task 1) | Klasse 1: `Api`/`ProviderId`/`ImagesApi`/`ImagesProviderId` sind `String` (TS `KnownX \| (string & {})` ist zur Laufzeit ein String); bekannte Werte als `KNOWN_*`-Konstanten. Klasse 1: `ApiOptionsMap`/`ApiStreamOptions` sind reine Typ-Ebene und entfallen — die Dispatch-Form ist `StreamOptions`/`SimpleStreamOptions`, konkrete Optionstypen liegen bei den API-Modulen. Klasse 1: `Model.compat` wird api-abhängig deserialisiert (`ModelCompat`), da Rust keine bedingten Typen kennt; APIs ohne Zuordnung behalten den Rohwert (`ModelCompat::Other`). Klasse 1: Content-Blöcke tragen `extra: Map` (JS-Objektoffenheit) — erhält Scratch-Felder abgebrochener Streams (`partialJson`) verlustfrei, bug-compat. Klasse 1: `Usage.total_tokens` ist optional (historische TS-Session-Dateien enthalten das Feld nicht; `estimate.ts` behandelt `undefined`/`0` gleich). Klasse 3: `signal: AbortSignal` → `CancellationToken`; `fetch` → `FetchFn`-Trait; TypeBox-`TSchema` → `serde_json::Value`. Klasse 1: `js_number` bildet `JSON.stringify`-Zahlformatierung nach (`0` statt `0.0`). |
| `src/utils/diagnostics.ts` | 45 | `utils/diagnostics.rs` | portiert (Task 3) | Klasse 1: TS unterscheidet `Error`-Instanzen von beliebig geworfenen Werten; in Rust trennen das zwei Funktionen (`extract_diagnostic_error`, `thrown_value_diagnostic`). `stack` gibt es nicht |
| `src/utils/event-stream.ts` | 88 | `utils/event_stream.rs` | verifiziert (Task 3) | Klasse 1: Waiter-Liste als `tokio::sync::Notify`; `result()` klont je Aufruf statt dieselbe Referenz zu liefern |
| `src/utils/uuid.ts` | 48 | `utils/uuid.rs` | verifiziert (Task 3) | Klasse 1: Modulzustand als `Mutex`; Uhr und Zufall sind für den portierten Test injizierbar (TS stubbt `Date.now`/`crypto`) |
| `src/utils/json-parse.ts` | 124 | `utils/json_parse.rs` | verifiziert (Task 3) | Klasse 3: `partial-json` (220 LOC) mitportiert, immer mit `Allow.ALL`. Klasse 1: Indizes zählen Unicode-Skalare statt UTF-16-Einheiten (alle strukturellen Zeichen sind ASCII); `NaN`/`Infinity` werden zu `null` wie bei `JSON.stringify`; Zahlen werden auf JS-Semantik normalisiert (ein f64-Typ) |
| `src/utils/retry.ts` | 228 | `utils/retry.rs` | verifiziert (Task 3) | Klasse 3: `AbortSignal` → `CancellationToken`; Klasse 1: Callbacks als `Arc<dyn Fn>` |
| `src/utils/provider-retry.ts` | 125 | `utils/provider_retry.rs` | verifiziert (Task 3) | Klasse 3: Die SDK-Fehlerform wird zum Trait `ProviderErrorInfo`, das die HTTP-Schicht implementiert |
| `src/utils/overflow.ts` | 180 | `utils/overflow.rs` | verifiziert (Task 3) | alle 25 Overflow- und 3 Nicht-Overflow-Muster in Quellreihenfolge |
| `src/utils/estimate.ts` | 143 | `utils/estimate.rs` | verifiziert (Task 3) | Klasse 1: Zeichenlängen zählen UTF-16-Einheiten wie `String.length` |
| `src/utils/validation.ts` | 350 | `utils/validation.rs` | verifiziert (Task 3) | Klasse 3: TypeBox `Compile().Check/.Errors` und `Value.Convert` mitportiert, inklusive der AJV-Meldungen. Klasse 1: TypeBox markiert seine Schemas mit einem Laufzeit-Symbol ohne JSON-Entsprechung — die Herkunft wird explizit übergeben (`SchemaOrigin`, Default `TypeBox`, weil alle App-Tools so definiert sind); empirisch belegt: `Value.Convert` ist für Plain-Schemas eine No-Op |
| `src/utils/text.ts` | 12 | `utils/text.rs` | verifiziert (Task 3) | Klasse 1: Überladung über `ContentRef` statt einer union-typisierten Signatur |
| `src/utils/hash.ts` | 13 | `utils/hash.rs` | portiert (Task 3) | Klasse 1: `charCodeAt` → `encode_utf16`; `toString(36)` nachgebildet |
| `src/utils/headers.ts` | 18 | `utils/headers.rs` | portiert (Task 3) | |
| `src/utils/sanitize-unicode.ts` | 25 | `utils/sanitize_unicode.rs` | portiert (Task 3) | Klasse 1: Rust-Strings können keine unpaarigen Surrogate enthalten; die Funktion ist die Identität, die UTF-16-Variante bleibt für Provider-Payloads |
| `src/utils/abort.ts` + `abort-signals.ts` | 91 | `utils/abort.rs` | portiert (Task 3) | Klasse 3: `AbortController` → `CancellationToken`; `combineAbortSignals` leitet über eine Task weiter, `cleanup()` bricht sie ab |
| `src/utils/error-body.ts` | 149 | `utils/error_body.rs` | portiert (Task 3) | Klasse 3: Statt SDK-Feldnamen zu erraten, liefert die HTTP-Schicht `RawProviderError`; Tests folgen in Task 13 |
| `src/utils/deferred-tools.ts` | 39 | `utils/deferred_tools.rs` | portiert (Task 3) | Tests folgen in Task 13 |
| `src/utils/provider-env.ts` | 52 | `utils/provider_env.rs` | portiert (Task 3) | Klasse 4: Der Bun-Sandbox-Fallback (`/proc/self/environ`, oven-sh/bun#27802) entfällt |
| `src/utils/node-http-proxy.ts` | 112 | `utils/node_http_proxy.rs` | verifiziert (Task 3) | Klasse 3: liefert die Proxy-URL für reqwest statt eines undici-Agents |
| `src/auth/types.ts` | 240 | `auth/types.rs` | portiert (Task 6) | Klasse 1: Interfaces mit Methoden werden Traits (`CredentialStore`, `AuthContext`, `ApiKeyAuth`, `OAuthAuth`, `AuthInteraction`); `modify` nimmt eine `FnOnce`, die leihen darf, damit der OAuth-Refresh unter dem Lock laufen kann. `OAuthCredential` behält Zusatzfelder (`extra`) wie die TS-Index-Signatur |
| `src/auth/credential-store.ts` | 67 | `auth/credential_store.rs` | verifiziert (Task 6) | Klasse 1: Serialisierung je Provider-ID über einen async-Mutex statt einer Promise-Kette |
| `src/auth/resolve.ts` | 205 | `auth/resolve.rs` | verifiziert (Task 6) | Klasse 3: `AbortSignal.any([signal, timeout])` → `child_token` plus Timeout-Task |
| `src/auth/context.ts` | 45 | `auth/context.rs` | portiert (Task 6) | Klasse 1: Die Browser-Guards (dynamischer `import`, fehlendes `process.env`) entfallen |
| `src/auth/helpers.ts` | 59 | `auth/helpers.rs` | portiert (Task 6) | Klasse 4: `lazyOAuth` entfällt — es verzögert einen dynamischen `import()` für Bundler; Rust linkt statisch |
| `src/env-api-keys.ts` | 188 | `env_api_keys.rs` | verifiziert (Task 6) | Klasse 1: Die verzögerte Node-Modul-Ladung entfällt; der ADC-Cache bleibt |
| `src/utils/typebox-helpers.ts` | 24 | — | ausgeschlossen (Task 3) | `StringEnum` erzeugt ein TypeBox-Schema; in Rust ist das ein JSON-Literal `{"type":"string","enum":[…]}` (Substitution Klasse 3) |
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
