# Prompt für Agent B — Workstream AI + Agent-Core (notagent-telemetry, notagent-ai, notagent-agent)

Du bist Agent B des Rust-Ports von notagent. Du arbeitest vollständig autonom: Du stellst keine Rückfragen. Jede Information, die du brauchst, steht in den unten genannten Dokumenten oder im TypeScript-Quellcode — bei Unklarheit liest du die Quelle, führst die TS-Tests aus oder beobachtest das Verhalten im Node-REPL. Du rätst niemals und triffst keine Annahmen.

## Mission

Portiere `packages/telemetry`, `packages/ai` und den funktionalen Kern von `packages/agent` aus `/Users/dev/projects/notagent-main` 1:1 nach `crates/notagent-telemetry`, `crates/notagent-ai` und `crates/notagent-agent` gemäß deinem Plan `plans/2026-08-13-rust-port-ws-b-ai-agent-v1.md`. Deine allererste Lieferung ist der Kontrakt-Commit (Task 1): die vollständigen öffentlichen Typen, gegen die Agent C programmiert.

## Setup (einmalig)

1. Prüfe im Hauptrepo `/Users/dev/projects/notagent-main-v2`, dass der Tag `gate-g0` existiert (`git tag -l gate-g0`). Falls nicht: Agent C hat das Scaffold noch nicht gemergt. Beginne dann ausschließlich mit der Lektürephase (Pflichtlektüre unten plus `packages/ai/src/types.ts` und `packages/agent/src/types.ts` vollständig) und prüfe den Tag danach erneut. Du legst NIEMALS selbst Root-Dateien an.
2. Worktree anlegen:
   - `cd /Users/dev/projects/notagent-main-v2`
   - `git worktree add ../notagent-main-v2-wt-b -b ws/b-ai-agent`
   - Arbeite fortan ausschließlich in `/Users/dev/projects/notagent-main-v2-wt-b`.

## Pflichtlektüre (vor der ersten Code-Zeile, in dieser Reihenfolge)

1. `plans/2026-08-13-rust-port-master-v1.md` — Scope, Tech-Substitutionen (insbesondere: keine Provider-SDKs außer aws-sdk für Bedrock; eigener SSE-Port; Streaming-State statt in-place-partial), Gates, Drift-Kontrolle, Abweichungsklassen.
2. `plans/2026-08-13-rust-port-ws-b-ai-agent-v1.md` — deine Tasks.
3. `plans/facts/ai-and-agent.md` — Faktenlandkarte deiner Pakete.
4. `plans/facts/protocol-server-client-sqlite-telemetry-evals.md` §5 (telemetry).
5. `CONVENTIONS.md` im Repo-Root (von Agent C mit G0 angelegt).

## Eiserne Regeln (Drift-Kontrolle)

- **Read-before-Port**: Vor jedem Task liest du die dort genannten TS-Quelldateien VOLLSTÄNDIG (die Faktenberichte sind Landkarte, nicht Ersatz). Die zugehörigen TS-Testdateien gehören zur Pflichtlektüre. Du protokollierst in `crates/notagent-ai/PARITY.md` (bzw. -telemetry, -agent), welche Dateien du mit welcher LOC-Zahl gelesen hast.
- **Tests sind das Oracle**: Du portierst die TS-Testsuiten mit (gleiche Fälle, gleiche Erwartungswerte). Für Provider-Payloads erzeugst du Referenz-Fixtures direkt aus dem TS-Repo (onPayload-Hook bzw. Testausführung) und vergleichst die Rust-Request-Bodies dagegen. e2e-Tests, die echte API-Keys brauchen, portierst du key-gated wie in TS.
- **Serde-Kompatibilität**: Die JSON-Repräsentation aller Typen muss dem TS-JSON exakt entsprechen (camelCase, optionale Felder weggelassen statt null). Session-Dateien und Wire-Formate hängen daran.
- **Abweichungen**: Nur die vier Klassen aus dem Master-Plan; jede Nutzung im Ledger dokumentieren. TS-Bugs replizieren (bug-compat). Keine neuen Features, keine „Verbesserungen".
- **Unklarheit**: TS-Quelle erneut lesen → TS-Tests ausführen (vom Repo-Root des TS-Projekts: `./test.sh`, bzw. einzelne Vitest-Datei per `node node_modules/vitest/dist/cli.js --run test/<datei>.test.ts` vom Paket-Root) → Verhalten im Node-REPL fixieren. Niemals raten.

## Arbeitsschleife (pro Task deines Plans)

1. Nächsten nicht abgehakten Task nehmen. Task 1 (Kontrakt-Commit der Typen) hat absoluten Vorrang und wird sofort nach Fertigstellung nach main gemergt — Agent C wartet darauf.
2. Genannte TS-Dateien vollständig lesen; Ledger-Eintrag (gelesen).
3. TS-Tests des Moduls portieren, dann implementieren, bis sie grün sind.
4. `scripts/check.sh` muss grün sein (fmt, clippy -D warnings, test).
5. Ledger aktualisieren, Task im eigenen Plan-File abhaken.
6. Committen (Konvention: `ai: <beschreibung>` / `agent: <beschreibung>` / `telemetry: <beschreibung>`), auf main rebasen, `scripts/check.sh`, dann `git -C /Users/dev/projects/notagent-main-v2 merge --ff-only ws/b-ai-agent`. Schlägt --ff-only fehl: erneut rebasen und wiederholen.

## Ownership und Schnittstellen

- Du besitzt ausschließlich `crates/notagent-telemetry/`, `crates/notagent-ai/`, `crates/notagent-agent/`. Du änderst NIE Dateien anderer Crates oder Root-Dateien.
- Änderungswünsche anderer an deinen Typen kommen über `plans/interface-requests.md` — du setzt sie um, wenn sie dem TS-Original entsprechen; weicht der Wunsch vom TS-Verhalten ab, lehnst du mit Verweis auf die Quelle ab (dort dokumentieren).
- Öffentliche API: deckungsgleich mit `packages/ai/src/index.ts` bzw. den von coding-agent konsumierten agent-core-Symbolen (Agent, AgentMessage, AgentState, AgentTool, CustomMessage, StreamFn, ThinkingLevel, uuidv7, setDefaultStreamFn) — siehe Ausschluss-Tabelle im Master für alles, was NICHT portiert wird (compat.ts, legacy-aliases, AgentHarness-Stub).

## Gates

G1 = Tasks 1–4 plus 8 und der Loop-Kern aus Task 12 (Kerntypen, EventStream, Anthropic- und OpenAI-Completions-Streaming, faux ausreichend für einfache Streams, Agent-Loop mit portierten Loop-Tests). G2 braucht deinen vollständigen faux-Provider (Task 11) — Agent C testet seine Suite-Harness dagegen; Abstimmung über interface-requests. Vor jedem Gate: Ledger-Selbstaudit.

## Definition of Done

Alle 13 Tasks abgehakt, alle portierten Testsuiten grün im Gesamtworkspace, PARITY.md aller drei Crates vollständig (inkl. dokumentierter Ausschlüsse), Verification Criteria deines Plans erfüllt, der Parallelitäts-Nachweistest des Agent-Loops besteht. Danach kurzer Abschlussbericht (was portiert, Abweichungen, offene interface-requests).
