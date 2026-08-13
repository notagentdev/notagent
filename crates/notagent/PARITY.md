# Parity-Ledger: notagent

TS-Quelle: `/Users/dev/projects/notagent-main/packages/coding-agent` (68 856 LOC in src/) — Workstream C (TUI-Teile mit A).

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

**Stand: Task 5 läuft.** `config.ts` und der Kern von `settings-manager.ts` sind portiert;
`migrations.ts`, `auth-storage.ts` und die typisierten Settings-Zugriffsmethoden stehen aus.

## Lektüre-Protokoll

| Datum | TS-/Referenz-Datei | LOC | gelesen von (Task) |
|---|---|---|---|
| 2026-08-13 | packages/coding-agent/src/config.ts | 576 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/src/core/settings-manager.ts (Zeilen 1-700) | 700 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/test/config.test.ts (Kopf) | 60 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/test/settings-manager.test.ts (Kopf) | 120 | C-Task 5 |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| src/config.ts | 576 | src/config.rs | portiert | Klasse 4: kein `package.json`-Lesen zur Laufzeit — Identität kommt aus Cargo; `bun-binary` → `InstallMethod::Binary` (Single-Binary, Update über die Releases-Seite). Klasse 1: die Erkennung liest eine explizite `InstallEnv` statt `process.execPath`/`argv`, damit sie ohne globalen Prozesszustand testbar ist. Asset-Pfade zeigen auf das Verzeichnis der Binary (die Assets sind eingebettet) |
| src/core/settings-manager.ts | 1 273 | src/core/settings_manager.rs | portiert (Kern) | Klasse 1: `Settings` behält unbekannte Schlüssel über `#[serde(flatten)] extra`, damit ein Load/Persist-Roundtrip wie in JS nichts verliert; Schreibvorgänge sind synchron (die TS-Promise-Queue entfällt, `flush()` bleibt als No-op erhalten); Feld-Setter arbeiten vorerst über die Wire-Namen statt über ~45 typisierte Methoden. Klasse 3: `proper-lockfile` → exklusiv angelegtes `.lock`-Verzeichnis mit derselben Retry-Semantik (10 × 20 ms). Die `extensions`-Einstellung entfällt (Extension-System), bleibt aber als unbekannter Schlüssel erhalten |
| test/settings-manager.test.ts (Kernfälle) | 587 | tests/settings_manager.rs | Tests portiert | 9 Tests: externe Änderungen bleiben erhalten, In-Memory gewinnt bei Konflikten, Projekt-über-Global-Merge inkl. verschachtelter Objekte, Trust-Gate, Load-Fehler ohne Dateiverlust, verschachtelte Felder, Migrationen, In-Memory-Storage |
| src/migrations.ts | 314 | — | offen | Task 5 |
| src/core/auth-storage.ts | 507 | — | offen | Task 5 |
| test/config.test.ts | 446 | — | offen | Task 5 (Distributionsmechanik; die darstellbaren Fälle folgen mit den restlichen Settings-Zugriffen) |
| test/auth-storage.test.ts | 535 | — | offen | Task 5 |

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| src/core/extensions/, src/extensions/ (außer llama) | Extension-System (Master-Plan, Ausschluss-Tabelle) |
| src/server/create-harness.ts | Stub, nur vom eigenen Test konsumiert |
| src/bun/, src/cli/experimental/ | Distributionsmechanik bzw. nicht verdrahtet |
