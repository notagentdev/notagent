# Abschluss-Parity-Audit (Entwurf)

Maschinell erzeugt von `scripts/parity-audit.sh` (Werkzeug: `tools/parity-audit.mjs`, Auftrag O-11).
Der Bericht prüft jede Datei unter `packages/*/src` des TS-Repos gegen alle `crates/*/PARITY.md`-Ledger.
Er ist ein Entwurf: die Lückenliste ist der Arbeitsvorrat für Gate G4, keine Bewertung.

- Lauf: 2026-08-16, Repo-Commit `b507cea`
- TS-Repo: `/Users/dev/projects/notagent-main`
- Gelesene Ledger: 9 (`notagent`, `notagent-agent`, `notagent-ai`, `notagent-client`, `notagent-protocol`, `notagent-server`, `notagent-session-sqlite`, `notagent-telemetry`, `notagent-tui`)
- src-Verzeichnisse: 12, Dateien gesamt: 632

## Ergebnis

| Kategorie | Dateien | Bedeutung |
|---|---:|---|
| verifiziert | 384 | Ledger-Zeile mit Status `verifiziert` |
| ausgeschlossen | 73 | in einer `Ausschlüsse`-Tabelle eines Ledgers geführt |
| Rahmen-Ausschluss | 8 | im Master-Plan ausgeschlossen, ohne eigenes Ledger |
| Ledger < verifiziert | 145 | Ledger-Zeile vorhanden, Status `gelesen`/`portiert`/`Tests portiert` |
| ohne Nachweis | 22 | keine Spur in irgendeinem Ledger — harte Lücke |

## Je Paket

| TS-Paket | Owner | Dateien | verifiziert | ausgeschl. | Rahmen | < verifiziert | ohne Nachweis |
|---|---|---:|---:|---:|---:|---:|---:|
| `packages/agent` | B | 50 | 7 | 43 | 0 | 0 | 0 |
| `packages/ai` | B | 215 | 191 | 5 | 0 | 19 | 0 |
| `packages/client` | C | 10 | 10 | 0 | 0 | 0 | 0 |
| `packages/coding-agent` | C | 258 | 127 | 25 | 0 | 84 | 22 |
| `packages/evals` | — | 8 | 0 | 0 | 8 | 0 | 0 |
| `packages/protocol` | C | 8 | 8 | 0 | 0 | 0 | 0 |
| `packages/server` | C | 17 | 13 | 0 | 0 | 4 | 0 |
| `packages/session-backends/sqlite-node` | C | 19 | 3 | 0 | 0 | 16 | 0 |
| `packages/telemetry` | B | 6 | 4 | 0 | 0 | 2 | 0 |
| `packages/tui` | A | 41 | 21 | 0 | 0 | 20 | 0 |

## Lücke 1 — Dateien ohne jede Ledger-Spur

Diese Dateien tauchen in keinem Ledger auf — weder als Zeile noch als Ausschluss.

### `packages/coding-agent` — 22 Dateien (Owner: C)

| Datei | LOC |
|---|---:|
| `src/client/index.ts` | 15 |
| `src/client/remote-session.ts` | 414 |
| `src/client/transcript.ts` | 101 |
| `src/core/event-bus.ts` | 33 |
| `src/core/exec.ts` | 107 |
| `src/core/export-html/vendor/highlight.min.js` | 1213 |
| `src/core/export-html/vendor/marked.min.js` | 78 |
| `src/core/index.ts` | 80 |
| `src/core/radius.ts` | 1 |
| `src/index.ts` | 408 |
| `src/modes/interactive/assets/clankolas.png` | 1759 |
| `src/utils/changelog.ts` | 196 |
| `src/utils/clipboard-image.ts` | 300 |
| `src/utils/clipboard-native.ts` | 33 |
| `src/utils/clipboard.ts` | 175 |
| `src/utils/deprecation.ts` | 14 |
| `src/utils/exif-orientation.ts` | 183 |
| `src/utils/highlight-js-lib-index.d.ts` | 19 |
| `src/utils/image-resize-worker.ts` | 42 |
| `src/utils/photon.ts` | 139 |
| `src/utils/sleep.ts` | 18 |
| `src/utils/tool-result-images.ts` | 62 |


## Lücke 2 — Ledger-Zeile, aber Status unter `verifiziert`

Für diese Dateien fehlt der grüne Testnachweis (Status-Leiter aus `CONVENTIONS.md` §7).

| Datei | bester Status | Beleg | Art |
|---|---|---|---|
| `packages/ai/src/api/github-copilot-headers.ts` | portiert | `crates/notagent-ai/PARITY.md:300` | datei |
| `packages/ai/src/api/lazy.ts` | portiert | `crates/notagent-ai/PARITY.md:303` | datei |
| `packages/ai/src/auth/context.ts` | portiert | `crates/notagent-ai/PARITY.md:307` | datei |
| `packages/ai/src/auth/helpers.ts` | portiert | `crates/notagent-ai/PARITY.md:344` | datei |
| `packages/ai/src/auth/types.ts` | portiert | `crates/notagent-ai/PARITY.md:304` | datei |
| `packages/ai/src/compat/extension-oauth-types.ts` | portiert | `crates/notagent-ai/PARITY.md:288` | datei |
| `packages/ai/src/image-models.ts` | portiert | `crates/notagent-ai/PARITY.md:423` | datei |
| `packages/ai/src/models-store.ts` | portiert | `crates/notagent-ai/PARITY.md:293` | datei |
| `packages/ai/src/models.generated.ts` | portiert | `crates/notagent-ai/PARITY.md:259` | datei |
| `packages/ai/src/oauth.ts` | portiert | `crates/notagent-ai/PARITY.md:433` | datei |
| `packages/ai/src/utils/abort-signals.ts` | portiert | `crates/notagent-ai/PARITY.md:252` | datei |
| `packages/ai/src/utils/abort.ts` | portiert | `crates/notagent-ai/PARITY.md:252` | datei |
| `packages/ai/src/utils/deferred-tools.ts` | portiert | `crates/notagent-ai/PARITY.md:255` | datei |
| `packages/ai/src/utils/diagnostics.ts` | portiert | `crates/notagent-ai/PARITY.md:239` | datei |
| `packages/ai/src/utils/error-body.ts` | portiert | `crates/notagent-ai/PARITY.md:254` | datei |
| `packages/ai/src/utils/hash.ts` | portiert | `crates/notagent-ai/PARITY.md:249` | datei |
| `packages/ai/src/utils/headers.ts` | portiert | `crates/notagent-ai/PARITY.md:250` | datei |
| `packages/ai/src/utils/provider-env.ts` | portiert | `crates/notagent-ai/PARITY.md:256` | datei |
| `packages/ai/src/utils/sanitize-unicode.ts` | portiert | `crates/notagent-ai/PARITY.md:251` | datei |
| `packages/coding-agent/src/cli.ts` | portiert | `crates/notagent/PARITY.md:513` | datei |
| `packages/coding-agent/src/cli/args.ts` | portiert | `crates/notagent/PARITY.md:511` | datei |
| `packages/coding-agent/src/cli/config-selector.ts` | gelesen | `crates/notagent/PARITY.md:167` | datei |
| `packages/coding-agent/src/cli/file-processor.ts` | portiert | `crates/notagent/PARITY.md:520` | datei |
| `packages/coding-agent/src/config.ts` | portiert | `crates/notagent/PARITY.md:357` | datei |
| `packages/coding-agent/src/core/agent-session-runtime.ts` | portiert | `crates/notagent/PARITY.md:509` | datei |
| `packages/coding-agent/src/core/agent-session-services.ts` | portiert | `crates/notagent/PARITY.md:508` | datei |
| `packages/coding-agent/src/core/agent-session.ts` | portiert | `crates/notagent/PARITY.md:497` | datei |
| `packages/coding-agent/src/core/auth-guidance.ts` | gelesen | `crates/notagent/PARITY.md:701` | datei |
| `packages/coding-agent/src/core/auth-storage.ts` | portiert | `crates/notagent/PARITY.md:370` | datei |
| `packages/coding-agent/src/core/bash-executor.ts` | portiert | `crates/notagent/PARITY.md:401` | datei |
| `packages/coding-agent/src/core/export-html/template.css` | unklar | `crates/notagent/PARITY.md:845` | datei |
| `packages/coding-agent/src/core/export-html/template.html` | gelesen | `crates/notagent/PARITY.md:817` | datei |
| `packages/coding-agent/src/core/export-html/template.js` | unklar | `crates/notagent/PARITY.md:845` | datei |
| `packages/coding-agent/src/core/export-html/tool-renderer.ts` | gelesen | `crates/notagent/PARITY.md:816` | datei |
| `packages/coding-agent/src/core/hooks/events.ts` | portiert | `crates/notagent/PARITY.md:441` | datei |
| `packages/coding-agent/src/core/hooks/extension.ts` | gelesen | `crates/notagent/PARITY.md:262` | datei |
| `packages/coding-agent/src/core/hooks/hooks.ts` | portiert | `crates/notagent/PARITY.md:442` | datei |
| `packages/coding-agent/src/core/hooks/payload.ts` | portiert | `crates/notagent/PARITY.md:443` | datei |
| `packages/coding-agent/src/core/hooks/runner.ts` | portiert | `crates/notagent/PARITY.md:444` | datei |
| `packages/coding-agent/src/core/hooks/runtime.ts` | portiert | `crates/notagent/PARITY.md:445` | datei |
| `packages/coding-agent/src/core/http-dispatcher.ts` | portiert | `crates/notagent/PARITY.md:648` | datei |
| `packages/coding-agent/src/core/messages.ts` | portiert | `crates/notagent/PARITY.md:374` | datei |
| `packages/coding-agent/src/core/mini-read/index.ts` | portiert | `crates/notagent/PARITY.md:406` | datei |
| `packages/coding-agent/src/core/mini-read/languages.ts` | portiert | `crates/notagent/PARITY.md:407` | datei |
| `packages/coding-agent/src/core/mini-read/minify-edit.ts` | portiert | `crates/notagent/PARITY.md:405` | datei |
| `packages/coding-agent/src/core/mini-read/minify.ts` | portiert | `crates/notagent/PARITY.md:404` | datei |
| `packages/coding-agent/src/core/modes/cycle.ts` | portiert | `crates/notagent/PARITY.md:428` | datei |
| `packages/coding-agent/src/core/modes/indicator.ts` | portiert | `crates/notagent/PARITY.md:429` | datei |
| `packages/coding-agent/src/core/modes/modes.ts` | portiert | `crates/notagent/PARITY.md:426` | datei |
| `packages/coding-agent/src/core/modes/shells.ts` | portiert | `crates/notagent/PARITY.md:427` | datei |
| `packages/coding-agent/src/core/output-guard.ts` | portiert | `crates/notagent/PARITY.md:528` | datei |
| `packages/coding-agent/src/core/permissions/chain.ts` | portiert | `crates/notagent/PARITY.md:434` | datei |
| `packages/coding-agent/src/core/permissions/coordinator.ts` | portiert | `crates/notagent/PARITY.md:438` | datei |
| `packages/coding-agent/src/core/permissions/extension.ts` | gelesen | `crates/notagent/PARITY.md:256` | datei |
| `packages/coding-agent/src/core/permissions/hook.ts` | portiert | `crates/notagent/PARITY.md:439` | datei |
| `packages/coding-agent/src/core/permissions/policies.ts` | portiert | `crates/notagent/PARITY.md:435` | datei |
| `packages/coding-agent/src/core/permissions/policy.ts` | portiert | `crates/notagent/PARITY.md:433` | datei |
| `packages/coding-agent/src/core/permissions/request.ts` | portiert | `crates/notagent/PARITY.md:437` | datei |
| `packages/coding-agent/src/core/permissions/user-rules.ts` | portiert | `crates/notagent/PARITY.md:436` | datei |
| `packages/coding-agent/src/core/project-trust.ts` | portiert | `crates/notagent/PARITY.md:447` | datei |
| `packages/coding-agent/src/core/resolve-config-value.ts` | portiert | `crates/notagent/PARITY.md:363` | datei |
| `packages/coding-agent/src/core/resource-loader.ts` | portiert | `crates/notagent/PARITY.md:489` | datei |
| `packages/coding-agent/src/core/sdk.ts` | portiert | `crates/notagent/PARITY.md:505` | datei |
| `packages/coding-agent/src/core/session-manager.ts` | portiert | `crates/notagent/PARITY.md:373` | datei |
| `packages/coding-agent/src/core/settings-manager.ts` | portiert | `crates/notagent/PARITY.md:359` | datei |
| `packages/coding-agent/src/core/source-info.ts` | portiert | `crates/notagent/PARITY.md:424` | datei |
| `packages/coding-agent/src/core/todos/render.ts` | portiert | `crates/notagent/PARITY.md:415` | datei |
| `packages/coding-agent/src/core/todos/todos.ts` | portiert | `crates/notagent/PARITY.md:414` | datei |
| `packages/coding-agent/src/core/tools/bash.ts` | portiert | `crates/notagent/PARITY.md:399` | datei |
| `packages/coding-agent/src/core/tools/file-mutation-queue.ts` | portiert | `crates/notagent/PARITY.md:394` | datei |
| `packages/coding-agent/src/core/tools/output-accumulator.ts` | portiert | `crates/notagent/PARITY.md:381` | datei |
| `packages/coding-agent/src/core/tools/path-utils.ts` | portiert | `crates/notagent/PARITY.md:377` | datei |
| `packages/coding-agent/src/core/tools/truncate.ts` | portiert | `crates/notagent/PARITY.md:376` | datei |
| `packages/coding-agent/src/core/trust-manager.ts` | portiert | `crates/notagent/PARITY.md:448` | datei |
| `packages/coding-agent/src/main.ts` | portiert | `crates/notagent/PARITY.md:516` | datei |
| `packages/coding-agent/src/migrations.ts` | portiert | `crates/notagent/PARITY.md:361` | datei |
| `packages/coding-agent/src/modes/index.ts` | portiert | `crates/notagent/PARITY.md:532` | datei |
| `packages/coding-agent/src/modes/interactive/components/extension-editor.ts` | gelesen | `crates/notagent/PARITY.md:113` | verzeichnis |
| `packages/coding-agent/src/modes/interactive/components/extension-input.ts` | gelesen | `crates/notagent/PARITY.md:113` | verzeichnis |
| `packages/coding-agent/src/modes/interactive/components/index.ts` | portiert | `crates/notagent/PARITY.md:587` | datei |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | gelesen | `crates/notagent/PARITY.md:194` | datei |
| `packages/coding-agent/src/modes/interactive/model-search.ts` | portiert | `crates/notagent/PARITY.md:425` | datei |
| `packages/coding-agent/src/modes/interactive/theme/theme-controller.ts` | portiert | `crates/notagent/PARITY.md:568` | datei |
| `packages/coding-agent/src/modes/interactive/theme/theme-schema.json` | gelesen | `crates/notagent/PARITY.md:68` | datei |
| `packages/coding-agent/src/modes/rpc/rpc-mode.ts` | portiert | `crates/notagent/PARITY.md:537` | datei |
| `packages/coding-agent/src/modes/rpc/rpc-types.ts` | portiert | `crates/notagent/PARITY.md:536` | datei |
| `packages/coding-agent/src/package-manager-cli.ts` | portiert | `crates/notagent/PARITY.md:797` | datei |
| `packages/coding-agent/src/rpc-entry.ts` | portiert | `crates/notagent/PARITY.md:514` | datei |
| `packages/coding-agent/src/utils/abort.ts` | portiert | `crates/notagent/PARITY.md:368` | datei |
| `packages/coding-agent/src/utils/ansi.ts` | portiert | `crates/notagent/PARITY.md:367` | datei |
| `packages/coding-agent/src/utils/child-process.ts` | portiert | `crates/notagent/PARITY.md:400` | datei |
| `packages/coding-agent/src/utils/frontmatter.ts` | portiert | `crates/notagent/PARITY.md:431` | datei |
| `packages/coding-agent/src/utils/html.ts` | portiert | `crates/notagent/PARITY.md:422` | datei |
| `packages/coding-agent/src/utils/image-convert.ts` | portiert | `crates/notagent/PARITY.md:391` | datei |
| `packages/coding-agent/src/utils/image-process.ts` | portiert | `crates/notagent/PARITY.md:391` | datei |
| `packages/coding-agent/src/utils/image-resize-core.ts` | portiert | `crates/notagent/PARITY.md:391` | datei |
| `packages/coding-agent/src/utils/image-resize.ts` | portiert | `crates/notagent/PARITY.md:391` | datei |
| `packages/coding-agent/src/utils/management-http.ts` | portiert | `crates/notagent/PARITY.md:385` | datei |
| `packages/coding-agent/src/utils/mime.ts` | portiert | `crates/notagent/PARITY.md:390` | datei |
| `packages/coding-agent/src/utils/paths.ts` | portiert | `crates/notagent/PARITY.md:365` | datei |
| `packages/coding-agent/src/utils/shell.ts` | portiert | `crates/notagent/PARITY.md:364` | datei |
| `packages/coding-agent/src/utils/syntax-highlight.ts` | portiert | `crates/notagent/PARITY.md:421` | datei |
| `packages/coding-agent/src/utils/tools-manager.ts` | portiert | `crates/notagent/PARITY.md:384` | datei |
| `packages/server/src/testing/client.ts` | Tests portiert | `crates/notagent-server/PARITY.md:58` | datei |
| `packages/server/src/testing/index.ts` | Tests portiert | `crates/notagent-server/PARITY.md:60` | datei |
| `packages/server/src/testing/server.ts` | Tests portiert | `crates/notagent-server/PARITY.md:59` | datei |
| `packages/server/src/testing/service.ts` | Tests portiert | `crates/notagent-server/PARITY.md:57` | datei |
| `packages/session-backends/sqlite-node/src/index.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:37` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/branch-cache.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:54` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/index.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:38` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/repo.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:56` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/search-backend.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:55` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/storage/branch-entries.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:53` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/storage/branch-tips.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:50` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/storage/entries.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:48` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/storage/facts.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:49` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/storage/lanes.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:51` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/storage/records.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:52` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/storage/session-sequences.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:45` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/storage/session-stats.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:46` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/storage/sessions.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:44` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:47` | datei |
| `packages/session-backends/sqlite-node/src/sqlite/types.ts` | portiert | `crates/notagent-session-sqlite/PARITY.md:39` | datei |
| `packages/telemetry/src/testing/index.ts` | portiert | `crates/notagent-telemetry/PARITY.md:30` | datei |
| `packages/telemetry/src/testing/types.ts` | portiert | `crates/notagent-telemetry/PARITY.md:29` | datei |
| `packages/tui/native/darwin/src/darwin-modifiers.c` | portiert | `crates/notagent-tui/PARITY.md:231` | datei |
| `packages/tui/native/win32/src/win32-console-mode.c` | portiert | `crates/notagent-tui/PARITY.md:232` | datei |
| `packages/tui/src/autocomplete.ts` | portiert | `crates/notagent-tui/PARITY.md:234` | datei |
| `packages/tui/src/components/alt-screen-flash.ts` | portiert | `crates/notagent-tui/PARITY.md:218` | datei |
| `packages/tui/src/components/box.ts` | portiert | `crates/notagent-tui/PARITY.md:217` | datei |
| `packages/tui/src/components/cancellable-loader.ts` | portiert | `crates/notagent-tui/PARITY.md:224` | datei |
| `packages/tui/src/components/editor.ts` | portiert | `crates/notagent-tui/PARITY.md:236` | datei |
| `packages/tui/src/components/image.ts` | portiert | `crates/notagent-tui/PARITY.md:221` | datei |
| `packages/tui/src/components/loader.ts` | portiert | `crates/notagent-tui/PARITY.md:219` | datei |
| `packages/tui/src/components/markdown.ts` | portiert | `crates/notagent-tui/PARITY.md:239` | datei |
| `packages/tui/src/components/spacer.ts` | portiert | `crates/notagent-tui/PARITY.md:215` | datei |
| `packages/tui/src/components/truncated-text.ts` | portiert | `crates/notagent-tui/PARITY.md:216` | datei |
| `packages/tui/src/editor-component.ts` | portiert | `crates/notagent-tui/PARITY.md:237` | datei |
| `packages/tui/src/index.ts` | portiert | `crates/notagent-tui/PARITY.md:195` | datei |
| `packages/tui/src/kill-ring.ts` | portiert | `crates/notagent-tui/PARITY.md:210` | datei |
| `packages/tui/src/latex.ts` | portiert | `crates/notagent-tui/PARITY.md:238` | datei |
| `packages/tui/src/native-modifiers.ts` | portiert | `crates/notagent-tui/PARITY.md:230` | datei |
| `packages/tui/src/terminal-image.ts` | portiert | `crates/notagent-tui/PARITY.md:228` | datei |
| `packages/tui/src/tui-alt-screen.ts` | portiert | `crates/notagent-tui/PARITY.md:194` | datei |
| `packages/tui/src/undo-stack.ts` | portiert | `crates/notagent-tui/PARITY.md:211` | datei |

## Lücke 3 — Ledger-Zeilen mit Status außerhalb der Leiter

`CONVENTIONS.md` §7 kennt genau vier Status-Werte (`gelesen`, `portiert`, `Tests portiert`,
`verifiziert`). Diese Zeilen decken src-Dateien ab, schreiben aber etwas anderes in die
Statusspalte; die Prüfung stuft sie höchstens als `portiert` ein. Formalbefund, kein Portmangel.

| Ledger | Zeile | Statustext | eingestuft als | Dateien |
|---|---:|---|---|---:|
| `crates/notagent/PARITY.md` | 386 | offen | unklar | 1 |
| `crates/notagent/PARITY.md` | 387 | offen | unklar | 1 |
| `crates/notagent/PARITY.md` | 416 | offen | unklar | 1 |
| `crates/notagent/PARITY.md` | 440 | nativ ersetzt | unklar | 1 |
| `crates/notagent/PARITY.md` | 446 | nativ ersetzt | unklar | 1 |
| `crates/notagent/PARITY.md` | 526 | offen (Task 13) | unklar | 1 |
| `crates/notagent/PARITY.md` | 527 | teilweise entfallen, Rest Task 13 | portiert | 1 |
| `crates/notagent/PARITY.md` | 572 | übernommen | unklar | 1 |
| `crates/notagent/PARITY.md` | 587 | teilweise portiert | portiert | 1 |
| `crates/notagent/PARITY.md` | 648 | teilweise portiert | portiert | 1 |
| `crates/notagent/PARITY.md` | 844 | teilportiert | unklar | 1 |
| `crates/notagent/PARITY.md` | 845 | übernommen | unklar | 3 |
| `crates/notagent/PARITY.md` | 851 | nachgezogen | unklar | 1 |
| `crates/notagent/PARITY.md` | 892 | offen (C) | unklar | 1 |
| `crates/notagent/PARITY.md` | 893 | offen (C) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 422 | übernommen (Task 10) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 716 | (leer) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 717 | (leer) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 718 | (leer) | unklar | 1 |
| `crates/notagent-ai/PARITY.md` | 719 | (leer) | unklar | 1 |
| `crates/notagent-tui/PARITY.md` | 194 | vollständig: Renderer-Kern (Enter/Exit-Sequenzen inkl. Maus- | portiert | 1 |
| `crates/notagent-tui/PARITY.md` | 195 | vollständig: das Export-Set deckt alle 38 portierten Module  | portiert | 1 |
| `crates/notagent-tui/PARITY.md` | 196 | vollständig portiert (Task 5) | portiert | 1 |
| `crates/notagent-tui/PARITY.md` | 197 | vollständig portiert (Task 4) | portiert | 1 |
| `crates/notagent-tui/PARITY.md` | 202 | vollständig: Renderer-Kern (Enter/Exit-Sequenzen inkl. Maus- | portiert | 1 |
| `crates/notagent-tui/PARITY.md` | 203 | vollständig: das Export-Set deckt alle 38 portierten Module  | portiert | 1 |
| `crates/notagent-tui/PARITY.md` | 234 | vollständig portiert (Slash-Commands, Datei-Pfad-Vervollstän | portiert | 1 |
| `crates/notagent-tui/PARITY.md` | 236 | vollständig portiert (wordWrapLine mit TextChunk-Mapping, Vi | portiert | 1 |
| `crates/notagent-tui/PARITY.md` | 237 | vollständig portiert | portiert | 1 |
| `crates/notagent-tui/PARITY.md` | 238 | vollständig portiert (Symboltabellen, Skript-/Bruch-/Wurzel- | portiert | 1 |
| `crates/notagent-tui/PARITY.md` | 239 | vollständig portiert (Token-Rendering, Listen mit Fortsetzun | portiert | 1 |

## Abdeckung über Sammelzeilen

Diese Dateien sind nicht einzeln geführt, sondern über eine Verzeichnis- oder Musterzeile.
Das ist zulässig, aber die schwächste Form des Nachweises — an G4 einmal gegenlesen, ob die
Zeile wirklich jede Datei darunter meint (einschränkende Prosa wie „außer X" wertet die
Prüfung nicht aus).

| Beleg | Angabe | Status | Dateien |
|---|---|---|---:|
| `crates/notagent-ai/PARITY.md:262` | `src/providers/` | verifiziert | 86 |
| `crates/notagent-ai/PARITY.md:260` | `src/providers/data/` | verifiziert | 40 |
| `crates/notagent-agent/PARITY.md:51` | `src/harness/**` | ausgeschlossen | 39 |
| `crates/notagent-ai/PARITY.md:273` | `src/api/*.lazy.ts` | verifiziert | 11 |
| `crates/notagent/PARITY.md:902` | `src/cli/experimental/` | ausgeschlossen | 8 |
| `crates/notagent/PARITY.md:899` | `src/core/extensions/` | ausgeschlossen | 5 |
| `crates/notagent/PARITY.md:902` | `src/bun/` | ausgeschlossen | 3 |
| `crates/notagent/PARITY.md:899` | `src/extensions/` | ausgeschlossen | 3 |
| `crates/notagent/PARITY.md:430` | `src/core/modes/builtin/auto/10-*.md` | verifiziert | 1 |
| `crates/notagent/PARITY.md:430` | `src/core/modes/builtin/manual/10-*.md` | verifiziert | 1 |
| `crates/notagent/PARITY.md:430` | `src/core/modes/builtin/plan/10-*.md` | verifiziert | 1 |
| `crates/notagent/PARITY.md:430` | `src/core/modes/builtin/yolo/10-*.md` | verifiziert | 1 |

## Rahmen-Ausschlüsse aus dem Master-Plan

| Pfad | betroffene src-Dateien | Beleg |
|---|---:|---|
| `packages/evals` | 8 | `plans/2026-08-13-rust-port-master-v1.md:31` |

## Unbekannte Ledger-Pfade

Pfadangaben, die im TS-Repo nicht (mehr) existieren — Tippfehler oder veraltete Zeilen:

| Ledger | Zeile | Angabe | Abschnitt |
|---|---:|---|---|
| `crates/notagent/PARITY.md` | 63 | `packages/coding-agent/test/session-info-modified-timestamp.ts` | Lektüre-Protokoll |

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
