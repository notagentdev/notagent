# Parity-Ledger: notagent-client

TS-Quelle: `/Users/dev/projects/notagent-main/packages/client` (1 225 LOC in src/) — Workstream C.

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

## Lektüre-Protokoll

| Datum | TS-/Referenz-Datei | LOC | gelesen von (Task) |
|---|---|---|---|
| 2026-08-13 | packages/client/src/index.ts | 18 | C-Task 3 |
| 2026-08-13 | packages/client/src/types.ts | 26 | C-Task 3 |
| 2026-08-13 | packages/client/src/transport.ts | 18 | C-Task 3 |
| 2026-08-13 | packages/client/src/promise.ts | 16 | C-Task 3 |
| 2026-08-13 | packages/client/src/errors.ts | 56 | C-Task 3 |
| 2026-08-13 | packages/client/src/state.ts | 156 | C-Task 3 |
| 2026-08-13 | packages/client/src/connection.ts | 236 | C-Task 3 |
| 2026-08-13 | packages/client/src/session-handle.ts | 111 | C-Task 3 |
| 2026-08-13 | packages/client/src/client.ts | 432 | C-Task 3 |
| 2026-08-13 | packages/client/src/unix.ts | 156 | C-Task 3 |
| 2026-08-13 | packages/client/test/support.ts | 153 | C-Task 3 |
| 2026-08-13 | packages/client/test/state.test.ts | 146 | C-Task 3 |
| 2026-08-13 | packages/client/test/connection.test.ts | 327 | C-Task 3 |
| 2026-08-13 | packages/client/test/requests.test.ts | 68 | C-Task 3 |
| 2026-08-13 | packages/client/test/sessions.test.ts | 261 | C-Task 3 |
| 2026-08-13 | packages/client/test/disposal.test.ts | 54 | C-Task 3 |
| 2026-08-13 | packages/client/test/unix.test.ts | 218 | C-Task 3 |

Gelesen: 17 Dateien, 2 452 LOC — das gesamte Paket inklusive Tests.

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| src/index.ts | 18 | src/lib.rs | verifiziert | — (Re-Export-Barrel) |
| src/types.ts | 26 | src/types.rs | verifiziert | Klasse 1: `Unsubscribe` ist `Box<dyn FnOnce()>`; `ListenerErrorHandler` ist ein `Arc<dyn Fn>` |
| src/transport.ts | 18 | src/transport.rs | verifiziert | Klasse 1: `ByteTransport` als Trait mit `BoxFuture`; Handler-Trio als Struct aus drei `Arc<dyn Fn>` |
| src/promise.ts | 16 | src/promise.rs | verifiziert | Klasse 3: `Promise.withResolvers()` → `tokio::sync::oneshot`; mehrfach abwartbare Promises → `futures::future::Shared` |
| src/errors.ts | 56 | src/errors.rs | verifiziert | Klasse 1: fünf Fehlerklassen → ein Enum mit `name()`/`code()`/`message()`; Texte wortgleich |
| src/state.ts | 156 | src/state.rs | verifiziert | Klasse 1: Listener-Sets als Listen mit IDs; Listener-Aufrufe ohne gehaltenes Lock; JS-Exceptions eines Listeners → `catch_unwind` |
| src/connection.ts | 236 | src/connection.rs | verifiziert | Klasse 1: Objektidentitäts-Vergleich `#lifecycle !== connected` → Epoch-Zähler; `void openTransport()` → `tokio::spawn` |
| src/session-handle.ts | 111 | src/session_handle.rs | verifiziert | Klasse 1: `subscribe`/`onEvent` liefern `Result` statt zu werfen; `AsyncDisposable` entfällt, `dispose()` bleibt |
| src/client.ts | 432 | src/client.rs | verifiziert | Klasse 1: Eager-Send-Semantik der TS-async-Funktionen wird durch nicht-async Methoden nachgebildet, die ein Future liefern; Lease-Token per laufender Nummer statt Objektidentität; `dispose()` liefert ein geteiltes Versprechen statt eines identischen Promise-Objekts |
| src/unix.ts | 156 | src/unix.rs | verifiziert | Klasse 3: `node:net` → `tokio::net::UnixStream`; Schreib-Serialisierung über faire tokio-Mutex statt Promise-Kette (Reihenfolge und `maxPendingBytes` identisch) |
| test/support.ts | 153 | tests/support/mod.rs | Tests portiert | — |
| test/state.test.ts | 146 | tests/state.rs | verifiziert | — (4 Tests) |
| test/connection.test.ts | 327 | tests/connection.rs | verifiziert | 3 der 4 `maxFrameLength`-Fälle nicht darstellbar (u64) |
| test/requests.test.ts | 68 | tests/requests.rs | verifiziert | — (3 Tests) |
| test/sessions.test.ts | 261 | tests/sessions.rs | verifiziert | — (7 Tests) |
| test/disposal.test.ts | 54 | tests/disposal.rs | verifiziert | Promise-Identität (`toBe`) nicht prüfbar; stattdessen gleiches Ergebnis beider Aufrufe |
| test/unix.test.ts | 218 | tests/unix.rs | verifiziert | `code: "ENOENT"` → Meldungsprüfung (Klasse 1) |

Testergebnis: 36 Tests (4 state, 14 connection, 3 requests, 7 sessions, 3 disposal, 5 unix), alle grün.

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| dist/ | Build-Artefakt |
| tsconfig*.json, vitest.config.ts, package.json | Distributionsmechanik (Abweichungsklasse 4) |
