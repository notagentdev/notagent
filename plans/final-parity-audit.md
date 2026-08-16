# Abschluss-Parity-Audit (Entwurf)

Maschinell erzeugt von `scripts/parity-audit.sh` (Werkzeug: `tools/parity-audit.mjs`, Auftrag O-11).
Der Bericht prüft jede Datei unter `packages/*/src` des TS-Repos gegen alle `crates/*/PARITY.md`-Ledger.
Er ist ein Entwurf: die Lückenliste ist der Arbeitsvorrat für Gate G4, keine Bewertung.

- Lauf: 2026-08-16, Repo-Commit `1b0fb13`
- TS-Repo: `/Users/dev/projects/notagent-main`
- Gelesene Ledger: 9 (`notagent`, `notagent-agent`, `notagent-ai`, `notagent-client`, `notagent-protocol`, `notagent-server`, `notagent-session-sqlite`, `notagent-telemetry`, `notagent-tui`)
- src-Verzeichnisse: 12, Dateien gesamt: 632

## Ergebnis

| Kategorie | Dateien | Bedeutung |
|---|---:|---|
| verifiziert | 455 | Ledger-Zeile mit Status `verifiziert` |
| ausgeschlossen | 74 | in einer `Ausschlüsse`-Tabelle eines Ledgers geführt |
| Rahmen-Ausschluss | 8 | im Master-Plan ausgeschlossen, ohne eigenes Ledger |
| Ledger < verifiziert | 95 | Ledger-Zeile vorhanden, Status `gelesen`/`portiert`/`Tests portiert` |
| ohne Nachweis | 0 | keine Spur in irgendeinem Ledger — harte Lücke |

## Je Paket

| TS-Paket | Owner | Dateien | verifiziert | ausgeschl. | Rahmen | < verifiziert | ohne Nachweis |
|---|---|---:|---:|---:|---:|---:|---:|
| `packages/agent` | B | 50 | 8 | 42 | 0 | 0 | 0 |
| `packages/ai` | B | 215 | 210 | 5 | 0 | 0 | 0 |
| `packages/client` | C | 10 | 10 | 0 | 0 | 0 | 0 |
| `packages/coding-agent` | C | 258 | 145 | 27 | 0 | 86 | 0 |
| `packages/evals` | — | 8 | 0 | 0 | 8 | 0 | 0 |
| `packages/protocol` | C | 8 | 8 | 0 | 0 | 0 | 0 |
| `packages/server` | C | 17 | 17 | 0 | 0 | 0 | 0 |
| `packages/session-backends/sqlite-node` | C | 19 | 19 | 0 | 0 | 0 | 0 |
| `packages/telemetry` | B | 6 | 6 | 0 | 0 | 0 | 0 |
| `packages/tui` | A | 41 | 32 | 0 | 0 | 9 | 0 |

## Lücke 1 — Dateien ohne jede Ledger-Spur

Keine. Jede src-Datei ist in mindestens einem Ledger geführt.

## Lücke 2 — Ledger-Zeile, aber Status unter `verifiziert`

Für diese Dateien fehlt der grüne Testnachweis (Status-Leiter aus `CONVENTIONS.md` §7).

| Datei | bester Status | Beleg | Art |
|---|---|---|---|
| `packages/coding-agent/src/cli.ts` | portiert | `crates/notagent/PARITY.md:539` | datei |
| `packages/coding-agent/src/cli/args.ts` | portiert | `crates/notagent/PARITY.md:537` | datei |
| `packages/coding-agent/src/cli/config-selector.ts` | portiert | `crates/notagent/PARITY.md:580` | datei |
| `packages/coding-agent/src/cli/file-processor.ts` | portiert | `crates/notagent/PARITY.md:546` | datei |
| `packages/coding-agent/src/config.ts` | portiert | `crates/notagent/PARITY.md:383` | datei |
| `packages/coding-agent/src/core/agent-session-runtime.ts` | portiert | `crates/notagent/PARITY.md:535` | datei |
| `packages/coding-agent/src/core/agent-session-services.ts` | portiert | `crates/notagent/PARITY.md:534` | datei |
| `packages/coding-agent/src/core/agent-session.ts` | portiert | `crates/notagent/PARITY.md:523` | datei |
| `packages/coding-agent/src/core/auth-guidance.ts` | gelesen | `crates/notagent/PARITY.md:751` | datei |
| `packages/coding-agent/src/core/auth-storage.ts` | portiert | `crates/notagent/PARITY.md:396` | datei |
| `packages/coding-agent/src/core/bash-executor.ts` | portiert | `crates/notagent/PARITY.md:427` | datei |
| `packages/coding-agent/src/core/export-html/template.css` | unklar | `crates/notagent/PARITY.md:895` | datei |
| `packages/coding-agent/src/core/export-html/template.html` | gelesen | `crates/notagent/PARITY.md:867` | datei |
| `packages/coding-agent/src/core/export-html/template.js` | unklar | `crates/notagent/PARITY.md:895` | datei |
| `packages/coding-agent/src/core/export-html/tool-renderer.ts` | gelesen | `crates/notagent/PARITY.md:866` | datei |
| `packages/coding-agent/src/core/hooks/events.ts` | portiert | `crates/notagent/PARITY.md:467` | datei |
| `packages/coding-agent/src/core/hooks/extension.ts` | gelesen | `crates/notagent/PARITY.md:262` | datei |
| `packages/coding-agent/src/core/hooks/hooks.ts` | portiert | `crates/notagent/PARITY.md:468` | datei |
| `packages/coding-agent/src/core/hooks/payload.ts` | portiert | `crates/notagent/PARITY.md:469` | datei |
| `packages/coding-agent/src/core/hooks/runner.ts` | portiert | `crates/notagent/PARITY.md:470` | datei |
| `packages/coding-agent/src/core/hooks/runtime.ts` | portiert | `crates/notagent/PARITY.md:471` | datei |
| `packages/coding-agent/src/core/http-dispatcher.ts` | portiert | `crates/notagent/PARITY.md:696` | datei |
| `packages/coding-agent/src/core/messages.ts` | portiert | `crates/notagent/PARITY.md:400` | datei |
| `packages/coding-agent/src/core/mini-read/index.ts` | portiert | `crates/notagent/PARITY.md:432` | datei |
| `packages/coding-agent/src/core/mini-read/languages.ts` | portiert | `crates/notagent/PARITY.md:433` | datei |
| `packages/coding-agent/src/core/mini-read/minify-edit.ts` | portiert | `crates/notagent/PARITY.md:431` | datei |
| `packages/coding-agent/src/core/mini-read/minify.ts` | portiert | `crates/notagent/PARITY.md:430` | datei |
| `packages/coding-agent/src/core/modes/cycle.ts` | portiert | `crates/notagent/PARITY.md:454` | datei |
| `packages/coding-agent/src/core/modes/indicator.ts` | portiert | `crates/notagent/PARITY.md:455` | datei |
| `packages/coding-agent/src/core/modes/modes.ts` | portiert | `crates/notagent/PARITY.md:452` | datei |
| `packages/coding-agent/src/core/modes/shells.ts` | portiert | `crates/notagent/PARITY.md:453` | datei |
| `packages/coding-agent/src/core/output-guard.ts` | portiert | `crates/notagent/PARITY.md:554` | datei |
| `packages/coding-agent/src/core/permissions/chain.ts` | portiert | `crates/notagent/PARITY.md:460` | datei |
| `packages/coding-agent/src/core/permissions/coordinator.ts` | portiert | `crates/notagent/PARITY.md:464` | datei |
| `packages/coding-agent/src/core/permissions/extension.ts` | gelesen | `crates/notagent/PARITY.md:256` | datei |
| `packages/coding-agent/src/core/permissions/hook.ts` | portiert | `crates/notagent/PARITY.md:465` | datei |
| `packages/coding-agent/src/core/permissions/policies.ts` | portiert | `crates/notagent/PARITY.md:461` | datei |
| `packages/coding-agent/src/core/permissions/policy.ts` | portiert | `crates/notagent/PARITY.md:459` | datei |
| `packages/coding-agent/src/core/permissions/request.ts` | portiert | `crates/notagent/PARITY.md:463` | datei |
| `packages/coding-agent/src/core/permissions/user-rules.ts` | portiert | `crates/notagent/PARITY.md:462` | datei |
| `packages/coding-agent/src/core/project-trust.ts` | portiert | `crates/notagent/PARITY.md:473` | datei |
| `packages/coding-agent/src/core/resolve-config-value.ts` | portiert | `crates/notagent/PARITY.md:389` | datei |
| `packages/coding-agent/src/core/resource-loader.ts` | portiert | `crates/notagent/PARITY.md:515` | datei |
| `packages/coding-agent/src/core/sdk.ts` | portiert | `crates/notagent/PARITY.md:531` | datei |
| `packages/coding-agent/src/core/session-manager.ts` | portiert | `crates/notagent/PARITY.md:399` | datei |
| `packages/coding-agent/src/core/settings-manager.ts` | portiert | `crates/notagent/PARITY.md:385` | datei |
| `packages/coding-agent/src/core/source-info.ts` | portiert | `crates/notagent/PARITY.md:450` | datei |
| `packages/coding-agent/src/core/todos/render.ts` | portiert | `crates/notagent/PARITY.md:441` | datei |
| `packages/coding-agent/src/core/todos/todos.ts` | portiert | `crates/notagent/PARITY.md:440` | datei |
| `packages/coding-agent/src/core/tools/bash.ts` | portiert | `crates/notagent/PARITY.md:425` | datei |
| `packages/coding-agent/src/core/tools/file-mutation-queue.ts` | portiert | `crates/notagent/PARITY.md:420` | datei |
| `packages/coding-agent/src/core/tools/output-accumulator.ts` | portiert | `crates/notagent/PARITY.md:407` | datei |
| `packages/coding-agent/src/core/tools/path-utils.ts` | portiert | `crates/notagent/PARITY.md:403` | datei |
| `packages/coding-agent/src/core/tools/truncate.ts` | portiert | `crates/notagent/PARITY.md:402` | datei |
| `packages/coding-agent/src/core/trust-manager.ts` | portiert | `crates/notagent/PARITY.md:474` | datei |
| `packages/coding-agent/src/main.ts` | portiert | `crates/notagent/PARITY.md:542` | datei |
| `packages/coding-agent/src/migrations.ts` | portiert | `crates/notagent/PARITY.md:387` | datei |
| `packages/coding-agent/src/modes/index.ts` | portiert | `crates/notagent/PARITY.md:558` | datei |
| `packages/coding-agent/src/modes/interactive/assets/clankolas.png` | unklar | `crates/notagent/PARITY.md:593` | datei |
| `packages/coding-agent/src/modes/interactive/components/extension-editor.ts` | gelesen | `crates/notagent/PARITY.md:113` | verzeichnis |
| `packages/coding-agent/src/modes/interactive/components/extension-input.ts` | gelesen | `crates/notagent/PARITY.md:113` | verzeichnis |
| `packages/coding-agent/src/modes/interactive/components/index.ts` | portiert | `crates/notagent/PARITY.md:635` | datei |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | portiert | `crates/notagent/PARITY.md:575` | datei |
| `packages/coding-agent/src/modes/interactive/model-search.ts` | portiert | `crates/notagent/PARITY.md:451` | datei |
| `packages/coding-agent/src/modes/interactive/theme/theme-controller.ts` | portiert | `crates/notagent/PARITY.md:616` | datei |
| `packages/coding-agent/src/modes/interactive/theme/theme-schema.json` | gelesen | `crates/notagent/PARITY.md:68` | datei |
| `packages/coding-agent/src/modes/rpc/rpc-mode.ts` | portiert | `crates/notagent/PARITY.md:563` | datei |
| `packages/coding-agent/src/modes/rpc/rpc-types.ts` | portiert | `crates/notagent/PARITY.md:562` | datei |
| `packages/coding-agent/src/package-manager-cli.ts` | portiert | `crates/notagent/PARITY.md:581` | datei |
| `packages/coding-agent/src/rpc-entry.ts` | portiert | `crates/notagent/PARITY.md:540` | datei |
| `packages/coding-agent/src/utils/abort.ts` | portiert | `crates/notagent/PARITY.md:394` | datei |
| `packages/coding-agent/src/utils/ansi.ts` | portiert | `crates/notagent/PARITY.md:393` | datei |
| `packages/coding-agent/src/utils/child-process.ts` | portiert | `crates/notagent/PARITY.md:426` | datei |
| `packages/coding-agent/src/utils/frontmatter.ts` | portiert | `crates/notagent/PARITY.md:457` | datei |
| `packages/coding-agent/src/utils/html.ts` | portiert | `crates/notagent/PARITY.md:448` | datei |
| `packages/coding-agent/src/utils/image-convert.ts` | portiert | `crates/notagent/PARITY.md:417` | datei |
| `packages/coding-agent/src/utils/image-process.ts` | portiert | `crates/notagent/PARITY.md:417` | datei |
| `packages/coding-agent/src/utils/image-resize-core.ts` | portiert | `crates/notagent/PARITY.md:417` | datei |
| `packages/coding-agent/src/utils/image-resize.ts` | portiert | `crates/notagent/PARITY.md:417` | datei |
| `packages/coding-agent/src/utils/management-http.ts` | portiert | `crates/notagent/PARITY.md:411` | datei |
| `packages/coding-agent/src/utils/mime.ts` | portiert | `crates/notagent/PARITY.md:416` | datei |
| `packages/coding-agent/src/utils/paths.ts` | portiert | `crates/notagent/PARITY.md:391` | datei |
| `packages/coding-agent/src/utils/shell.ts` | portiert | `crates/notagent/PARITY.md:390` | datei |
| `packages/coding-agent/src/utils/syntax-highlight.ts` | portiert | `crates/notagent/PARITY.md:447` | datei |
| `packages/coding-agent/src/utils/tool-result-images.ts` | unklar | `crates/notagent/PARITY.md:592` | datei |
| `packages/coding-agent/src/utils/tools-manager.ts` | portiert | `crates/notagent/PARITY.md:410` | datei |
| `packages/tui/native/darwin/src/darwin-modifiers.c` | portiert | `crates/notagent-tui/PARITY.md:107` | datei |
| `packages/tui/native/win32/src/win32-console-mode.c` | portiert | `crates/notagent-tui/PARITY.md:108` | datei |
| `packages/tui/src/components/alt-screen-flash.ts` | portiert | `crates/notagent-tui/PARITY.md:94` | datei |
| `packages/tui/src/components/box.ts` | portiert | `crates/notagent-tui/PARITY.md:93` | datei |
| `packages/tui/src/components/cancellable-loader.ts` | portiert | `crates/notagent-tui/PARITY.md:100` | datei |
| `packages/tui/src/components/image.ts` | portiert | `crates/notagent-tui/PARITY.md:97` | datei |
| `packages/tui/src/components/loader.ts` | portiert | `crates/notagent-tui/PARITY.md:95` | datei |
| `packages/tui/src/components/spacer.ts` | portiert | `crates/notagent-tui/PARITY.md:91` | datei |
| `packages/tui/src/native-modifiers.ts` | portiert | `crates/notagent-tui/PARITY.md:106` | datei |

## Lücke 3 — Ledger-Zeilen mit Status außerhalb der Leiter

`CONVENTIONS.md` §7 kennt genau vier Status-Werte (`gelesen`, `portiert`, `Tests portiert`,
`verifiziert`). Diese Zeilen decken src-Dateien ab, schreiben aber etwas anderes in die
Statusspalte; die Prüfung stuft sie höchstens als `portiert` ein. Formalbefund, kein Portmangel.

| Ledger | Zeile | Statustext | eingestuft als | Dateien |
|---|---:|---|---|---:|
| `crates/notagent/PARITY.md` | 412 | offen | unklar | 1 |
| `crates/notagent/PARITY.md` | 413 | offen | unklar | 1 |
| `crates/notagent/PARITY.md` | 442 | offen | unklar | 1 |
| `crates/notagent/PARITY.md` | 466 | nativ ersetzt | unklar | 1 |
| `crates/notagent/PARITY.md` | 472 | nativ ersetzt | unklar | 1 |
| `crates/notagent/PARITY.md` | 552 | offen (Task 13) | unklar | 1 |
| `crates/notagent/PARITY.md` | 553 | teilweise entfallen, Rest Task 13 | portiert | 1 |
| `crates/notagent/PARITY.md` | 592 | offen (Task 15) | unklar | 1 |
| `crates/notagent/PARITY.md` | 593 | offen (Task 15) | unklar | 1 |
| `crates/notagent/PARITY.md` | 594 | übertragen an B (O-12 Punkt 2) | unklar | 3 |
| `crates/notagent/PARITY.md` | 620 | übernommen | unklar | 1 |
| `crates/notagent/PARITY.md` | 635 | teilweise portiert | portiert | 1 |
| `crates/notagent/PARITY.md` | 696 | teilweise portiert | portiert | 1 |
| `crates/notagent/PARITY.md` | 894 | teilportiert | unklar | 1 |
| `crates/notagent/PARITY.md` | 895 | übernommen | unklar | 3 |
| `crates/notagent/PARITY.md` | 901 | nachgezogen | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 422 | übernommen (Task 10) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 716 | (leer) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 717 | (leer) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 718 | (leer) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 719 | (leer) | unklar | 1 |

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
| `crates/notagent/PARITY.md:1008` | `src/cli/experimental/` | ausgeschlossen | 8 |
| `crates/notagent/PARITY.md:1005` | `src/core/extensions/` | ausgeschlossen | 5 |
| `crates/notagent/PARITY.md:1008` | `src/bun/` | ausgeschlossen | 3 |
| `crates/notagent/PARITY.md:456` | `src/core/modes/builtin/auto/10-*.md` | verifiziert | 1 |
| `crates/notagent/PARITY.md:456` | `src/core/modes/builtin/manual/10-*.md` | verifiziert | 1 |
| `crates/notagent/PARITY.md:456` | `src/core/modes/builtin/plan/10-*.md` | verifiziert | 1 |
| `crates/notagent/PARITY.md:456` | `src/core/modes/builtin/yolo/10-*.md` | verifiziert | 1 |
| `crates/notagent/PARITY.md:1005` | `src/extensions/` | ausgeschlossen | 1 |

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
