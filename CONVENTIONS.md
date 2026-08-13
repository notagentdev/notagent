# Konventionen des Rust-Ports

Verbindlich für alle drei Workstreams (A: TUI, B: AI+Agent, C: App). Ergänzt den
Master-Plan `plans/2026-08-13-rust-port-master-v1.md`; bei Widerspruch gilt der Master-Plan.

## 1. Workspace-Layout

- Cargo-Workspace mit `members = ["crates/*"]` (Glob) — neue Crates brauchen **keine**
  Änderung an der Root-`Cargo.toml`.
- Ein Crate je TS-Paket, 1:1-Spiegelung (keine feinere Aufteilung, Master-Plan
  „Alternative Approaches" 3):

  | Crate | TS-Paket |
  |---|---|
  | `notagent-tui` | `packages/tui` |
  | `notagent-telemetry` | `packages/telemetry` |
  | `notagent-ai` | `packages/ai` |
  | `notagent-agent` | `packages/agent` |
  | `notagent-protocol` | `packages/protocol` |
  | `notagent-client` | `packages/client` |
  | `notagent-server` | `packages/server` |
  | `notagent-session-sqlite` | `packages/session-backends/sqlite-node` |
  | `notagent` (lib + bin) | `packages/coding-agent` |

- Ownership (niemand editiert fremde Crates): A = `crates/notagent-tui/`,
  B = `crates/notagent-{telemetry,ai,agent}/`, C = alles übrige inkl. Root-Dateien.
  Schnittstellenwünsche: `plans/interface-requests.md` (append-only, Owner setzt um).

## 2. Modul-Layout

- **Die Rust-Modulstruktur spiegelt die TS-Dateistruktur.** `src/core/tools/edit.ts`
  → `crates/notagent/src/core/tools/edit.rs`. Verzeichnis-Module als
  `mod.rs`-freie Form (`core/tools.rs` + `core/tools/`), damit Pfad = TS-Pfad bleibt.
- Keine Umbenennungen „aus Geschmack": TS-Dateiname (kebab-case) → Rust-Modulname
  (snake_case, `-` → `_`). `agent-session.ts` → `agent_session.rs`.
- Namen von Typen und Funktionen folgen der TS-Vorlage, angepasst an Rust-Casing:
  TS `buildSystemPrompt` → Rust `build_system_prompt`; TS `SessionManager` bleibt
  `SessionManager`. Konstanten behalten ihren TS-Namen (`DEFAULT_MAX_LINES`).
- `pub` nur, was in TS exportiert wird; interne Helfer bleiben privat.

## 3. Fehlerbehandlung

- Je Crate ein `thiserror`-Fehlertyp (bzw. eine kleine Enum-Familie), definiert im
  Modul, das dem TS-Fehlerort entspricht (z. B. `ProtocolValidationError`).
- **Fehlermeldungen sind Teil der Parität**: Texte, die in TS für Nutzer oder Modell
  sichtbar sind (Tool-Fehler, CLI-Ausgaben, Protokollfehler), werden wortgleich
  übernommen.
- `anyhow` nur im Bin-Crate an den Rändern (main, Commands), nie in Bibliotheks-APIs.
- TS-`throw` in Kontrollflusspfaden → `Result`; TS-„rejectet nie"-Semantik
  (z. B. `runDelegation`) wird als `Result`-freier Rückgabewert nachgebildet.

## 4. Async / Nebenläufigkeit

- tokio Multi-Thread-Runtime. Subagenten und Shell-Tasks sind echte `tokio::spawn`-Tasks.
- Abbruch über `tokio_util::sync::CancellationToken` (Ersatz für `AbortController`);
  Prüfpunkte an denselben Stellen wie in TS (Schleifengrenzen, nach jedem `await`).
- Keine `block_on`-Aufrufe innerhalb von Bibliothekscode.

## 5. Abhängigkeiten

- Versionen stehen **ausschließlich** in `[workspace.dependencies]` der Root-`Cargo.toml`;
  Crates schreiben `dep = { workspace = true }`.
- Neue Abhängigkeit nötig? Nur wenn sie einer Tech-Substitution des Master-Plans
  entspricht. Andernfalls ist sie Drift und muss begründet im Ledger stehen.
  Eintrag in die Root-`Cargo.toml` macht der Owner (C) auf Zuruf via
  `plans/interface-requests.md`; kleine, sofort gemergte Commits.
- `serde_json` läuft mit `preserve_order`: JSON-Objekte behalten Einfügereihenfolge
  wie in JavaScript (nötig für byte-identische Tool-Schemas und Settings-Rewrites).
- `Cargo.lock` ist eingecheckt. Bei Merge-Konflikten im Lock: Konflikt zugunsten von
  `main` auflösen und `cargo check --workspace` neu erzeugen lassen — nie von Hand editieren.

## 6. Tests

- **Die TS-Testsuiten sind das Oracle** und werden mitportiert (gleiche Fälle, gleiche
  Erwartungswerte, gleiche Testnamen als Kommentar/Testname).
- Unit-Tests als `#[cfg(test)] mod tests` in derselben Datei, wenn die TS-Tests eng am
  Modul liegen; portierte Testdateien (`packages/*/test/*.test.ts`) werden zu
  `crates/<crate>/tests/<name>.rs` mit gleichem Dateinamen-Stamm.
- Fixtures aus dem TS-Repo werden als Dateien nach `crates/<crate>/tests/fixtures/`
  kopiert (nicht neu erzeugt) und im Ledger vermerkt.
- Kein Test darf Netzwerkzugriff oder echte Provider-Keys brauchen; key-gated TS-Tests
  werden ebenso key-gated portiert (`#[ignore]` + Env-Prüfung).

## 7. Parity-Ledger

Je Crate `crates/<name>/PARITY.md` mit drei Abschnitten:

1. **Lektüre-Protokoll** — Datum, gelesene TS-/Referenz-Datei, LOC, Task.
   Read-before-Port ist Pflicht: portieren ohne vollständige Lektüre ist verboten.
2. **Ledger** — eine Zeile je TS-Quelldatei:

   | TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
   |---|---|---|---|---|

   Status-Werte (aufsteigend): `gelesen` → `portiert` → `Tests portiert` → `verifiziert`.
   `verifiziert` heißt: portierte Tests dieser Datei laufen grün.
3. **Ausschlüsse** — TS-Datei/Verzeichnis + Begründung mit Verweis auf Master-Plan
   oder Faktenbericht.

**Abweichungsklassen** (alles andere ist Drift):
1. Sprachidiomatik ohne Verhaltensänderung
2. Extension-Entfernung exakt nach `plans/facts/extension-boundary.md`
3. Tech-Substitution aus der Master-Tabelle
4. Distributionsmechanik

TS-Bugs werden repliziert und als `bug-compat` in der Abweichungsspalte markiert.

## 8. Commits

- Format: `<bereich>: <beschreibung>` — Bereiche: `tui`, `telemetry`, `ai`, `agent`,
  `protocol`, `client`, `server`, `sqlite`, `app`, `plans`, `scaffold`.
- Ein Commit je abgeschlossenem Plan-Task (inkl. Ledger-Update und Häkchen im Plan-File).
- Vor jedem Merge nach `main`: `scripts/check.sh` grün, dann Rebase auf `main`,
  dann `git merge --ff-only`.

## 9. Formatierung und Lints

- `cargo fmt` mit Default-Einstellungen (keine `rustfmt.toml`).
- `cargo clippy --workspace --all-targets -- -D warnings` muss grün sein.
  `#[allow(...)]` nur lokal und mit Begründungskommentar, wenn die 1:1-Struktur der
  TS-Vorlage sonst verletzt würde (z. B. `too_many_arguments` bei Options-Structs).
- Gesamtprüfung: `scripts/check.sh`.
