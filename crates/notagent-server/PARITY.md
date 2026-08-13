# Parity-Ledger: notagent-server

TS-Quelle: `/Users/dev/projects/notagent-main/packages/server` (2 299 LOC in src/) — Workstream C.

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

## Lektüre-Protokoll

| Datum | TS-/Referenz-Datei | LOC | gelesen von (Task) |
|---|---|---|---|
| 2026-08-13 | packages/server/src/index.ts | 5 | C-Task 3 |
| 2026-08-13 | packages/server/src/types.ts | 63 | C-Task 3 |
| 2026-08-13 | packages/server/src/errors.ts | 58 | C-Task 3 |
| 2026-08-13 | packages/server/src/connection.ts | 36 | C-Task 3 |
| 2026-08-13 | packages/server/src/listener.ts | 10 | C-Task 3 |
| 2026-08-13 | packages/server/src/snapshots.ts | 62 | C-Task 3 |
| 2026-08-13 | packages/server/src/server.ts | 396 | C-Task 3 |
| 2026-08-13 | packages/server/src/sessions.ts | 346 | C-Task 3 |
| 2026-08-13 | packages/server/src/protocol.ts | 382 | C-Task 3 |
| 2026-08-13 | packages/server/src/transports/unix/listener.ts | 434 | C-Task 3 |
| 2026-08-13 | packages/server/src/transports/unix/types.ts | 15 | C-Task 3 |
| 2026-08-13 | packages/server/src/transports/unix/preset.ts | 23 | C-Task 3 |
| 2026-08-13 | packages/server/src/transports/unix/index.ts | 3 | C-Task 3 |
| 2026-08-13 | packages/server/src/testing/service.ts | 287 | C-Task 3 |
| 2026-08-13 | packages/server/src/testing/client.ts | 147 | C-Task 3 |
| 2026-08-13 | packages/server/src/testing/server.ts | 27 | C-Task 3 |
| 2026-08-13 | packages/server/src/testing/index.ts | 5 | C-Task 3 |

| 2026-08-13 | packages/server/test/conformance.test.ts | 378 | C-Task 3 |
| 2026-08-13 | packages/server/test/sessions.test.ts | 429 | C-Task 3 |
| 2026-08-13 | packages/server/test/protocol.test.ts | 301 | C-Task 3 |
| 2026-08-13 | packages/server/test/unix.test.ts | 117 | C-Task 3 |
| 2026-08-13 | packages/server/test/server.test.ts | 100 | C-Task 3 |
| 2026-08-13 | packages/server/test/unix-connection.test.ts | 65 | C-Task 3 |
| 2026-08-13 | packages/server/test/listener.test.ts | 57 | C-Task 3 |

Gelesen: 24 Dateien, 3 746 LOC — das gesamte Paket inklusive Tests.

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| src/index.ts | 5 | src/lib.rs | verifiziert | — (Re-Export-Barrel) |
| src/types.ts | 63 | src/types.rs | verifiziert | Klasse 1: `PiServerService`/`PiSessionRuntime` als `#[async_trait]`-Traits; `subscribe` liefert `Box<dyn FnOnce>` |
| src/errors.ts | 58 | src/errors.rs | verifiziert | Klasse 1: Fehlerklassen → `PiServerError`-Struct plus `ServerError`-Enum für die Boundary (`Internal` hält die Ursache, serialisiert sie nie) |
| src/connection.ts | 36 | src/connection.rs | verifiziert | Klasse 1: `ConnectionState` mit innerem Mutex; `isTerminalConnection` als Methode |
| src/listener.ts | 10 | src/listener.rs | verifiziert | Klasse 1: Trait mit `async_trait`; Acceptor als `Arc<dyn Fn>` |
| src/snapshots.ts | 62 | src/snapshots.rs + server.rs (`perform_broadcast`) | verifiziert | Klasse 1: die Broadcast-Warteschlange (Promise-Kette) wird zur fairen `tokio::sync::Mutex`; der Publisher hält keine Callbacks, `PiServer` ruft ihn direkt |
| src/server.ts | 396 | src/server.rs | verifiziert | Klasse 1: `void promise`-Fire-and-forget → `tokio::spawn`; Handshake-Timeout als abbrechbarer Task; Stage-Automat und Fehler-Sanitizing 1:1 |
| src/sessions.ts | 346 | src/sessions.rs | verifiziert | Klasse 1: Callback-Objekt → `Weak<PiServerInner>`; `disposing`/`opening` als `futures::future::Shared`; Connection-Sets über `Arc::ptr_eq` |
| src/protocol.ts | 382 | src/protocol.rs | verifiziert | Klasse 1: die `ExactKeys`-Typprüfungen entfallen (kein Rust-Äquivalent) — die exhaustiven `match`-Arme und Struct-Literale erzwingen dasselbe zur Compile-Zeit. `toProtocolJsonValue`/`sanitizeProtocolDetails` sind Identitäten, weil `serde_json::Value` bereits der JSON-Subset ist |
| src/transports/unix/listener.ts | 434 | src/transports/unix/listener.rs | verifiziert | Klasse 3: `node:net`/`node:fs` → tokio; Publikationsverfahren (.p-hash + link), dev/ino-Verifikation, Stale-Probe mit 1-s-Timeout und Pfadlimits 1:1 |
| src/transports/unix/types.ts | 15 | src/transports/unix/types.rs | verifiziert | Klasse 1: `UnixServerOptions` wiederholt die Felder statt sie zu erben |
| src/transports/unix/preset.ts | 23 | src/transports/unix/preset.rs | verifiziert | — |
| src/transports/unix/index.ts | 3 | src/transports/unix.rs | verifiziert | — |
| src/testing/service.ts | 287 | src/testing/service.rs | Tests portiert | Klasse 1: `TestServerService` ist ein `Clone` über gemeinsamen Zustand; Unterklassen der TS-Tests werden zu `HookedService` (Delegation mit optionalen Hooks) |
| src/testing/client.ts | 147 | src/testing/client.rs | Tests portiert | Klasse 1: `next()` liefert `Option` (None statt Reject, wenn die Verbindung vorher schließt) |
| src/testing/server.ts | 27 | src/testing/server.rs | Tests portiert | — |
| src/testing/index.ts | 5 | src/testing.rs | Tests portiert | — |
| test/conformance.test.ts | 378 | tests/conformance.rs | verifiziert | — (14 Tests) |
| test/sessions.test.ts | 429 | tests/sessions.rs | verifiziert | — (12 Tests) |
| test/protocol.test.ts | 301 | tests/protocol.rs | verifiziert | „rejects lossy tool input conversions" und „rejects sparse execution data" sind nicht darstellbar (Infinity/bigint/undefined/Zyklen/Array-Löcher); NaN-Timestamp → negativer Timestamp |
| test/unix.test.ts | 117 | tests/unix.rs | verifiziert | Der Stale-Socket wird durch ein verworfenes `std`-Listener-Binding erzeugt statt durch einen gekillten Subprozess |
| test/server.test.ts | 100 | tests/server.rs | verifiziert | „requires explicit listeners" nicht darstellbar (Pflichtfeld im Options-Struct) |
| test/unix-connection.test.ts | 65 | tests/unix_connection.rs | verifiziert | Statt eines Mock-Sockets prüft der Test die beobachtbare Reihenfolge auf einem echten Socket-Paar |
| test/listener.test.ts | 57 | tests/listener.rs | verifiziert | — (2 Tests) |

Testergebnis: 47 Tests (14 conformance, 12 sessions, 7 protocol, 6 server, 5 unix, 2 listener, 1 unix-connection), alle grün.

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| dist/ | Build-Artefakt |
| tsconfig*.json, vitest.config.ts, package.json | Distributionsmechanik (Abweichungsklasse 4) |
