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
| `src/agent-loop.ts` | 796 | `agent_loop.rs` | portiert (Task 12, laufend) | Master-Architektur: parallele Ausführung als `JoinSet` mit echten tokio-Tasks; TS-Semantik erhalten (sequentieller Preflight mit Abbruchprüfung je Call, `tool_execution_end` in Abschlussreihenfolge, Result-Messages in Assistant-Quellreihenfolge). Klasse 1: Event-Sink als `Arc<dyn Fn -> BoxFuture>`; `emit` wird wie in TS vor dem Weiterlaufen abgewartet. Offen: `agent.ts` (Agent-Klasse, Queues, Listener) |
| `src/agent.ts` | 592 | `agent.rs` | portiert (Task 12) | Klasse 1: Getter/Setter mit Kopiersemantik werden Methoden; `state()` liefert einen Snapshot. Listener sind `Arc<dyn Fn>` und werden weiterhin sequentiell in Subscription-Reihenfolge abgewartet. `waitForIdle` über `tokio::sync::Notify`. Klasse 1: `prepareNextTurn`/`prepareNextTurnWithContext` sind ein Callback (die kontextlose Variante ist eine Teilmenge) |
| `src/stream-fn.ts` | 20 | `stream_fn.rs` | portiert (Task 1, Tests in Task 12) | Klasse 1: `throw` → `Result<_, NoDefaultStreamFn>` mit wortgleicher Meldung |
| `src/harness/messages.ts` | 168 | `harness/messages.rs` | portiert (Task 1, Tests in Task 12) | Klasse 1: `timestamp: string \| number` → `i64`; die String-Variante wandelt der Aufrufer (App) um |
| `src/index.ts` | 145 | `lib.rs` | verifiziert (Task 13) | Klasse 1: Rust-Module sind öffentlich; `lib.rs` re-exportiert zusätzlich `agent`, `agent_loop`, `types`, `harness::messages`, `stream_fn`, `uuidv7` und die Telemetrie-Typen flach wie `index.ts`. Die Harness-Re-Exports (agent-harness, compaction, prompt-templates, result, session, tools, skills) entfallen laut Ausschlusstabelle; die TS-Typinferenz-Exporte von `@notagent/telemetry` haben laut Faktenbericht §5 keine Laufzeitentsprechung |

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
