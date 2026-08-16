# Abschluss-Parity-Audit (Entwurf)

Maschinell erzeugt von `scripts/parity-audit.sh` (Werkzeug: `tools/parity-audit.mjs`, Auftrag O-11).
Der Bericht prüft jede Datei unter `packages/*/src` des TS-Repos gegen alle `crates/*/PARITY.md`-Ledger.
Er ist ein Entwurf: die Lückenliste ist der Arbeitsvorrat für Gate G4, keine Bewertung.

- Lauf: 2026-08-16, Repo-Commit `2a168f1`
- TS-Repo: `/Users/dev/projects/notagent-main`
- Gelesene Ledger: 9 (`notagent`, `notagent-agent`, `notagent-ai`, `notagent-client`, `notagent-protocol`, `notagent-server`, `notagent-session-sqlite`, `notagent-telemetry`, `notagent-tui`)
- src-Verzeichnisse: 12, Dateien gesamt: 632

## Ergebnis

| Kategorie | Dateien | Bedeutung |
|---|---:|---|
| verifiziert | 544 | Ledger-Zeile mit Status `verifiziert` |
| ausgeschlossen | 77 | in einer `Ausschlüsse`-Tabelle eines Ledgers geführt |
| Rahmen-Ausschluss | 8 | im Master-Plan ausgeschlossen, ohne eigenes Ledger |
| Ledger < verifiziert | 3 | Ledger-Zeile vorhanden, Status `gelesen`/`portiert`/`Tests portiert` |
| ohne Nachweis | 0 | keine Spur in irgendeinem Ledger — harte Lücke |

## Je Paket

| TS-Paket | Owner | Dateien | verifiziert | ausgeschl. | Rahmen | < verifiziert | ohne Nachweis |
|---|---|---:|---:|---:|---:|---:|---:|
| `packages/agent` | B | 50 | 8 | 42 | 0 | 0 | 0 |
| `packages/ai` | B | 215 | 210 | 5 | 0 | 0 | 0 |
| `packages/client` | C | 10 | 10 | 0 | 0 | 0 | 0 |
| `packages/coding-agent` | C | 258 | 228 | 30 | 0 | 0 | 0 |
| `packages/evals` | — | 8 | 0 | 0 | 8 | 0 | 0 |
| `packages/protocol` | C | 8 | 8 | 0 | 0 | 0 | 0 |
| `packages/server` | C | 17 | 17 | 0 | 0 | 0 | 0 |
| `packages/session-backends/sqlite-node` | C | 19 | 19 | 0 | 0 | 0 | 0 |
| `packages/telemetry` | B | 6 | 6 | 0 | 0 | 0 | 0 |
| `packages/tui` | A | 41 | 38 | 0 | 0 | 3 | 0 |

## Lücke 1 — Dateien ohne jede Ledger-Spur

Keine. Jede src-Datei ist in mindestens einem Ledger geführt.

## Lücke 2 — Ledger-Zeile, aber Status unter `verifiziert`

Für diese Dateien fehlt der grüne Testnachweis (Status-Leiter aus `CONVENTIONS.md` §7).

| Datei | bester Status | Beleg | Art |
|---|---|---|---|
| `packages/tui/native/darwin/src/darwin-modifiers.c` | portiert | `crates/notagent-tui/PARITY.md:108` | datei |
| `packages/tui/native/win32/src/win32-console-mode.c` | portiert | `crates/notagent-tui/PARITY.md:109` | datei |
| `packages/tui/src/native-modifiers.ts` | portiert | `crates/notagent-tui/PARITY.md:107` | datei |

## Lücke 3 — Ledger-Zeilen mit Status außerhalb der Leiter

`CONVENTIONS.md` §7 kennt genau vier Status-Werte (`gelesen`, `portiert`, `Tests portiert`,
`verifiziert`). Diese Zeilen decken src-Dateien ab, schreiben aber etwas anderes in die
Statusspalte; die Prüfung stuft sie höchstens als `portiert` ein. Formalbefund, kein Portmangel.

| Ledger | Zeile | Statustext | eingestuft als | Dateien |
|---|---:|---|---|---:|
| `crates/notagent-ai/PARITY.md` | 422 | übernommen (Task 10) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 716 | (leer) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 717 | (leer) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 718 | (leer) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 719 | (leer) | unklar | 1 |
| `crates/notagent-tui/PARITY.md` | 226 | (leer) | unklar | 1 |
| `crates/notagent-tui/PARITY.md` | 227 | (leer) | unklar | 1 |
| `crates/notagent-tui/PARITY.md` | 228 | (leer) | unklar | 1 |

## Abdeckung über Sammelzeilen

Diese Dateien sind nicht einzeln geführt, sondern über eine Verzeichnis- oder Musterzeile.
Das ist zulässig, aber die schwächste Form des Nachweises — an G4 einmal gegenlesen, ob die
Zeile wirklich jede Datei darunter meint (einschränkende Prosa wie „außer X" wertet die
Prüfung nicht aus).

| Beleg | Angabe | Status | Dateien |
|---|---|---|---:|
| `crates/notagent-ai/PARITY.md:262` | `src/providers/` | verifiziert | 86 |
| `crates/notagent-ai/PARITY.md:260` | `src/providers/data/` | verifiziert | 40 |
| `crates/notagent-agent/PARITY.md:51` | `src/harness/**` | ausgeschlossen | 38 |
| `crates/notagent-ai/PARITY.md:273` | `src/api/*.lazy.ts` | verifiziert | 11 |
| `crates/notagent/PARITY.md:1015` | `src/cli/experimental/` | ausgeschlossen | 8 |
| `crates/notagent/PARITY.md:1012` | `src/core/extensions/` | ausgeschlossen | 5 |
| `crates/notagent/PARITY.md:1015` | `src/bun/` | ausgeschlossen | 3 |
| `crates/notagent/PARITY.md:457` | `src/core/modes/builtin/auto/10-*.md` | verifiziert | 1 |
| `crates/notagent/PARITY.md:457` | `src/core/modes/builtin/manual/10-*.md` | verifiziert | 1 |
| `crates/notagent/PARITY.md:457` | `src/core/modes/builtin/plan/10-*.md` | verifiziert | 1 |
| `crates/notagent/PARITY.md:457` | `src/core/modes/builtin/yolo/10-*.md` | verifiziert | 1 |
| `crates/notagent/PARITY.md:1012` | `src/extensions/` | ausgeschlossen | 1 |

## Rahmen-Ausschlüsse aus dem Master-Plan

| Pfad | betroffene src-Dateien | Beleg |
|---|---:|---|
| `packages/evals` | 8 | `plans/2026-08-13-rust-port-master-v1.md:31` |

## Unbekannte Ledger-Pfade

Keine. Jede Pfadangabe in den Ledgern zeigt auf eine existierende Datei oder ein existierendes Verzeichnis.

## Methode

1. **Bestand**: alle Dateien unter jedem `src`-Verzeichnis unterhalb von `packages/` (also auch
   `packages/session-backends/sqlite-node/src`). Keine Filterung nach Endung.
2. **Ledger**: jede Markdown-Tabelle in `crates/*/PARITY.md`. Die Statusspalte liefert den Status
   (`gelesen` → `portiert` → `Tests portiert` → `verifiziert`); Tabellen unter einer
   `Ausschlüsse`-Überschrift zählen als dokumentierter Ausschluss; das Lektüre-Protokoll zählt
   höchstens als `gelesen`.
3. **Pfadauflösung**: Angaben werden relativ zum Paket der Kopfzeile (`TS-Quelle:`), zu dessen `src/`
   oder repo-relativ (`packages/…`) aufgelöst. Verzeichniszeilen (`src/providers/data/`) und Muster
   (`src/api/*.lazy.ts`) gelten für alle darunterliegenden Dateien; solche Belege sind in Lücke 2 als
   `verzeichnis`/`muster` markiert und decken schwächer ab als eine eigene Zeile.
4. **Rangfolge je Datei**: `verifiziert` > dokumentierter Ausschluss > `Tests portiert` > `portiert` >
   `gelesen`. Mehrere Ledger dürfen dieselbe Datei führen (geteilte Pakete wie `coding-agent`).
5. **Nachbarpakete und Geschwisterdateien**: Ein Pfad, den das eigene Paket nicht auflöst, wird gegen
   alle Paketwurzeln probiert und nur bei genau einem Treffer übernommen. Ein blanker Dateiname
   (`… + image-convert.ts + …`) gilt relativ zum Verzeichnis der vorhergehenden Pfadangabe derselben Zeile.

**Was die Prüfung nicht kann**: Prosa in der Statusspalte („außer `messages.ts`", „Kernfälle") wertet
sie nicht aus, und sie sagt nichts über die inhaltliche Güte eines Ports — nur darüber, ob die
Buchführung eine Aussage zu der Datei enthält.

Erneut laufen lassen: `scripts/parity-audit.sh` (Optionen: `--ts-repo`, `--out`, `--check`, `--quiet`,
`--explain <datei>`). `--check` endet mit Exit-Code 1, solange Dateien ohne jede Ledger-Spur bleiben —
für Gate G4. `--explain packages/…/foo.ts` zeigt jede Ledger-Spur einer einzelnen Datei.

---

# G4-Abnahme (von Hand, Workstream C als Gate-Verwalter, 2026-08-16)

## 1. Zahlen des Laufs oben

| Kategorie | Dateien | Bewertung |
|---|---:|---|
| verifiziert | 544 | 86 % aller 632 src-Dateien |
| dokumentierter Ausschluss (Ledger + Master-Plan) | 85 | 13 % |
| Ledger-Zeile unter `verifiziert` | 3 | siehe Punkt 2 |
| ohne jede Ledger-Spur | 0 | `scripts/parity-audit.sh --out /tmp/parity.md --check` endet mit Exit 0 |

## 2. Die drei Dateien unter `verifiziert` — Prüfobergrenze, kein offener Port

`packages/tui/src/native-modifiers.ts`, `native/darwin/src/darwin-modifiers.c` und
`native/win32/src/win32-console-mode.c`. Workstream A führt sie mit Status `portiert` und
einer eigenen Tabelle „Prüfobergrenzen" (`crates/notagent-tui/PARITY.md:224-228`):

- Der Aufruf erreicht die Plattform und antwortet — das belegt `tests/base_components.rs`.
- Ein `true` verlangt eine **im Messmoment gedrückte** Modifier-Taste; die kann kein Test
  herstellen, weder in TypeScript noch in Rust (die TS-Seite hat für diese drei Dateien
  ebenfalls keine Suite).
- Der Windows-Zweig kompiliert nur unter `cfg(target_os = "windows")` und braucht eine echte
  Windows-Konsole; diese Maschine ist macOS.

Bewertung: das ist die im Master-Plan (Risiko 3, Contingency) vorgesehene dokumentierte
Plattformgrenze, kein Portmangel. Alle anderen Dateien tragen `verifiziert` oder einen
Ausschluss.

## 3. „Lücke 3" nach der Bereinigung

Die neun Zeilen aus Cs Ledger sind aufgelöst: sechs waren überholte Doppel-Zeilen aus der
Task, in der die Datei noch offen war (der Nachweis stand längst in einer zweiten Zeile), zwei
sind die nativ ersetzten Extension-Hüllen und stehen jetzt in der Ausschluss-Tabelle, eine
war die an B übertragene Client-Schicht. Was bleibt, sind acht Zeilen aus dreispaltigen
**Nachtrags-** und **Prüfobergrenzen-Tabellen** von A und B, die die Prüfung als Ledger-Zeilen
ohne Statusspalte liest — Formalbefund des Parsers, keine Aussage über einen Port.

## 4. Feature-Checkliste gegen die sechs Faktendokumente

| Faktendokument | Prüfpunkt | Beleg |
|---|---|---|
| `coding-agent-core.md` | 16 Tools mit Registry und Presets | `core/tools/`, Suiten `bash_tool`, `bash_background`, `minified_tools`, `mini_read_multi_edit`, `todo_and_skill_tools`, `task_tool(s)`, `render_utils`, `tool_render_oracle` |
| | Session-JSONL v3, 9 Entry-Typen, Branching, Kontextaufbau | `core/session_manager.rs`, `tests/session_manager.rs` mit den TS-Fixtures |
| | Modes/Permissions/Hooks nativ | `core/modes*`, `core/permissions/`, `core/hooks/`; `tests/modes.rs`, `tests/permissions.rs`, `tests/permission_end_to_end.rs`, `tests/hooks.rs`, `tests/hook_dispatch.rs` |
| | Tasks und Delegation echt parallel | `core/tasks/`, `core/delegation/`; `tests/tasks_parallel.rs` (Wanduhrzeit), `tests/delegation_*.rs` |
| | Compaction mit beiden Prompts und Overflow-Recovery | `core/compaction/`, `tests/compaction.rs`, `tests/agent_session_compaction.rs` |
| | CLI-Flags und Subkommandos | `cli/args.rs`, `tests/args.rs`; Smoke-Schritte 2-5 |
| | Print/JSON/RPC | `modes/`, `tests/headless_end_to_end.rs`, `tests/rpc_*.rs`; Smoke-Schritte 6-8 |
| `extension-boundary.md` §2 | Permissions nativ statt Extension | Ausschluss-Tabelle + `permissions/gate.rs` im Agent-Loop |
| | Hooks nativ statt Extension (16 Namen, nur PreToolUse blockiert) | `hooks/dispatch.rs`; `tests/hook_dispatch.rs` prüft alle 16 Namen; `PostToolUse` seit Task 15 aus dem `after_tool_call` der Session |
| | llama.cpp-Provider + `/llama` | `core/llama/`, `modes/interactive/llama_command.rs`; `tests/llama_extension.rs`, `tests/llama_command.rs`; Smoke-Schritt 11 |
| §3 Integrationspunkte | jeder Punkt nativ ersetzt oder als entfallend geführt | die Ledger-Zeilen der genannten Dateien; die Extension-eigenen Punkte stehen in der Ausschluss-Tabelle |
| `ai-and-agent.md` | Provider, APIs, Streaming, Agent-Loop | Workstream B, `crates/notagent-ai/PARITY.md` und `crates/notagent-agent/PARITY.md` |
| `tui.md` | Renderer, Layout, Komponenten, Editor, Keybindings | Workstream A, `crates/notagent-tui/PARITY.md`; G3-Szenarien in `tests/g3_interactive_e2e.rs` |
| `protocol-…-evals.md` | protocol/client/server/sqlite | die vier Crates, alle Ledger vollständig `verifiziert`; evals ist Master-Plan-Ausschluss |
| `rust-minify-reference.md` | Minify nach der Rust-Referenz, byte-genau | `core/mini_read/`, Unit-Tests plus `tests/minified_tools.rs` und die Testbench-Fälle |

## 5. Gate-Kriterien

**G3 — Interaktive Parität**
- Interactive-Mode vollständig verdrahtet: Editor, Slash-Commands (alle aus der Faktenliste,
  inklusive `/llama` und der beiden Easter Eggs), Selektoren, Themes, Keybindings (45
  Actions), Footer, Panels, Fullscreen — Plan-Tasks 13 bis 16 abgehakt.
- End-to-End über das virtuelle Terminal: `tests/g3_interactive_e2e.rs` — **20 Fälle grün,
  kein `#[ignore]`**; dazu 40 Fälle in `tests/interactive_mode_wiring.rs`.
- `scripts/check.sh` grün auf main.

**G4 — Release-Parität**
- Alle portierten Suiten grün im Gesamtworkspace: **3 861 Tests in 249 Suiten**, `check.sh`
  (fmt, clippy `-D warnings`, test) mit Exit 0.
- Parity-Ledger aller neun Crates vollständig: 0 Dateien ohne Nachweis, `--check` Exit 0.
- Feature-Checkliste gegen alle sechs Faktendokumente: Abschnitt 4.
- Manueller Smoke-Test: `plans/g4-smoke-report.md`, 12 Schritte, kein Fehlschlag.
