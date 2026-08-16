# Abschluss-Parity-Audit (Entwurf)

Maschinell erzeugt von `scripts/parity-audit.sh` (Werkzeug: `tools/parity-audit.mjs`, Auftrag O-11).
Der Bericht prüft jede Datei unter `packages/*/src` des TS-Repos gegen alle `crates/*/PARITY.md`-Ledger.
Er ist ein Entwurf: die Lückenliste ist der Arbeitsvorrat für Gate G4, keine Bewertung.

- Lauf: 2026-08-16, Repo-Commit `16ad74d`
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
