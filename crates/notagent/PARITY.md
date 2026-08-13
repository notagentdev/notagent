# Parity-Ledger: notagent

TS-Quelle: `/Users/dev/projects/notagent-main/packages/coding-agent` (68 856 LOC in src/) — Workstream C (TUI-Teile mit A).

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

**Stand: Task 8 läuft.** `config.ts`, `core/settings-manager.ts` (inkl. aller
typisierten Zugriffsmethoden), `migrations.ts`, `core/resolve-config-value.ts`,
`core/auth-storage.ts` und die Utilities (`utils/paths.ts`, `utils/shell.ts`,
`utils/abort.ts` + Lockfile-Ersatz) sind portiert und testbelegt. Offen bleibt nur, was
laut Plan zu späteren Tasks gehört: `utils/shell.ts` liefert erst mit Task 8 die
Bash-Ausführung nach, die keybindings.json-Migration folgt mit Task 13. Task 6
(`core/session-manager.ts`) ist portiert und mit den TS-Fixtures roundtrip-getestet;
offen bleibt daraus `resolveSessionPath`, das in `src/main.ts` sitzt und zu Task 12 gehört.
Task 7 ist bis auf zwei Dateien portiert: `tools/render-utils.ts` braucht das Theme aus
A-Batch 0 (Interface-Request C-5) und `tools/index.ts` ist die Registry über alle 16 Tools,
also erst nach den Tasks 8-10 vollständig baubar. Beide sind unten als offen geführt.

## Lektüre-Protokoll

| Datum | TS-/Referenz-Datei | LOC | gelesen von (Task) |
|---|---|---|---|
| 2026-08-13 | packages/coding-agent/src/config.ts | 576 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/src/core/settings-manager.ts (Zeilen 1-700) | 700 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/test/settings-manager.test.ts (Kopf) | 120 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/src/migrations.ts | 314 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/src/core/resolve-config-value.ts | 287 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/src/utils/shell.ts | 259 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/src/utils/paths.ts | 137 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/src/utils/abort.ts | 46 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/test/paths.test.ts | 184 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/src/core/auth-storage.ts | 507 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/test/auth-storage.test.ts | 535 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/src/core/settings-manager.ts (Zeilen 660-1273) | 613 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/test/settings-manager.test.ts | 587 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/test/config.test.ts | 446 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/src/core/http-dispatcher.ts (parseHttpIdleTimeoutMs) | 40 | C-Task 5 |
| 2026-08-13 | packages/coding-agent/src/core/session-manager.ts | 1 714 | C-Task 6 |
| 2026-08-13 | packages/coding-agent/src/core/messages.ts | 195 | C-Task 6 |
| 2026-08-13 | packages/coding-agent/test/session-manager/*.ts (7 Dateien) | 1 791 | C-Task 6 |
| 2026-08-13 | packages/coding-agent/test/session-info-modified-timestamp.ts | 83 | C-Task 6 |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/theme/theme.ts | 1 335 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/theme/theme-controller.ts | 139 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/theme/dark.json | 90 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/theme/light.json | 89 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/theme/theme-schema.json | 352 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/test/theme-detection.test.ts | 174 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/test/theme-export.test.ts | 104 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/test/theme-picker.test.ts | 51 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/test/scrollbar-theme.test.ts | 70 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/test/test-theme-colors.ts | 249 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/src/utils/syntax-highlight.ts (Abhängigkeit von theme.ts) | 146 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/src/utils/fs-watch.ts (Abhängigkeit von theme.ts) | 30 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/src/core/source-info.ts (Abhängigkeit von theme.ts) | 40 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/src/core/tools/render-utils.ts (Konsument von Theme) | 85 | A-Task 15 (Batch 0) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/index.ts | 38 | A-Task 15 (Batch 1) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/custom-editor.ts | 96 | A-Task 15 (Batch 1) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/status-indicator.ts | 114 | A-Task 15 (Batch 1) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/bordered-loader.ts | 68 | A-Task 15 (Batch 1) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/keybinding-hints.ts | 48 | A-Task 15 (Batch 1) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/visual-truncate.ts | 50 | A-Task 15 (Batch 1) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/countdown-timer.ts | 39 | A-Task 15 (Batch 1) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/dynamic-border.ts | 25 | A-Task 15 (Batch 1) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/markdown-transform.ts | 29 | A-Task 15 (Batch 1) |
| 2026-08-13 | packages/coding-agent/test/status-indicator.test.ts | 32 | A-Task 15 (Batch 1) |
| 2026-08-13 | packages/coding-agent/src/core/keybindings.ts (Abhängigkeit von custom-editor.ts) | 386 | A-Task 15 (Batch 1) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/diff.ts | 147 | A-Task 15 (Batch 2) |
| 2026-08-13 | node_modules/diff/libesm/diff/base.js (jsdiff 8.0.4) | 253 | A-Task 15 (Batch 2) |
| 2026-08-13 | node_modules/diff/libesm/diff/word.js (jsdiff 8.0.4) | 281 | A-Task 15 (Batch 2) |
| 2026-08-13 | node_modules/diff/libesm/util/string.js (jsdiff 8.0.4) | 184 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/user-message.ts | 70 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/assistant-message.ts | 197 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/compaction-summary-message.ts | 59 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/branch-summary-message.ts | 58 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/skill-invocation-message.ts | 55 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/custom-message.ts | 113 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/custom-entry.ts | 62 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/bash-execution.ts | 220 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/footer.ts | 253 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/tool-execution.ts | 377 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/todo-list.ts | 216 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/mermaid.ts | 89 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/test/assistant-message.test.ts | 241 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/test/custom-message.test.ts | 44 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/test/bash-execution-width.test.ts | 80 | A-Task 15 (Batch 2) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/extension-selector.ts | 112 | A-Task 15 (Batch 3) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/thinking-selector.ts | 75 | A-Task 15 (Batch 3) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/theme-selector.ts | 67 | A-Task 15 (Batch 3) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/show-images-selector.ts | 50 | A-Task 15 (Batch 3) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/user-message-selector.ts | 155 | A-Task 15 (Batch 3) |
| 2026-08-13 | packages/coding-agent/src/modes/interactive/components/ (Importköpfe der übrigen 12 Selektoren) | — | A-Task 15 (Batch 3, Abhängigkeitsprüfung) |
| 2026-08-13 | packages/coding-agent/src/core/tools/bash.ts | 771 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/utils/shell.ts (erneut, Spawn-/Signalteil) | 259 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/utils/child-process.ts | 137 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/core/bash-executor.ts | 156 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/core/tasks/types.ts (Schnittstelle für den Managed-Pfad) | 139 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/core/tasks/shell-task.ts (dito) | 101 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/test/tools.test.ts (Abschnitt „bash tool") | 1 213 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/test/bash-background.test.ts | 133 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/test/bash-close-hang-windows.test.ts | 126 | C-Task 8 |
| 2026-08-13 | notagent-main-rust/crates/notagent_services/src/tool_services/minify.rs (Referenz) | 897 | C-Task 8 |
| 2026-08-13 | notagent-main-rust/crates/notagent_services/src/tool_services/minify_edit.rs (Referenz) | 913 | C-Task 8 |
| 2026-08-13 | notagent-main-rust/crates/notagent_services/src/tool_services/minify_multi_edit_tests.rs (Referenz) | 244 | C-Task 8 |
| 2026-08-13 | notagent-main-rust/minified-edit-testbench/ (PROMPT.md, RESULTS.md, edge_cases.rs) | — | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/core/mini-read/index.ts | 137 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/core/mini-read/languages.ts | 100 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/core/mini-read/parser.ts | 121 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/core/mini-read/minify.ts | 582 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/core/mini-read/minify-edit.ts | 493 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/test/mini-read-minify.test.ts | 269 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/test/mini-read-minify-edit.test.ts | 413 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/core/tools/read-minified.ts | 210 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/src/core/tools/patch-minified.ts | 367 | C-Task 8 |
| 2026-08-13 | packages/coding-agent/test/patch-minified-tool.test.ts | 183 | C-Task 8 |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| src/config.ts | 576 | src/config.rs | portiert (vollständig) | Klasse 4: kein `package.json`-Lesen zur Laufzeit — Identität kommt aus Cargo; `bun-binary` → `InstallMethod::Binary` (Single-Binary, Update über die Releases-Seite). Klasse 1: die Erkennung liest eine explizite `InstallEnv` statt `process.execPath`/`argv`, damit sie ohne globalen Prozesszustand testbar ist. Asset-Pfade zeigen auf das Verzeichnis der Binary (die Assets sind eingebettet). Klasse 1: `detectInstallMethod` prüft die Pfad-Evidenz VOR dem Standalone-Flag — anders als `isBunBinary` ist „standalone" bei einer Rust-Binary keine Distributionsaussage, sondern der Rückfall; Wurf → `Result` bei den Self-Update-Buildern |
| test/config.test.ts | 446 | tests/config.rs | Tests portiert | 15 Tests: npm-Prefix-Inferenz inkl. Windows-Ausnahme, Anführungszeichen im Display, konfiguriertes/leeres `npmCommand`, umbenannte Pakete bei npm/pnpm/yarn/bun, pnpm-v11-Store-Auflösung über den Entrypoint, Schreibrechte-Prüfung, unbekannte Wrapper, Standalone-Binary. Die Package-Manager werden wie in TS über PATH-Skripte gestellt |
| src/core/settings-manager.ts | 1 273 | src/core/settings_manager.rs | portiert | Klasse 1: `Settings` behält unbekannte Schlüssel über `#[serde(flatten)] extra`, damit ein Load/Persist-Roundtrip wie in JS nichts verliert; Schreibvorgänge sind synchron (die TS-Promise-Queue entfällt, `flush()` bleibt als No-op erhalten); die ~45 typisierten Zugriffsmethoden sind portiert, `set_global_field`/`set_project_field` bleiben als Wire-Name-Einstieg daneben bestehen. Felder, die TS beim Lesen erneut prüft (`theme`, `tuiMode`, `outputPad`, `httpIdleTimeoutMs`, …), liegen als rohes JSON in `Settings`, damit eine von der TS-App geschriebene settings.json auch dann lädt, wenn der Wert nicht dem deklarierten Typ entspricht — die Rückfalllogik sitzt wie in TS im Accessor. Klasse 3: `proper-lockfile` → exklusiv angelegtes `.lock`-Verzeichnis mit derselben Retry-Semantik (10 × 20 ms). Die `extensions`-Einstellung entfällt (Extension-System), bleibt aber als unbekannter Schlüssel erhalten |
| test/settings-manager.test.ts | 587 | tests/settings_manager.rs | Tests portiert | 27 Tests: externe Änderungen bleiben erhalten, In-Memory gewinnt bei Konflikten, Projekt-über-Global-Merge inkl. verschachtelter Objekte, Trust-Gate, Load-Fehler ohne Dateiverlust, verschachtelte Felder, Migrationen, In-Memory-Storage, httpIdleTimeoutMs (Default/Merge/ungültig/String-Werte), externalEditor-Reihenfolge, TUI-Modus, Fullscreen-Einstellungen, outputPad, markdown.mermaid, shellCommandPrefix, sessionDir/shellPath inkl. Tilde, Theme-Paare, defaultProjectTrust, Zahlen-Clamping, Tracking-ID |
| src/migrations.ts | 314 | src/migrations.rs | portiert | Klasse 2: `checkDeprecatedExtensionDirs`/`showDeprecationWarnings` entfallen — sie verwiesen ausschließlich auf das Extension-System (extension-boundary §6); `commands/` → `prompts/` bleibt. Die keybindings.json-Migration folgt mit `core/keybindings.rs` (Task 13). Klasse 1: verzeichnis-parametrisierte Varianten für Tests |
| — (neu) | — | tests/migrations.rs | verifiziert | 5 Tests: auth.json-Migration inkl. apiKeys-Entfernung und 0600-Rechten, Überspringen bei vorhandener auth.json, Session-Umzug mit cwd-Kodierung, vorhandene Zieldatei bleibt |
| src/core/resolve-config-value.ts | 287 | src/core/resolve_config_value.rs | portiert | Klasse 1: `parseConfigValueTemplate` arbeitet auf Byte-Indizes statt auf JS-Regex-Gruppen (gleiche Fälle: `$VAR`, `${VAR}`, `$$`→`$`, `$!`→`!`, ungültige Namen bleiben literal); der Prozess-Cache ist ein `LazyLock<Mutex<HashMap>>`. Klasse 3: `execSync(timeout: 10_000)` → `spawn` + Deadline-Poll mit `kill`; die Windows-Sonderroute (`executeWithConfiguredShell`) ist `#[cfg(any(windows, test))]`, damit sie auch auf Unix getestet wird |
| src/utils/shell.ts | 259 | src/utils/shell.rs | portiert (genutzte Teile) | `getShellConfig`/`getShellEnv`/`sanitizeBinaryOutput`/Prozessbaum-Signale portiert. Klasse 1: Lone-Surrogate-Filter entfällt (Rust-`str` ist immer gültiges UTF-8); `process.kill(-pid)` → `libc::kill(-pid)` mit Fallback auf den Einzelprozess. Der Spawn-Teil sitzt wie in TS im Bash-Tool. Offen: die Registrierung von `killTrackedDetachedChildren` an den Prozess-Exit-Signalen, die in `src/main.ts` liegt (Task 12) |
| src/utils/paths.ts | 137 | src/utils/paths.rs | portiert | Klasse 1: `normalizePath` gibt `Result` zurück, weil `fileURLToPath` wirft; `path.resolve`/`path.relative`/`fileURLToPath`/`pathToFileURL` sind als Node-Semantik nachgebaut (Rusts `Path` normalisiert Punkt-Segmente nicht) |
| test/paths.test.ts | 184 | src/utils/paths.rs (Testmodul) | Tests portiert | 13 Tests: canonicalize inkl. Symlinks/danglings, cwd-relative Pfade, Tilde-Regeln, file:-URLs inkl. Fehlerfällen, Windows-Shell-Pfade, isLocalPath |
| src/utils/ansi.ts | 60 | src/utils/ansi.rs | portiert | Dieselbe Grammatik als `regex`-Literal; der MIT-Hinweis der abgeleiteten Pakete steht im Modulkopf |
| src/utils/abort.ts | 46 | src/utils/abort.rs | portiert | Klasse 3: `AbortSignal` → `CancellationToken`; `raceWithAbortSignal` verlangt, dass abgebrochene Arbeit vom Aufrufer am Leben gehalten wird (Task/Shared-Future), weil ein fallengelassenes Rust-Future abbricht |
| — (neu) | — | src/utils/lockfile.rs | neu (Tech-Substitution) | Klasse 3: `proper-lockfile` → `<datei>.lock`-Verzeichnis mit identischer Semantik: atomares `mkdir`, `ELOCKED`, Stale-Übernahme, Heartbeat-Thread der die mtime auffrischt, `onCompromised` |
| src/core/auth-storage.ts | 507 | src/core/auth_storage.rs | portiert | Klasse 1: der Backend-Callback gibt nur `next` zurück (das `result`-Feld entfällt, weil ein Rust-Closure in seinen Aufrufer schreiben kann) — dadurch bleibt der Trait objektsicher; die Daten bleiben als `serde_json::Map` liegen, damit unbekannte Einträge verlustfrei erhalten bleiben (TS validiert in `AuthStorage` ebenfalls nicht), und werden erst beim Lesen in `Credential` überführt; eine abgebrochene In-Memory-Mutation wird beim Verwerfen des Futures gestoppt statt im Hintergrund weiterzulaufen (sie kann in beiden Fällen nicht mehr schreiben). Klasse 3: `proper-lockfile` → `src/utils/lockfile.rs`; `AbortSignal` → `CancellationToken`; der koaleszierte Reload läuft als `tokio::spawn` + `Shared`, damit ein abbrechender Leser die übrigen nicht mitreißt |
| test/auth-storage.test.ts | 535 | tests/auth_storage.rs | Tests portiert | 26 Tests. Statt `vi.spyOn(lockfile, …)` werden echte Locks gehalten bzw. ein zählendes Backend benutzt; der Fall „releases a file lock acquired concurrently with cancellation" ist ohne Mock nicht deterministisch auslösbar und geht in „aborts while waiting for a held file lock" auf (dort wird zusätzlich geprüft, dass kein Lock zurückbleibt). Die zwei OAuth-Fälle liefen bis zur Umsetzung von Interface-Request C-4 unter `#[ignore]`; seit der Umsetzung durch B (`#[serde(rename = "oauth")]`) sind die Marker entfernt und alle 26 Tests grün |

| src/core/session-manager.ts | 1 714 | src/core/session_manager.rs | portiert | Klasse 1: Einträge werden als `SessionEntry`-Enum mit Default-Feldern und `#[serde(flatten)] extra` gelesen, unbekannte `type`-Werte landen in `SessionEntry::Unknown` — das hält die TS-Eigenschaft „Sessions werden ohne Validierung gelesen"; die `message`-Nutzlast bleibt rohes JSON und wird erst beim Kontextaufbau in `AgentMessage` überführt (eine nicht lesbare Nachricht fällt aus dem Kontext, TS reicht sie ungeprüft weiter). Werfende Methoden geben `Result` zurück; `leafId` als `LeafSelector` unterscheidet TS' `undefined` (letzter Eintrag) von `null` (leerer Pfad). Klasse 3: `readline`/`createReadStream` → tokio-`BufReader`; die 10 parallelen Info-Ladevorgänge laufen über `futures::stream::buffered`. `resolveSessionPath` liegt in `main.ts` und folgt mit Task 12 |
| src/core/messages.ts | 195 | src/core/messages.rs | portiert (Re-Export) | Die Datei ist inhaltsgleich mit `packages/agent/src/harness/messages.ts`, und TS führt beide Deklarationen über Declaration Merging zusammen. Rust kennt das nicht: die vier Custom-Rollen liegen einmal in `notagent-agent` und werden hier unter den Namen der Coding-Agent-Datei re-exportiert |
| test/session-manager/*.ts | 1 791 | tests/session_manager.rs | Tests portiert | 73 Tests: Append-/Leaf-Verhalten, Baum mit Branches und Waisen, Labels inkl. Fork-Neuverkettung, Kontextaufbau mit Compaction und Branch-Summaries, Datei-Operationen (Header-Scan-Limit, Migration, leere/ungültige Dateien, findMostRecentSession, list/listAll), eigene Session-IDs, SessionInfo-Zeitstempel. Zusätzlich zwei Roundtrip-Tests gegen die TS-Fixtures `before-compaction.jsonl` und `large-session.jsonl` (2 022 Zeilen): v1→v3-Migration ohne Feldverlust und byte-genauer v3-Roundtrip |
| src/core/tools/truncate.ts | 276 | src/core/tools/truncate.rs | portiert | Klasse 1: `truncateLine` schneidet auf einer Zeichengrenze — JS zählt UTF-16-Einheiten und kann ein Surrogatpaar zerteilen, Rust behält dann ein Zeichen weniger |
| src/core/tools/path-utils.ts | 118 | src/core/tools/path_utils.rs | portiert | Klasse 1: die sync- und async-Variante von `resolveReadPath` fallen zu einer Funktion zusammen (die Prüfungen sind reine `stat`-Aufrufe) |
| src/core/extensions/types.ts (`ToolDefinition`) + src/core/tools/tool-definition-wrapper.ts | 59 + Auszug | src/core/tools/tool_definition.rs | portiert | Klasse 2: `ExtensionContext` → `ToolContext` mit genau den Feldern, die Built-ins lesen (Session-ID/-Datei, Thinking-Level, Modell). Klasse 1: `description`/`parameters` sind Borrow-Getter statt JS-Gettern; `renderCall`/`renderResult` gehören zur TUI-Schicht und folgen mit Task 13 |
| src/core/tools/read.ts | 358 | src/core/tools/read.rs | portiert (Tool-Hälfte) | Bild-Pipeline, offset/limit, Fortsetzungs-Notizen und alle Fehlertexte portiert. **Offen:** `renderCall`/`renderResult` inkl. Kompakt-Klassifikation (docs/resource/skill) — Task 13 |
| src/core/tools/write.ts | 274 | src/core/tools/write.rs | portiert (Tool-Hälfte) | bug-compat: die Byte-Angabe der Erfolgsmeldung ist wie in JS die UTF-16-Länge. **Offen:** Renderer inkl. Syntax-Highlight-Cache — Task 13 |
| src/core/tools/output-accumulator.ts | 222 | src/core/tools/output_accumulator.rs | portiert | Klasse 1: `TextDecoder({stream:true})` ist als eigener Streaming-UTF-8-Decoder nachgebaut (unvollständige Zeichen warten auf den nächsten Chunk, ungültige Bytes werden zu U+FFFD); der Schreib-Stream ist eine `std::fs::File` |
| src/core/tools/find.ts | 380 | src/core/tools/find.rs | portiert (Tool-Hälfte) | fd-Aufruf inkl. Git-Repo-Erkennung, `--full-path`-Regel für Pfad-Muster, Ergebnis-Relativierung und Limit-Hinweise. **Offen:** Renderer — Task 13 |
| src/core/tools/grep.ts | 390 | src/core/tools/grep.rs | portiert (Tool-Hälfte) | rg `--json`-Streaming, Kill beim Match-Limit, Kontext-Blöcke mit Datei-Cache, Zeilenkürzung auf 500 Zeichen und alle Hinweise. **Offen:** Renderer — Task 13 |
| src/utils/tools-manager.ts | 371 | src/utils/tools_manager.rs | portiert | Klasse 3: `fetch` → `reqwest`; Entpacken weiterhin über `tar`/`unzip`/PowerShell wie in TS |
| src/utils/management-http.ts | 68 | src/utils/management_http.rs | portiert | Klasse 3: `AbortSignal.timeout` → Deadline pro Versuch; die Anfrage wird je Versuch neu gebaut |
| src/core/tools/render-utils.ts | 85 | — | offen | braucht `Theme` (A-Batch 0 aus Interface-Request C-5) und die TUI-Capabilities — Task 13 |
| src/core/tools/index.ts | 380 | — | offen | Registry über alle 16 Tools; vollständig baubar erst nach den Tasks 8-10 |
| src/core/tools/ls.ts | 230 | src/core/tools/ls.rs | portiert (Tool-Hälfte) | Klasse 1: Sortierung über `to_lowercase()`-Ordnung statt `localeCompare` (unterschiedlich nur bei akzentabhängiger Locale-Sortierung). **Offen:** Renderer — Task 13 |
| src/core/experimental.ts | 9 | src/core/experimental.rs | portiert | nur die beiden von den Tools genutzten Funktionen; der Rest der Datei existiert nicht |
| src/utils/mime.ts | 116 | src/utils/mime.rs | portiert | vollständig, inkl. APNG-/BMP-Plausibilitätsprüfungen |
| src/utils/image-process.ts + image-convert.ts + image-resize-core.ts + image-resize.ts | 411 | src/utils/image.rs | portiert | Klasse 3: Photon/WASM + Worker-Thread → `image`-Crate im Prozess; EXIF-Orientierung kommt vom Decoder statt aus `exif-orientation.ts`. Resize-Strategie (2000×2000, PNG/JPEG-Kandidaten, Qualitätsstufen, 25-%-Schrumpfen bis 1×1) identisch |
| src/core/tools/edit.ts | 443 | src/core/tools/edit.rs | portiert (Tool-Hälfte) | inkl. `prepareArguments` (Legacy-oldText/newText, `edits` als JSON-String), BOM- und CRLF-Erhalt, Diff + Unified-Patch in den Details. **Offen:** Renderer mit Live-Preview — Task 13 |
| src/core/tools/edit-diff.ts | 560 | src/core/tools/edit_diff.rs | portiert | Portiert ist die Text-Hälfte: Zeilenenden-Erkennung, LF-Normalisierung, Fuzzy-Normalisierung (NFKC, Trailing-Whitespace, Quotes/Dashes/Spaces), BOM, `fuzzyFindText`, `applyReplacementsPreservingUnchangedLines`, `applyEditsToNormalizedContent` mit allen Fehlertexten. dazu `generateDiffString` und `generateUnifiedPatch`. Klasse 3: das `diff`-npm-Paket → `similar`-Crate (beide Zeilen-LCS). Klasse 1: Offsets sind Byte- statt UTF-16-Indizes (modulintern konsistent). **Offen:** `computeEditsDiff` (Preview für die TUI) — Task 13 |
| src/core/tools/file-mutation-queue.ts | 61 | src/core/tools/file_mutation_queue.rs | portiert | Klasse 1: der Schlüssel wird synchron aufgelöst, dadurch entfällt die `registrationQueue`, die in TS nur die Reihenfolge des asynchronen `realpath` sichert; die Serialisierung selbst ist eine faire `tokio::sync::Mutex` pro Datei |
| test/path-utils.test.ts + test/file-mutation-queue.test.ts (Kernfälle) | 448 | (Testmodule) | Tests portiert | 20 Tests. Der NFC/NFD-Fallback ist auf APFS nicht über das Dateisystem beobachtbar (normalisierungsunempfindlicher Vergleich) und wird deshalb auf der Varianten-Funktion geprüft; die vier Abbruch-Fälle aus dem TS-Test hängen an write/edit und folgen mit diesen |
| test/session-file-invalid.test.ts | 65 | — | offen | CLI-E2E (spawnt die Binary) — folgt mit Task 12 |
| test/session-cwd.test.ts | 91 | — | offen | braucht `core/session-cwd.ts` und die Runtime — folgt mit Task 11 |
| test/session-id-readonly.test.ts | 190 | — | offen | CLI-E2E — folgt mit Task 12 |
| src/core/tools/bash.ts | 771 | src/core/tools/bash.rs | portiert (Tool-Hälfte) | Klasse 1: `BashToolSources` reicht einen [`BashTaskManager`]-Trait statt des konkreten `TaskManager` durch — TS erreicht dieselbe Indirektion über die `sources`-Closures, in Rust erlaubt sie zusätzlich, das Tool vor der Task-Maschinerie (Task 10) zu bauen und zu testen. Klasse 1: `ops.exec` lehnt mit einem `BashExecError`-Enum ab statt mit `Error`-Nachrichten (`aborted`, `timeout:<s>`); ein fehlgeschlagener Spawn meldet `spawn <shell> ENOENT` wie Node. Klasse 3: Node-Timer → tokio-Deadlines im selben `select!`, `AbortSignal` → `CancellationToken`, `detached: true` → `process_group(0)`. **Offen:** `renderCall`/`renderResult` inkl. Preview-Zeilen und Dauer-Anzeige — Task 13; der Managed-Pfad ist implementiert, seine Verdrahtung an den echten Manager folgt mit Task 10 |
| src/utils/child-process.ts | 137 | src/core/tools/bash.rs (Lese-Schleife) | portiert (genutzter Teil) | `waitForChildProcess` ist als Zustandsautomat in der Exec-Schleife nachgebaut: nach `exit` wird auf das Leerlaufen der Pipes gewartet, der 100-ms-Grace-Timer bei jedem weiteren Chunk neu gestellt (notagentdev/notagent#5303). `spawnProcess`/`spawnProcessSync` (cross-spawn nur auf Windows) entfallen — `std::process::Command` braucht die Shim nicht |
| src/core/bash-executor.ts | 156 | src/core/bash_executor.rs | portiert | Klasse 1: der Rolling-Buffer (2 × 50 KiB) zählt Zeichen statt UTF-16-Einheiten; er ist eine Speichergrenze, kein beobachtbarer Wert |
| test/tools.test.ts (Abschnitt „bash tool") + test/bash-background.test.ts (Gate-Fälle) | 1 213 + 133 | tests/bash_tool.rs | Tests portiert | 27 Tests. Statt `vi.spyOn(shellModule, "getShellConfig")` gibt es zwei Seams: `bash::testing::local_bash_operations_with_shell_config` für den stdin-Transport (geprüft gegen `cat`) und ein Skript mit fehlendem Interpreter für den ENOENT-Spawnfehler. Zusätzlich ein Abbruch-Test gegen einen echten Prozess (TS skriptet nur die Ablehnung). **Offen:** die manager-gestützten Fälle von bash-background (Foreground-Release, Auto-Backgrounding gegen den echten Manager, Task-Log) — Task 10 |
| test/bash-close-hang-windows.test.ts | 126 | — | entfällt (dokumentiert) | `describe.skipIf(process.platform !== "win32")` — die Testumgebung ist macOS; die Eigenschaft, die er prüft (Auflösen trotz offener geerbter Handles), deckt die portierte Idle-Grace-Logik plattformunabhängig ab |
| src/core/mini-read/minify.ts | 582 | src/core/mini_read/minify.rs | portiert (aus der Rust-Referenz) | Nutzer-Vorgabe (`plans/facts/rust-minify-reference.md`): der Port übernimmt die native Referenz `minify.rs` statt die WASM-basierte TS-Datei — die TS-Datei ist selbst ein 1:1-Port davon in UTF-16-Offsets, während die Referenz in Bytes rechnet, wo ein Rust-`str` indiziert. Inhaltlich identisch (Span-Sammlung, Zeilenklassifikation, monotone Re-Indentierung, `src_map`) |
| src/core/mini-read/minify-edit.ts | 493 | src/core/mini_read/minify_edit.rs | portiert (aus der Rust-Referenz) | wie oben. Klasse 1: `anyhow::Result` → `Result<_, String>` — die Fehlertexte sind wörtlich dieselben und gehen unverändert an den Aufrufer |
| src/core/mini-read/index.ts | 137 | src/core/mini_read.rs | portiert | die pfadbasierten Einstiegspunkte sind in der Referenz Teil von `minify.rs`/`minify_edit.rs`; das Modul re-exportiert sie. Klasse 3: kein `async`, weil nichts geladen werden muss |
| src/core/mini-read/languages.ts | 100 | src/core/mini_read/minify.rs (`language_for_extension`) | portiert | identische Erweiterungstabelle (`h`→C, `jsx`→JavaScript); `LanguageKind`/`WASM_FILE_BY_KIND` entfallen mit dem WASM-Laden |
| src/core/mini-read/parser.ts | 121 | — | entfällt (Tech-Substitution) | Klasse 3: natives tree-sitter statt web-tree-sitter — kein Runtime-Init, keine wasm-Assets, kein Grammatik-Cache. Damit entfällt auch die UTF-16-Rechnung: Offsets sind Bytes |
| test/mini-read-minify.test.ts | 269 | src/core/mini_read/minify.rs (Testmodul) | Tests portiert | 26 Tests. Die TS-Datei ist ausweislich ihres Kopfes ein 1:1-Port des Referenz-Testmoduls; portiert ist deshalb das Referenzmodul selbst, mit denselben Fixtures und Erwartungswerten |
| test/mini-read-minify-edit.test.ts | 413 | src/core/mini_read/minify_edit.rs (Testmodul) + tests/mini_read_multi_edit.rs | Tests portiert | 24 + 7 Tests. Die letzten sechs TS-Fälle stammen aus `minify_multi_edit_tests.rs` und liegen deshalb in der Integrationsdatei; dort steht zusätzlich ein deterministischer Durchlauf der Referenz-Testbench (`edge_cases.rs` als Fixture, die zehn Edits aus PROMPT.md, Bewertung nach den Grading-Regeln). Dokumentiert dort: eine `///`-Doc-Kommentarzeile steht im View verbatim (tree-sitter-rust zieht den Zeilenumbruch in den Knoten), weshalb Suchtext MIT View-Einrückung überindentiert — Referenzverhalten, bug-kompatibel übernommen |
| src/core/tools/read-minified.ts | 210 | src/core/tools/read_minified.rs | portiert (Tool-Hälfte) | Schema, Beschreibung, Offset/Limit vor der Minifizierung, Rohtext-Rückfall (unbekannte Sprache, unparsbar oder leerer View) und beide Fußzeilen wörtlich. Klasse 3: `minifyForPath` ist synchron, weil keine Grammatik geladen wird. **Offen:** Renderer inkl. Syntax-Highlight — Task 13 |
| src/core/tools/patch-minified.ts | 367 | src/core/tools/patch_minified.rs | portiert (Tool-Hälfte) | Klasse 1: `patch_minified` und `multi_patch_minified` sind eine Definition mit `multi`-Flag statt zweier Fabriken über `buildRenderers` — Name, Label, Beschreibung, Schema und Prompt-Beiträge bleiben je Variante wörtlich getrennt. Sequentielles Anwenden mit Neu-Minifizierung, Atomarität, BOM-Erhalt, Mutations-Queue und der Rohtext-Rückfall sind übernommen. **Offen:** Renderer mit Diff-Anzeige — Task 13 |
| test/patch-minified-tool.test.ts | 183 | tests/minified_tools.rs | Tests portiert | 17 Tests: die 11 TS-Fälle plus sechs für `read_minified`, das im TS-Repo keine eigene Testdatei hat (Kompaktansicht, keep_comments, Offset/Limit vor der Minifizierung, Offset hinter Dateiende, Rohtext-Rückfall, Ablehnung von Bildern) |

## A: interactive components

Ownership-Übergabe durch `plans/interface-requests.md` O-1 und C-5: Workstream A
besitzt `crates/notagent/src/modes/interactive/theme/` und (ab Batch 1)
`crates/notagent/src/modes/interactive/components/`.

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| src/modes/interactive/theme/theme.ts | 1 335 | src/modes/interactive/theme/theme.rs | verifiziert (bis auf den Syntax-Highlighter, s. u.) | Klasse 3: `chalk` → direkte ANSI-Sequenzen. Der Wrap-Algorithmus ist exakt nachgebaut (`stringReplaceAll` behält den Close-Code und hängt den Open-Code an, `stringEncaseCRLFWithFirstIndex` klammert jede Zeile einzeln, leerer String bleibt leer) — gegen chalk 5 verifiziert. NICHT nachgebaut ist chalks TTY-Farbstufen-Erkennung (`supports-color`): der Port gibt die Sequenzen unbedingt aus, so wie `fg`/`bg` es in TS ohnehin tun. Klasse 3: TypeBox `Compile` → handgeschriebener Validator; Fehlertexte, Fehlerreihenfolge (Schema-Deklarationsreihenfolge, nicht Dateireihenfolge) und die 8-Fehler-Obergrenze von `Errors()` sind empirisch gegen die TS-Implementierung abgeglichen und in `tests/theme_validation.rs` gepinnt. Klasse 4: die eingebauten Themes liegen per `include_str!` in der Binary statt in `dist/theme/` daneben; `getAvailableThemesWithPaths` meldet weiterhin `get_themes_dir()/<name>.json`. Klasse 1: das globale Theme liegt in Prozess-Globals statt in `globalThis` (dadurch auch aus den tokio-Worker-Threads der Tools lesbar); `on_theme_change` verlangt `Send + Sync`; `fg`/`bg` panicken bei unbekanntem Slot wie der uncaught `throw` in TS; werfende Funktionen geben `Result<_, ThemeError>` zurück; `ThemeColor`/`ThemeBg` sind Enums statt String-Literale, die Farbmaps sind einfügereihenfolge-erhaltend, damit `Object.entries` und die Spread-Semantik von `withThemeColorFallbacks` erhalten bleiben; `readdirSync` wird sortiert, damit die Auswahl bei doppelten Theme-Namen deterministisch ist. **Offen:** `highlightCode` und `getMarkdownTheme().highlightCode` nehmen bis auf Weiteres immer den TS-Pfad „keine gültige Sprache" — der Syntax-Highlighter (`src/utils/syntax-highlight.ts`) gehört C (Task 13) und liegt noch nicht auf main, siehe Interface-Request A-5. `Theme.sourceInfo` fehlt, bis C `core/source-info.ts` portiert hat (A-5) |
| src/modes/interactive/theme/theme-controller.ts | 139 | src/modes/interactive/theme/theme_controller.rs | portiert | Klasse 1: der Controller ist ein `Rc<RefCell<…>>`-Handle (wie `TuiCore`), weil der Color-Scheme-Listener `this` einfängt; `unsubscribe`-Closure → `ListenerId` + `remove_terminal_color_scheme_listener`; `TUI` ist strukturell typisiert, deshalb implementiert der Port `TerminalBackgroundThemeDetector`/`TerminalAutoThemeDetector` explizit für `TuiCore`; `settingsManager.flush()` ist im Port synchron |
| src/modes/interactive/theme/dark.json | 90 | src/modes/interactive/theme/dark.json | verifiziert | unverändert übernommen (Asset) |
| src/modes/interactive/theme/light.json | 89 | src/modes/interactive/theme/light.json | verifiziert | unverändert übernommen (Asset) |
| src/modes/interactive/theme/theme-schema.json | 352 | src/modes/interactive/theme/theme-schema.json | übernommen | unverändert übernommen (Asset). Reine Editor-Unterstützung: zur Laufzeit validiert `theme.ts` über sein TypeBox-Schema, das Datei-Schema wird nur über `$schema` in den Theme-Dateien referenziert |
| test/theme-detection.test.ts | 174 | tests/theme_detection.rs | verifiziert | 11 Tests (TS: 9 `it`-Blöcke; die beiden `toMatchObject`-Blöcke mit je zwei Erwartungen bleiben zusammen). Klasse 1: Rust-Futures sind lazy — der Fall „starts both queries" pollt das Future einmal, was dem synchronen Start der TS-`async`-Funktion entspricht |
| test/theme-export.test.ts | 104 | tests/theme_export.rs | verifiziert | 2 Tests, unverändert |
| test/theme-picker.test.ts | 51 | tests/theme_picker.rs | verifiziert | 1 Test, unverändert |
| test/scrollbar-theme.test.ts | 70 | tests/scrollbar_theme.rs | verifiziert | 4 Tests, unverändert |
| — (neu) | — | tests/theme_validation.rs | verifiziert | 15 Tests, die die TS-Testsuite nicht abdeckt: alle Validierungs-Fehlertexte, Fehlerreihenfolge, 8-Fehler-Obergrenze, Var-Auflösungsfehler, unbekannte Farbschlüssel. Erwartungswerte stammen aus Läufen gegen `theme.ts` (`npx tsx`), nicht aus dem Schema-Text |
| — (neu) | — | tests/theme_runtime.rs | verifiziert | 9 Tests: chalk-Ersatz (12 Fälle gegen chalk 5 abgeglichen), 256-Farb-Quantisierung (40 Hex-Werte gegen `rgbTo256` der TS-Implementierung), leere Farbwerte, Palettenindizes, Dark-Fallback bei ungültigem Theme, Change-Callback, Live-Reload inkl. Debounce und `stopThemeWatcher` |
| test/test-theme-colors.ts | 249 | — | ausgeschlossen | manuelles CLI-Skript (Kontrastrechner/Theme-Vorschau, `npx tsx test-theme-colors.ts light|dark|contrast|test`), kein Test-Runner-Ziel — wie die manuellen Harnesses des tui-Pakets klassifiziert |
| src/modes/interactive/components/visual-truncate.ts | 50 | src/modes/interactive/components/visual_truncate.rs | verifiziert | Klasse 1: bug-compat — `slice(-0)` ist in JS `slice(0)`, ein Limit von 0 behält also alle Zeilen und meldet sie zugleich als übersprungen; im Port explizit nachgebildet |
| src/modes/interactive/components/dynamic-border.ts | 25 | src/modes/interactive/components/dynamic_border.rs | verifiziert | Klasse 1: der Default-Parameter wird zu `Option<StyleFn>`; beide Wege lesen das globale Theme beim Rendern. Der jiti-Hinweis im TS-Kommentar entfällt mit dem Extension-System (Klasse 2) |
| src/modes/interactive/components/countdown-timer.ts | 39 | src/modes/interactive/components/countdown_timer.rs | verifiziert | Klasse 1: `setInterval` + `onTick`/`onExpire` → Poll-Schnittstelle (`deadline()`/`tick()`), wie alle Timer der tui-Crate (A-4, „Timer rufen nie zurück"); ein Tick auf 0 meldet wie in TS beides (`onTick(0)` und `onExpire`) und entsorgt den Timer |
| src/modes/interactive/components/keybinding-hints.ts | 48 | src/modes/interactive/components/keybinding_hints.rs | verifiziert | Klasse 1: `Keybinding`/`KeyId` sind im Port `&str` (TS: String-Literal-Unions); `process.platform === "darwin"` → `cfg!(target_os = "macos")`; `charAt(0).toUpperCase()` → erstes Zeichen statt erster UTF-16-Einheit |
| src/modes/interactive/components/status-indicator.ts | 114 | src/modes/interactive/components/status_indicator.rs | verifiziert | Klasse 1: die vier Loader-Unterklassen werden zu Konstruktoren einer Struktur (Rust kennt keine Vererbung); der Retry-Countdown liegt als optionales Feld daneben und wird über `tick_countdown()` gepumpt statt über `setInterval` + `tui.requestRender()`. `WorkingIndicatorOptions` (Extension-Typ) → `LoaderIndicatorOptions` der tui-Crate (Klasse 2) |
| src/modes/interactive/components/bordered-loader.ts | 68 | src/modes/interactive/components/bordered_loader.rs | verifiziert | Klasse 1: Vererbung von `Container` → Komposition; die beiden Loader-Varianten liegen in einem Enum, weil Rust keinen Union-Typ für Felder hat. Klasse 3: `AbortController`/`AbortSignal` → `CancellationToken`; die tui-Loader brauchen keinen `TUI`-Parameter mehr (sie werden gepollt), deshalb entfällt er auch hier |
| src/modes/interactive/components/markdown-transform.ts | 29 | src/modes/interactive/components/markdown_transform.rs | verifiziert | Klasse 2: `MarkdownTransformer`/`MarkdownTransformContext` stammen aus `core/extensions/types.ts` und ziehen mit in diese Datei um — der Mermaid-Renderer ist ein Kern-Transformer (`interactive-mode.ts:471,1991`), das Extension-System entfällt. Damit entfallen auch die beiden Schutzprüfungen (`try`/`catch` und `typeof === "string"`), die nur ungetypten Extension-Code abfingen |
| src/modes/interactive/components/index.ts | 38 | src/modes/interactive/components.rs | teilweise portiert | Die Barrel-Datei wird zur Modulliste; sie wächst mit den Batches. Klasse 2: `ExtensionEditorComponent`/`ExtensionInputComponent` entfallen |
| src/modes/interactive/components/custom-editor.ts | 96 | — | offen | Braucht `src/core/keybindings.ts` (`AppKeybinding`, `KeybindingsManager`) — Workstream C, Task 13; siehe Interface-Request A-7 |
| test/status-indicator.test.ts | 32 | tests/status_indicator.rs | verifiziert | 4 Tests (TS: 2). Der TS-Fall „disposes retry countdown updates" prüft über gefälschte Timer, dass nach `dispose()` kein `requestRender` mehr kommt; der gepollte Port prüft, dass keine Deadline übrig ist. Zwei zusätzliche Fälle decken die Countdown-Meldung und die Labels aller vier Arten ab |
| — (neu) | — | tests/component_utilities.rs | verifiziert | 20 Tests für die Batch-1-Bausteine, die die TS-Suite nicht abdeckt: visual-truncate inkl. Zeilenumbruch, Padding und Null-Limit, DynamicBorder, keybinding-hints (Formatierung, macOS-Option, Auflösung über die globale Registry, Theming), CountdownTimer und markdown-transform, BorderedLoader in beiden Varianten |
| src/modes/interactive/components/diff.ts | 147 | src/modes/interactive/components/diff.rs | verifiziert | Klasse 3: das npm-Paket `diff` (jsdiff 8.0.4) hat für `diffWords` keine Rust-Entsprechung — die Master-Tabelle setzt `similar` nur für Unified-Patches ein, und dessen Gruppierung weicht ab. Der Port bringt jsdiffs Wortdiff selbst mit (siehe nächste Zeile). Klasse 1: die Regex `/^([+-\s])(\s*\d*)\s(.*)$/` ist als Backtracking-Suche über Zeichenklassen nachgebaut (gleiche Kandidatenreihenfolge; `.` matcht wie in JS keinen Zeilenumbruch) |
| node_modules/diff (jsdiff 8.0.4): base.js, word.js, util/string.js | 718 | src/modes/interactive/components/diff/word_diff.rs | verifiziert | Klasse 3: Port der Bibliothek für genau den Optionssatz, den `renderDiff` benutzt (kein Segmenter, kein `ignoreCase`, kein Comparator, kein `oneChangePerToken`, keine Edit-/Zeitgrenze). Enthalten: Tokenizer inkl. Whitespace-Anheftung, `equals` über `trim`, der Myers-Kern mit Diagonalen-Pruning, `buildValues`, `join` und die vollständige Whitespace-Nachbearbeitung. Der Rückgabewert `undefined` bei erschöpfter Editlänge ist ohne `maxEditLength` unerreichbar |
| src/modes/interactive/components/user-message.ts | 70 | src/modes/interactive/components/user_message.rs | verifiziert | Klasse 1: Vererbung von `Container` → Komposition; Default-Parameter werden `Option` |
| src/modes/interactive/components/assistant-message.ts | 197 | src/modes/interactive/components/assistant_message.rs | verifiziert | Klasse 1: Vererbung → Komposition; der `contentContainer` wird direkt gerendert statt zusätzlich als eigenes Kind gehalten (identische Ausgabe); `errorMessage` ist `Option<String>`, leerer String verhält sich wie in JS falsy |
| src/modes/interactive/components/compaction-summary-message.ts | 59 | src/modes/interactive/components/compaction_summary_message.rs | verifiziert | Klasse 3: `Number.prototype.toLocaleString()` → `components::to_locale_string` mit der en-US-Gruppierung, die Node in dieser App auflöst (statt einer ICU-Abhängigkeit, die die Master-Tabelle nicht führt) |
| src/modes/interactive/components/branch-summary-message.ts | 58 | src/modes/interactive/components/branch_summary_message.rs | verifiziert | Klasse 1: Vererbung von `Box` → Komposition |
| src/modes/interactive/components/custom-message.ts | 113 | src/modes/interactive/components/custom_message.rs | verifiziert | Klasse 2: der optionale `MessageRenderer` entfällt — er kommt ausschließlich aus `extensionRunner.getMessageRenderer` (`interactive-mode.ts:3728`), und `plans/facts/extension-boundary.md` führt Message-Renderer-Overrides als ersatzlos entfallend. Der Default-Renderpfad bleibt vollständig, weil `CustomMessage` ein Kern-Nachrichtentyp ist. `setExpanded`/`setOutputPad` bleiben als API erhalten, wirken aber wie in TS nur auf den entfallenen Renderer |
| src/modes/interactive/components/bash-execution.ts | 220 | src/modes/interactive/components/bash_execution.rs | verifiziert | Klasse 1: das anonyme Komponenten-Literal mit Breiten-Cache wird zur Struktur `PreviewLines`; der Loader braucht keinen `TUI`-Parameter mehr (er wird gepollt). bug-compat: der Kommandokopf wird in `updateDisplay` immer in `bashMode` gefärbt, auch wenn der Konstruktor für `!!`-Kommandos `dim` gewählt hat |
| src/modes/interactive/components/custom-entry.ts | 62 | — | ausgeschlossen | Klasse 2: die Komponente verlangt zwingend einen `EntryRenderer`, den nur `extensionRunner.getEntryRenderer` liefert (`interactive-mode.ts:3689`); Entry-Renderer entfallen laut `plans/facts/extension-boundary.md` ersatzlos, ebenso die Custom-Entries, die nur Extensions schreiben |
| src/modes/interactive/components/skill-invocation-message.ts | 55 | — | offen | braucht `ParsedSkillBlock` aus `core/agent-session.ts` (Workstream C, Task 11) — Interface-Request A-8 |
| src/modes/interactive/components/todo-list.ts | 216 | — | offen | braucht `Todo`/`TodoStatus` aus `core/todos/todos.ts` (Workstream C) — A-8 |
| src/modes/interactive/components/tool-execution.ts | 377 | — | offen | braucht `core/tools/render-utils.ts`, `createAllToolDefinitions`/`ToolName` aus `core/tools/index.ts` und `utils/image-convert.ts` (Workstream C) — A-8 |
| src/modes/interactive/components/footer.ts | 253 | — | offen | braucht `core/agent-session.ts`, `core/footer-data-provider.ts`, `core/modes/indicator.ts` und `core/usage-totals.ts` (Workstream C) — A-8 |
| src/modes/interactive/components/mermaid.ts | 89 | — | offen | braucht den Ersatz für `grok-mermaid` (Master-Plan, WS-C Task 15) — A-8 |
| test/assistant-message.test.ts | 241 | tests/assistant_message.rs | verifiziert | 10 Tests (TS: 11). Der Fall „continues the Markdown transformer chain when a transformer throws" entfällt mit der Schutzprüfung, die er testet: ein Rust-Transformer kann nicht werfen, und die Prüfung fing nur ungetypten Extension-Code ab |
| test/custom-message.test.ts | 44 | tests/custom_message.rs | verifiziert | 2 Tests. Der TS-Fall treibt die Komponente über einen Custom-Renderer; beobachtbar bleibt der Default-Renderpfad, den die beiden Fälle prüfen |
| test/bash-execution-width.test.ts | 80 | tests/bash_execution_width.rs | verifiziert | 2 Tests, unverändert; der TUI-Stub entfällt, weil die portierten Komponenten gepollt werden |
| — (neu) | — | tests/diff_words_oracle.rs | verifiziert | 900 Fälle (30 × 30 Zeilenpaare): der Wortdiff stimmt zeichengenau mit jsdiff 8.0.4 überein. Generator `tools/gen-diff-words-oracle.mjs` |
| — (neu) | — | tests/render_diff_oracle.rs | verifiziert | 20 Diff-Texte byteweise gegen die TS-Komponente (dark, truecolor, `FORCE_COLOR=1`). Generator `tools/gen-render-diff-oracle.mjs` |
| — (neu) | — | tests/summary_messages.rs | verifiziert | 3 Tests für die beiden Summary-Komponenten (Collapse/Expand, Tausendertrennung, Bold-Label), die die TS-Suite nicht abdeckt |
| src/modes/interactive/components/extension-selector.ts | 112 | src/modes/interactive/components/list_selector.rs | verifiziert | Klasse 2: umbenannt zu `ListSelectorComponent` (Interface-Request C-5) — die Komponente ist ein generischer Listen-Selektor, den vier Kern-Stellen benutzen; das Wort „extension" verschwindet mit dem Extension-System. Klasse 1: der `tui`-Parameter der Optionen entfällt, weil der Countdown gepollt wird (`countdown_deadline()`/`tick_countdown()`) |
| src/modes/interactive/components/thinking-selector.ts | 75 | src/modes/interactive/components/thinking_selector.rs | verifiziert | Klasse 1: `Record<ThinkingLevel, string>` → `match`; Vererbung von `Container` → Komposition |
| src/modes/interactive/components/theme-selector.ts | 67 | src/modes/interactive/components/theme_selector.rs | verifiziert | Klasse 1: Vererbung → Komposition |
| src/modes/interactive/components/show-images-selector.ts | 50 | src/modes/interactive/components/show_images_selector.rs | verifiziert | Klasse 1: Vererbung → Komposition |
| src/modes/interactive/components/user-message-selector.ts | 155 | src/modes/interactive/components/user_message_selector.rs | verifiziert | Klasse 1: Vererbung → Komposition; das `setTimeout(onCancel, 100)` für die leere Liste wird zu `is_empty()`, das der Aufrufer abfragt (Timer rufen nie zurück) |
| src/modes/interactive/components/approval-selector.ts | 84 | — | offen | braucht `core/permissions/request.ts` (Workstream C, nativ nachzubauen) — A-9 |
| src/modes/interactive/components/trust-selector.ts | 134 | — | offen | braucht `core/trust-manager.ts` (Workstream C) — A-9 |
| src/modes/interactive/components/first-time-setup.ts | 145 | — | offen | portierbar, in dieser Sitzung nicht mehr erreicht — A-9 |
| src/modes/interactive/components/session-selector-search.ts | 194 | — | offen | portierbar, in dieser Sitzung nicht mehr erreicht — A-9 |
| src/modes/interactive/components/tree-selector.ts | 1 437 | — | offen | portierbar (`SessionTreeNode` liegt auf main), in dieser Sitzung nicht mehr erreicht — A-9 |
| src/modes/interactive/components/oauth-selector.ts | 206 | — | offen | portierbar (`ApiKeyAuth`/`AuthCheck`/`OAuthAuth` liegen auf main), in dieser Sitzung nicht mehr erreicht — A-9 |
| src/modes/interactive/components/session-selector.ts | 1 031 | — | offen | braucht `core/keybindings.ts` — A-7 |
| src/modes/interactive/components/model-selector.ts | 364 | — | offen | braucht `core/model-runtime.ts` und `modes/interactive/model-search.ts` — A-9 |
| src/modes/interactive/components/scoped-models-selector.ts | 403 | — | offen | braucht `modes/interactive/model-search.ts` — A-9 |
| src/modes/interactive/components/settings-selector.ts | 881 | — | offen | braucht `core/http-dispatcher.ts` — A-9 |
| src/modes/interactive/components/config-selector.ts | 942 | — | offen | braucht `core/package-manager.ts` (Workstream C, Task 14) — A-9 |
| src/modes/interactive/components/login-dialog.ts | 233 | — | offen | braucht `utils/open-browser.ts` — A-9 |
| — (neu) | — | tests/selectors.rs | verifiziert | 8 Tests für die fünf portierten Selektoren (Cursor, Wrap-Around, Timeout-Countdown, Vorauswahl, Preview-Callback, leere Liste), die die TS-Suite nicht abdeckt |

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| src/core/extensions/, src/extensions/ (außer llama) | Extension-System (Master-Plan, Ausschluss-Tabelle) |
| src/server/create-harness.ts | Stub, nur vom eigenen Test konsumiert |
| src/bun/, src/cli/experimental/ | Distributionsmechanik bzw. nicht verdrahtet |
