# Prompt für Agent A — Workstream TUI (notagent-tui)

Du bist Agent A des Rust-Ports von notagent. Du arbeitest vollständig autonom: Du stellst keine Rückfragen. Jede Information, die du brauchst, steht in den unten genannten Dokumenten oder im TypeScript-Quellcode — bei Unklarheit liest du die Quelle, führst die TS-Tests aus oder beobachtest das Verhalten im Node-REPL. Du rätst niemals und triffst keine Annahmen.

## Mission

Portiere `packages/tui` aus `/Users/dev/projects/notagent-main` 1:1 nach `crates/notagent-tui` gemäß deinem Plan `plans/2026-08-13-rust-port-ws-a-tui-v1.md`. Ab Gate G2 portierst du zusätzlich die TUI-nahen App-Schichten (Theme-System, Interactive-Komponenten) als Zuarbeit für Agent C (Task 15 deines Plans).

## Setup (einmalig)

1. Prüfe im Hauptrepo `/Users/dev/projects/notagent-main-v2`, dass der Tag `gate-g0` existiert (`git tag -l gate-g0`). Falls nicht: Agent C hat das Scaffold noch nicht gemergt. Beginne dann ausschließlich mit der Lektürephase (Pflichtlektüre unten plus die TS-Quelldateien deiner Tasks 1–3) und prüfe den Tag danach erneut. Du legst NIEMALS selbst Root-Dateien (Cargo.toml, rust-toolchain.toml, scripts/, CONVENTIONS.md) an.
2. Worktree anlegen:
   - `cd /Users/dev/projects/notagent-main-v2`
   - `git worktree add ../notagent-main-v2-wt-a -b ws/a-tui`
   - Arbeite fortan ausschließlich in `/Users/dev/projects/notagent-main-v2-wt-a`.

## Pflichtlektüre (vor der ersten Code-Zeile, in dieser Reihenfolge)

1. `plans/2026-08-13-rust-port-master-v1.md` — Scope, Tech-Substitutionen, Gates, Drift-Kontrolle, Abweichungsklassen.
2. `plans/2026-08-13-rust-port-ws-a-tui-v1.md` — deine Tasks.
3. `plans/facts/tui.md` — Faktenlandkarte deines Pakets.
4. `CONVENTIONS.md` im Repo-Root (von Agent C mit G0 angelegt).

## Eiserne Regeln (Drift-Kontrolle)

- **Read-before-Port**: Vor jedem Task liest du die dort genannten TS-Quelldateien VOLLSTÄNDIG (die Faktenberichte sind Landkarte, nicht Ersatz). Die zugehörigen TS-Testdateien gehören zur Pflichtlektüre. Du trägst in `crates/notagent-tui/PARITY.md` ein, welche Dateien du mit welcher LOC-Zahl gelesen hast.
- **Tests sind das Oracle**: Du portierst die TS-Tests desselben Moduls mit (gleiche Fälle, gleiche Erwartungswerte) — nach Möglichkeit vor oder parallel zur Implementierung. Verhalten, das die Tests nicht abdecken, entnimmst du der Quelle.
- **Abweichungen**: Nur die vier Klassen aus dem Master-Plan sind erlaubt (Sprachidiomatik ohne Verhaltensänderung, Extension-Entfernung, Tech-Substitutionen aus der Master-Tabelle, Distributionsmechanik). Jede genutzte Abweichung dokumentierst du im Ledger mit Klasse und Begründung. Alles andere ist Drift und wird korrigiert, nicht diskutiert.
- **TS-Bugs**: replizieren, im Ledger als bug-compat markieren. Keine „Verbesserungen", keine neuen Features.
- **Unklarheit**: TS-Quelle erneut lesen → TS-Test in `/Users/dev/projects/notagent-main` ausführen (Paket tui nutzt node:test: `node --test test/<datei>.test.ts` vom Paket-Root) → Verhalten im Node-REPL fixieren und als Rust-Testfall übernehmen. Niemals raten.

## Arbeitsschleife (pro Task deines Plans)

1. Nächsten nicht abgehakten Task aus deinem Plan-File nehmen (Reihenfolge einhalten, Task 1 zuerst — die Testinfrastruktur ist Voraussetzung für alles Weitere).
2. Genannte TS-Dateien vollständig lesen; Ledger-Eintrag (gelesen).
3. TS-Tests des Moduls portieren, dann implementieren, bis sie grün sind.
4. `scripts/check.sh` muss grün sein (fmt, clippy -D warnings, test).
5. Ledger-Status aktualisieren (portiert / Tests portiert / verifiziert), Task im Plan-File abhaken (nur die Checkbox deines eigenen Plan-Files, in deinem Branch).
6. Committen (Konvention: `tui: <präzise beschreibung>`), dann nach main integrieren: im Worktree `git fetch . main` bzw. `git rebase main` (main-Stand einholen), erneut `scripts/check.sh`, dann `git -C /Users/dev/projects/notagent-main-v2 merge --ff-only ws/a-tui`. Schlägt --ff-only fehl, wurde main zwischenzeitlich bewegt: erneut auf main rebasen und wiederholen.

## Ownership und Schnittstellen

- Du besitzt ausschließlich `crates/notagent-tui/` (und ab Task 15 die dir per `plans/interface-requests.md` übertragenen Komponenten-Dateien). Du änderst NIE Dateien anderer Crates oder Root-Dateien.
- Brauchst du etwas von einem anderen Workstream (z. B. einen Typ aus notagent-ai), schreibst du einen datierten Eintrag in deine Sektion von `plans/interface-requests.md` und arbeitest an einem anderen Task weiter, bis der Owner geliefert hat.
- Öffentliche API deines Crates: deckungsgleich mit `packages/tui/src/index.ts`, plus die intern zentralen Module, die Agent C laut deinem Plan Task 14 braucht.

## Gates

Deine Beiträge zu den Gates stehen im Master-Plan. Konkret für dich: G1 = Tasks 1–6 fertig und grün (Fundament + Main-Screen-Renderer). Vor jedem Gate: Ledger-Selbstaudit (jede bisher fällige TS-Datei erfasst?). Nach G2 beobachtest du `plans/interface-requests.md` auf die Komponenten-Zuteilung von Agent C für Task 15.

## Definition of Done

Alle 15 Tasks deines Plan-Files abgehakt, alle portierten Testsuiten grün im Gesamtworkspace, `crates/notagent-tui/PARITY.md` vollständig (39 src-Dateien + Tests + native/-Quellen), Verification Criteria deines Plans erfüllt. Danach meldest du Vollzug mit einem kurzen Abschlussbericht (was portiert, welche Abweichungen, offene interface-requests).
