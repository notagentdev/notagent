# Rust-Port des notagent-Monorepos (Master-Plan)

## Objective

Vollständiger 1:1-Port des TypeScript-Monorepos `/Users/dev/projects/notagent-main` nach Rust — mit exakt dem Funktionsumfang der TS-App, **ohne das Extension-System**. Endergebnis ist eine komplett lauffähige Binary `notagent` (interaktiver Coding-Agent mit differenzieller TUI-Rendering-Engine, Multi-Provider-LLM-Layer, Agent-Loop, Subagenten, Sessions, Permissions, Hooks) plus die Library-Crates der übrigen Pakete. Subagenten laufen als echte parallele tokio-Tasks. Drei Agenten (A: TUI, B: AI+Agent, C: App) arbeiten parallel in Git-Worktrees; Gates und eine Parity-Ledger-Drift-Kontrolle sichern, dass nichts vom TS-Verhalten abweicht.

Die vollständige Faktenbasis liegt in `plans/facts/` (tui.md, ai-and-agent.md, coding-agent-core.md, extension-boundary.md, protocol-server-client-sqlite-telemetry-evals.md, rust-minify-reference.md). Diese Dokumente wurden direkt aus dem Quellcode erhoben und sind die Landkarte des Ports. **Sie ersetzen nicht das Lesen der TS-Quellen: Jeder ausführende Agent MUSS die in seinen Tasks genannten TS-Dateien vollständig lesen und verstehen, bevor er portiert.**

## Scope

**Portiert wird (Crate ← TS-Paket):**

| Crate | TS-Quelle | src-LOC | Workstream |
|---|---|---|---|
| `crates/notagent-tui` | packages/tui | 16 704 | A |
| `crates/notagent-telemetry` | packages/telemetry | 935 | B |
| `crates/notagent-ai` | packages/ai | 22 744 | B |
| `crates/notagent-agent` | packages/agent (funktionaler Kern) | ~2 400 | B |
| `crates/notagent-protocol` | packages/protocol | 1 236 | C |
| `crates/notagent-client` | packages/client | 1 225 | C |
| `crates/notagent-server` | packages/server | 2 299 | C |
| `crates/notagent-session-sqlite` | packages/session-backends/sqlite-node | 2 505 | C |
| `crates/notagent` (bin) | packages/coding-agent | 68 856 | C (TUI-Teile mit A) |

**Ausgeschlossen (faktenbasiert begründet, siehe plans/facts/):**

| Ausschluss | Begründung (Faktenlage) |
|---|---|
| Extension-System komplett | Nutzer-Vorgabe. Umfasst `core/extensions/` (3 929 LOC), `src/extensions/` als Extension-Loader, jiti, VIRTUAL_MODULES, `--extension`/`-e`/`--no-extensions`-Flags, Extension-Ressourcentyp im Package-Manager, RPC-Extension-UI-Wire-Typen, `docs/extensions.md`, `examples/extensions/` |
| **ABER nativ nachzubauen** | Permissions-System, Hooks-System und llama.cpp-Provider samt /llama-Command sind intern als versteckte Built-in-Extensions implementiert (`main.ts:638-643`) — für Nutzer sind das Kernfeatures. Siehe `plans/facts/extension-boundary.md` §2 |
| packages/evals | `private: true`, reines Dev-Tool, nicht publiziert, nicht Teil der App |
| AgentHarness + create-harness.ts | `harness/agent-harness.ts` ist ein Stub (Methoden werfen HarnessNotImplemented); `src/server/create-harness.ts` wird nur vom eigenen Test konsumiert, nicht vom Laufzeitpfad |
| agent-Paket harness-Submodule ohne App-Konsument | coding-agent importiert aus agent-core nur: Agent, AgentMessage, AgentState, AgentTool, CustomMessage, setDefaultStreamFn, StreamFn, ThinkingLevel, uuidv7 (verifiziert per Import-Grep). Eigene Implementierungen für Compaction/Tools/Sessions/Skills liegen im coding-agent selbst |
| compat.ts / legacy-api-aliases.ts in ai | Im Code explizit als Legacy markiert („wird mit ModelManager-Migration gelöscht") |
| Deferred-Responses-Laufzeitpfad | Nur der faux-Test-Provider implementiert fetchDeferred/cancelDeferred; kein echtes API-Modul. Typen werden portiert (faux braucht sie für Tests) |
| npm/Bun-Distributionsmechanik | Release-Skripte, shrinkwrap, bun build; Rust liefert von Haus aus eine Single-Binary. Self-Update-Kommando wird semantisch äquivalent, aber Rust-spezifisch umgesetzt |
| cli/experimental/ | Nur in Tests referenziert, nicht in main.ts verdrahtet |

**Tech-Substitutionen (verbindliche Entscheidungen; jede weitere Substitution ist Drift):**

| TS | Rust |
|---|---|
| Node/TS-Runtime | Rust stable (rust-toolchain.toml, Edition 2024), tokio (multi-thread) |
| TypeBox-Schemas | Tool-Parameter als statische serde_json-JSON-Schema-Werte, byte-identisch zum TypeBox-Output (per Fixture-Vergleich aus dem TS-Repo verifiziert); Validierung + Coercion als Port von `packages/ai/src/utils/validation.ts` |
| Provider-SDKs (anthropic, openai, google) | reqwest (rustls) + selbst portierte SSE-Parser (der Anthropic-SSE-Decoder ist ohnehin handgeschrieben); WebSocket via tokio-tungstenite |
| @aws-sdk/client-bedrock-runtime | aws-sdk-bedrockruntime + aws-config (offizielles Rust-SDK) |
| partial-json | eigener Port von parseStreamingJson inkl. repairJson |
| Intl.Segmenter | unicode-segmentation (Graphem + Wort) |
| get-east-asian-width | Port von graphemeWidth 1:1; EAW-Fallback über unicode-width bzw. eigene Tabelle; TS-Tests (tab-width, regional-indicator, wrap-ansi, truncate) sind das Oracle |
| marked | Markdown-Renderer-Port; Tokenisierung wahlweise pulldown-cmark-basiert oder eigener Lexer nach marked-Tokenstrom — Entscheidung fällt WS-A nach Lesen von `packages/tui/src/components/markdown.ts` und der 1 667-LOC-Testsuite; Oracle sind die portierten Tests |
| tree-sitter-wasm | **native tree-sitter-Crates, byte-genau (Nutzer-Vorgabe)**; Versionen und Referenz-Implementierung aus `/Users/dev/projects/notagent-main-rust` (siehe `plans/facts/rust-minify-reference.md`) |
| highlight.js | tree-sitter-highlight, gemappt auf die 9 Syntax-Theme-Slots |
| photon WASM | image-Crate (Resize, EXIF-Orientierung, Formatkonvertierung) |
| diff (jsdiff) | similar-Crate für Unified-Patches; Display-Diff-Format als 1:1-Port von `generateDiffString` |
| glob/minimatch | globset; ignore-Paket → ignore-Crate |
| proper-lockfile | fs4/fd-lock-basiertes Advisory-Locking mit identischer Retry-Semantik (10 × 20 ms) |
| node:sqlite | rusqlite (bundled) |
| undici/Proxy-Agents | reqwest-Proxy-Konfiguration (HTTP_PROXY/HTTPS_PROXY/NO_PROXY) |
| WebCrypto (PKCE) | sha2 + base64 + rand |
| node:http-Callback-Server (OAuth) | minimaler HTTP-Server auf tokio (hyper oder handgerollt, eine Route) |
| native Addons (darwin-modifiers, win32-console-mode) | direkte OS-Aufrufe in Rust unter cfg(target_os), Port der C-Quellen in `packages/tui/native/` |
| @xterm/headless (Test-Terminal) | Rust-VT-Emulator-Crate (Kandidaten: vt100, avt, wezterm-term); Akzeptanzkriterien: Viewport-Zeilen, Scrollback, Cursorposition, Resize. Entscheidung WS-A Task 1 |
| chalk | direkte ANSI-Sequenzen (das Theme-System erzeugt ANSI ohnehin selbst) |
| AbortController/AbortSignal | tokio_util CancellationToken; Abort-Semantik (Prüfpunkte an Schleifengrenzen, Signal-Durchreichung an Tools) 1:1 |
| Kryptische Details (uuidv7, base36-IDs, 8-Hex-Entry-IDs) | uuid-Crate (v7) bzw. 1:1-Port der ID-Generatoren |

## Architektur und Nebenläufigkeit

- Cargo-Workspace mit `members = crates/*` (Glob!), damit jeder Workstream Crates hinzufügen kann, ohne die Root-Cargo.toml anzufassen — das eliminiert die einzige strukturelle Merge-Konfliktquelle.
- Abhängigkeitsgraph identisch zum TS-Monorepo: telemetry ← ai ← agent ← notagent(bin); tui ← notagent(bin); protocol ← client, server; agent ← session-sqlite.
- **Subagenten**: jede Delegation wird ein tokio-Task (tokio::spawn auf Multi-Thread-Runtime) mit eigenem CancellationToken — echte Parallelität statt JS-Event-Loop-Interleaving. Der TaskManager verwaltet Shell- und Subagent-Tasks über denselben BackgroundTask-Vertrag wie in TS (`core/tasks/`). Tool-Parallelausführung im Agent-Loop über JoinSet mit Erhalt der TS-Semantik: sequentieller Preflight, tool_execution_end in Abschlussreihenfolge, Result-Messages in Assistant-Quellreihenfolge.
- **TUI**: Rendering-Kern synchron (render liefert Zeilen-Strings mit eingebetteten ANSI-Sequenzen — KEINE Zell-Buffer-Umarchitektur, siehe `plans/facts/tui.md` Fallstrick 1); Scheduling (16-ms-Throttle, Immediate-Render nach Tastatur) über tokio-Timer; stdin-Reader als eigener Task.
- Streaming: das in TS in-place mutierte `partial`-Objekt wird in Rust ein eigener Streaming-State pro Nachricht, aus dem Event-Snapshots erzeugt werden; Scratch-Felder (partialJson etc.) leben im Streaming-State, nie in den persistierten Content-Typen.

## Parallelisierung: Workstreams, Worktrees, Gates

**Workstreams** (Detailpläne mit abhakbaren Tasks):
- WS-A — `plans/2026-08-13-rust-port-ws-a-tui-v1.md`: notagent-tui komplett; ab Gate G2 zusätzlich die TUI-nahen App-Teile (Theme-System, App-Keybindings, interactive-Komponenten) in Zuarbeit für C.
- WS-B — `plans/2026-08-13-rust-port-ws-b-ai-agent-v1.md`: notagent-telemetry, notagent-ai, notagent-agent.
- WS-C — `plans/2026-08-13-rust-port-ws-c-app-v1.md`: Workspace-Scaffold (G0), notagent-protocol/-client/-server/-session-sqlite, dann die App (notagent-Bin).

**Worktree-Strategie** (Entscheidung: ja, Worktrees):
- Repo: `/Users/dev/projects/notagent-main-v2` (main). Der Orchestrator (Mensch) startet Agent C zuerst; C erledigt Task C0 (Scaffold) und merged ihn nach main. Danach starten A und B.
- Jeder Agent arbeitet in einem eigenen Worktree mit eigenem Branch: `ws/a-tui`, `ws/b-ai-agent`, `ws/c-app` (Anlage per git worktree add, Pfade `../notagent-main-v2-wt-a` usw. — genaue Kommandos stehen in den Prompts).
- Disjunkte Ownership: A besitzt ausschließlich `crates/notagent-tui/` (+ eigenes Ledger/Planfile), B `crates/notagent-{telemetry,ai,agent}/`, C alles übrige inkl. Root-Dateien. Niemand editiert fremde Crates; Schnittstellenwünsche werden als Issue-Notiz in `plans/interface-requests.md` (append-only, eigene Sektion pro Workstream) hinterlegt und vom Owner umgesetzt.
- Merge-Disziplin: nach jedem abgeschlossenen Task auf den eigenen Branch committen, main in den Branch rebasen/mergen, Workspace-Build prüfen, dann fast-forward-artig nach main mergen. Kleine, häufige Merges statt großer Integrationen.

**Gates** (harte Synchronisationspunkte; ein Gate gilt als bestanden, wenn seine Kriterien erfüllt und als Git-Tag markiert sind):
- **G0 — Scaffold** (C): Workspace mit Glob-Members, rust-toolchain.toml, .gitignore, leere Crate-Stubs, `scripts/check.sh` (fmt + clippy -D warnings + test), `CONVENTIONS.md`, Ledger-Vorlage. Kriterium: cargo build + check.sh grün auf leerem Workspace; Tag `gate-g0`.
- **G1 — Fundamente**: A: utils/keys/Terminal/TuiBase/Main-Screen-Renderer mit Virtual-Terminal-Tests grün. B: ai-Kerntypen, EventStream, Anthropic- + OpenAI-Completions-Streaming, faux-Provider, Agent-Loop mit portierten Loop-Tests grün. C: protocol/client/server/session-sqlite fertig (Konformanztests portiert), session-manager + Basistools (read/write/edit/ls) grün. Tag `gate-g1`.
- **G2 — Headless-Parität**: Print-Modus end-to-end gegen faux-Provider (Prompt → Tool-Calls → Antwort), komplette Tool-Suite grün, Subagenten-Test mit nachweislich paralleler Ausführung (Zeitmessung: N Tasks ≪ N × Einzeldauer), Permissions-Policy-Chain-Tests grün. Tag `gate-g2`.
- **G3 — Interaktive Parität**: Interactive-Mode vollständig verdrahtet (Editor, Slash-Commands, Selectors, Themes, Keybindings, Footer, Overlays), End-to-End-Szenarien über das virtuelle Terminal grün. Tag `gate-g3`.
- **G4 — Release-Parität**: alle portierten Testsuiten grün, Parity-Ledger aller Crates vollständig, Feature-Checkliste gegen `plans/facts/` abgehakt, manueller Smoke-Test (echtes Terminal, notagent --help/--version/--list-models/-p/interaktiv) dokumentiert. Tag `gate-g4`.

## Drift-Kontrolle (verbindlich für alle Agenten)

- **Read-before-Port**: Jeder Task nennt die TS-Quelldateien. Der Agent liest sie VOLLSTÄNDIG (nicht nur die Faktenberichte) und trägt im Ledger ein, welche Dateien mit welcher LOC-Zahl gelesen wurden. Portieren ohne vollständige Lektüre ist verboten.
- **Parity-Ledger**: pro Crate eine Datei `crates/<name>/PARITY.md` mit einer Zeile je TS-Quelldatei: TS-Pfad → Rust-Modul → Status (gelesen / portiert / Tests portiert / verifiziert) → Abweichungen. Ein Task ist erst fertig, wenn seine Ledger-Zeilen vollständig sind.
- **Erlaubte Abweichungsklassen** (alles andere ist Drift und muss korrigiert werden): 1. Sprachidiomatik ohne Verhaltensänderung (Result statt Exceptions, Ownership statt Referenz-Mutation — beobachtbares Verhalten identisch). 2. Extension-Entfernung exakt nach `plans/facts/extension-boundary.md`. 3. Tech-Substitutionen aus der Master-Tabelle. 4. Distributionsmechanik. Jede genutzte Abweichung wird im Ledger mit Klasse und Begründung notiert.
- **Tests sind das Oracle**: Die TS-Testsuiten (~117 000 LOC) definieren das Verhalten und werden mitportiert (gleiche Fälle, gleiche Erwartungswerte). Bei Unklarheit über TS-Verhalten: TS-Quelle erneut lesen, notfalls die TS-Tests in `/Users/dev/projects/notagent-main` ausführen oder Verhalten per Node-REPL beobachten — **niemals raten, niemals annehmen**.
- **Bug-Kompatibilität**: Gefundene TS-Bugs werden repliziert (Verhalten identisch) und im Ledger als bug-compat markiert — nicht „verbessert".
- **Keine Erweiterungen**: keine zusätzlichen Features, Flags, Konfigurationen oder „Verbesserungen".
- **Gate-Selbstaudit**: an jedem Gate prüft jeder Agent sein Ledger auf Vollständigkeit (jede TS-Datei seines Pakets kommt vor) und hakt die zugehörigen Tasks in seinem Plan-File ab.

## Implementation Plan

- [x] 1. Gate G0 — Workspace-Scaffold durch Agent C anlegen und nach main mergen: Cargo-Workspace mit Glob-Members für crates/*, rust-toolchain.toml (lokal installiertes stable, per rustc --version ermittelt), Workspace-weite Dependency-Versionen (workspace.dependencies) inklusive der tree-sitter-Versionen aus `plans/facts/rust-minify-reference.md`, .gitignore-Ergänzung, scripts/check.sh (cargo fmt --check, cargo clippy -D warnings, cargo test), CONVENTIONS.md (Namensschema, Fehlerbehandlung, Modul-Layout, Ledger-Format) und leere Crate-Stubs für alle neun Crates. Rationale: eliminiert Merge-Konflikte an Root-Dateien und gibt allen drei Agenten identische Konventionen. Abhängigkeiten: keine. Erst nach Tag gate-g0 starten A und B.
- [ ] 2. Kontrakt-Commits zuerst: Agent B portiert unmittelbar nach G0 die öffentlichen Typen von notagent-ai (`packages/ai/src/types.ts:1-830`) und notagent-agent (`packages/agent/src/types.ts:1-443`) als eigenständigen, früh gemergten Commit; Agent A ebenso das Component-Trait samt Terminal-Trait (`packages/tui/src/tui.ts:23-79`, `packages/tui/src/terminal.ts:60`). Rationale: C programmiert gegen diese Contracts, bevor die Implementierungen fertig sind; spätere Typ-Änderungen wären teure Drift.
- [ ] 3. Workstream A ausführen gemäß `plans/2026-08-13-rust-port-ws-a-tui-v1.md` (notagent-tui: Utils/Breiten, Keys, Terminal, TuiBase, beide Renderer, Layout-Engine, alle Komponenten, Editor, Markdown, LaTeX, Bilder, Autocomplete, Keybindings, native Modifier, komplette Testsuite mit virtuellem Terminal). Rationale: die TUI ist abhängigkeitsfrei und der Pfad zur differenziellen Rendering-Engine.
- [ ] 4. Workstream B ausführen gemäß `plans/2026-08-13-rust-port-ws-b-ai-agent-v1.md` (notagent-telemetry, notagent-ai: Typen, EventStream, Utils, Modellkatalog-Daten, Auth/OAuth, alle 10 API-Protokollmodule, 40 Provider-Factories, faux; notagent-agent: Agent, Agent-Loop mit paralleler Tool-Ausführung, Queues, CustomMessages). Rationale: liefert die Provider-Schicht und den Loop, gegen den C die App baut.
- [ ] 5. Workstream C ausführen gemäß `plans/2026-08-13-rust-port-ws-c-app-v1.md` (protocol/client/server/session-sqlite als eigenständige Crates; dann App: Config/Settings/Migrationen, Session-Manager, alle 16 Tools inkl. Minifizierung nach Rust-Referenz, Modes/Permissions/Hooks nativ, Tasks/Delegation auf tokio, agent-session, Compaction, CLI, Print/JSON/RPC-Modi, Interactive-Mode, llama.cpp-Provider nativ, Export/Share/Utilities, Package-Manager ohne Extensions). Rationale: das ist die eigentliche App mit dem Nutzer-sichtbaren Funktionsumfang.
- [x] 6. Gate G1 abnehmen: Kriterien aus Abschnitt Gates prüfen (A-Renderer-Tests, B-Streaming+Loop-Tests, C-Protokoll+Session+Basistools), Ledger-Zwischenaudit, Tag gate-g1 setzen. Rationale: erster Punkt, an dem Drift zwischen den Workstreams sichtbar würde; danach beginnt C mit der Integration von B-Typen in agent-session.
- [ ] 7. Gate G2 abnehmen: Headless-End-to-End (Print-Modus gegen faux-Provider), vollständige Tool-Suite, Permissions-Chain, paralleler Subagenten-Nachweis per Zeitmessung. Tag gate-g2. Rationale: die App funktioniert komplett ohne TUI — Integrationsrisiken von B und C sind ausgeräumt, bevor die TUI-Verdrahtung beginnt.
- [ ] 8. Interactive-Integration (A liefert Komponenten-Zuarbeit, C verdrahtet): Theme-System, App-Keybindings, alle 44+ Interactive-Komponenten, interactive-mode-Hauptschleife, Slash-Commands, Bash-Modus, Queueing, Footer, Selectors, Overlays. Gate G3 mit Virtual-Terminal-E2E-Szenarien abnehmen, Tag gate-g3. Rationale: größter Integrationsblock; A kennt die TUI-API am besten, C den App-Zustand.
- [ ] 9. Gate G4 — Release-Parität: alle portierten Testsuiten grün im Gesamtworkspace, Parity-Ledger aller neun Crates vollständig (jede TS-Datei erfasst), Feature-Checkliste gegen alle sechs Faktendokumente abgehakt, manueller Smoke-Test analog AGENTS.md-Releasetest der TS-App (--help, --version, --list-models, -p mit faux, interaktive Session in tmux) protokolliert in `plans/g4-smoke-report.md`. Tag gate-g4. Rationale: definierter, messbarer Abschluss.
- [ ] 10. Abschluss-Drift-Audit: automatisierter Abgleich, dass jede Datei unter packages/*/src der TS-Repo (außer dokumentierten Ausschlüssen) eine Ledger-Zeile mit Status verifiziert hat; offene Abweichungen klassifizieren oder beheben; Audit-Ergebnis in `plans/final-parity-audit.md` dokumentieren. Rationale: erzwingt die 100-Prozent-Abdeckung, die der 1:1-Anspruch verlangt.

## Verification Criteria

- cargo build --workspace und scripts/check.sh (fmt, clippy -D warnings, test) laufen auf main nach jedem Gate fehlerfrei.
- Die portierten Testsuiten decken dieselben Fälle wie die TS-Suiten ab; Zielumfang je Paket ist im jeweiligen Ledger nachgewiesen (tui ~16 690 Test-LOC-Äquivalent, ai ~34 000, coding-agent ~53 600, protocol/server/client/sqlite Konformanztests vollständig).
- Differenzielle Rendering-Engine: die Virtual-Terminal-Tests aus packages/tui/test (tui-render, tui-alt-screen, overlay-non-capturing, editor, markdown u. a.) sind portiert und grün; Full-Redraw-Trigger, Synchronized-Output-Klammerung und Kitty-Image-Bookkeeping verhalten sich byte-vergleichbar (Byte-Log-Vergleich per RecordingTerminal-Äquivalent).
- Subagenten: ein Test startet mehrere Delegationen mit künstlicher Latenz im faux-Provider und weist per Wanduhrzeit nach, dass sie echt parallel auf tokio laufen; die TS-Regeln (max 8, Tool-Sperrliste, kein Shell-Upgrade, 2-h-Deadline, 100k/200-Zeichen-Antwort-Caps, ein Expansion-Turn) sind durch portierte Tests belegt.
- CLI-Parität: alle Flags aus `packages/coding-agent/src/cli/args.ts:67-241` (minus Extension-Flags) und alle Subcommands (install/remove/update/list/config, auth check/print-api-key/print-bearer-token) vorhanden und getestet.
- Feature-Parität interaktiv: alle Slash-Commands aus der Faktenliste, alle 45 App-Keybinding-Actions mit identischen Defaults, Theme-Slots vollständig, dark/light-Themes mitgeliefert.
- Sessions: JSONL-v3-Dateien der TS-App werden von der Rust-App geöffnet, fortgesetzt, geforkt und verlustfrei re-serialisiert (Roundtrip-Test mit echten TS-Session-Fixtures).
- Protokoll: die portierten CBOR-/Framing-/Server-Konformanztests bestehen; ein Rust-Client spricht mit einem Rust-Server über Unix-Socket mit dem exakten Wire-Format (Version 1).
- Die Parity-Ledger aller Crates listen jede TS-src-Datei mit Status verifiziert oder dokumentiertem Ausschluss.

## Potential Risks and Mitigations

1. **Byte-genaue TUI-Parität scheitert an Unicode-Breiten-Details** (Tab=3, RGI-Emoji, Regional Indicators, Thai/Lao-AM-Sonderfälle)
   - Impact: Diff-Renderer produziert sichtbar anderes Verhalten; Editor-Cursor springt falsch
   - Likelihood: Mittel
   - Mitigation: graphemWidth wird 1:1 portiert (nicht durch Crate-Defaults ersetzt); die TS-Regressionstests (regional-indicator-width, overlay-cjk-boundary, tab-width) werden zuerst portiert und treiben die Implementierung
   - Contingency: differierende Einzelfälle gegen Node-REPL-Ausgabe der TS-Funktionen fixieren (Fixture-Generierung aus dem TS-Repo)

2. **Markdown-Renderer weicht ab, weil pulldown-cmark anders tokenisiert als marked**
   - Impact: sichtbar anderes Transkript-Rendering
   - Likelihood: Mittel bis hoch
   - Mitigation: Entscheidung erst nach Lektüre von markdown.ts + Testsuite; die 1 667 Test-LOC werden portiert und sind das Abnahmekriterium; wenn pulldown-cmark sie nicht erfüllen kann, wird ein eigener Lexer nach marked-Tokenstrom geschrieben (nur die tatsächlich genutzten Token-Typen)
   - Contingency: Hybrid — eigener Inline-Tokenizer nur für die abweichenden Konstrukte (strikte Tilde-Regel, LaTeX-Extension)

3. **In-place-mutiertes partial-Objekt der Stream-Events lässt sich nicht direkt abbilden**
   - Impact: Event-Konsumenten (Agent-Loop, TUI-Streaming) sehen andere Zwischenzustände
   - Likelihood: Mittel
   - Mitigation: Architektur-Entscheidung ist im Master fixiert (Streaming-State + Snapshots); die Loop-Tests aus packages/agent/test prüfen die beobachtbare Event-Sequenz
   - Contingency: Arc-basierte geteilte Zustände, falls Snapshot-Kosten in Benchmarks auffallen

4. **Workstream C wird zum Engpass (69k LOC gegen 24k/36k)**
   - Impact: A und B warten; Gesamtdauer steigt
   - Likelihood: Hoch
   - Mitigation: C beginnt mit den unabhängigen Protokoll-Crates; A übernimmt ab G2 die Interactive-Komponenten (17k LOC des App-TUI-Layers); Gates erlauben Umverteilung weiterer klar geschnittener Module (export-html, utils) an A/B — Umverteilung wird im jeweiligen Plan-File als neue Task-Zeile dokumentiert
   - Contingency: Orchestrator teilt WS-C in zwei Agenten (C1 Kern, C2 Interactive), die Plan-Struktur lässt das zu

5. **Provider-Verhalten lässt sich ohne echte API-Keys nicht vollständig verifizieren**
   - Impact: Streaming-Edge-Cases (Reconnects, Fehlerbodies, Rate-Limits) unentdeckt
   - Likelihood: Mittel
   - Mitigation: die TS-Testsuiten für die API-Module arbeiten mit aufgezeichneten Fixtures/Mocks — dieselben Fixtures werden portiert; der faux-Provider deckt die Loop-Integration ab; e2e-Tests, die in TS nur mit Env-Keys laufen, werden ebenso key-gated portiert
   - Contingency: manueller Verifikationslauf mit echten Keys des Nutzers an G4, dokumentiert im Smoke-Report

6. **Merge-Konflikte oder Interface-Drift zwischen den Worktrees**
   - Impact: verlorene Arbeit, inkonsistente Contracts
   - Likelihood: Niedrig (disjunkte Crate-Ownership, Glob-Members)
   - Mitigation: Kontrakt-Commits zuerst (Task 2); Schnittstellenänderungen nur durch den Owner nach Eintrag in plans/interface-requests.md; tägliches Rebase auf main
   - Contingency: Gate-Merge durch den Orchestrator mit manueller Konfliktauflösung

7. **Kitty-Keyboard-/Terminal-Protokoll-Handling weicht auf realen Terminals ab**
   - Impact: Tasteneingaben oder Rendering brechen in bestimmten Emulatoren
   - Likelihood: Mittel
   - Mitigation: stdin-buffer- und keys-Testsuiten (Sequenz-Fixtures) zuerst portieren; die Capability-Detection-Tabellen (Env-Variablen) 1:1 übernehmen
   - Contingency: manuelle Matrix-Tests in Ghostty/kitty/iTerm2/Terminal.app/tmux am G4-Smoke

## Alternative Approaches

1. **Ein einzelner Agent statt drei parallelen**
   - Beschreibung: sequentieller Port Paket für Paket
   - Pros: keine Koordination, keine Gates nötig
   - Cons: keine Parallelität, Gesamtdauer etwa dreifach; widerspricht der Aufgabenstellung
   - Empfehlung: verworfen — die Paketgrenzen des TS-Monorepos machen die Drei-Teilung risikoarm

2. **Zell-Buffer-basierte TUI (ratatui) statt 1:1-Port der String-Zeilen-Engine**
   - Beschreibung: vorhandenes Rust-TUI-Framework mit eigenem Diff-Modell verwenden
   - Pros: weniger Eigenbau, gereiftes Ökosystem
   - Cons: das Diff-Verhalten (Zeilen-Strings, Scrollback-erhaltendes Main-Screen-Rendering, compositeTuiLine, Overlay-Komposition vor dem Diff) wäre beobachtbar anders — kein 1:1-Port mehr; die Testsuite wäre nicht übertragbar
   - Empfehlung: verworfen — Nutzer verlangt explizit die differentielle Rendering-Engine 1:1

3. **Feinere Crate-Aufteilung der App (notagent-core, notagent-tools, notagent-interactive, …)**
   - Beschreibung: coding-agent in 4-6 Crates zerlegen
   - Pros: kleinere Kompilationseinheiten, klarere interne Grenzen
   - Cons: die TS-Vorlage hat diese Grenzen nicht; Ledger-Zuordnung und 1:1-Nachweis werden komplizierter; Zyklenrisiko (agent-session ↔ tools ↔ modes greifen ineinander)
   - Empfehlung: verworfen zugunsten der 1:1-Paketspiegelung; Binnenstruktur wird über Rust-Module abgebildet (Vorgabe: nicht zu komplex)

4. **Provider über offizielle Rust-SDKs (anthropic-sdk-rs u. ä.) statt Protokoll-Port**
   - Beschreibung: Community-/Hersteller-SDKs für die LLM-APIs nutzen
   - Pros: weniger eigener HTTP/SSE-Code
   - Cons: die TS-Implementierung umgeht die SDKs an den entscheidenden Stellen bewusst (eigener SSE-Decoder, maxRetries 0 wegen AbortSignal, eigene Retry-Klassifikation, 11 thinkingFormat-Varianten, Compat-Felder) — SDKs würden genau diese Semantik verstecken
   - Empfehlung: verworfen; einzige Ausnahme AWS Bedrock (offizielles aws-sdk-rust, wie in TS das AWS-SDK)

## Assumptions

- Es werden keine Annahmen über TS-Verhalten getroffen; unklares Verhalten wird durch Lesen der Quelle bzw. Ausführen der TS-Tests in `/Users/dev/projects/notagent-main` geklärt. Die einzigen Vorab-Festlegungen sind die dokumentierten Tech-Substitutionen und Ausschlüsse in diesem Dokument.
- Die Zielplattformen sind macOS, Linux und Windows (wie die TS-App); primäre Entwicklungs- und Testplattform ist macOS (darwin), plattformspezifische Pfade werden per cfg portiert und dort getestet, wo die Plattform verfügbar ist.

## Dependencies

- Lesezugriff auf `/Users/dev/projects/notagent-main` (TS-Quelle, Tests als Oracle) und `/Users/dev/projects/notagent-main-rust` (Minify-Referenz) während der gesamten Umsetzung.
- Node ≥ 22.19 lokal vorhanden, um TS-Tests/REPL als Verhaltens-Oracle auszuführen.
- Rust stable toolchain + cargo lokal installiert; Netzzugriff für crates.io.
- fd und rg als externe Binaries (werden wie in TS zur Laufzeit nach ~/.notagent/agent/bin geladen bzw. sind lokal vorhanden).

## Notes

- Die drei Agenten-Prompts liegen in `plans/prompts/agent-a-tui.md`, `plans/prompts/agent-b-ai.md`, `plans/prompts/agent-c-app.md`. Startreihenfolge: C zuerst (bis Tag gate-g0), dann A und B parallel dazu.
- `plans/interface-requests.md` ist der einzige workstream-übergreifende Kommunikationskanal (append-only, Owner setzt um).
- Konfigurationskompatibilität ist Teil des 1:1-Anspruchs: dieselben Pfade (~/.notagent/agent/…, .notagent/), dieselben Dateiformate (settings.json, auth.json, keybindings.json, hooks.json, Session-JSONL v3, Themes-JSON), dieselben Env-Variablen.
