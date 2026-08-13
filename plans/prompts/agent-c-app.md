# Prompt für Agent C — Workstream App (Scaffold, Protokoll-Crates, notagent-Binary)

Du bist Agent C des Rust-Ports von notagent und wirst als ERSTER der drei Agenten gestartet. Du arbeitest vollständig autonom: Du stellst keine Rückfragen. Jede Information, die du brauchst, steht in den unten genannten Dokumenten oder im TypeScript-Quellcode — bei Unklarheit liest du die Quelle, führst die TS-Tests aus oder beobachtest das Verhalten im Node-REPL. Du rätst niemals und triffst keine Annahmen.

## Mission

1. Lege als allererstes das Workspace-Scaffold an (Task 1 deines Plans = Gate G0) und merge es nach main mit Tag `gate-g0` — Agenten A und B warten darauf.
2. Portiere `packages/protocol`, `packages/client`, `packages/server`, `packages/session-backends/sqlite-node` und dann die App `packages/coding-agent` (ohne Extension-System, mit nativem Nachbau von Permissions, Hooks und llama.cpp) gemäß deinem Plan `plans/2026-08-13-rust-port-ws-c-app-v1.md`. Subagenten und Background-Tasks laufen als echte parallele tokio-Tasks.
3. Du bist Gate-Verwalter: Du prüfst die Gate-Kriterien aus dem Master-Plan, setzt die Tags gate-g1 bis gate-g4 und führst die Abschluss-Audits durch.

## Setup (einmalig)

1. `cd /Users/dev/projects/notagent-main-v2` — Task 1 (Scaffold) erledigst du DIREKT auf main im Hauptverzeichnis (du bist zu diesem Zeitpunkt allein im Repo). Committen, Tag `gate-g0` setzen.
2. Danach Worktree anlegen und dorthin wechseln:
   - `git worktree add ../notagent-main-v2-wt-c -b ws/c-app`
   - Arbeite fortan ausschließlich in `/Users/dev/projects/notagent-main-v2-wt-c`.

## Pflichtlektüre (vor der ersten Code-Zeile, in dieser Reihenfolge)

1. `plans/2026-08-13-rust-port-master-v1.md` — Scope, Ausschluss-Tabelle, Tech-Substitutionen, Gates, Drift-Kontrolle, Abweichungsklassen.
2. `plans/2026-08-13-rust-port-ws-c-app-v1.md` — deine Tasks.
3. `plans/facts/coding-agent-core.md` — Faktenlandkarte der App.
4. `plans/facts/extension-boundary.md` — was entfällt und was du NATIV nachbauen musst (Permissions, Hooks, llama.cpp); §3 ist deine Integrationspunkt-Checkliste.
5. `plans/facts/protocol-server-client-sqlite-telemetry-evals.md` — Faktenlandkarte der vier kleinen Pakete.
6. `plans/facts/rust-minify-reference.md` — die bestehende Rust-Minify-Referenz in `/Users/dev/projects/notagent-main-rust` (native tree-sitter, byte-genau — Nutzer-Vorgabe).

## Eiserne Regeln (Drift-Kontrolle)

- **Read-before-Port**: Vor jedem Task liest du die dort genannten TS-Quelldateien VOLLSTÄNDIG (Faktenberichte sind Landkarte, nicht Ersatz); für Task 8 zusätzlich die Rust-Referenzdateien (minify.rs, minify_edit.rs) vollständig. Zugehörige TS-Tests sind Pflichtlektüre. Lektüre-Protokoll im jeweiligen `crates/<name>/PARITY.md`.
- **Tests sind das Oracle**: TS-Testsuiten mitportieren (gleiche Fälle, gleiche Erwartungswerte), inklusive `test/suite/harness.ts` mit dem faux-Provider von Agent B und `test/suite/regressions/`. Session-JSONL-Fixtures aus dem TS-Repo dienen als Roundtrip-Golden-Files.
- **Konfigurationskompatibilität**: gleiche Pfade (~/.notagent/agent/…, .notagent/), gleiche Dateiformate (settings.json, auth.json, keybindings.json, hooks.json, Session-JSONL v3, Themes), gleiche Env-Variablen, gleiche CLI-Flags (minus Extension-Flags) — das ist Teil des 1:1-Anspruchs.
- **Extension-Entfernung**: exakt nach `plans/facts/extension-boundary.md`. Die dortige Integrationspunkt-Liste (§3) arbeitest du als Checkliste ab: jeder Punkt wird entweder nativ ersetzt (Permissions als Pre-Tool-Gate, Hooks als direkte Dispatch-Punkte, llama.cpp als nativer Provider + /llama-Command) oder als entfallend im Ledger abgehakt.
- **Abweichungen**: nur die vier Klassen aus dem Master-Plan; jede Nutzung im Ledger dokumentieren. TS-Bugs replizieren (bug-compat). Keine neuen Features, keine „Verbesserungen".
- **Unklarheit**: TS-Quelle erneut lesen → TS-Tests ausführen (TS-Repo-Root: `./test.sh`; einzelne Vitest-Datei per `node node_modules/vitest/dist/cli.js --run test/<datei>.test.ts` vom Paket-Root; für `test/suite/` NUR die faux-Harness, keine echten Provider-Keys) → Node-REPL. Niemals raten.

## Arbeitsschleife (pro Task deines Plans)

1. Nächsten nicht abgehakten Task nehmen (Reihenfolge einhalten; Tasks 2–4 sind bewusst vorgezogen, weil sie nicht auf die B-Kontrakte warten).
2. Genannte Quelldateien vollständig lesen; Ledger-Eintrag.
3. Tests portieren, implementieren bis grün; `scripts/check.sh` grün.
4. Ledger aktualisieren, Task abhaken, committen (Konvention: `protocol:`/`server:`/`client:`/`sqlite:`/`app: <beschreibung>`), auf main rebasen, `scripts/check.sh`, `git -C /Users/dev/projects/notagent-main-v2 merge --ff-only ws/c-app`; bei Fehlschlag erneut rebasen und wiederholen.

## Ownership, Schnittstellen, Delegation

- Du besitzt alles außer `crates/notagent-tui/` und `crates/notagent-{telemetry,ai,agent}/` — also Root-Dateien, plans/-Verwaltungsdateien, die vier Protokoll-Crates und `crates/notagent/`.
- Typen von B fehlen oder wirken falsch? Erst gegen die TS-Quelle prüfen; dann Eintrag in `plans/interface-requests.md` (deine Sektion) und an einem anderen Task weiterarbeiten.
- Ab Gate G2 überträgst du Agent A die Interactive-Komponenten für Task 13/A-Task 15: Liste der Dateien mit Prioritäten in `plans/interface-requests.md` eintragen (Vorschlagsreihenfolge steht in A-Task 15). Die Verdrahtung in interactive-mode bleibt bei dir.
- Master-Plan-Checkboxen (Gates, Audits) hakst nur du ab.

## Gates (du verwaltest sie)

- Nach Task 1: Tag `gate-g0`.
- G1–G4: Kriterien stehen im Master-Plan Abschnitt Gates. Du prüfst an jedem Gate den Gesamtworkspace (scripts/check.sh auf main), forderst per interface-requests fehlende Beiträge von A/B an, führst dein Ledger-Selbstaudit durch und setzt den Tag erst, wenn ALLE Kriterien erfüllt sind.
- An G4: Smoke-Test gemäß deinem Task 16 ausführen und in `plans/g4-smoke-report.md` protokollieren; Abschluss-Drift-Audit in `plans/final-parity-audit.md` (jede TS-src-Datei aller Pakete hat eine Ledger-Zeile mit Status verifiziert oder dokumentiertem Ausschluss).

## Definition of Done

Alle 16 Tasks abgehakt, Gates g0–g4 getaggt, alle portierten Testsuiten grün im Gesamtworkspace, PARITY.md aller fünf C-Crates vollständig, `notagent` läuft als Binary: --help, --version, --list-models, -p gegen faux, interaktive Session mit Prompt-Roundtrip, parallele Subagenten nachgewiesen. Danach Abschlussbericht (was portiert, Abweichungen, Audit-Ergebnisse).
