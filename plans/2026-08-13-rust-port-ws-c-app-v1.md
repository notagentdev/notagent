# Rust-Port Workstream C: Protokoll-Crates und die App (notagent-Binary)

## Objective

Anlage des Workspace-Scaffolds (Gate G0), 1:1-Port der vier eigenständigen Library-Crates `notagent-protocol` (1 236 LOC), `notagent-client` (1 225), `notagent-server` (2 299), `notagent-session-sqlite` (2 505) und anschließend der App `crates/notagent` (Bin, aus `packages/coding-agent`, 68 856 LOC src — ohne Extension-System, mit nativem Nachbau von Permissions, Hooks und llama.cpp-Provider). Subagenten und Background-Tasks laufen als echte parallele tokio-Tasks. Faktenbasis: `plans/facts/coding-agent-core.md`, `plans/facts/extension-boundary.md`, `plans/facts/protocol-server-client-sqlite-telemetry-evals.md`, `plans/facts/rust-minify-reference.md`; verbindliche Regeln im Master-Plan `plans/2026-08-13-rust-port-master-v1.md`.

TS-Quelle: `/Users/dev/projects/notagent-main/packages/coding-agent` (und die vier kleinen Pakete). Rust-Minify-Referenz: `/Users/dev/projects/notagent-main-rust` (lesen und adaptieren, siehe Task 7). Read-before-Port und Ledger-Pflicht gelten für jede Task.

## Implementation Plan

- [x] 1. Gate G0 — Workspace-Scaffold (allererste Task, danach Tag gate-g0 und Merge nach main): Cargo-Workspace mit members-Glob für crates/*, resolver 2, workspace.dependencies (tokio, serde, serde_json, reqwest+rustls, tokio-tungstenite, rusqlite bundled, uuid v7, tokio-util, unicode-segmentation, unicode-width, similar, globset, ignore, image, sha2, base64, rand, semver, tree-sitter-Familie in den Versionen aus `plans/facts/rust-minify-reference.md`), rust-toolchain.toml auf das lokal installierte stable (per rustc --version ermitteln), neun leere Crate-Stubs mit Cargo.toml + lib.rs bzw. main.rs, scripts/check.sh (cargo fmt --check, cargo clippy --workspace --all-targets -D warnings, cargo test --workspace), CONVENTIONS.md (Modul-Layout spiegelt TS-Dateistruktur, Fehlerbehandlung via thiserror-Typen je Crate, Ledger-Format mit Spalten TS-Datei/Rust-Modul/Status/Abweichung, Commit-Konvention), leere PARITY.md je Crate, plans/interface-requests.md anlegen. Rationale: einziger Task mit Root-Datei-Ownership; alles Weitere ist konfliktfrei parallelisierbar.
- [x] 2. notagent-protocol portieren: alle Dateien unter `packages/protocol/src/` lesen (schemas.ts 450, codec.ts 172, framing.ts 165, cbor/encoder.ts 216, cbor/decoder.ts 168, cbor/options.ts 52) und portieren — 4-Byte-BE-Längenpräfix-Framing mit inkrementellem FrameDecoder (64-KiB-Blöcke, Nulllängen-Frames, Truncation-Erkennung via end), strikter CBOR-Subset-Encoder/-Decoder mit EXAKT den Ablehnungsregeln (keine Tags, kein Indefinite-Length, nur Simple 20/21/22/27, kanonisch kürzeste Integer, keine doppelten Map-Keys, kein Trailing-Data, UTF-8-Validierung, Limits 16 MiB / 1e6 Container / Tiefe 64) — KEIN ciborium, Eigenbau nach Vorlage; alle Nachrichtentypen aus schemas.ts als serde-Typen mit deny_unknown_fields (additionalProperties false), PROTOCOL_VERSION 1, Codec mit ProtocolValidationError. Die Testsuiten (protocol 424, cbor 175, framing 117 LOC) vollständig portieren. Rationale: exakt spezifiziertes, abhängigkeitsfreies Paket — idealer Start, während A/B ihre Fundamente legen.
- [x] 3. notagent-client und notagent-server portieren: alle Dateien beider Pakete lesen (client: client.ts 432, connection.ts 236, state.ts 156, unix.ts 156, session-handle.ts 111, errors/types/transport/promise; server: server.ts 396, sessions.ts 346, protocol.ts 382, snapshots.ts 62, errors.ts 58, connection.ts, listener.ts, transports/unix/listener.ts 434 + preset + types, testing/ 466) und portieren — Client: Request-Sequenz-IDs, Response-Command-Validierung, Lease-System (shared/exclusive mit Ownership-Fehlern, Zähler, Generationen, detach beim letzten Release, Cleanup-Rekonziliation), Revisions-Guard im State, KEIN Auto-Reconnect; Server: Stage-Automat awaitingHello→closed, Handshake mit Snapshot und Revisions-Nachlieferung, Fehler-Sanitizing (internal_error verbirgt Ursache), LiveSessionManager (Server generiert Session-ID, Auto-Dispose bei idle ohne Connections/Ops, normalizedSnapshot, terminate bei Runtime-Fehler), ai↔protocol-Bridge; Unix-Transport mit dem Publikationsverfahren privater Pfad .p-hash + link, mkdir 0o700, chmod 0o600, Stale-Erkennung per Probe-Connect mit 1-s-Timeout, Entfernen/Cleanup via rename mit dev/ino-Verifikation, Pfadlängen-Limits 107/103, serialisierte Writes mit maxPendingBytes, graceful Close 5 s. Test-Utilities (TestServerService, ProtocolTestClient) und die Konformanztests (1 456 + 1 227 LOC) portieren. Rationale: in sich geschlossenes Client/Server-Paar; der Unix-Listener ist laut Faktenlage der subtilste Teil.
- [x] 4. notagent-session-sqlite portieren: alle Dateien unter `packages/session-backends/sqlite-node/src/` lesen (repo.ts 953, search-backend.ts 188, branch-cache.ts 101, storage/ 10 Dateien, migrations, sql.ts, 001_initial.sql) und auf rusqlite portieren — alle 11 Tabellen (WITHOUT ROWID außer entries) plus migrations-Tabelle und lazy FTS5-Virtualtabelle mit Trigram-Tokenizer und 3 Triggern, PRAGMAs WAL/synchronous FULL/busy_timeout 5000, Writer-Lease mit owner_id/monotonem fence/expires_at_ms (ttl 30 s, Heartbeat 10 s), jeder Write in BEGIN-IMMEDIATE-Transaktion mit Lease-Erneuerung, SerialOperationQueue, bm25-Suche mit frischer DB pro Suchaufruf; die SessionRepo/SessionStorage-Trait-Definitionen aus `packages/agent/src/harness/session/types.ts:361` werden dafür (als Teil dieses Crates oder von notagent-agent, Abstimmung via interface-requests) mitportiert; Konformanz-Testsuite (1 016 LOC in agent + 1 804 LOC eigene Tests) portieren. Rationale: publiziertes Paket des Monorepos, vollständig spezifiziert durch Schema + Tests; unabhängig von allem anderen.
- [x] 5. App-Fundament portieren: `src/config.ts` (576 — alle Pfade, agentDir-Auflösung, Install-Detection, Self-Update-Kommandobau), `src/core/settings-manager.ts` (1 273 — vollständige Settings-Struktur mit allen Defaults aus der Faktenliste, global/project-Scopes, Deep-Merge, Trust-Gate, Datei-Locking mit 10 × 20-ms-Retry, Settings-Migrationen), `src/migrations.ts` (314 — auth.json-Migration, Session-Verzeichnis-Migration, prompts/bin-Umzüge, Keybinding-Namen), `src/core/auth-storage.ts` (507 — auth.json, ReadOnlyAuthStorage) lesen und portieren; Tests mitportieren. Rationale: Konfigurationskompatibilität (gleiche Pfade, gleiche Dateien) ist Teil des 1:1-Anspruchs und Voraussetzung jeder weiteren App-Task.
> **Abschluss Task 5 (2026-08-13):** portiert und testbelegt sind `config.ts` (Pfade,
> Install-Erkennung inkl. npm-Prefix-Inferenz und Package-Manager-Prüfung, Self-Update-
> Kommandos), `settings-manager.ts` vollständig (Settings-Typ mit Erhalt unbekannter
> Schlüssel, Deep-Merge, Global/Projekt-Scopes mit Trust-Gate, Datei-Locking, vier
> Migrationen, alle typisierten Zugriffsmethoden), `migrations.ts`, `core/auth-storage.ts`
> und die dafür nötigen Utilities (`utils/paths.ts`, `utils/shell.ts`, `utils/abort.ts`,
> `core/resolve-config-value.ts` sowie ein Lockfile-Modul als Ersatz für proper-lockfile).
> Testsuiten: config (15), settings-manager (27), migrations (4), auth-storage (26, alle grün;
> die 2 OAuth-Fälle seit Umsetzung von Interface-Request C-4 entsperrt), paths/shell/abort/lockfile/resolve-config-value
> als Unit-Tests im Modul.

- [x] 6. Session-Manager portieren: `src/core/session-manager.ts` (1 714 LOC) vollständig lesen und portieren — JSONL v3 mit Header und allen 9 Entry-Typen als Baum (id/parentId, 8-Hex-IDs mit Kollisionsprüfung, uuidv7-Session-IDs mit Validierung), Speicherorte mit cwd-Encoding, verzögerte Datei-Anlage bis zur ersten Assistant-Message (wx-open, danach append), Header-Scan mit 1-MiB-Grenze und Vollast-Fallback, v1→v2→v3-Migrationen mit Datei-Rewrite, continueRecent/inMemory/forkFrom (neue Datei mit parentSession), list/listAll mit paralleler Info-Ladung (max 10), Branching (branch, branchWithSummary, createBranchedSession mit Pfad-Extraktion und Label-Neuanlage, getTree mit Timestamp-Sortierung und Orphan-Roots), Kontextaufbau (buildContextEntries mit letzter Compaction, sessionEntryToContextMessages, buildSessionContext mit thinkingLevel/model-Ableitung), resolveSessionPath-Auflösungslogik. Session-Fixtures aus dem TS-Repo als Roundtrip-Tests (TS-Datei öffnen → fortsetzen → forken → re-serialisieren). Rationale: Datenformat-Kompatibilität mit bestehenden Nutzer-Sessions.
> **Abschluss Task 6 (2026-08-13):** `core/session-manager.ts` ist vollständig portiert
> (JSONL-Baum mit allen neun Eintragstypen, v1→v2→v3-Migrationen, begrenzter Header-Scan mit
> Voll-Ladung als Rückfall, verzögerte Dateianlage, Branching inkl. createBranchedSession mit
> Label-Neuverkettung, compaction-bewusster Kontextaufbau, nebenläufige Session-Listen) und
> mit 73 Tests belegt, darunter Roundtrips gegen die TS-Fixtures `before-compaction.jsonl`
> und `large-session.jsonl`. `core/messages.ts` ist als Re-Export der inhaltsgleichen
> Agent-Datei portiert. **Verbleibend aus der Task-Beschreibung:** `resolveSessionPath` liegt
> in `src/main.ts` und wird mit Task 12 portiert; die drei session-bezogenen CLI-E2E-Tests
> ebenso.

- [x] 7. Tools portieren (Teil 1, Dateisystem + Suche): `src/core/tools/index.ts` (Registry, 16 ToolNames, Presets), `truncate.ts` (2000 Zeilen / 50 KiB / 500-Zeichen-Grep-Zeilen, nie Teilzeilen), `path-utils.ts` (macOS-Screenshot-Fallbacks), `read.ts` (Bild-Pipeline mit Auto-Resize 2000×2000, offset/limit, Fortsetzungs-Notizen), `write.ts` (mkdir-p, FileMutationQueue), `edit.ts` + `edit-diff.ts` (BOM/Zeilenenden-Erhalt, Fuzzy-Matching mit NFKC/Smart-Quotes/Dashes/Spaces-Normalisierung, Eindeutigkeitszwang, Überlappungsprüfung, Rückwärts-Anwendung, Original-Byte-Erhalt außerhalb betroffener Zeilen, Display-Diff + Unified-Patch), `file-mutation-queue.ts` (Serialisierung pro realpath, Abort-Checks nach jedem await), `ls.ts`, `find.ts` (fd mit --no-require-git-Logik, Pfad-Normalisierung), `grep.ts` (ripgrep --json-Streaming, Kill bei Limit, Kontext-Nachlesen mit Datei-Cache), `utils/tools-manager.ts` (ensureTool-Download von fd/rg nach ~/.notagent/agent/bin), `render-utils.ts`, `output-accumulator.ts` lesen und portieren; Tool-Definitionen mit dynamischen description/parameters-GETTERN als Funktionen abbilden; ToolDefinition-Kontext ohne Extension-Infrastruktur (nur die tatsächlich genutzten Felder, siehe extension-boundary §2.4). Die Tool-Testsuiten mitportieren. Rationale: Grundwerkzeuge für jede Agent-Interaktion; edit ist der heikelste Einzelport (Fuzzy-Matching).
> **Abschluss Task 7 (2026-08-13):** portiert und testbelegt sind `tools/truncate.ts`,
> `tools/path-utils.ts`, `tools/file-mutation-queue.ts`, `tools/edit-diff.ts` (inkl. Display-
> Diff und Unified-Patch), die Tools `read`, `write`, `edit`, `ls`, `find` und `grep` jeweils
> als Tool-Hälfte, `tools/output-accumulator.ts`, `utils/tools-manager.ts` samt
> `utils/management-http.ts`, dazu die von `read` benötigten `utils/mime.ts` und die
> Bild-Pipeline (`image-process/-convert/-resize` → `utils/image.rs` auf dem `image`-Crate)
> sowie der native `ToolDefinition`-Kontrakt als Ersatz für den Extension-Typ.
> **Verbleibend aus der Task-Beschreibung:** `tools/render-utils.ts` braucht das Theme aus
> A-Batch 0 (Interface-Request C-5) und wird mit Task 13 verdrahtet; `tools/index.ts` ist die
> Registry über alle 16 Tools und wird geschlossen, sobald die Tasks 8-10 ihre Tools liefern.
> Die `renderCall`/`renderResult`-Hälften aller Tools gehören ebenfalls zu Task 13.

- [x] 8. Tools portieren (Teil 2, bash + Minifizierung): `src/core/tools/bash.ts` (Kein PTY: spawn mit pipes, detached auf Unix; Shell-Auflösungskette inkl. Git-Bash/WSL-stdin-Transport; Timeouts 60/300 bzw. 600/86400 s geklemmt; AUTO-BACKGROUNDING statt Kill bei Deadline; dreistufiger Abbruch terminate→2 s→kill des Prozessbaums; OutputAccumulator mit Tempfile, 100-ms-Throttle, 5-Zeilen-Preview; Env-Exporte NOTAGENT_SESSION_ID usw.; shellCommandPrefix; austauschbare BashOperations), `src/utils/shell.ts`, `src/core/bash-executor.ts` (156, Streaming-!-Ausführung mit ANSI-Strip, Binär-Sanitizing, Rolling-Buffer 2×50 KiB); Minifizierung: die Rust-Referenz `/Users/dev/projects/notagent-main-rust/crates/notagent_services/src/tool_services/minify.rs` (897) und `minify_edit.rs` (913) VOLLSTÄNDIG lesen und als mini_read-Modul adaptieren (natives tree-sitter, byte-genaue src_map — Nutzer-Vorgabe), dazu die TS-Seite `src/core/mini-read/` (1 433) und `src/core/tools/read-minified.ts` / `patch-minified.ts` lesen, um Tool-Verhalten (Schemas, Fehlertexte, Fallback auf Rohtext, Re-Minifizierung zwischen Multi-Edits, warnings) exakt zu übernehmen; `todo-write.ts` (Replace-Semantik, Auto-Clear bei Komplett-completed, 1000-Zeichen-Limit) und `skill.ts` (Mode-vor-Skill-Auflösung, Envelope-Format) portieren. Tests inkl. der Minify-Testbench-Fälle mitportieren. Rationale: bash ist das meistgenutzte Tool; für die Minifizierung existiert erprobter Rust-Code, der Doppelarbeit vermeidet.
> **Abschluss Task 8 (2026-08-13):** portiert und testbelegt sind `tools/bash.ts` (lokale
> Ausführung in eigener Prozessgruppe, `waitForChildProcess` als Zustandsautomat, Timeout-
> Klemmung 60/300 bzw. 600/86400 s, dreistufiger Abbruch, 100-ms-Update-Throttle,
> NOTAGENT_*-Env-Exporte, commandPrefix, austauschbare `BashOperations`), der Spawn-Teil von
> `utils/shell.ts`, `utils/child-process.ts`, `core/bash-executor.ts`, das Minify-Modul
> `core/mini_read/` (adaptiert aus der Rust-Referenz, natives tree-sitter, byte-genaue
> `src_map`), die Tools `read_minified`, `patch_minified` und `multi_patch_minified`,
> `core/todos/` (Store + Modell-Ausgabe + Transkript-Diff), `tools/todo-write.ts` und
> `tools/skill.ts`. Tests: 27 (bash) + 50 (Minify-Referenzmodule) + 7 (Multi-Edit inkl.
> deterministischem Testbench-Durchlauf über `edge_cases.rs`) + 17 (minified Tools) + 18 + 16
> (todos/skill).
> **Verbleibend aus der Task-Beschreibung:** der Managed-Pfad von bash (Auto-Backgrounding
> über den TaskManager) ist implementiert, aber gegen den `BashTaskManager`-Trait — die
> Verdrahtung an den echten Manager und die manager-gestützten Fälle von
> `bash-background.test.ts` folgen mit Task 10. `skill.ts` löst gegen Sichten auf, auf die
> Task 9 (`Mode`) und Task 11 (`Skill`) ihre echten Typen abbilden; `todos/reminder.ts`
> gehört zu Task 11. Alle `renderCall`/`renderResult`-Hälften gehören zu Task 13.
- [x] 9. Modes, Permissions, Hooks nativ portieren: `src/core/modes/modes.ts` + `shells.ts` + die vier builtin-Mode-Markdown-Verzeichnisse (Verzeichnis=Mode, Frontmatter shell/approval/tools/subagents, Delta-Regeln, Suchreihenfolge builtin→global→projekt, renderModeBlock als verstecktes custom-Message vor der nächsten User-Message), `src/core/permissions/` (chain.ts mit POLICY_ORDER aller 13 Policies in exakter Reihenfolge — destructive-command-ask VOR yolo —, coordinator, policies mit den vollständigen Regex-Listen für sensible Pfade und destruktive Kommandos, user-rules, request; ask→deny ohne TTY) als NATIVEN Pre-Tool-Gate im Agent-Loop (beforeToolCall-Hook von notagent-agent; die Extension-Indirektion aus permissions/extension.ts entfällt, Verhalten identisch), `src/core/hooks/` (events.ts mit allen 16 Hook-Namen, hooks.json-Discovery auf beiden Ebenen, Runner mit 30-s-Default/300-s-Cap, Matcher nur auf Tool-Events, NUR PreToolUse blockierend, UserPromptSubmit-Injektion als hook_context-Message, Laden VOR allem anderen) als native Event-Dispatch-Punkte gemäß der Mapping-Tabelle in extension-boundary §2.2 lesen und portieren. `src/core/project-trust.ts` + `trust-manager.ts` (ohne Extension-Übernahmepfad) mitportieren. Permissions- und Hooks-Testsuiten portieren. Rationale: sicherheitskritischer Kern, der in TS als versteckte Extensions realisiert ist — der native Nachbau ist die zentrale Strukturabweichung des Ports und muss zuerst durch Tests abgesichert werden.
- [x] 10. Tasks und Delegation auf tokio portieren: `src/core/tasks/` (manager.ts 616 — ID-Schema prefix-8-base36, Stati inkl. lost-Reconcile, dreistufiges Stoppen abort→5 s→forceStop, TaskStore mit atomarem rename + output.log, Vordergrund ohne Log bis Detach, TaskNotifier mit Unterdrückung, Manager pro sessionId mit Shutdown bei Wechsel; shell-task, subagent-task, store, notification, output, types) und `src/core/delegation/` (run.ts 324 + limits.ts 84 — Kind als neue Agent-Instanz mit geerbtem Provider-Wiring und EIGENER sessionId, Tool-Sperrliste für Kinder, kein Shell-Upgrade via exceedsParent, mode.subagents-Allowlist, Duplikat-Fingerprint, 2-h-Deadline via Env überschreibbar mit Fortsetzungsaufforderung statt Teiltext, 100k-Kürzung, 200-Zeichen-Minimum mit genau einem Expansion-Turn, WeakMap-Transkript-Cache → prozesslokale Map, runDelegation rejectet nie, Live-Token-Meldung) sowie `src/core/tools/task.ts` + `task-tools.ts` (max 8 parallel, session_id-Fortsetzung, run_in_background-Schema-Dynamik, task_list/task_output mit 32-KiB-Tail und Nie-Warten, task_stop mit Notification-Unterdrückung) lesen und portieren — jede Delegation und jeder Shell-Task als tokio::spawn mit CancellationToken, echte Parallelität. Testsuiten plus neuer Wanduhrzeit-Parallelitätstest. Rationale: Kernanforderung des Nutzers (Subagenten echt parallel via tokio); das TS-Design ist bereits sauber getrennt und lässt sich direkt auf Tasks abbilden.
> **Abschluss Task 10 (2026-08-14):** portiert und testbelegt sind `core/tasks/` vollständig
> (types, store mit atomarem Record-Schreiben und output.log, output mit Ring/Ceiling/
> verzögertem Persistieren, manager mit ID-Schema, lost-Reconcile, dreistufigem Stoppen,
> Auto-Backgrounding und Notification-Unterdrückung, shell-task, subagent-task,
> notification inkl. TaskNotifier und Compaction-Reminder), `core/delegation/`
> (limits und run: geerbtes Provider-Wiring, eigene sessionId, Tool-Sperrliste, 2-h-Deadline
> mit Env-Override, 100k-Kürzung, ein Expansions-Turn, Live-Token-Meldung) sowie die vier
> Tools `task`, `task_list`, `task_output`, `task_stop`. Damit ist auch `tools/index.ts` als
> Registry über alle 16 Tools geschlossen und der `BashTaskManager`-Trait aus Task 8 am
> echten Manager verdrahtet. Jede Delegation und jeder Shell-Task ist ein `tokio::spawn`;
> `tests/tasks_parallel.rs` belegt die Parallelität an der Wanduhr (8 × `sleep 0.5` detacht
> und 8 Kinder aus einem `task`-Aufruf, je in gut einer halben Sekunde, Spitzenparallelität 8).
> Testsuiten: 25 (task-manager) + 24 (task-tools inkl. Notification) + 22 (task-tool) +
> 20 (delegation-run) + 12 (delegation-limits) + 11 (bash-background am echten Manager) +
> 4 (Wanduhr-Parallelität).
> **Voraussetzung geklärt:** Interface-Request C-8 (Lesezugriff auf die Provider-Verdrahtung
> des `Agent`) ist von Workstream B umgesetzt; `create_child` kopiert alle neun TS-Felder.
> **Verbleibend aus der Task-Beschreibung:** die `renderCall`/`renderResult`-Hälften der vier
> Tools gehören zu Task 13. Der Manager je sessionId samt Shutdown beim Sitzungswechsel steht
> in TS in `agent-session.ts:1179-1267` und wird deshalb mit Task 11 verdrahtet; ebenso der
> TaskNotifier (Host-Trait steht) und der Aufruf von `active_task_reminder` in der Compaction.

- [x] 11. agent-session und Compaction portieren: `src/core/agent-session.ts` (3 797 LOC — die größte Datei: Bindung von Agent, SessionManager, Tools, Modes, Permissions-Gate, Hooks-Dispatch, Steering/Follow-up-Weiterleitung, Modell-/Thinking-Wechsel, Skill-Expansion, Bash-Integration, Retry-Logik mit _isRetryableError/_prepareRetry/abortRetry, Import/Export, Statistiken), `agent-session-runtime.ts` (441), `agent-session-services.ts` (ohne Extension-Provider-Registrierung), `src/core/compaction/` (1 513 — Trigger-Formeln, Cut-Point-Suche mit gültigen Entry-Typen und NIE-toolResult-Regel, Split-Turn-Prefix-Pfad, beide Summarization-Prompts wortgleich, maxTokens-Formeln 0.8/0.5×reserve, kumulative File-Operations, cacheRetention none + eigene sessionId, Overflow-Recovery mit Einmal-Retry und Message-Entfernung aus dem Agent-State), `src/core/system-prompt.ts` (173 — exakte Abschnittsfolge und Guideline-Deduplizierung), `src/core/model-resolver.ts` (769 — parseModelPattern mit Letzt-Doppelpunkt-Rekursion, Scope-Auflösung mit Alias-vor-datiert-Regel, Initialwahl-Prioritätskette), `model-runtime.ts` (787), `model-registry.ts`, `model-config.ts`, `provider-composer.ts` (572), `remote-catalog-provider.ts`, `runtime-credentials.ts`, `auth-guidance.ts`, `footer-data-provider.ts` (388, Git-Pfad-Erkennung), `slash-commands.ts`, `skills.ts` (487, Frontmatter-Validierungsregeln, Ignore-Files), `prompt-templates.ts` (285, bash-ähnliche Substitutionsgrammatik), `resource-loader.ts` (1 096 — AGENTS.md/CLAUDE.md-Kandidatenreihenfolge, Ancestor-Kette, Worktree-Shadowing; Extension-Teile entfallen), `todos/` (387), `core/event-bus.ts`-Ersatz entfällt (nur Extension-Backing) lesen und portieren. Die suite-Testinfrastruktur (`test/suite/harness.ts` mit faux-Provider) portieren und die agent-session-Testsuiten nachziehen. Rationale: das ist die Integrationsmitte der App; hängt auf den Kontrakt-Typen von B und allen vorherigen C-Tasks. (O-4: model-resolver, model-runtime, model-registry, model-config, provider-composer, remote-catalog-provider, runtime-credentials, auth-guidance sind an B übertragen — hier nur noch konsumieren)
> **Abschluss Task 11 (2026-08-14):** portiert und testbelegt sind `agent-session.ts`
> (Agent-Bindung mit Persistenz, Modes samt Mode-Block-Zustellung, Tool-Registry und
> System-Prompt-Neubau, Prompting mit Skill-/Template-Expansion, beide Queues,
> Modell-/Thinking-Wechsel, manuelle und automatische Compaction mit Overflow-Recovery,
> Retry mit Backoff, Bash mit Aufschub, Baum-Navigation, Statistiken, JSONL-Export,
> `reload`/`dispose`), `agent-session-runtime.ts`, `agent-session-services.ts`,
> `sdk.ts` (`createAgentSession`), `core/compaction/` vollständig, `system-prompt.ts`,
> `skills.ts`, `prompt-templates.ts`, `slash-commands.ts`, `resource-loader.ts`,
> `footer-data-provider.ts`, `todos/reminder.ts`, `session-cwd.ts`,
> `provider-attribution.ts`, `diagnostics.ts` (Ressourcen-Hälfte, vorgezogen aus
> Task 12) und `utils/fs-watch.ts`.
> **Extension-Entfernung:** der `ExtensionRunner` ist ersatzlos weg. `tool_call` ist der
> native `PermissionGate` aus Task 9, die Lebenszyklus-Emits sind `HookDispatcher`-
> Aufrufe an denselben Stellen (extension-boundary §2.2), und Message-Rewriting,
> Input-Transformation, `resources_discover`, die Compaction-/Tree-Cancel-Punkte sowie
> die 29 Bind-Actions entfallen mit den Extensions.
> **Schnittstellen:** C-9 (fs-watch unter utils/), C-10 (inkrementelle Kompilierung aus,
> Plattenplatz), C-11 (`PackageResources` für Bs Package-Manager — offen, blockiert nur
> die Auto-Entdeckung im Binary), C-12 (auth-guidance: B hatte sie schon), C-13
> (`SessionModelRuntime` — von C selbst gelöst, Bs `ModelRuntime` bleibt unberührt).
> **Tests:** 148 neue Tests — skills 29, prompt-templates 54, system-prompt 8, todo-
> reminder 7, resource-loader 32, footer-data-provider 14, compaction 27 + 6,
> session-cwd 3, provider-attribution 2, auth-guidance (B), sowie 56 über die
> portierte suite-Harness gegen den Faux-Provider: prompt 12, queue 11, retry/events 8,
> bash 7, mode-block 6, compaction/tree/stats 12. Die Ereignisreihenfolge eines Prompts
> stimmt Element für Element mit der TS-Erwartung überein.
> **Verbleibend:** die Auto-Entdeckung der Standard-Ressourcenverzeichnisse hängt an
> C-11; `exportToHtml` gehört mit O-4 zu B; `renderCall`/`renderResult` und die
> Interactive-Verdrahtung gehören zu Task 13.

- [x] 12. CLI und Headless-Modi portieren: `src/cli/args.ts` (435 — vollständige Flag-Tabelle minus Extension-Flags, @pfad-Args, unknownFlags jetzt = Fehler statt Extension-Weiterleitung, Help-Text mit Env-Var-Liste), `src/main.ts` (1 062 — Erkennungsreihenfolge auth→package→config→parseArgs, resolveAppMode-Regeln inkl. TTY-Erkennung und stdin-Pipe-Umschaltung, Sonderpfade version/export/help/list-models/Startup-Benchmark, takeOverStdout), `src/cli/` (auth-command mit check/print-api-key/print-bearer-token und Exit-Codes, auth-check, credential-print, session-picker, startup-ui, file-processor mit file-Tag-Einbettung und Bild-Anhang, initial-message, list-models, project-trust), `src/modes/print-mode.ts` (169) und `json-event.ts` (40), `src/modes/rpc/` (1 765 — JSONL-über-stdio-Protokoll: rpc-mode, rpc-client, rpc-types ohne Extension-UI-Typen, jsonl-Framing), `src/core/output-guard.ts`, `src/core/diagnostics.ts`, `src/core/timings.ts`, `src/core/experimental.ts` lesen und portieren; `src/bun/` entfällt (dokumentiert). CLI-Testsuiten portieren; damit Gate G2 (Headless-E2E gegen faux) herstellen. Rationale: ab hier ist die App ohne TUI vollständig nutzbar und end-to-end testbar. (Abschluss 2026-08-15: `cli.ts`, `main.ts` als `main_app.rs`, alle Auth-Kommandos, file-processor, initial-message, list-models, output-guard, timings, json-event, print-mode und `rpc/` sind portiert; `rpc-client.ts` ist als Testwerkzeug dokumentiert ausgeschlossen. Nicht enthalten, weil an der TUI-Renderschleife hängend und deshalb mit Task 13 fällig: die Dialoghälfte von `startup-ui.ts`, `session-picker.ts` und die `select`-Hälfte von `cli/project-trust.ts` — Grund und Kontrakt-Wunsch in Interface-Request C-14. `--export` und `handlePackageCommand` sind nach dem Rebase auf Bs Tasks 15/16 verdrahtet; `handleConfigCommand` öffnet die Ressourcen-TUI und wartet auf C-14.)
- [x] 13. Interactive-Mode portieren (gemeinsam mit Workstream A ab Gate G2; Komponenten-Zuteilung via plans/interface-requests.md): `src/core/keybindings.ts` ist mit Interface-Request O-2 an Workstream A übergegangen und entfällt hier (A hat es mit A-10 geliefert; die Verdrahtung im Interactive-Mode bleibt bei C), `src/modes/interactive/interactive-mode.ts` (6 688 — Hauptverdrahtung: Editor-Submit-Pfade, alle Slash-Commands aus der Faktenliste inkl. /debug und Easter Eggs, Bash-Modus mit !-/!!-Semantik und Border-Umschaltung, Queueing steer/followUp/dequeue mit Compaction-Sonderpfad, Autocomplete-Anbindung mit /model- und /login-Argument-Completions, externer Editor Ctrl+G, Doppel-Escape-Aktion, Selectors-Aufrufe, Footer-Updates, Fullscreen/Regular-Umschaltung mit Exit-Verhalten), `external-editor.ts`, `model-search.ts` sowie die von C zu verdrahtenden Komponenten aus A-Task 15 lesen und portieren; `utils/` -Reste (clipboard-image, syntax-highlight via tree-sitter-highlight auf die 9 Theme-Slots, git-Utilities, Pfad-Utilities) portieren. Interactive-E2E-Szenarien über das virtuelle Terminal aus notagent-tui aufsetzen (Startup, Prompt-Roundtrip mit faux, Tool-Anzeige, Selector-Bedienung, Theme-Wechsel, Resize). Rationale: die nutzersichtbare Oberfläche; klar getrennt von der Headless-Logik, daher nach G2 risikoarm integrierbar.
>
> **Abschluss Task 13 (2026-08-16):** `interactive-mode.ts` ist in fünf einzeln gemergten
> Scheiben portiert — Einstiegspunkt mit Terminal-Naht (A-23) und Hauptschleife, Slash-Command-
> Tabelle inklusive `handleConfigCommand`, Bash-Modus mit Queueing und Compaction-Sonderpfad,
> Autocomplete und sämtliche Selektoren (Settings, Model, Scoped-Models, Fork, Clone, Tree,
> Trust, Session, Tasks, Login/Logout, Approval), Footer- und Panel-Aktualisierung, Fullscreen-
> Umschaltung und Doppel-Escape. Dazu die `utils/`-Reste dieser Task: `clipboard.rs`,
> `clipboard_image.rs` und `changelog.rs` (O-12 Punkt 3). `main_app` startet den Modus über
> `run_until` und bindet `permissionPresent` und `hookReport` an ihn.
> **Tests:** 30 Fälle in `tests/interactive_mode_wiring.rs` über das virtuelle Terminal am
> App-Runtime der G2-Suiten; die G3-Szenarien selbst liegen bei Workstream A
> (`tests/g3_interactive_e2e.rs`), die den Einstiegspunkt jetzt nutzen können.
> **Offen und im Ledger dokumentiert:** `maybeSaveImplicitProjectTrustAfterReload`, der
> `MissingSessionCwdError`-Zweig von `/import`/`/resume`, die dritte Baum-Antwort (C-23) und die
> Signal-Handler, die ins Binary gehören.

- [x] 14. llama.cpp nativ nachbauen und Package-Manager portieren: `src/extensions/llama/` (1 410 LOC: provider.ts — nativer Provider für lokale llama.cpp-Server über die OpenAI-Completions-API mit refreshModels; client.ts, huggingface.ts, ui.ts — /llama-Command-TUI mit Modell-Laden/-Entladen/HuggingFace-Suche/Download) lesen und als native Teile portieren (Provider-Registrierung direkt im Provider-Composer statt registerProvider; /llama als eingebauter Slash-Command); `src/core/package-manager.ts` (2 677) und `src/package-manager-cli.ts` (889) für die verbleibenden Ressourcentypen skills/prompts/themes portieren (Extension-Ressourcentyp entfällt; install/remove/update/list/config bleiben inkl. -l/--force und update-Targets self/models), `src/core/notagent-manifest.ts`/`source-info.ts`/`resolve-config-value.ts` soweit von skills/prompts/themes gebraucht. Tests mitportieren. Rationale: llama.cpp und das Ressourcen-Management sind nutzersichtbarer Funktionsumfang, der laut Faktenlage NICHT mit dem Extension-System entfallen darf. (O-4: package-manager.ts und package-manager-cli.ts sind an B übertragen; llama bleibt hier)

> **Abschluss Task 14 (2026-08-16):** Der Package-Manager-Teil liegt seit B-Task 15 auf main
> (`core/package_manager.rs`, `package_manager_cli.rs`, 118 Tests), `handleConfigCommand`
> hat C mit Task 13 nachgezogen. Neu in dieser Task ist die llama-Hälfte, die O-8 bei C
> gelassen hat: `ui.ts` als `modes/interactive/components/llama.rs` (Modellliste,
> Auswahl- und Bestätigungsdialoge, Hugging-Face-Suche mit Debounce und Abbruch,
> Fortschrittsanzeige) und `index.ts` als `modes/interactive/llama_command.rs` (Katalog-Sync
> über den Provider-Controller, Laden mit Ersetzen-Rückfrage und Wiederherstellung,
> Entladen, Download samt Quantisierungswahl und Gated-Hinweis, `runWithProgress`).
> Verdrahtet ist es als eingebauter Slash-Command `/llama` samt Autocomplete-Eintrag; die
> Provider-Registrierung, die in TS `registerProvider` der versteckten Extension war, macht
> jetzt `create_agent_session_services` nativ.
> **Tests:** 12 Fälle in `tests/llama_command.rs` (Komponenten, Fluss gegen den
> Loopback-Server) und 3 in `tests/interactive_mode_wiring.rs` (Manager öffnen und schließen,
> `/login`-Hinweis ohne Credential, native Provider-Registrierung) — darunter die native Form
> des ersten TS-Falls aus `test/llama-extension.test.ts`, den B ausgenommen hatte.
- [x] 15. Rest-Features portieren: `src/core/export-html/` (746 — HTML-Export mit Template und ANSI→HTML; die Vendor-JS-Dateien marked.min.js/highlight.min.js werden als statische Assets übernommen, da sie im exportierten HTML clientseitig laufen), /share (GitHub-Gist, Viewer-URL-Env), JSONL-Export/Import, `src/utils/image-*.ts` + `photon.ts`-Ersatz durch image-Crate + `exif-orientation.ts`, Mermaid-Rendering (`components/mermaid.ts`, grok-mermaid-Ersatzentscheidung: Port des genutzten Funktionsumfangs, Modi off/final/streaming), `src/core/cache-stats.ts`, `usage-totals.ts`, `src/core/telemetry.ts` (Install-Ping, opt-out), `utils/version-check.ts` + Self-Update semantisch äquivalent für die Rust-Distribution (dokumentierte Substitution), `src/index.ts`-Äquivalent als lib-Oberfläche des Bin-Crates (SDK-Nutzung ohne Extension-Factories) lesen und portieren; verbleibende Testsuiten (Ziel äquivalent zu ~53 600 TS-Test-LOC inkl. test/suite/regressions/) portieren. Rationale: komplettiert den Funktionsumfang bis zur G4-Parität. (O-4: export-html, share, version-check/self-update, Install-Telemetrie, cache-stats, usage-totals sind an B übertragen; Bilder/Mermaid/lib-Oberfläche bleiben hier)

> **Abschluss Task 15 (2026-08-16):** Der an B übertragene Teil (export-html, /share, JSONL-Export,
> version-check/self-update, Telemetrie, cache-stats, usage-totals) liegt seit B-Task 16 auf main,
> Mermaid seit A-Task 15 (O-6), die lib-Oberfläche (`src/index.ts` → `lib.rs`) ist mit C-24
> gebucht. Neu in dieser Task: `utils/tool-result-images.ts` samt der Stelle, an der TS es
> aufruft — das `afterToolCall` von `_installAgentHooks`, das im Port bis dahin ganz fehlte und
> mit ihm auch der `PostToolUse`-Dispatch der Session; `clankolas.png` ist als Asset übernommen
> und einkompiliert; die Bildzeile des Ledgers ist auf `verifiziert` gehoben.
> Dazu die drei bei Task 13 offen dokumentierten Zweige: `maybeSaveImplicitProjectTrustAfterReload`,
> der `MissingSessionCwdError`-Zweig von `/import` und `/resume` (mit dem dafür nötigen
> typisierten `SessionOpenError` statt der Zeichenkette) und die Signal-Handler, die als
> SIGTERM/SIGHUP-Task des Binaries auf einen Shutdown-Token laufen, den die Modus-Schleife
> abwartet.
> **Tests:** 7 portierte Fälle in `tests/tool_result_images.rs`, 2 in
> `tests/agent_session_tool_result_images.rs`, 3 neue in `tests/interactive_mode_wiring.rs`
> (implizites Projektvertrauen nach `/reload`, `/import` einer Sitzung ohne existierendes cwd,
> Shutdown-Token) und die erweiterte Bildprüfung in `tests/panels.rs`.
- [x] 16. Gate-Abnahmen und Abschluss-Audit durchführen: G1/G2/G3/G4-Kriterien aus dem Master prüfen und taggen; an G4 den Smoke-Test analog TS-Release-Prozedur ausführen (--help, --version, --list-models, -p gegen faux und einen echten Provider des Nutzers, interaktive Session in tmux mit Prompt-Roundtrip) und in plans/g4-smoke-report.md protokollieren; Abschluss-Drift-Audit über alle Ledger (jede TS-src-Datei erfasst) in plans/final-parity-audit.md; PARITY.md aller fünf C-Crates finalisieren. Rationale: messbarer Abschluss gemäß Master-Plan Task 9 und 10.

> **Abschluss Task 16 (2026-08-16):** G3 und G4 sind in einem Durchgang abgenommen, weil
> Workstream A seine G3-Szenarien und die C-23-Komponente zusammen geliefert hat.
> **G3:** `tests/g3_interactive_e2e.rs` fährt 20 Fälle über das virtuelle Terminal, kein
> `#[ignore]` mehr; die letzte offene Verdrahtungsstelle (dritte Baum-Antwort
> „Summarize with custom prompt") liegt auf As `text_input_dialog.rs`. Tag `gate-g3`.
> **G4:** `scripts/check.sh` grün auf main (3 861 Tests in 249 Suiten), Smoke-Test in
> `plans/g4-smoke-report.md` (12 Schritte über die gebaute Binary — Hilfe, Version,
> Modelle, Katalog-Refresh, Print, JSON, interaktiv gegen einen lokalen llama.cpp-Router
> **und** gegen einen echten Provider, `/llama`), Abschluss-Audit in
> `plans/final-parity-audit.md` (0 Dateien ohne Ledger-Spur, 544 `verifiziert`,
> 85 Ausschlüsse, 3 dokumentierte Prüfobergrenzen von A). Tag `gate-g4`.
> **Im Audit nachgezogen:** 63 Ledger-Zeilen von `portiert` auf `verifiziert` mit
> benannter Suite, zwei Extension-Hüllen in die Ausschluss-Tabelle, sechs überholte
> Doppel-Zeilen aufgelöst, `createToolHtmlRenderer` (B-8) portiert und verdrahtet, dazu
> die beiden Dateien ohne jede Suite (`cli/file-processor.ts`, `core/output-guard.ts`)
> mit neuen Tests belegt.

## Verification Criteria

- Gate-Kriterien G0-G4 aus dem Master-Plan sind erfüllt und als Tags markiert; scripts/check.sh läuft an jedem Gate grün.
- Protokoll: portierte CBOR-/Framing-/Server-/Client-Konformanztests grün; Rust-Client↔Rust-Server-Roundtrip über Unix-Socket mit Version-1-Handshake, Lease-Verhalten und Progress-Events.
- Sessions: Roundtrip-Tests mit echten TS-Session-JSONL-Fixtures (öffnen, fortsetzen, forken, branchen, re-serialisieren) verlustfrei; Kontextaufbau nach Compaction identisch zur TS-Erwartung.
- Tools: alle 16 Tools bestehen die portierten Suiten; edit-Fuzzy-Fälle, bash-Auto-Backgrounding, grep/find-Integration mit echten fd/rg-Binaries und todo_write-Replace-Semantik einzeln nachgewiesen; read_minified/patch_minified bestehen TS-Tests plus Minify-Testbench.
- Permissions: die 13 Policies feuern in exakter Reihenfolge (Test je Policy-Übergang, insbesondere destruktiv-vor-yolo und ask→deny ohne TTY); Hooks: alle 16 Events mit Payload-Fixtures, nur PreToolUse blockiert.
- Subagenten: Wanduhrzeit-Test belegt parallele tokio-Ausführung; alle Delegations-Regeln (max 8, Sperrliste, kein Upgrade, Deadline-Verhalten, Antwort-Caps, ein Expansion-Turn, Fortsetzung per session_id) testbelegt.
- CLI: Flag- und Subcommand-Parität (Faktenliste minus Extension-Flags) durch Tests; Print/JSON/RPC-Modi end-to-end gegen faux.
- Interactive: E2E-Szenarien über das virtuelle Terminal grün; alle Slash-Commands und die 45 Keybinding-Actions vorhanden; /llama-Command funktional gegen einen gemockten llama.cpp-Server.
- Ledger aller fünf C-Crates vollständig; Ausschlüsse (Extension-System, evals, bun/, cli/experimental, create-harness, RPC-Extension-UI-Typen) dokumentiert.

## Potential Risks and Mitigations

1. **agent-session.ts (3 797 LOC) ist zu verflochten für einen Stück-Port**
   - Impact: Task 11 stockt, Integrationsfehler häufen sich
   - Likelihood: Mittel bis hoch
   - Mitigation: Vollständige Lektüre vor Beginn; Port entlang der TS-Methodenstruktur mit der suite-Test-Harness als Sicherheitsnetz nach jedem Abschnitt; die vorher fertigen Bausteine (Tools, Modes, Permissions, Tasks, Compaction) sind bereits einzeln getestet
   - Contingency: temporäre Feature-Flags im Bin-Crate, um Teilpfade (z. B. ohne Compaction) e2e zu testen, vor G2 wieder entfernt

2. **Extension-Entkopplung hinterlässt Lücken (versteckte Aufrufstellen)**
   - Impact: fehlende Kernfunktion fällt erst spät auf
   - Likelihood: Mittel
   - Mitigation: die Integrationspunkt-Liste in extension-boundary §3 wird als Checkliste abgearbeitet — jeder der dort genannten Dispatch-Punkte wird entweder nativ ersetzt (Permissions, Hooks, llama) oder als entfallend abgehakt
   - Contingency: Nach-Port einzelner Punkte; die Liste ist vollständig (per Grep erhoben)

3. **Prozessbaum-Handling von bash (detach, kill, Windows) plattformspezifisch fehlerhaft**
   - Impact: hängende Kindprozesse, kaputtes Auto-Backgrounding
   - Likelihood: Mittel
   - Mitigation: Unix zuerst (setsid/Prozessgruppen, kill auf Gruppen-PID), portierte bash-Tests mit langen/forkeneden Kommandos; Windows-Pfad (taskkill-Äquivalent, Git-Bash-Suche) separat mit cfg-Tests
   - Contingency: Windows-Support als dokumentierter Folgeschritt nach G4, falls die Plattform in der Testumgebung fehlt — Entscheidung liegt beim Nutzer und wird nicht stillschweigend getroffen

4. **RPC-JSONL-Protokoll driftet mangels externem Konsumenten unbemerkt**
   - Impact: Fremd-Clients der TS-App brechen
   - Likelihood: Niedrig
   - Mitigation: rpc-client.ts wird mitportiert und die TS-RPC-Tests laufen Client-gegen-Server im selben Prozess; Wire-Fixtures aus der TS-Suite als Golden Files

## Alternative Approaches

1. **App zuerst, Protokoll-Crates zuletzt**: verworfen — die vier kleinen Pakete sind während der Wartezeit auf die B-Kontrakte die einzigen konfliktfrei portierbaren Blöcke und liefern früh Konformanz-Erfolge.
2. **Permissions/Hooks als generisches internes Event-Bus-System nachbauen** (Extension-Architektur ohne Loader): verworfen — mehr Indirektion ohne Nutzen; der native Pre-Tool-Gate und direkte Dispatch-Punkte sind einfacher und verhaltensgleich; die Extension-Event-Semantik bleibt nur dort erhalten, wo Kernfeatures sie brauchen.
3. **PTY für das bash-Tool einführen**: verworfen — die TS-App nutzt bewusst kein PTY (pipes + detached); ein PTY würde beobachtbares Verhalten ändern (TTY-Detection in Kindprozessen).
4. **Interactive-Mode komplett bei Workstream A**: verworfen — die Hauptschleife hängt an agent-session-Zustand und Slash-Command-Dispatch (C-Domäne); nur die Komponenten wandern zu A, die Verdrahtung bleibt bei C.
