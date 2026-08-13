# Parity-Ledger: notagent-server

TS-Quelle: `/Users/dev/projects/notagent-main/packages/server` (2 299 LOC in src/) — Workstream C.

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

**Stand: Task 3 ist noch offen** — Kern, Unix-Transport und die ai↔protocol-Bridge sind portiert;
`testing/` und die sechs Testsuiten (1 456 LOC) stehen aus.

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

Gelesen: 17 Dateien, 2 299 LOC — die vollständige src/. Die Testsuiten stehen noch aus.

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| src/index.ts | 5 | src/lib.rs | portiert | — (Re-Export-Barrel) |
| src/types.ts | 63 | src/types.rs | portiert | Klasse 1: `PiServerService`/`PiSessionRuntime` als `#[async_trait]`-Traits; `subscribe` liefert `Box<dyn FnOnce>` |
| src/errors.ts | 58 | src/errors.rs | portiert | Klasse 1: Fehlerklassen → `PiServerError`-Struct plus `ServerError`-Enum für die Boundary (`Internal` hält die Ursache, serialisiert sie nie) |
| src/connection.ts | 36 | src/connection.rs | portiert | Klasse 1: `ConnectionState` mit innerem Mutex; `isTerminalConnection` als Methode |
| src/listener.ts | 10 | src/listener.rs | portiert | Klasse 1: Trait mit `async_trait`; Acceptor als `Arc<dyn Fn>` |
| src/snapshots.ts | 62 | src/snapshots.rs + server.rs (`perform_broadcast`) | portiert | Klasse 1: die Broadcast-Warteschlange (Promise-Kette) wird zur fairen `tokio::sync::Mutex`; der Publisher hält keine Callbacks, `PiServer` ruft ihn direkt |
| src/server.ts | 396 | src/server.rs | portiert | Klasse 1: `void promise`-Fire-and-forget → `tokio::spawn`; Handshake-Timeout als abbrechbarer Task; Stage-Automat und Fehler-Sanitizing 1:1 |
| src/sessions.ts | 346 | src/sessions.rs | portiert | Klasse 1: Callback-Objekt → `Weak<PiServerInner>`; `disposing`/`opening` als `futures::future::Shared`; Connection-Sets über `Arc::ptr_eq` |
| src/protocol.ts | 382 | src/protocol.rs | portiert (unvollständig) | Klasse 1: die `ExactKeys`-Typprüfungen entfallen (kein Rust-Äquivalent) — die exhaustiven `match`-Arme und Struct-Literale erzwingen dasselbe zur Compile-Zeit. **Offen**: `toProtocolModelMetadata` braucht `get_supported_thinking_levels` aus notagent-ai (Interface-Request C-1) |
| src/transports/unix/listener.ts | 434 | src/transports/unix/listener.rs | portiert | Klasse 3: `node:net`/`node:fs` → tokio; Publikationsverfahren (.p-hash + link), dev/ino-Verifikation, Stale-Probe mit 1-s-Timeout und Pfadlimits 1:1 |
| src/transports/unix/types.ts | 15 | src/transports/unix/types.rs | portiert | Klasse 1: `UnixServerOptions` wiederholt die Felder statt sie zu erben |
| src/transports/unix/preset.ts | 23 | src/transports/unix/preset.rs | portiert | — |
| src/transports/unix/index.ts | 3 | src/transports/unix.rs | portiert | — |
| src/testing/service.ts | 287 | — | gelesen | offen (Task 3) |
| src/testing/client.ts | 147 | — | gelesen | offen (Task 3) |
| src/testing/server.ts | 27 | — | gelesen | offen (Task 3) |
| src/testing/index.ts | 5 | — | gelesen | offen (Task 3) |
| test/*.test.ts | 1 456 | — | offen | offen (Task 3) |

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| dist/ | Build-Artefakt |
| tsconfig*.json, vitest.config.ts, package.json | Distributionsmechanik (Abweichungsklasse 4) |
