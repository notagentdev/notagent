# Parity-Ledger: notagent-session-sqlite

TS-Quelle: `/Users/dev/projects/notagent-main/packages/session-backends/sqlite-node` (2 505 LOC in src/) — Workstream C.

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

**Stand: Task 4 läuft.** Fundament (Schema, Migrationen, SQL-Komposition, Datenbank-Adapter,
Session-Typoberfläche), alle zehn Storage-Module und der Branch-Cache sind portiert; Repository
(`repo.ts`), Suche (`search-backend.ts`), die `Session`-Klasse und die neun Testsuiten stehen aus.

## Lektüre-Protokoll

| Datum | TS-/Referenz-Datei | LOC | gelesen von (Task) |
|---|---|---|---|
| 2026-08-13 | packages/session-backends/sqlite-node/src/index.ts | 108 | C-Task 4 |
| 2026-08-13 | packages/session-backends/sqlite-node/src/sqlite/index.ts | 18 | C-Task 4 |
| 2026-08-13 | packages/session-backends/sqlite-node/src/sqlite/types.ts | 52 | C-Task 4 |
| 2026-08-13 | packages/session-backends/sqlite-node/src/sqlite/sql.ts | 66 | C-Task 4 |
| 2026-08-13 | packages/session-backends/sqlite-node/src/sqlite/migrations.ts | 49 | C-Task 4 |
| 2026-08-13 | packages/session-backends/sqlite-node/src/sqlite/migrations/001_initial.sql | 122 | C-Task 4 |
| 2026-08-13 | packages/agent/src/harness/session/types.ts | 393 | C-Task 4 |
| 2026-08-13 | packages/session-backends/sqlite-node/test/sql.test.ts | 39 | C-Task 4 |
| 2026-08-13 | packages/session-backends/sqlite-node/test/migrations.test.ts | 61 | C-Task 4 |
| 2026-08-13 | packages/session-backends/sqlite-node/test/test-utils.ts (Auszug) | 40 | C-Task 4 |
| 2026-08-13 | packages/session-backends/sqlite-node/src/sqlite/repo.ts (Kopf, Zeilen 1-200) | 200 | C-Task 4 |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| src/index.ts | 108 | src/lib.rs + src/sqlite/database.rs | portiert | Klasse 3: `node:sqlite` → rusqlite; Zeilen kommen als JSON-Objekte zurück, damit die typisierte `get<TRow>`/`all<TRow>`-Oberfläche erhalten bleibt |
| src/sqlite/index.ts | 18 | src/sqlite.rs | portiert | — |
| src/sqlite/types.ts | 52 | src/sqlite/types.rs | portiert | Klasse 1: `db.transaction(fn)` wird zu `with_transaction(db, body)`, damit das Trait objekt-sicher bleibt |
| src/sqlite/sql.ts | 66 | src/sqlite/sql.rs | verifiziert | Klasse 1: Tagged Templates gibt es in Rust nicht — `SqlQuery::compose` + `sql!`-Makro mit `text`/`param`/`fragment`; Parameterreihenfolge und Inlining identisch |
| src/sqlite/migrations.ts | 49 | src/sqlite/migrations.rs | verifiziert | Klasse 4: die Migrations-SQL ist eingebettet (`include_str!`) statt zur Laufzeit gelesen |
| src/sqlite/migrations/001_initial.sql | 122 | src/sqlite/migrations/001_initial.sql | verifiziert | unverändert übernommen |
| (agent) src/harness/session/types.ts | 393 | src/session_types.rs | portiert | Klasse 1: TS-Unions → Rust-Enums mit `#[serde(tag)]`; `ProvisionedEntry` ist ein eigenes Enum statt eines gemappten Typs; Query-Objekte werden Structs mit `Option`-Feldern |
| src/sqlite/storage/sessions.ts | 131 | src/sqlite/storage/sessions.rs | portiert | Klasse 1: `assertJsonSerializable` entfällt (`serde_json::Value` ist per Konstruktion serialisierbar) |
| src/sqlite/storage/session-sequences.ts | 29 | src/sqlite/storage/session_sequences.rs | portiert | — |
| src/sqlite/storage/session-stats.ts | 54 | src/sqlite/storage/session_stats.rs | portiert | Klasse 1: `usage.totalTokens` ist optional (B-Kontrakt), `unwrap_or(0)` entspricht dem TS-Verhalten |
| src/sqlite/storage/writer-leases.ts | 58 | src/sqlite/storage/writer_leases.rs | portiert | — |
| src/sqlite/storage/entries.ts | 78 | src/sqlite/storage/entries.rs | portiert | — |
| src/sqlite/storage/facts.ts | 64 | src/sqlite/storage/facts.rs | portiert | — |
| src/sqlite/storage/branch-tips.ts | 35 | src/sqlite/storage/branch_tips.rs | portiert | — |
| src/sqlite/storage/lanes.ts | 124 | src/sqlite/storage/lanes.rs | portiert | — |
| src/sqlite/storage/records.ts | 95 | src/sqlite/storage/records.rs | portiert | — |
| src/sqlite/storage/branch-entries.ts | 174 | src/sqlite/storage/branch_entries.rs | portiert | — |
| src/sqlite/branch-cache.ts | 101 | src/sqlite/branch_cache.rs | portiert | — |
| src/sqlite/search-backend.ts | 188 | — | offen | Task 4 |
| src/sqlite/repo.ts | 953 | — | teilweise gelesen | Task 4 |
| (agent) src/harness/session/session.ts | 299 | — | offen | Task 4 (SessionRepo liefert `Session`) |
| test/sql.test.ts | 39 | tests/sql.rs | verifiziert | — (2 Tests) |
| test/migrations.test.ts | 61 | tests/migrations.rs | verifiziert | — (1 Test) |
| test/{adapter,branch-cache,branch-query,conformance,facts-query,log-query,repository,search,writer-leases}.test.ts | 1 704 | — | offen | Task 4 |

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| dist/ | Build-Artefakt |
| tsconfig*.json, vitest.config.ts, package.json | Distributionsmechanik (Abweichungsklasse 4) |
