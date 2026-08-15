# Parity-Ledger: notagent-agent

TS-Quelle: `/Users/dev/projects/notagent-main/packages/agent` (funktionaler Kern, ~2 400 LOC) — Workstream B.

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

## Lektüre-Protokoll

| Datum | TS-/Referenz-Datei | LOC | gelesen von (Task) |
|---|---|---|---|
| 2026-08-13 | `packages/agent/src/types.ts` | 443 | 1 |
| 2026-08-13 | `packages/agent/src/index.ts` | 145 | 1 |
| 2026-08-13 | `packages/agent/src/stream-fn.ts` | 20 | 1 |
| 2026-08-13 | `packages/agent/src/harness/messages.ts` | 168 | 1 |
| 2026-08-13 | `packages/coding-agent/src/core/messages.ts` (Konsumentenbeleg) | 120 (Auszug) | 1 |
| 2026-08-13 | `packages/agent/src/agent-loop.ts` | 796 | 12 |
| 2026-08-13 | `packages/agent/test/agent-loop.test.ts` | 1607 | 12 |
| 2026-08-13 | `packages/agent/src/agent.ts` | 592 | 12 |
| 2026-08-13 | `packages/agent/test/agent.test.ts` | 810 | 12 |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| `src/types.ts` | 443 | `types.rs` | verifiziert (Task 1) | Klasse 1: Kein Declaration Merging — die vier Custom-Rollen aus `harness/messages.ts:55-62` (identisch in `coding-agent/src/core/messages.ts:69-76`) sind feste `AgentMessage`-Varianten. Klasse 1: `AgentTool` ist ein Trait (TS-Interface mit `execute`), `AgentToolResult.details` ist `Option<Value>` (TS `T` kann `undefined` sein). Klasse 1: Callbacks als `Arc<dyn Fn(..) -> BoxFuture>`. Klasse 1: `pendingToolCalls` als `BTreeSet` (nur Mitgliedschaft wird ausgewertet). Klasse 3: `AbortSignal` → `CancellationToken`. |
| `src/agent-loop.ts` | 796 | `agent_loop.rs` | verifiziert (Task 12) | Master-Architektur: parallele Ausführung als `JoinSet` mit echten tokio-Tasks; TS-Semantik erhalten (sequentieller Preflight mit Abbruchprüfung je Call, `tool_execution_end` in Abschlussreihenfolge, Result-Messages in Assistant-Quellreihenfolge). Klasse 1: Event-Sink als `Arc<dyn Fn -> BoxFuture>`; `emit` wird wie in TS vor dem Weiterlaufen abgewartet |
| `src/agent.ts` | 592 | `agent.rs` | verifiziert (Task 12, Nachzug C-8) | Klasse 1: Getter/Setter mit Kopiersemantik werden Methoden; `state()` liefert einen Snapshot. Klasse 1: Die öffentlichen Verdrahtungsfelder von TS (`streamFunction`, `getApiKey`, `onPayload`, `onResponse`, `beforeToolCall`, `afterToolCall`, `thinkingBudgets`, `transport`, `maxRetryDelayMs`, `toolExecution`) liegen in `AgentOptions`; Lesen und Ändern zur Laufzeit über `options()`/`update_options()` (Interface-Request C-8). `onPayload`/`onResponse` fehlten und sind nachgezogen — sie wandern wie in TS (`agent.ts:452-453`) in die `SimpleStreamOptions`. Listener sind `Arc<dyn Fn>` und werden weiterhin sequentiell in Subscription-Reihenfolge abgewartet. `waitForIdle` über `tokio::sync::Notify`. Klasse 1: `prepareNextTurn`/`prepareNextTurnWithContext` sind ein Callback (die kontextlose Variante ist eine Teilmenge) |
| `src/stream-fn.ts` | 20 | `stream_fn.rs` | portiert (Task 1, Tests in Task 12) | Klasse 1: `throw` → `Result<_, NoDefaultStreamFn>` mit wortgleicher Meldung |
| `src/harness/messages.ts` | 168 | `harness/messages.rs` | portiert (Task 1, Tests in Task 12) | Klasse 1: `timestamp: string \| number` → `i64`; die String-Variante wandelt der Aufrufer (App) um |
| `src/index.ts` | 145 | `lib.rs` | verifiziert (Task 13) | Klasse 1: Rust-Module sind öffentlich; `lib.rs` re-exportiert zusätzlich `agent`, `agent_loop`, `types`, `harness::messages`, `stream_fn`, `uuidv7` und die Telemetrie-Typen flach wie `index.ts`. Die Harness-Re-Exports (agent-harness, compaction, prompt-templates, result, session, tools, skills) entfallen laut Ausschlusstabelle; die TS-Typinferenz-Exporte von `@notagent/telemetry` haben laut Faktenbericht §5 keine Laufzeitentsprechung |

## Datei-Abdeckung

Der Workstream portiert den funktionalen Kern von `packages/agent`; alles unter
`src/harness/` außer `messages.ts` ist laut Master-Plan ausgeschlossen (der coding-agent
von Workstream C bringt Compaction, Tools, Sessions und Skills selbst mit). Die Tabelle
führt jede Datei mit Status.

| TS-Datei | LOC | Rust | Status |
|---|---|---|---|
| `src/agent-loop.ts` | 796 | `agent_loop.rs` | verifiziert (Task 12) |
| `src/agent.ts` | 592 | `agent.rs` | verifiziert (Task 12) |
| `src/types.ts` | 443 | `types.rs` | verifiziert (Task 1) |
| `src/index.ts` | 145 | `lib.rs` | verifiziert (Task 13) |
| `src/harness/messages.ts` | 168 | `harness/messages.rs` | verifiziert (Task 12) |
| `src/stream-fn.ts` | 20 | `stream_fn.rs` | verifiziert (Task 12) |
| `src/node.ts` | 2 | — | ausgeschlossen (Node-Einstiegspunkt, re-exportiert `harness/env/nodejs.ts`) |
| `src/proxy.ts` | 370 | — | ausgeschlossen (siehe unten) |
| `src/search/index.ts`, `src/search/scanning.ts` | 32 + 176 | — | ausgeschlossen (siehe unten) |
| `src/harness/**` (39 Dateien, 9 862 LOC) | 9 862 | — | ausgeschlossen (Master-Plan, Scope-Tabelle) |
| `test/agent-loop.test.ts` | 1607 | `tests/agent_loop.rs` | portiert (Task 12) |
| `test/agent.test.ts` | 810 | `tests/agent.rs` | portiert (Task 12) |
| `test/e2e.test.ts` | 415 | — | ausgeschlossen: fährt die AgentHarness (Tools, Sessions, Node-Env) — alles Ausschlüsse dieses Workstreams |
| `test/proxy.test.ts` | 79 | — | ausgeschlossen: testet das ausgeschlossene `proxy.ts` |
| `test/harness/**` (19 Dateien, 5 674 LOC) | 5 674 | — | ausgeschlossen: testen ausschließlich ausgeschlossene Harness-Module |
| `test/utils/calculate.ts`, `test/utils/get-current-time.ts` | 40 + 46 | — | ausgeschlossen: Werkzeuge ausschließlich für `test/e2e.test.ts` (belegt: `grep -rn "utils/calculate\|utils/get-current-time" test/` trifft nur dort) |
| `test/harness/session-test-utils.ts` | 20 | — | ausgeschlossen: Helfer der ausgeschlossenen Session-Suiten |
| — | — | `tests/session_message_parity.rs` | zusätzlich (Task 12): Serde-Roundtrip der 44 Session-Fixture-Nachrichten |

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| `src/harness/agent-harness.ts` | Master-Plan, Scope-Tabelle: Stub, dessen Methoden `HarnessNotImplemented` werfen |
| `src/harness/**` außer `messages.ts` | Master-Plan: coding-agent importiert aus agent-core nur Agent, AgentMessage, AgentState, AgentTool, CustomMessage, setDefaultStreamFn, StreamFn, ThinkingLevel, uuidv7; Compaction/Tools/Sessions/Skills hat der coding-agent selbst (WS-C) |
| `src/proxy.ts` (370) | Konsumenten-Verifikation (Task 12): `streamProxy` wird von keinem Paket des Repos importiert — `grep -rn "streamProxy\|from \"../proxy"` über coding-agent, server, client und evals liefert nichts. Damit greift die Master-Regel „harness-/Zusatzmodule ohne App-Konsument" |
| `src/search/` (`index.ts`, `scanning.ts`, 176 LOC) | Konsumenten-Verifikation (Task 12): kein Import in coding-agent, server, client oder evals. Die Module bauen zudem auf `harness/session/types.ts` auf, das laut Master-Plan ausgeschlossen ist (der coding-agent hat einen eigenen JSONL-SessionManager). Ein Port wäre toter Code über ausgeschlossenen Typen |
| `src/harness/agent-harness.ts` (Re-Export in `index.ts`) | Master-Plan: Stub |

## Fixtures

| Datei | Herkunft |
|---|---|
| `tests/fixtures/session-messages.jsonl` | 44 unveränderte `message`-Einträge aus `packages/coding-agent/test/fixtures/{before-compaction,large-session}.jsonl` (alle 17 vorkommenden Feld-/Content-Signaturen) |

## Gegenlesen der C-15-Korrektur (2026-08-15)

TS-Oracle: `packages/agent/src/agent.ts:328` — `waitForIdle()` liest `activeRun?.promise`
synchron und gibt genau dieses Promise zurück. Zwei Eigenschaften folgen daraus:

1. Ein Promise rastet ein. Beendet sich der Lauf zwischen Lesen und `await`, ist es bereits
   erfüllt — der Warter kann keinen Weckruf verpassen.
2. Gewartet wird auf *diesen* Lauf. Ein Nachfolgelauf, der `activeRun` danach neu belegt,
   verlängert die Wartezeit nicht.

Cs Korrektur (`notified()` vor der Prüfung mit `tokio::pin!` + `enable()` registrieren)
stellt Eigenschaft 1 her und ist bestätigt — sie ist gegen `Notify` auch die einzige
richtige Reihenfolge. Eigenschaft 2 blieb offen: die Nachprüfung war
`active_run…is_none()`, also „läuft überhaupt etwas", nicht „läuft noch mein Lauf".
Endet Lauf A und beginnt Lauf B im Fenster zwischen `enable()` und der Nachprüfung, wartet
der Warter auf As bereits gefeuertes `Notify` und hängt, während TS sofort zurückkehrt.
Korrigiert zu einem Identitätsvergleich (`Arc::ptr_eq` auf das `idle` des Laufs); der
geklonte `Arc` hält die Allokation am Leben, der Zeigervergleich ist damit eindeutig.
Regressionstest: `tests/agent.rs::wait_for_idle_never_misses_the_end_of_its_own_run`
(300 Runden, 4 Worker-Threads, vier gleichzeitige Warter über zwei aufeinanderfolgende
Läufe).
