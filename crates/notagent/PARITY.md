# Parity-Ledger: notagent

TS-Quelle: `/Users/dev/projects/notagent-main/packages/coding-agent` (68 856 LOC in src/) — Workstream C (TUI-Teile mit A).

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

**Stand: Task 7 läuft.** `config.ts`, `core/settings-manager.ts` (inkl. aller
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
| src/utils/shell.ts | 259 | src/utils/shell.rs | portiert (genutzte Teile) | `getShellConfig`/`getShellEnv`/`sanitizeBinaryOutput`/Prozessbaum-Signale portiert. Klasse 1: Lone-Surrogate-Filter entfällt (Rust-`str` ist immer gültiges UTF-8); `process.kill(-pid)` → `libc::kill(-pid)` mit Fallback auf den Einzelprozess. Offen (Task 7, Bash-Tool): `spawnShellCommand` und die Exit-Handler-Registrierung |
| src/utils/paths.ts | 137 | src/utils/paths.rs | portiert | Klasse 1: `normalizePath` gibt `Result` zurück, weil `fileURLToPath` wirft; `path.resolve`/`path.relative`/`fileURLToPath`/`pathToFileURL` sind als Node-Semantik nachgebaut (Rusts `Path` normalisiert Punkt-Segmente nicht) |
| test/paths.test.ts | 184 | src/utils/paths.rs (Testmodul) | Tests portiert | 13 Tests: canonicalize inkl. Symlinks/danglings, cwd-relative Pfade, Tilde-Regeln, file:-URLs inkl. Fehlerfällen, Windows-Shell-Pfade, isLocalPath |
| src/utils/ansi.ts | 60 | src/utils/ansi.rs | portiert | Dieselbe Grammatik als `regex`-Literal; der MIT-Hinweis der abgeleiteten Pakete steht im Modulkopf |
| src/utils/abort.ts | 46 | src/utils/abort.rs | portiert | Klasse 3: `AbortSignal` → `CancellationToken`; `raceWithAbortSignal` verlangt, dass abgebrochene Arbeit vom Aufrufer am Leben gehalten wird (Task/Shared-Future), weil ein fallengelassenes Rust-Future abbricht |
| — (neu) | — | src/utils/lockfile.rs | neu (Tech-Substitution) | Klasse 3: `proper-lockfile` → `<datei>.lock`-Verzeichnis mit identischer Semantik: atomares `mkdir`, `ELOCKED`, Stale-Übernahme, Heartbeat-Thread der die mtime auffrischt, `onCompromised` |
| src/core/auth-storage.ts | 507 | src/core/auth_storage.rs | portiert | Klasse 1: der Backend-Callback gibt nur `next` zurück (das `result`-Feld entfällt, weil ein Rust-Closure in seinen Aufrufer schreiben kann) — dadurch bleibt der Trait objektsicher; die Daten bleiben als `serde_json::Map` liegen, damit unbekannte Einträge verlustfrei erhalten bleiben (TS validiert in `AuthStorage` ebenfalls nicht), und werden erst beim Lesen in `Credential` überführt; eine abgebrochene In-Memory-Mutation wird beim Verwerfen des Futures gestoppt statt im Hintergrund weiterzulaufen (sie kann in beiden Fällen nicht mehr schreiben). Klasse 3: `proper-lockfile` → `src/utils/lockfile.rs`; `AbortSignal` → `CancellationToken`; der koaleszierte Reload läuft als `tokio::spawn` + `Shared`, damit ein abbrechender Leser die übrigen nicht mitreißt |
| test/auth-storage.test.ts | 535 | tests/auth_storage.rs | Tests portiert | 26 Tests. Statt `vi.spyOn(lockfile, …)` werden echte Locks gehalten bzw. ein zählendes Backend benutzt; der Fall „releases a file lock acquired concurrently with cancellation" ist ohne Mock nicht deterministisch auslösbar und geht in „aborts while waiting for a held file lock" auf (dort wird zusätzlich geprüft, dass kein Lock zurückbleibt). Zwei OAuth-Fälle sind bis zur Umsetzung von Interface-Request C-4 `#[ignore]` |

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

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| src/core/extensions/, src/extensions/ (außer llama) | Extension-System (Master-Plan, Ausschluss-Tabelle) |
| src/server/create-harness.ts | Stub, nur vom eigenen Test konsumiert |
| src/bun/, src/cli/experimental/ | Distributionsmechanik bzw. nicht verdrahtet |
