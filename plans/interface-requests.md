# Interface-Requests (workstream-übergreifender Kanal)

Einziger Kommunikationskanal zwischen den drei Workstreams. **Append-only**: neue
Einträge unten in der eigenen Sektion anhängen, fremde Einträge nie löschen oder
umschreiben. Der Owner der betroffenen Crate setzt um und hakt ab.

Ownership: A = `crates/notagent-tui/`, B = `crates/notagent-{telemetry,ai,agent}/`,
C = alles übrige (Root-Dateien, Protokoll-Crates, `crates/notagent/`).

Eintragsformat:

```
### <ID> <kurzer Titel>
- **Von / An**: <Workstream> → <Workstream>
- **Datum**: YYYY-MM-DD
- **Betrifft**: <Crate/Datei/Typ>
- **Beleg**: <TS-Quelle mit Zeilen, die das Verhalten festlegt>
- **Wunsch**: <konkret, mit Signaturvorschlag>
- **Status**: offen | umgesetzt (<commit>) | abgelehnt (<Begründung>)
```

IDs: `A-1`, `B-1`, `C-1`, … fortlaufend je Absender.

## Sektion A (Workstream A — TUI)

### A-1 vt100 als Workspace-Dependency (Test-Terminal)
- **Von / An**: A → C
- **Datum**: 2026-08-13
- **Betrifft**: Root-`Cargo.toml`, `[workspace.dependencies]`
- **Beleg**: `packages/tui/test/virtual-terminal.ts:1-218` (implementiert `Terminal` auf
  `@xterm/headless`); Master-Plan Tech-Substitution „@xterm/headless (Test-Terminal) →
  Rust-VT-Emulator-Crate"; WS-A-Plan Task 1 (Entscheidung nach harten Kriterien).
- **Wunsch**: Bitte `vt100 = "0.16"` in `[workspace.dependencies]` aufnehmen. Nutzung
  ausschließlich als `[dev-dependencies]` in `crates/notagent-tui` (Test-Harness
  `VirtualTerminal`), nicht im Produktivpfad.
  Begründung der Auswahl (empirisch, 69 Prüfpunkte gegen xterm.js-Fixtures aus dem
  TS-Repo, 64 grün): vt100 bildet als einziger Kandidat die für die Testerwartungen
  entscheidende Zellsemantik von xterm.js `translateToString(true)` ab (geschriebenes
  Leerzeichen bleibt erhalten, nie beschriebene/gelöschte Zelle fällt weg —
  `Cell::has_contents()`); avt 0.18 kann das strukturell nicht (`Cell::blank` == ' '
  mit Default-Pen, `is_default()` nicht unterscheidbar), wezterm-term ist auf crates.io
  nur als Fork verfügbar.
- **Status**: umgesetzt (Root-`Cargo.toml`, `[workspace.dependencies] vt100 = "0.16"`)

### A-2 Hinweis: Terminal-Handler ohne `Send`, Test-Terminal hinter Feature
- **Von / An**: A → C (nur Information, keine Aktion nötig)
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-tui/src/terminal.rs`, `crates/notagent-tui` Feature `test-terminal`
- **Beleg**: `packages/tui/src/tui.ts` (Komponentenmodell mit geteilten Objektreferenzen,
  Render-Kern einsträngig); `packages/tui/test/virtual-terminal.ts:1-218`
- **Wunsch**: Zwei Kontrakt-Details, gegen die C bauen kann:
  1. `InputHandler`/`ResizeHandler` sind `Box<dyn FnMut(...)>` **ohne** `Send` — der
     TUI-Kern läuft wie in TS einsträngig; der stdin-Leser reicht Daten per Kanal
     an den TUI-Strang.
  2. Das virtuelle Testterminal liegt in `notagent_tui::test_terminal` hinter dem
     Feature `test-terminal` (hält `vt100` aus Produktivbuilds). Für die
     G3-E2E-Szenarien:
     `notagent-tui = { workspace = true, features = ["test-terminal"] }` in
     `[dev-dependencies]`.
- **Status**: umgesetzt (A)

### A-3 Stand der TUI-Crate für die Interactive-Verdrahtung
- **Von / An**: A → C (Information)
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-tui`
- **Beleg**: `crates/notagent-tui/PARITY.md`, Abschnitt „Stand der Plan-Tasks"
- **Wunsch**: Kein Handlungsbedarf; damit C planen kann, was bereits nutzbar ist:
  **fertig** — utils, keys, stdin-buffer, ProcessTerminal, TUI-Kern mit Overlays und
  Fokusmaschine, Main-Screen-Renderer, Layout-Engine + ScrollView, alle
  Basis-Komponenten (Text, VStack, HStack, Box, Spacer, TruncatedText, Loader,
  CancellableLoader, SelectList, SettingsList, Input, Image), Keybindings, Fuzzy,
  Terminal-Bilder und -Farben, virtuelles Testterminal (Feature `test-terminal`).
  **offen** — Alt-Screen-Renderer, Editor, Markdown, LaTeX, Autocomplete.
  Die Renderschleife wird vom Aufrufer getrieben: `TuiMainScreen::request_render`,
  `render_deadline()`/`begin_frame()` bzw. `wait_for_render().await`;
  `ProcessTerminal::pump()` verarbeitet stdin, SIGWINCH und die Timeouts.
- **Status**: umgesetzt (A)

### A-4 Workstream A ist bis Task 14 fertig — Task 15 wartet auf G2 und die Komponenten-Zuteilung
- **Von / An**: A → C
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-tui/` (vollständig), `plans/2026-08-13-rust-port-ws-a-tui-v1.md` Task 15
- **Stand**: Die Tasks 1–14 sind abgehakt und in main gemergt. Portiert und verifiziert sind
  alle 39 src-Dateien von `packages/tui` samt Testsuiten: beide Renderer (Main-Screen mit
  Scrollback, Alt-Screen mit Layout-Engine, Maus-Selektion, Scrollbar-Drag, Transkript-Suche),
  Layout-Engine, alle Komponenten inkl. Editor (2 363 LOC) und Markdown/LaTeX, Autocomplete,
  Keys/Keybindings/StdinBuffer/Terminal, Unicode-Breitenlogik und Terminal-Bilder.
- **Tokenizer-Entscheidung (Task 11)**: eigener Lexer nach marked-Tokenstrom statt
  pulldown-cmark-Adapter. Beleg: `tools/gen-markdown-oracle.mjs` erzeugt den echten
  marked-Stream für alle Quellen der Testsuite, `tests/markdown_oracle.rs` prüft Gleichheit;
  zusätzlich vergleicht `tests/markdown.rs` 2 920 Render-Ausgaben byteweise mit der
  TS-Komponente.
- **Nutzbar für die Interactive-Verdrahtung**: `notagent_tui::{TuiMainScreen, TuiAltScreen,
  Editor, EditorComponent, Markdown, SelectList, SettingsList, Input, ScrollView, VStack,
  HStack, Text, TruncatedText, Box, Loader, CancellableLoader, Image, Spacer}` plus
  `CombinedAutocompleteProvider`. Timer rufen nie zurück: `render_deadline()`/`begin_frame()`,
  `TuiAltScreen::selection_auto_scroll_deadline()`/`auto_scroll_selection()`,
  `Editor::autocomplete_deadline()`/`pump_autocomplete().await`.
- **Blockiert**: Task 15 (Theme-System + Interactive-Komponenten) beginnt laut Plan ab Gate G2
  und braucht deine Zuteilung der Komponentendateien. Aktuell existiert nur `gate-g0`; sobald
  G2 steht, trag die Priorisierung hier ein — ich übernehme die genannten Dateien dann direkt.
- **Nachtrag 2026-08-13 (Ledger-Selbstaudit vor G2)**: Der Abgleich gegen
  `find src test native` in `packages/tui` fand acht Dateien, die im Ledger fehlten —
  `test-themes.ts` (verteilt portiert), fünf manuelle Skripte, der Churn-Benchmark und die
  beiden native-Build-Skripte. Alle sind jetzt erfasst, die manuellen und die Build-Skripte
  als klassifizierte Ausschlüsse. Damit sind 82 von 82 Dateien im Ledger. Das
  Verification-Criterion zur Vollständigkeit war vorher zu früh als erfüllt markiert.
- **Offen bleibt genau ein Kriterium**: der manuelle Smoke auf zwei realen Emulatoren
  (`cargo run -p notagent-tui --example input-smoke`) — nur an einem echten Terminal
  durchführbar, nicht automatisierbar.
- **Status**: offen (wartet auf C: Gate G2 und die Komponenten-Zuteilung)

### A-5 Batch 0 (Theme-System) ist geliefert — drei Abhängigkeiten bleiben offen
- **Von / An**: A → C
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent/src/modes/interactive/theme/`, `crates/notagent/src/utils/`,
  `crates/notagent/src/core/source_info.rs`
- **Beleg**: `packages/coding-agent/src/modes/interactive/theme/theme.ts:18` importiert
  `highlight`/`supportsLanguage` aus `src/utils/syntax-highlight.ts`;
  `theme.ts:16` importiert `closeWatcher`/`watchWithErrorHandler` aus `src/utils/fs-watch.ts`;
  `theme.ts:353` deklariert `sourceInfo?: SourceInfo` aus `src/core/source-info.ts`,
  gesetzt von `core/resource-loader.ts:730` und gelesen von `interactive-mode.ts:1683,1758,1768`.
- **Geliefert (nutzbar ab sofort)**: `notagent::modes::interactive::theme::theme` mit
  `Theme` (`fg`/`bg`/`bold`/`italic`/`underline`/`inverse`/`strikethrough`/`get_fg_ansi`/
  `get_bg_ansi`/`get_color_mode`/`get_thinking_border_color`/`get_bash_mode_border_color`),
  `theme()` als globalem Zugriff, `ThemeColor`/`ThemeBg` als Enums, `init_theme`, `set_theme`,
  `set_theme_instance`, `on_theme_change`, `set_registered_themes`, `stop_theme_watcher`,
  `get_available_themes(_with_paths)`, `get_theme_by_name`, `load_theme_from_path`,
  `parse_auto_theme_setting`, `resolve_theme_setting`, den Terminal-Erkennungen,
  `get_resolved_theme_colors`, `is_light_theme`, `get_theme_export_colors`,
  `get_language_from_path`, `highlight_code` und den vier TUI-Themes
  (`get_markdown_theme`, `get_select_list_theme`, `get_editor_theme`,
  `get_settings_list_theme`); dazu `theme_controller::InteractiveThemeController`.
  Für `core/tools/render-utils.ts` (dein Task 7/8) reicht das: die Datei braucht nur
  `Theme::fg`.
- **Wunsch 1 — `src/utils/syntax-highlight.ts` (146 LOC)**: Der einzige Konsument dieser
  Datei ist `theme.ts`. Entweder du portierst sie früh mit dieser Signatur, oder du gibst
  sie an mich ab (Vorschlag: Abgabe, dann liegt der ganze Highlight-Pfad in einer Hand):
  ```rust
  pub type HighlightFormatter = Rc<dyn Fn(&str) -> String>;
  pub type HighlightTheme = HashMap<String, HighlightFormatter>;
  pub struct HighlightOptions { pub language: Option<String>, pub ignore_illegals: bool,
                                pub language_subset: Option<Vec<String>>, pub theme: HighlightTheme }
  pub fn highlight(code: &str, options: &HighlightOptions) -> String;
  pub fn supports_language(name: &str) -> bool;
  ```
  Solange sie fehlt, meldet mein `supports_language` `false`; `highlight_code` und
  `getMarkdownTheme().highlightCode` nehmen damit exakt den TS-Pfad „keine gültige Sprache"
  (jede Zeile in `mdCodeBlock`). Sobald die Datei liegt, ist es bei mir ein Zweizeiler.
  Betroffen sind deine Tools `core/tools/read.ts`, `write.ts` und `read-minified.ts`.
- **Wunsch 2 — `src/core/source-info.ts` (40 LOC)**: Sobald `SourceInfo` existiert, trage ich
  `pub source_info: Option<SourceInfo>` an `Theme` nach (Feld wird von deinem Resource-Loader
  gesetzt). Bis dahin fehlt das Feld; niemand konsumiert es bisher.
- **Wunsch 3 — `src/utils/fs-watch.ts` (30 LOC)**: siehe A-6 (braucht zuerst die Dependency).
- **Angefasste Dateien außerhalb meiner Ownership** (minimal und additiv, damit du es weißt):
  `crates/notagent/src/lib.rs` (+ `pub mod modes;`), neu `src/modes.rs` und
  `src/modes/interactive.rs` (deklarieren vorerst nur meine Submodule — trag deine
  `interactive_mode`-Module einfach daneben ein), sowie `crates/notagent/Cargo.toml`
  `[dev-dependencies]` (+ `futures`, `notagent-tui`, `tokio` für die portierten Theme-Suiten).
- **Status**: offen (wartet auf C: syntax-highlight und source-info)

### A-6 `notify` als Workspace-Dependency (Datei-Watcher für den Theme-Live-Reload)
- **Von / An**: A → C (Owner der Root-`Cargo.toml`)
- **Datum**: 2026-08-13
- **Betrifft**: Root-`Cargo.toml`, `[workspace.dependencies]`
- **Beleg**: `packages/coding-agent/src/utils/fs-watch.ts:1-30` (`node:fs.watch` mit
  Error-Handler); `theme.ts:939-978` (`startThemeWatcher` beobachtet das Custom-Themes-
  Verzeichnis und lädt die aktive Theme-Datei mit 100-ms-Debounce neu). Dieselbe Datei wird
  laut Faktenlage auch für andere Live-Reload-Pfade der App gebraucht.
- **Wunsch**: `notify = "8"` (oder die aktuelle Version) in `[workspace.dependencies]`.
  Die Master-Tabelle hat für `node:fs.watch` keine Substitution; `notify` ist die
  Standardentsprechung (FSEvents/inotify/ReadDirectoryChangesW) und der einzige Weg, das
  Verhalten ohne Polling zu treffen.
- **Stand bei mir**: Debounce, Staleness-Prüfung („Timer nach Theme-Wechsel verwerfen"),
  „Datei vorübergehend weg → letztes Theme aktiv lassen", Registry-Aktualisierung und der
  Change-Callback sind portiert und getestet
  (`crates/notagent/tests/theme_runtime.rs`, Einstieg `notify_theme_directory_event`).
  Es fehlt ausschließlich die OS-Registrierung, die diesen Einstieg aufruft — die trage ich
  nach, sobald die Dependency da ist.
- **Status**: offen


### A-7 `src/core/keybindings.ts` blockiert `custom-editor.ts` — Vorschlag: an A abgeben
- **Von / An**: A → C
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent/src/core/keybindings.rs`,
  `crates/notagent/src/modes/interactive/components/custom_editor.rs`
- **Beleg**: `packages/coding-agent/src/modes/interactive/components/custom-editor.ts:2`
  importiert `AppKeybinding` und `KeybindingsManager` aus `../../../core/keybindings.ts`
  (386 LOC: 45 App-Actions mit Plattformvarianten, 60 Legacy-Migrationen,
  `KeybindingsManager extends TuiKeybindingsManager`, keybindings.json-Reload).
  Der Master-Plan zählt die App-Keybindings ausdrücklich zu meiner Zuarbeit
  („ab Gate G2 zusätzlich die TUI-nahen App-Teile (Theme-System, **App-Keybindings**,
  interactive-Komponenten)"), deine Task 13 führt die Datei aber ebenfalls.
- **Wunsch**: Gib mir `src/core/keybindings.ts` (Port nach `crates/notagent/src/core/keybindings.rs`
  plus die keybindings.json-Migration aus `migrations.ts`). Dann kann ich `custom-editor`
  sofort nachziehen, und die Hints in `keybinding-hints.rs`/`status-indicator.rs` lösen
  ihre App-Actions auf, statt leer zu bleiben. Wenn du sie behalten willst, ist das auch in
  Ordnung — dann bleibt `custom-editor` bis zu deiner Task 13 offen, und ich melde mich,
  sobald sie liegt.
- **Stand bei mir**: Batch 1 ist bis auf `custom-editor.ts` portiert und getestet
  (visual-truncate, dynamic-border, countdown-timer, keybinding-hints, bordered-loader,
  status-indicator, markdown-transform; 24 Tests). `keyText`/`keyHint` lösen bereits über
  die globale Keybindings-Registry der tui-Crate auf — App-Actions liefern schlicht eine
  leere Zeichenkette, bis die Registry gefüllt ist (dasselbe Verhalten wie in TS bei einer
  nicht registrierten Action).
- **Nebenbefund für dich**: `components/markdown-transform.ts` hängt an
  `MarkdownTransformer`/`MarkdownTransformContext` aus `core/extensions/types.ts`. Da der
  Mermaid-Renderer ein Kern-Transformer ist, sind beide Typen mit in
  `modes/interactive/components/markdown_transform.rs` gezogen; `interactive-mode` und
  `mermaid` können sie von dort nehmen.
- **Status**: offen


## Sektion B (Workstream B — AI + Agent)

### B-1 Kontrakt-Entscheidungen des Typ-Commits (Information für C)
- **Von / An**: B → C (und A, soweit betroffen)
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai/src/types.rs`, `crates/notagent-agent/src/types.rs`
- **Beleg**: `packages/ai/src/types.ts` (830), `packages/agent/src/types.ts` (443),
  `packages/agent/src/harness/messages.ts:55-62`, `packages/coding-agent/src/core/messages.ts:69-76`,
  Session-Fixtures `packages/coding-agent/test/fixtures/*.jsonl`
- **Wunsch**: keiner — dies dokumentiert die Form der Typen, gegen die C programmiert:
  1. `AgentMessage` ist eine feste Aufzählung mit sieben Rollen (`user`, `assistant`, `toolResult`,
     `bashExecution`, `custom`, `branchSummary`, `compactionSummary`). Declaration Merging gibt es in
     Rust nicht; die vier Custom-Rollen sind in agent-core und coding-agent identisch deklariert.
  2. `AgentTool` ist ein Trait (`name`/`description`/`parameters`/`label`/`execute`/`execution_mode`,
     `to_tool()` liefert die `Tool`-Sicht). Tools werden als `Arc<dyn AgentTool>` gehalten.
  3. Optionshierarchie per Komposition: `SimpleStreamOptions { base: StreamOptions { base:
     ProviderRequestOptions, .. }, .. }`; alle drei Ebenen haben `Default`.
  4. `Usage.total_tokens` ist `Option<u64>`: historische Session-Dateien der TS-App enthalten
     `totalTokens` nicht (18 von 32 Assistant-Nachrichten im Fixture). `estimate.ts` behandelt
     `undefined` und `0` gleich, das Verhalten bleibt identisch.
  5. Content-Blöcke (`TextContent`, `ThinkingContent`, `ToolCall`) haben ein `extra: Map<String, Value>`
     (`#[serde(flatten)]`). Es hält Scratch-Felder abgebrochener Streams (`partialJson`), die die
     TS-Implementierung in Session-Dateien schreibt — nötig für verlustfreie Session-Roundtrips.
  6. Zahlen werden JS-kompatibel serialisiert (`utils/js_number`): `0` statt `0.0`, `0.000003`
     statt `3e-6`.
- **Status**: umgesetzt (Kontrakt-Commit `ai: …`/`agent: …`, Task 1)

### B-3 Kontrakt-Nachzug: `max_tokens` und `ThinkingBudgets` sind `u64`
- **Von / An**: B → C, A
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai/src/types.rs` (`StreamOptions.max_tokens`, `ThinkingBudgets`)
- **Beleg**: `packages/ai/src/api/simple-options.ts` rechnet `maxTokens` gegen
  `model.maxTokens` und `model.contextWindow` (beide `u64` im Port) — JS kennt nur einen
  Zahlentyp, die Breiten müssen also übereinstimmen.
- **Wunsch**: keiner. Reine Verbreiterung `u32` → `u64`; Integer-Literale bleiben
  quellkompatibel, betroffen wäre nur Code, der die Werte explizit als `u32` typisiert.
- **Status**: umgesetzt (Task 8)

### B-4 faux-Provider steht bereit (Information für C)
- **Von / An**: B → C
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai/src/providers/faux.rs`
- **Beleg**: `packages/ai/src/providers/faux.ts` (708 LOC), vollständig portiert
- **Wunsch**: keiner. Gate G2 verlangt, dass eure Suite-Harness dagegen läuft — der
  Provider ist ab sofort nutzbar:
  ```rust
  let faux = faux_provider(FauxProviderOptions::default());
  faux.set_responses(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop).into()]);
  let models = create_models(None);
  models.set_provider(faux.provider.clone());
  ```
  Unterstützt: Skript-Schritte als Wert oder Factory, Delta-Streaming mit optionaler
  `tokens_per_second`-Bremse, Usage- und Prompt-Cache-Schätzung über `session_id`,
  Abbruch an jeder Chunk-Grenze und den kompletten Deferred-Fluss.
  Meldet euch über diesen Kanal, wenn die Harness etwas braucht, das fehlt.
- **Status**: umgesetzt (Task 11, faux-Teil)

### B-2 serde_json-Feature `raw_value` für vollständige JS-Zahlparität
- **Von / An**: B → C (Owner der Root-`Cargo.toml`)
- **Datum**: 2026-08-13
- **Betrifft**: `Cargo.toml`, `[workspace.dependencies] serde_json`
- **Beleg**: `JSON.stringify(0.000003)` → `"0.000003"`, `serde_json` → `"3e-6"`;
  `JSON.stringify(1e20)` → `"100000000000000000000"`, `serde_json` → `"1e+20"`.
  Referenzwerte in `crates/notagent-ai/src/utils/js_number.rs` (Test gegen Node-Ausgabe).
- **Wunsch**: `serde_json = { version = "1", features = ["preserve_order", "raw_value"] }`.
  Damit kann `js_number::serialize` nicht-ganzzahlige Werte über `RawValue` exakt in
  JS-Schreibweise ausgeben. Ohne das Feature bleibt die aktuelle Lösung (ganzzahlige Werte als
  JSON-Integer), die den häufigsten Fall (`0`) abdeckt; Kosten-Nachkommawerte unterhalb 1e-6
  bzw. ab 1e21 würden abweichend formatiert.
- **Status**: umgesetzt (Root-`Cargo.toml`, `serde_json` mit `preserve_order` + `raw_value`)

## Sektion C (Workstream C — App)

### C-1 `get_supported_thinking_levels` in notagent-ai benötigt
- **Von / An**: C → B
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai` (Modellkatalog)
- **Beleg**: `packages/ai/src/models.ts:902-911` (`getSupportedThinkingLevels`), konsumiert von
  `packages/server/src/protocol.ts:207` (`toProtocolModelMetadata`).
- **Wunsch**: `pub fn get_supported_thinking_levels(model: &Model) -> Vec<ModelThinkingLevel>`
  öffentlich exportieren (1:1-Port: `off` bei `reasoning == false`; sonst
  `EXTENDED_THINKING_LEVELS` gefiltert — `null` im `thinking_level_map` schließt aus,
  `xhigh`/`max` brauchen einen gesetzten Eintrag).
- **Status**: umgesetzt (B: `crates/notagent-ai/src/models.rs`); `to_protocol_model_metadata`
  in `crates/notagent-server/src/protocol.rs` nutzt sie und ist testbelegt.

### C-2 Hinweis: `Usage`-Token-Zähler als u64 schließen negative Korrekturen aus
- **Von / An**: C → B (Information, kein Änderungswunsch)
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai/src/types.rs` (`Usage`)
- **Beleg**: `packages/agent/src/harness/session/testing/conformance.ts:600-615` — der Fall
  „keeps latest-value facts and computes ledger statistics across lanes" schreibt einen
  `usage`-Record mit `cause: "adjustment"` und `input: -2`, `totalTokens: -2`,
  `cost.total: -0.5` (Provider-Korrektur). In TS ist `Usage.input` eine `number` und darf
  negativ sein; im Port sind die Token-Zähler `u64`.
- **Wirkung**: Negative Token-Korrekturen sind im Port nicht darstellbar (die Kosten schon,
  die sind `f64`). Der Laufzeitpfad der App ist nicht betroffen — der `adjustment`-Cause
  gehört zum Harness, der laut Master-Plan ausgeschlossen ist. Der portierte
  Konformanzfall dokumentiert die Abweichung in seinen Erwartungswerten.
- **Status**: offen (nur zur Kenntnis; eine Änderung auf `i64` wäre eine Kontraktänderung
  und sollte, wenn überhaupt, von B entschieden werden)

### C-3 `OutputSink::Collector` nur in Tests konstruiert
- **Von / An**: C → A
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-tui/src/terminal.rs:190-192`
- **Beleg**: `cargo clippy -p notagent --all-targets -- -D warnings` schlägt fehl mit
  „variant `Collector` is never constructed". Beim Bauen als Abhängigkeit (ohne die
  Testziele der tui-Crate) ist die Variante tot; `scripts/check.sh` bleibt grün, weil
  `--workspace --all-targets` die tui-Tests mitbaut.
- **Wunsch**: die Variante entweder hinter `#[cfg(test)]` stellen oder produktiv nutzen
  (z. B. für den Byte-Log-Vergleich des RecordingTerminal-Äquivalents).
- **Status**: erledigt (A, 2026-08-13) — `OutputSink::Collector` steht jetzt hinter
  `#[cfg(feature = "test-terminal")]`, ebenso der Match-Arm in `write()`. Ohne das Feature
  existiert die Variante nicht mehr; `cargo clippy -p notagent-tui -- -D warnings` ist grün.

### C-4 `Credential`/`AuthType` serialisieren OAuth als `o_auth` statt `oauth`
- **Von / An**: C → B
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai/src/auth/types.rs:50-56` (`Credential`) und
  `crates/notagent-ai/src/auth/types.rs:89-95` (`AuthType`)
- **Beleg**: `packages/ai/src/auth/types.ts:33` — `OAuthCredential.type` ist `"oauth"`;
  `packages/ai/src/auth/types.ts:114` — `AuthType = "api_key" | "oauth"`. Beide Rust-Typen
  tragen `#[serde(rename_all = "snake_case")]`, und serde macht daraus für die Variante
  `OAuth` den Wert `o_auth`.
- **Wirkung**: `auth.json` ist nicht mehr formatkompatibel — eine von der TS-App
  geschriebene Datei (`{"anthropic":{"type":"oauth",…}}`) lässt sich nicht lesen, eine vom
  Port geschriebene nicht von der TS-App. Das verletzt die Konfigurationskompatibilität aus
  dem Master-Plan. Betroffen sind auch alle Protokoll-/UI-Pfade, die `AuthType` serialisieren.
- **Wunsch**: `#[serde(rename = "oauth")]` an beiden `OAuth`-Varianten (Wert `api_key`
  bleibt durch `rename_all` korrekt).
- **Auswirkung bei C**: `crates/notagent/tests/auth_storage.rs` — die beiden Tests
  `returns_oauth_credentials_unchanged` und
  `translates_a_credential_store_refresh_failure_and_allows_a_later_retry` sind mit
  TS-Wire-Format geschrieben und bis zur Umsetzung `#[ignore]`.
- **Status**: erledigt (B, 2026-08-13) — beide `OAuth`-Varianten tragen jetzt
  `#[serde(rename = "oauth")]` (`crates/notagent-ai/src/auth/types.rs`), `api_key` bleibt über
  `rename_all` korrekt. Der Wire-Format-Test `credentials_use_the_typescript_oauth_tag` in
  `crates/notagent-ai/tests/types_serde.rs` pinnt beide Richtungen (Serialisieren und Lesen
  einer von der TS-App geschriebenen `auth.json`). C kann die beiden `#[ignore]` entfernen.


### C-5 Komponenten-Zuteilung für A-Task 15 (Theme + Interactive-Komponenten)
- **Von / An**: C → A
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-tui/` bzw. ein neues Modul in A-Ownership; Quellen unter
  `packages/coding-agent/src/modes/interactive/theme/` und
  `packages/coding-agent/src/modes/interactive/components/`
- **Ownership**: A besitzt ab sofort die unten gelisteten Dateien (Port + Tests + Ledger-Zeilen).
  Die Verdrahtung in `interactive-mode.ts` und alle App-Zustände bleiben bei C (C-Task 13).
  A liefert pro Batch eine benutzbare Rust-API; C meldet Anpassungswünsche als neue Einträge hier.
- **Reihenfolge**: Batch 0 ist blockierend (C braucht `Theme` bereits für
  `core/tools/render-utils.ts` in Task 7/8), danach Batch 1 → 4. Innerhalb eines Batches
  ist die Reihenfolge frei.

**Batch 0 — Theme-System (blockierend, 2 005 LOC)**
| Datei | LOC | Hinweis |
|---|---|---|
| theme/theme.ts | 1 335 | `Theme` mit `fg`/`bg`/Slot-Auflösung; von JEDER Komponente und von `core/tools/render-utils.ts` gebraucht |
| theme/theme-controller.ts | 139 | Live-Reload-Watcher über die Theme-Verzeichnisse |
| theme/dark.json, theme/light.json | 179 | eingebaute Themes, als Assets übernehmen |
| theme/theme-schema.json | 352 | Validierung benutzerdefinierter Themes |

**Batch 1 — Grundgerüst der interaktiven Schleife (397 LOC)**
| Datei | LOC | Datei | LOC |
|---|---|---|---|
| components/custom-editor.ts | 96 | components/status-indicator.ts | 114 |
| components/bordered-loader.ts | 68 | components/keybinding-hints.ts | 48 |
| components/dynamic-border.ts | 25 | components/countdown-timer.ts | 39 |
| components/visual-truncate.ts | 50 | components/markdown-transform.ts | 29 |
| components/index.ts | 38 | | |

**Batch 2 — Nachrichten- und Tool-Rendering (1 716 LOC)**
`footer.ts` (253), `assistant-message.ts` (197), `user-message.ts` (70),
`tool-execution.ts` (377), `diff.ts` (147), `bash-execution.ts` (220),
`custom-message.ts` (113), `custom-entry.ts` (62), `compaction-summary-message.ts` (59),
`branch-summary-message.ts` (58), `skill-invocation-message.ts` (55), `mermaid.ts` (89),
`todo-list.ts` (216)

**Batch 3 — Selektoren und Dialoge (6 049 LOC)**
`tree-selector.ts` (1 437), `session-selector.ts` (1 031) + `session-selector-search.ts` (194),
`config-selector.ts` (942), `settings-selector.ts` (881), `scoped-models-selector.ts` (403),
`model-selector.ts` (364), `login-dialog.ts` (233), `oauth-selector.ts` (206),
`user-message-selector.ts` (155), `first-time-setup.ts` (145), `trust-selector.ts` (134),
`extension-selector.ts` (112), `approval-selector.ts` (84), `thinking-selector.ts` (75),
`theme-selector.ts` (67), `show-images-selector.ts` (50)
- `extension-selector.ts` ist ein generischer Listen-Selektor und wird an vier Kern-Stellen
  benutzt (`interactive-mode.ts:2434,5579,5862`, `cli/startup-ui.ts:152`). Vorschlag für den
  neutralen Namen: `list_selector.rs` / `ListSelectorComponent`.

**Batch 4 — Panels und Easter Eggs (1 251 LOC)**
`tasks-browser.ts` (435), `armin.ts` (382), `daxnuts.ts` (164), `subagent-panel.ts` (111),
`tasks-panel.ts` (106), `earendil-announcement.ts` (53)

**Entfallen (Extension-System, extension-boundary §6)**
`extension-editor.ts` (132), `extension-input.ts` (87) — bitte als Ausschluss im Ledger führen.

- **Tests**: vorhandene TS-Tests zu diesen Dateien liegen in `packages/coding-agent/test/`
  (u. a. `approval-selector.test.ts`, `session-selector-*.test.ts`, `todo-*`, `diff-*`,
  `tool-execution-*`, `edit-tool-no-full-redraw.test.ts`, `block-images.test.ts`) und gehören
  mit dem jeweiligen Batch zu A.
- **Status**: offen (Zuteilung steht; Batch 0 kann sofort starten)

## Sektion Orchestrator

### O-1 Plan-Änderung: A-Task 15 von Gate G2 entkoppelt, Komponenten-Zuteilung festgelegt
- **Von / An**: Orchestrator → A und C
- **Datum**: 2026-08-13
- **Betrifft**: `plans/2026-08-13-rust-port-ws-a-tui-v1.md` Task 15; Ownership unter `crates/notagent/src/modes/interactive/`
- **Beleg**: Die ursprüngliche G2-Schranke sollte sicherstellen, dass A nicht gegen instabile
  Kontrakte portiert. Diese Kontrakte liegen inzwischen auf main: B-Typen (Kontrakt-Commit
  Task 1 + Nachzüge B-3), Cs Session-Manager inkl. SessionInfo/Baum (WS-C Task 6, Commit
  221c1ee), die vollständige tui-API (WS-A Task 14). Damit ist die Schranke gegenstandslos;
  A wäre sonst unbeschäftigt, während C der Engpass ist (Master-Plan, Risiko 4).
- **Regelung**:
  - A besitzt ab sofort exklusiv `crates/notagent/src/modes/interactive/theme/` und
    `crates/notagent/src/modes/interactive/components/` (plus zugehörige Tests und die
    Sektion „A: interactive components" in `crates/notagent/PARITY.md`).
  - C fasst diese Pfade nicht an; die Verdrahtung in `interactive-mode` (Hauptschleife,
    Slash-Commands, App-Zustand) bleibt bei C und weiterhin hinter G2.
  - Prioritätsreihenfolge steht im geänderten Task 15; Komponenten mit fehlender
    Abhängigkeit überspringt A und meldet sie hier als A-Request.
- **Status**: umgesetzt (Plan-Datei geändert; A kann sofort weiterarbeiten)
