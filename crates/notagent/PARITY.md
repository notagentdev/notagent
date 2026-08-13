# Parity-Ledger: notagent

TS-Quelle: `/Users/dev/projects/notagent-main/packages/coding-agent` (68 856 LOC in src/) — Workstream C (TUI-Teile mit A).

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

**Stand: Task 5 abgeschlossen.** `config.ts`, `core/settings-manager.ts` (inkl. aller
typisierten Zugriffsmethoden), `migrations.ts`, `core/resolve-config-value.ts`,
`core/auth-storage.ts` und die Utilities (`utils/paths.ts`, `utils/shell.ts`,
`utils/abort.ts` + Lockfile-Ersatz) sind portiert und testbelegt. Offen bleibt nur, was
laut Plan zu späteren Tasks gehört: `utils/shell.ts` liefert erst mit Task 8 die
Bash-Ausführung nach, die keybindings.json-Migration folgt mit Task 13.

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
| src/utils/abort.ts | 46 | src/utils/abort.rs | portiert | Klasse 3: `AbortSignal` → `CancellationToken`; `raceWithAbortSignal` verlangt, dass abgebrochene Arbeit vom Aufrufer am Leben gehalten wird (Task/Shared-Future), weil ein fallengelassenes Rust-Future abbricht |
| — (neu) | — | src/utils/lockfile.rs | neu (Tech-Substitution) | Klasse 3: `proper-lockfile` → `<datei>.lock`-Verzeichnis mit identischer Semantik: atomares `mkdir`, `ELOCKED`, Stale-Übernahme, Heartbeat-Thread der die mtime auffrischt, `onCompromised` |
| src/core/auth-storage.ts | 507 | src/core/auth_storage.rs | portiert | Klasse 1: der Backend-Callback gibt nur `next` zurück (das `result`-Feld entfällt, weil ein Rust-Closure in seinen Aufrufer schreiben kann) — dadurch bleibt der Trait objektsicher; die Daten bleiben als `serde_json::Map` liegen, damit unbekannte Einträge verlustfrei erhalten bleiben (TS validiert in `AuthStorage` ebenfalls nicht), und werden erst beim Lesen in `Credential` überführt; eine abgebrochene In-Memory-Mutation wird beim Verwerfen des Futures gestoppt statt im Hintergrund weiterzulaufen (sie kann in beiden Fällen nicht mehr schreiben). Klasse 3: `proper-lockfile` → `src/utils/lockfile.rs`; `AbortSignal` → `CancellationToken`; der koaleszierte Reload läuft als `tokio::spawn` + `Shared`, damit ein abbrechender Leser die übrigen nicht mitreißt |
| test/auth-storage.test.ts | 535 | tests/auth_storage.rs | Tests portiert | 26 Tests. Statt `vi.spyOn(lockfile, …)` werden echte Locks gehalten bzw. ein zählendes Backend benutzt; der Fall „releases a file lock acquired concurrently with cancellation" ist ohne Mock nicht deterministisch auslösbar und geht in „aborts while waiting for a held file lock" auf (dort wird zusätzlich geprüft, dass kein Lock zurückbleibt). Zwei OAuth-Fälle sind bis zur Umsetzung von Interface-Request C-4 `#[ignore]` |

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| src/core/extensions/, src/extensions/ (außer llama) | Extension-System (Master-Plan, Ausschluss-Tabelle) |
| src/server/create-harness.ts | Stub, nur vom eigenen Test konsumiert |
| src/bun/, src/cli/experimental/ | Distributionsmechanik bzw. nicht verdrahtet |
