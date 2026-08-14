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
- **Wunsch 3 — `src/utils/fs-watch.ts` (30 LOC)**: erledigt (A, 2026-08-13) — die Datei geht in `theme.rs` auf, siehe A-12. Du brauchst sie nicht zu portieren.
- **Angefasste Dateien außerhalb meiner Ownership** (minimal und additiv, damit du es weißt):
  `crates/notagent/src/lib.rs` (+ `pub mod modes;`), neu `src/modes.rs` und
  `src/modes/interactive.rs` (deklarieren vorerst nur meine Submodule — trag deine
  `interactive_mode`-Module einfach daneben ein), sowie `crates/notagent/Cargo.toml`
  `[dev-dependencies]` (+ `futures`, `notagent-tui`, `tokio` für die portierten Theme-Suiten).
- **Status**: erledigt (A, 2026-08-14) — C-6 hat syntax-highlight und source-info geliefert, beides ist in `theme.rs` nachgezogen (A-13); fs-watch ist mit A-12 erledigt

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
  nach, sobald die Dependency da ist. (Nachgetragen am 2026-08-13, siehe A-12.)
- **Status**: umgesetzt (Orchestrator, O-2) — `notify = "6"` in `[workspace.dependencies]`;
  Version 6 statt 8, weil sie in der lokalen Rust-Referenz erprobt ist (Cargo.lock 6.1.1)
  und damit offline-sicher auflöst. Upgrade auf 8 steht A frei, wenn ein Feature fehlt.


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
- **Status**: umgesetzt (Orchestrator, O-2) — Übergabe genehmigt: A besitzt ab sofort
  `crates/notagent/src/core/keybindings.rs` (Port von `src/core/keybindings.ts` inkl.
  keybindings.json-Migration aus `migrations.ts`) und darf `custom-editor` darauf bauen.
  C streicht die Datei aus seinem Task-13-Umfang.


### A-8 Batch 2 zur Hälfte geliefert — fünf Komponenten warten auf C-Module
- **Von / An**: A → C
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent/src/modes/interactive/components/`
- **Geliefert**: `diff` (inkl. Wortdiff), `user_message`, `assistant_message`,
  `compaction_summary_message`, `branch_summary_message`, `custom_message`,
  `bash_execution`. Alles mit Tests; die drei zugehörigen TS-Suiten sind portiert.
- **Für dich sofort nutzbar**:
  `notagent::modes::interactive::components::diff::{render_diff, RenderDiffOptions}` —
  das brauchen deine Tools `core/tools/edit.ts` (Zeile 234, 272) und
  `core/tools/patch-minified.ts` (Zeile 263).
- **Nebenprodukt, das dir Arbeit spart**: `components/diff/word_diff.rs` ist ein
  vollständiger Port von jsdiff 8.0.4 (`base.js` Myers-Kern, `word.js`, `util/string.js`)
  für den Optionssatz von `diffWords`. Zeichengenau gegen die Bibliothek geprüft
  (`tests/diff_words_oracle.rs`, 900 Fälle). Dein `core/tools/edit-diff.ts` braucht laut
  eurem Ledger noch `Diff.diffLines` und `Diff.createTwoFilesPatch`: der Myers-Kern dort ist
  tokenizer-agnostisch, ich hebe ihn dir auf Zuruf in ein gemeinsames Modul (Vorschlag:
  `crates/notagent/src/utils/jsdiff.rs`, dann gehört er dir) und du setzt den Zeilen-Tokenizer
  darauf. Sag Bescheid, sonst lasse ich ihn, wo er ist.
- **Ausschluss, den ich eingetragen habe**: `components/custom-entry.ts` entfällt vollständig
  (Klasse 2). Die Komponente verlangt zwingend einen `EntryRenderer`, den nur
  `extensionRunner.getEntryRenderer` liefert (`interactive-mode.ts:3689`);
  `plans/facts/extension-boundary.md` führt Entry-Renderer als ersatzlos entfallend.
  Bei `custom-message.ts` ist nur der optionale Renderer-Pfad entfallen, der Default-Pfad bleibt.
- **Offen, weil dein Modul fehlt** (ich ziehe sie sofort nach, sobald es liegt):
  | Komponente | LOC | fehlende Abhängigkeit |
  |---|---|---|
  | `tool-execution.ts` | 377 | `core/tools/render-utils.ts`, `createAllToolDefinitions`/`ToolName` aus `core/tools/index.ts`, `utils/image-convert.ts` |
  | `footer.ts` | 253 | `core/agent-session.ts`, `core/footer-data-provider.ts`, `core/modes/indicator.ts`, `core/usage-totals.ts` |
  | `todo-list.ts` | 216 | `core/todos/todos.ts` (`Todo`, `TodoStatus`) |
  | `mermaid.ts` | 89 | Ersatz für `grok-mermaid` (Master-Plan, deine Task 15) |
  | `skill-invocation-message.ts` | 55 | `ParsedSkillBlock` aus `core/agent-session.ts` |
  Die günstigste Reihenfolge für mich wäre `core/todos/todos.ts` und `core/tools/render-utils.ts`
  zuerst — damit fallen `todo-list` und `tool-execution` (zusammen 593 LOC) sofort.
- **Zwei kleine Dinge, die ich in gemeinsamen Dateien angelegt habe**:
  `components::to_locale_string` (die en-US-Gruppierung von `Number.prototype.toLocaleString`,
  die auch dein `interactive-mode.ts:6313-6334` braucht) und die Typen
  `MarkdownTransformer`/`MarkdownTransformContext` in `components/markdown_transform.rs`.
- **Status**: offen (wartet auf C)


### A-9 Batch 3 angefangen — Zuschnitt der restlichen Selektoren
- **Von / An**: A → C
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent/src/modes/interactive/components/`
- **Geliefert**: `list_selector` (der umbenannte `extension-selector`, wie in C-5 vorgeschlagen),
  `thinking_selector`, `theme_selector`, `show_images_selector`, `user_message_selector` —
  mit 8 Tests.
- **Sofort portierbar, aber in dieser Sitzung nicht mehr erreicht** (alle Abhängigkeiten
  liegen auf main): `tree-selector.ts` (1 437), `oauth-selector.ts` (206),
  `session-selector-search.ts` (194), `first-time-setup.ts` (145). Das ist der nächste Block,
  den ich nehme.
- **Blockiert, geordnet nach dem, was mir am meisten aufschließt**:
  | Komponente | LOC | fehlende Abhängigkeit |
  |---|---|---|
  | `settings-selector.ts` | 881 | `core/http-dispatcher.ts` (`formatHttpIdleTimeoutMs`, `HTTP_IDLE_TIMEOUT_CHOICES`) |
  | `session-selector.ts` | 1 031 | `core/keybindings.ts` (siehe A-7) |
  | `config-selector.ts` | 942 | `core/package-manager.ts` (`PathMetadata`, `ResolvedPaths`, `ResolvedResource`, `PackageSource`) |
  | `scoped-models-selector.ts` | 403 | `modes/interactive/model-search.ts` |
  | `model-selector.ts` | 364 | `core/model-runtime.ts` + `model-search.ts` |
  | `login-dialog.ts` | 233 | `utils/open-browser.ts` |
  | `trust-selector.ts` | 134 | `core/trust-manager.ts` |
  | `approval-selector.ts` | 84 | `core/permissions/request.ts` (nativer Nachbau) |
  `modes/interactive/model-search.ts` schließt allein 767 LOC auf — falls du dort einen
  günstigen Einstieg suchst.
- **Status**: teilweise erledigt (A, 2026-08-13) — der genannte naechste Block (tree-selector, oauth-selector, session-selector-search, first-time-setup) ist portiert und getestet, siehe A-11; die blockierten Selektoren der Tabelle bleiben offen


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

### A-10 keybindings.ts portiert (O-2) — zwei Dinge in deinen Dateien
- **Von / An**: A -> C
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent/src/core/keybindings.rs`, `crates/notagent/src/migrations.rs`,
  `crates/notagent/Cargo.toml`
- **Geliefert**: `notagent::core::keybindings` mit `KeybindingsManager` (`new`, `create`,
  `reload`, `matches`, `get_keys`, `get_definition`, `get_conflicts`, `set_user_bindings`,
  `get_user_bindings`, `get_resolved_bindings`, `get_effective_config`), `keybindings()`
  (TUI-Registry + 45 App-Actions), `APP_KEYBINDINGS`, `migrate_keybindings_config` und
  `migrate_keybindings_config_file`. Tests: `crates/notagent/tests/keybindings.rs` (6).
- **Wichtig fuer dich — der Manager installiert sich nicht selbst**: In TS ist die
  App-Instanz zugleich die globale Registry. Der Port trennt das: Du haeltst den
  `KeybindingsManager` (am besten in `Rc<RefCell<…>>`, weil `CustomEditor` dieselbe Instanz
  braucht) und installierst die Registry mit
  `notagent_tui::keybindings::set_keybindings(manager.borrow().to_tui())` — einmal beim Start
  und erneut nach jedem `reload()`, sonst sehen die globalen `keybindings_match`-Aufrufe der
  Komponenten die alten Bindings.
- **Zwei Zeilen in deinen Dateien** (beides additiv, jederzeit von dir aenderbar):
  1. `migrations.rs`: `run_migrations` ruft jetzt
     `crate::core::keybindings::migrate_keybindings_config_file(&agent_dir)` an derselben
     Stelle wie `migrations.ts:311` (nach `migrate_tools_to_bin`). Damit ist der TODO-Kommentar
     dort erledigt.
  2. `crates/notagent/Cargo.toml`: die Dev-Dependency `notagent-tui` traegt jetzt
     `features = ["test-terminal"]` (die portierte custom-editor-Suite braucht
     `VirtualTerminal`).
- **Status**: umgesetzt

### A-11 Batch 3: tree-selector, oauth-selector, session-selector-search, first-time-setup, custom-editor
- **Von / An**: A -> C
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent/src/modes/interactive/components/`
- **Geliefert**:
  - `tree_selector::{TreeSelectorComponent, TreeSelectorOptions, TreeList, FilterMode}` —
    vollstaendig inklusive Fold-/Branch-Navigation, Filtermodi, Suche, Label-Editor und
    horizontalem Viewport; die TS-Suite (702 LOC, 17 Faelle) laeuft gruen.
  - `oauth_selector::{OAuthSelectorComponent, AuthSelectorProvider, AuthSelectorMethod,
    AuthSelectorMode, format_auth_selector_provider_type}`.
  - `session_selector_search::{filter_and_sort_sessions, parse_search_query, match_session,
    has_session_name, SortMode, NameFilter}` — arbeitet direkt auf deinem `SessionInfo`.
  - `first_time_setup::{FirstTimeSetupComponent, FirstTimeSetupOptions, FirstTimeSetupResult}`.
  - `custom_editor::{CustomEditor, AppActionHandler}`.
- **Was du beim Verdrahten wissen musst**:
  - `TreeSelectorComponent::new(tree, current_leaf_id, terminal_height, on_select, on_cancel,
    TreeSelectorOptions { on_label_change, initial_selected_id, initial_filter_mode })`.
    `on_copy` setzt du wie in TS nach dem Konstruieren. Bei leerem Baum ersetzt `is_empty()`
    das `setTimeout(onCancel, 100)`.
  - `FirstTimeSetupComponent` liefert `FirstTimeSetupResult { theme, share_analytics }`;
    `shouldRunFirstTimeSetup` (`cli/startup-ui.ts`) und der Settings-Pfad bleiben bei dir,
    ebenso die beiden TS-Suiten `first-time-setup*.test.ts`, die genau das pruefen.
  - `OAuthSelectorComponent` erwartet `AuthType` aus `notagent_ai::auth::types` (nicht eine
    eigene String-Union) und `AuthSelectorMethod::{ApiKey, OAuth}` fuer `provider.method`.
- **Noch offen aus A-9** (unveraendert): settings-selector (`core/http-dispatcher.ts`),
  session-selector (1 031 LOC, jetzt nur noch durch nichts blockiert — ich nehme sie als
  naechstes), config-selector (`core/package-manager.ts`), scoped-models-/model-selector
  (`modes/interactive/model-search.ts`, `core/model-runtime.ts`), login-dialog
  (`utils/open-browser.ts`), trust-selector (`core/trust-manager.ts`), approval-selector
  (`core/permissions/request.ts`).
- **Status**: umgesetzt

### A-12 Theme-Live-Reload ist verdrahtet — `utils/fs-watch.ts` ist damit erledigt
- **Von / An**: A -> C
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent/src/modes/interactive/theme/theme.rs`, `crates/notagent/Cargo.toml`
- **Beleg**: A-6 (notify als Workspace-Dependency, mit O-2 auf main); `theme.ts:939-978`
  (`startThemeWatcher`), `utils/fs-watch.ts:17-30` (`watchWithErrorHandler`).
- **Umgesetzt**: `start_theme_watcher` registriert jetzt einen `notify`-Watcher auf dem
  Custom-Themes-Verzeichnis; `stop_theme_watcher` verwirft ihn. Der Debounce (100 ms), die
  Staleness-Pruefung, das Verhalten bei kurzzeitig fehlender Datei und die
  Registry-Aktualisierung waren schon da. Neuer Test
  `crates/notagent/tests/theme_runtime.rs::reloads_from_a_real_filesystem_event` faehrt den
  echten OS-Event (Datei aendern, ohne `notify_theme_directory_event` von Hand zu rufen).
- **Fuer dich**: `utils/fs-watch.ts` brauchst du nicht mehr zu portieren — Wunsch 3 aus A-5
  ist damit erledigt. Sollte ein zweiter Live-Reload-Pfad die Datei brauchen, sag Bescheid,
  dann hebe ich die beiden Funktionen nach `crates/notagent/src/utils/fs_watch.rs`.
  Offen aus A-5 bleiben nur noch `utils/syntax-highlight.ts` und `core/source-info.ts`.
- **Eine Zeile in deiner Datei**: `crates/notagent/Cargo.toml` `[dependencies]` +
  `notify = { workspace = true }`.
- **Status**: umgesetzt

### C-6 A-5 und A-9 erfüllt: syntax-highlight, html, source-info, model-search liegen auf main
- **Von / An**: C → A
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent/src/utils/syntax_highlight.rs`, `crates/notagent/src/utils/html.rs`,
  `crates/notagent/src/core/source_info.rs`, `crates/notagent/src/modes/interactive/model_search.rs`
- **Beleg**: A-5 (Wunsch 1 und 2), A-9 (Zeile `scoped-models-selector.ts` / `model-selector.ts`);
  Master-Plan-Substitutionstabelle Zeile „highlight.js → tree-sitter-highlight, gemappt auf die
  9 Syntax-Theme-Slots"; `packages/coding-agent/test/syntax-highlight.test.ts`.
- **Geliefert**:
  ```rust
  // notagent::utils::syntax_highlight
  pub type HighlightFormatter = Rc<dyn Fn(&str) -> String>;   // wie von dir vorgeschlagen
  pub type HighlightTheme = HashMap<String, HighlightFormatter>;
  pub struct HighlightOptions { pub language: Option<String>, pub ignore_illegals: bool,
                                pub language_subset: Option<Vec<String>>, pub theme: HighlightTheme }
  pub fn render_highlighted_html(html: &str, theme: &HighlightTheme) -> String;
  pub fn highlight(code: &str, options: &HighlightOptions) -> Result<String, HighlightError>;
  pub fn supports_language(name: &str) -> bool;

  // notagent::core::source_info
  pub struct SourceInfo { pub path: String, pub source: String, pub scope: SourceScope,
                          pub origin: SourceOrigin, pub base_dir: Option<String> }
  pub enum SourceScope { User, Project, Temporary }        // serde: "user" | "project" | "temporary"
  pub enum SourceOrigin { Package, TopLevel }              // serde: "package" | "top-level"
  pub struct PathMetadata { … }                            // hier deklariert, package_manager re-exportiert später
  pub fn create_source_info(path: impl Into<String>, metadata: &PathMetadata) -> SourceInfo;
  pub fn create_synthetic_source_info(path: impl Into<String>, options: SyntheticSourceInfoOptions) -> SourceInfo;

  // notagent::modes::interactive::model_search
  pub struct ModelSearchItem { pub id: String, pub provider: String, pub name: Option<String> }
  pub fn get_model_search_text(item: &ModelSearchItem) -> String;
  pub fn get_model_selector_search_text(item: &ModelSearchItem) -> String;
  ```
- **Eine Abweichung von deinem Signaturvorschlag**: `highlight` gibt `Result<String, HighlightError>`
  zurück statt `String`. Das ist der `try`/`catch`, den `highlightCode` und
  `getMarkdownTheme().highlightCode` in `theme.ts:1195,1301` um den Aufruf legen — ohne `Result`
  wäre dein `catch`-Zweig toter Code. `HighlightError::UnknownLanguage` trägt denselben Text wie
  highlight.js (`Unknown language: "x"`). `Rc` ist wie gewünscht: die Formatter-Map wird nie über
  Threadgrenzen gereicht; falls dein Cache für `getCliHighlightTheme` prozessglobal statt
  thread-lokal sein soll, sag Bescheid, dann stelle ich auf `Arc<… + Send + Sync>` um (Einzeiler).
- **Was du beim Nachziehen von `theme.rs` wissen musst** (die Substitution ist nicht farbgleich):
  - `supports_language` meldet `true` für die 13 gebündelten tree-sitter-Grammatiken
    (bash, c, cpp, css, go, java, javascript, json, python, ruby, rust, typescript/tsx, xml/html
    — jeweils mit den highlight.js-Aliassen, `getLanguage` lowercased) und zusätzlich für `diff`.
    Für alle anderen Sprachen aus `getLanguageFromPath` (yaml, toml, markdown, sql, php, kotlin,
    swift, lua, …) bleibt es `false` — dein bestehender Pfad „keine gültige Sprache" greift dann,
    also flach in `mdCodeBlock`. Das ist die sichtbarste Folge der Substitution.
  - `diff` habe ich aus `highlight.js/lib/languages/diff.js` handportiert (es gibt keine
    tree-sitter-Grammatik dafür), byte-genau gegen highlight.js 11 geprüft. Die beiden
    `diff`-Erwartungen aus dem zweiten `describe` von `test/syntax-highlight.test.ts`
    (`toolDiffRemoved`/`toolDiffAdded`) sollten damit exakt aufgehen.
  - Von den drei Erwartungen in „keeps cli-highlight default styled scopes mapped to theme styles"
    treffen zwei: JS-Regexliteral → Scope `string` (dieselbe Farbe wie highlight.js' `regexp`)
    und HTML-`div` → Scope `name`. Die dritte weicht ab: der Python-Dekorator ist bei tree-sitter
    `function` (`syntaxFunction`), highlight.js sagte `meta` (`muted`). Bitte beim Portieren
    dieses Falls als dokumentierte Abweichung führen — die Erwartung `\x1b[38;2;128;128;128m`
    stimmt nicht mehr. (Randnotiz: die `/`-Begrenzer eines JS-Regex sind zusätzlich als
    `operator` gefärbt; der Testfall prüft nur `toContain` auf den Regexkörper.)
  - `buildCliHighlightTheme` kannst du unverändert nach den highlight.js-Scopes schlüsseln
    (`keyword`, `built_in`, `literal`, `number`, `regexp`, `string`, `comment`, `doctag`, `meta`,
    `function`, `title`, `class`, `type`, `tag`, `name`, `attr`, `variable`, `params`, `operator`,
    `punctuation`, `emphasis`, `strong`, `link`, `addition`, `deletion`): die Übersetzung der
    tree-sitter-Capture-Namen auf genau diese Scopes steckt in `HIGHLIGHT_SCOPES` in meinem Modul.
    Scopes ohne Theme-Eintrag (`subst`, `symbol`) erben die umgebende Farbe — genau wie unter
    highlight.js, siehe der dritte Test des ersten `describe`.
- **Ledger**: Die Zeile zu `theme.ts` in `crates/notagent/PARITY.md` trägt noch den Vermerk
  „**Offen:** … Syntax-Highlighter … liegt noch nicht auf main" und „`Theme.sourceInfo` fehlt".
  Beides ist jetzt erfüllt; die Zeile gehört dir, deshalb habe ich sie nicht angefasst.
- **Noch offen aus A-8** (nicht Teil dieser Lieferung): `core/tools/render-utils.ts` und
  `core/tools/index.ts` kommen mit Task 13 bzw. sobald die Registry über alle 16 Tools baubar ist;
  `core/todos/todos.ts` liegt seit Task 8 auf main (`notagent::core::todos`, `Todo`/`TodoStatus`),
  `todo-list.ts` ist damit frei. `core/modes/*` und `core/permissions/request.ts` liefert Task 9.
- **Status**: umgesetzt (siehe Commit `app: port syntax highlighting, source info and model search`)

### C-7 Task 9 liegt auf main: Modes, Permissions (Gate), Hooks (Dispatcher), Project-Trust
- **Von / An**: C → A und B
- **Datum**: 2026-08-14
- **Betrifft**: `crates/notagent/src/core/{modes,permissions,hooks,project_trust,trust_manager}.rs`,
  `crates/notagent/src/utils/frontmatter.rs`, `crates/notagent/src/core/tools.rs` (`ToolName`)
- **Beleg**: WS-C-Plan Task 9; `plans/facts/extension-boundary.md` §2.1 und §2.2
  (Permissions und Hooks sind in TS versteckte Inline-Extensions und werden nativ nachgebaut).
- **Für A sofort nutzbar** (beides stand in A-8/A-9 als blockiert):
  - `approval-selector.ts` (84 LOC): `notagent::core::permissions::request` liefert
    `ApprovalRequest { tool_name, target, policy_name, reason, mode_id }`, `ApprovalAnswer`
    (`ApproveOnce`/`ApproveAlways`/`Deny` mit `as_str`), `APPROVAL_ANSWERS` in Dialogreihenfolge,
    `format_request_summary`, `format_request_explanation`, `answer_allows`, `answer_persists`.
    Der Selektor muss nur die drei Labels anzeigen und die Antwort zurückgeben; die
    Verdrahtung (`interactiveMode.requestApproval` → `ApprovalPresenter`) mache ich in Task 13.
  - `todo-list.ts` (216 LOC): `core/todos` liegt seit Task 8 auf main — das war der letzte
    fehlende Baustein aus A-8 für diese Komponente.
  - Für Footer und Mode-Anzeige: `core::modes::indicator::{indicator_color_key,
    format_mode_label, format_mode_switch_notice}` und `core::modes::cycle::{order_modes,
    next_mode_id, initial_mode_id}` sind portiert.
- **Neue Workspace-Dependency**: `serde_yaml_ng = "0.10"` als Ersatz für das npm-Paket `yaml`
  (Frontmatter von Modes, Skills, Prompt-Templates). Der Master-Plan führt für YAML keine
  Substitution; ich habe sie als Klasse 3 im Ledger dokumentiert, weil ohne YAML-Parser weder
  Modes noch Skills ladbar sind. Falls der Orchestrator eine andere Crate vorzieht, ist der
  Tausch auf `utils/frontmatter.rs` beschränkt.
- **Für B (Information)**: der Agent-Loop braucht in Task 11 zwei Einstiegspunkte, die jetzt
  existieren und keine Extension-Infrastruktur verlangen:
  `PermissionGate::before_tool_call(tool_name, input, signal) -> Option<PermissionBlock>`
  (vor jedem Tool-Aufruf; `terminate: true` beendet den Batch) und die Methoden von
  `HookDispatcher` an den bisherigen Emit-Stellen (`session_start`, `session_shutdown`,
  `before_agent_start` → optionale `hook_context`-Message, `agent_end`/`agent_settled`,
  `tool_result`, `session_before_compact`, `session_compact`).
- **Status**: umgesetzt (Commit `app: port modes, permissions and hooks`)
### A-13 Batch 3 und 4 weitgehend fertig — offen sind nur noch C-Module
- **Von / An**: A -> C
- **Datum**: 2026-08-14
- **Betrifft**: `crates/notagent/src/modes/interactive/components/`,
  `crates/notagent/src/modes/interactive/theme/theme.rs`
- **Geliefert seit A-11**:
  - `theme`: C-6 ist eingelöst. `highlight_code`, `getMarkdownTheme().highlightCode` und
    `buildCliHighlightTheme` laufen über `utils::syntax_highlight`, `Theme` traegt
    `source_info: Option<SourceInfo>`. Deine beiden angekündigten Abweichungen sind als
    Tests festgehalten (`tests/theme_syntax_highlight.rs`): Python-Dekorator ist `function`
    statt `meta`, und die `/`-Begrenzer eines JS-Regex sind `operator` — die Erwartung im
    zweiten `describe` von `syntax-highlight.test.ts` ist damit in genau zwei Punkten
    angepasst, die beiden diff-Faelle stimmen byte-genau. Danke fuer die Vorwarnung.
  - `todo_list::{TodoListComponent, TodoVisibility, build_todo_display, count_todos,
    format_todo_summary, format_hidden_todo_summary, format_todo_line, todo_display_limit}`
    gegen dein `core::todos`. `TodoVisibility` wird gepollt: `hide_deadline()` + `tick()`.
  - `session_selector::{SessionSelectorComponent, SessionSelectorOptions, SessionList,
    SessionScope, LoadRequest, LoadReason, delete_session_file}`.
  - `scoped_models_selector::{ScopedModelsSelectorComponent, ModelsConfig, ModelsCallbacks}`
    gegen dein `model_search`.
  - `armin::ArminComponent`, `daxnuts::DaxnutsComponent`,
    `earendil_announcement::EarendilAnnouncementComponent`.
- **Wichtig beim Verdrahten des Session-Selektors**: Die beiden `SessionsLoader`-Promises
  sind umgedreht, weil die Komponente keine Futures halten kann. Ablauf bei dir:
  ```rust
  while let Some(request) = selector.take_pending_load() {
      let result = match request.scope {
          SessionScope::Current => load_current(|loaded, total|
              selector.report_load_progress(&request, loaded, total)).await,
          SessionScope::All => load_all(…).await,
      };
      selector.apply_load_result(request, result.map_err(|e| e.to_string()));
  }
  ```
  Alle Zustandsübergänge (Scope-Vergleich, `allLoadSeq`, Loading-Flags, Fehlermeldung im
  Header) stecken weiterhin in der Komponente — du lieferst nur das Ergebnis. Der
  Rename-Callback ist synchron (`FnMut(&str, &str)`), passend zu den synchronen
  Session-Schreibpfaden des Ports. Der Status-Timer im Header wird gepollt
  (`status_deadline()` / `tick_status()`), ebenso die Animationen von armin und daxnuts
  (`deadline()` / `tick()`).
- **Neu blockiert — `src/core/tasks/types.ts`**: `tasks-browser.ts` (435),
  `subagent-panel.ts` (111) und `tasks-panel.ts` (106) brauchen `TaskInfo`, `TaskStatus`,
  `SubagentTaskInfo` und `isTerminalTaskStatus`. Deine `core/tools/bash.rs` verweist in einem
  Kommentar schon auf `TaskInfo`, das Modul liegt aber noch nicht auf main. Sobald es da ist,
  ziehe ich die drei Panels sofort nach — zusammen 652 LOC und der komplette Rest von Batch 4.
- **Direkt aus C-7 nachgezogen**: `approval-selector.ts` (84) und `trust-selector.ts` (134) sind
  portiert und getestet (`tests/permission_selectors.rs`) — deine `core::permissions::request`
  und `core::trust_manager` haben genau gepasst, keine Nachfragen.
- **Weiterhin blockiert (aus A-9)**: `settings-selector.ts` (`core/http-dispatcher.ts`),
  `config-selector.ts` (`core/package-manager.ts`), `model-selector.ts` (`core/model-runtime.ts`),
  `login-dialog.ts` (`utils/open-browser.ts`),
  `tool-execution.ts`
  (`core/tools/render-utils.ts`, `createAllToolDefinitions`), `footer.ts` (`core/agent-session.ts`,
  `core/footer-data-provider.ts`, `core/modes/indicator.ts`, `core/usage-totals.ts`),
  `skill-invocation-message.ts` (`ParsedSkillBlock`), `mermaid.ts` (grok-mermaid-Ersatz).
  Die günstigste Reihenfolge für mich bleibt `core/tasks/types.ts` (schließt 652 LOC auf),
  danach `core/tools/render-utils.ts` (377) und `core/agent-session.ts` (308).
- **Status**: umgesetzt

### C-8 `Agent`: Lesezugriff auf die Provider-Verdrahtung (blockiert Delegation, Task 10/11)
- **Von / An**: C → B
- **Datum**: 2026-08-14
- **Betrifft**: `crates/notagent-agent/src/agent.rs` (`Agent`), `AgentOptions`
- **Beleg**: `packages/coding-agent/src/core/delegation/run.ts:170-203` (`createChild`) baut das
  Kind aus dem Elternteil: `initialState` aus `parent.state` (systemPrompt, model,
  thinkingLevel) plus `convertToLlm`, `streamFn` und danach neun Zuweisungen —
  `getApiKey`, `onPayload`, `onResponse`, `beforeToolCall`, `afterToolCall`,
  `thinkingBudgets`, `transport`, `maxRetryDelayMs`, `toolExecution`. In TS sind das
  öffentliche Felder der `Agent`-Klasse (`packages/agent/src/agent.ts:183-201`); der
  Rust-Port hat sie in das private `options: Mutex<AgentOptions>` gefaltet, und `Agent`
  bietet weder Getter noch Setter dafür. Damit ist `createChild` nicht portierbar.
- **Warum eine Kopie beim Elternbau nicht reicht**: die Felder werden zur Laufzeit
  geändert — `interactive-mode.ts:4726` setzt `session.agent.transport` beim
  Transport-Wechsel, `agent-session.ts:546,567` setzt `beforeToolCall`/`afterToolCall`
  (Permission-Gate und Hooks, C-Task 11). Ein beim Konstruieren gezogener Schnappschuss
  wäre also beobachtbar veraltet.
- **Wunsch (klein gehalten, zwei Methoden auf `Agent`)**:
  - `pub fn options(&self) -> AgentOptions` — Klon des aktuellen Optionsstands
    (`AgentOptions` ist bereits `Clone`, `create_loop_config` klont es schon).
  - `pub fn update_options(&self, f: impl FnOnce(&mut AgentOptions))` — deckt die
    TS-Feldzuweisungen ab, die C in Task 11 (`beforeToolCall`/`afterToolCall`) und
    Task 13 (`transport`) braucht.
- **Zusätzlicher Befund (gegen die TS-Quelle geprüft, kein Blocker für C)**:
  `onPayload`/`onResponse` fehlen im Rust-Agent vollständig. TS reicht sie in
  `createLoopConfig` an `streamSimple` weiter (`packages/agent/src/agent.ts:452-453`,
  Felder in Zeile 104/183); Konsumenten sind `coding-agent/src/core/sdk.ts:335,342`
  (C-Task 15) und eben `delegation/run.ts`. Bis sie existieren, kopiert mein
  `create_child` die übrigen Felder und lässt diese beiden aus (im Ledger vermerkt).
- **Solange offen**: C portiert Task 10 ohne `delegation/run.rs` und `tools/task.rs`
  (Tasks-Maschinerie, Store, Notification, `task_list`/`task_output`/`task_stop`,
  Shell-Tasks am echten Manager) und zieht die beiden Dateien nach, sobald die Methoden
  auf main liegen.
- **Rückmeldung C (2026-08-14)**: erledigt und verbraucht. `create_child` kopiert alle neun
  Felder über `parent.options()`; `tests/delegation_run.rs` und `tests/task_tool.rs` sind
  grün, Task 10 ist damit vollständig. Danke auch für `on_payload`/`on_response` — die
  brauche ich in Task 15 für `core/sdk.ts`.
- **Antwort B (2026-08-14)**: umgesetzt, beides. `Agent::options()` und
  `Agent::update_options(|options| …)` liegen auf main; TS führt genau diese Felder
  öffentlich (`packages/agent/src/agent.ts:180-201`), der Wunsch entspricht dem Original.
  `on_payload`/`on_response` waren im Port tatsächlich nicht vorhanden — sie sind jetzt
  Felder von `AgentOptions` und wandern wie in TS (`agent.ts:452-453`) über
  `create_loop_config` in die `SimpleStreamOptions`. `create_child` kann also alle neun
  Zuweisungen kopieren. Belege: `crates/notagent-agent/tests/agent.rs`
  (`the_provider_wiring_is_readable_and_writable`,
  `on_payload_and_on_response_reach_the_stream_options`).
- **Status**: umgesetzt (B, 2026-08-14)

### A-14 Task 15 ist vollstaendig blockiert — elf Komponenten warten auf fremde Module
- **Von / An**: A -> C und B (Aufteilung nach O-4)
- **Datum**: 2026-08-14
- **Betrifft**: `crates/notagent/src/modes/interactive/components/`
- **Stand**: Alles, was ohne dich portierbar war, ist portiert. 32 Komponenten-Module liegen
  auf main, dazu 20 Testdateien. In dieser Sitzung habe ich die letzte Luecke in meinem
  eigenen Bestand geschlossen: fuer drei bereits portierte Komponenten existierten
  TS-Suiten, die ich uebersehen hatte — `approval-selector.test.ts` (7 Faelle),
  `trust-selector.test.ts` (4) und `user-message.test.ts` (3) sind jetzt portiert und gruen.
  Dabei ist ein Fehler von mir aufgefallen: ich hatte `createPendingApproval` als
  Klasse-2-Ausschluss gefuehrt („Extension-System"), aber der Helfer hat damit nichts zu tun
  und wird von der TS-Suite geprueft. Er ist jetzt als `create_pending_approval`
  (`tokio::sync::oneshot`) portiert, die Ledger-Zeile ist korrigiert.
- **Was jetzt fehlt** — jede offene Komponente haengt an genau einem deiner Module:
  | fehlendes C-Modul | schliesst auf | LOC | zugehoerige TS-Suiten |
  |---|---|---|---|
  | ~~`core/tasks/types.ts`~~ (erledigt, deine Task 10) | tasks-browser, tasks-panel, subagent-panel | 652 | portiert am 2026-08-14, 52 Tests |
  | ~~`core/tools/render-utils.ts`~~ (erledigt, O-5) + `createAllToolDefinitions` samt der `renderCall`/`renderResult`-Hälften (C, Task 13) | tool-execution | 377 | `tool-execution-component.test.ts`, `edit-tool-no-full-redraw.test.ts` |
  | `core/agent-session.ts` (`ParsedSkillBlock`) | skill-invocation-message | 55 | — |
  | `core/agent-session.ts` + `core/footer-data-provider.ts` (C) + `core/usage-totals.ts` (**B**, seit O-4) | footer | 253 | `footer-width.test.ts` |
  | `core/model-runtime.ts` (**B**, seit O-4) | model-selector | 364 | `model-selector.test.ts` |
  | `core/http-dispatcher.ts` | settings-selector | 881 | `settings-selector.test.ts` |
  | `core/package-manager.ts` (**B**, seit O-4) | config-selector | 942 | — |
  | `utils/open-browser.ts` | login-dialog | 233 | — |
  | grok-mermaid-Ersatz (deine Task 15) | mermaid | 89 | `mermaid.test.ts` |
  `ToolName` liegt seit C-7 auf main und `utils::image::convert_image_bytes_to_png` deckt
  `utils/image-convert.ts` ab.
- **Nachtrag 2026-08-14 (O-5)**: `core/tools/render-utils.ts` ist portiert
  (`crates/notagent/src/core/tools/render_utils.rs`, 7 Tests). Damit fehlt `tool-execution`
  nur noch `createAllToolDefinitions(cwd)` und die `renderCall`/`renderResult`-Hälften in den
  Tool-Dateien; beides bleibt laut O-5 bei dir (Task 13). Sobald deine `ToolDefinition` die
  beiden Renderer trägt und die Registry baubar ist, ziehe ich `tool-execution.ts` (377) und
  die beiden Suiten (`tool-execution-component.test.ts` 537, `edit-tool-no-full-redraw.test.ts`
  235) nach. **Für dich sofort nutzbar**: `notagent::core::tools::render_utils::{shorten_path,
  link_path, str_arg, replace_tabs, normalize_display_text, get_text_output, invalid_arg_text,
  render_tool_path}` — genau die Helfer, die deine `renderCall`/`renderResult`-Hälften
  brauchen. Ich fasse `core/tools/*.rs` sonst nicht an.
- **Guenstigste Reihenfolge fuer mich** (Aufschluss pro Modul): `core/tasks/types.ts` (652 LOC
  und der komplette Rest von Batch 4), dann `core/tools/render-utils.ts` (377 LOC plus zwei
  Testsuiten), dann `core/agent-session.ts` (308 LOC ueber zwei Komponenten).
  `utils/open-browser.ts` ist die kleinste Einzelposition — falls du sie lieber abgibst,
  nehme ich sie, sag einfach Bescheid.
- **An B**: mit O-4 gehoeren `core/model_runtime.rs`, `core/package_manager/` und
  `core/usage_totals.rs` dir. Sie schliessen bei mir `model-selector.ts` (364) und
  `config-selector.ts` (942) auf, `usage_totals` ist ein Drittel der Footer-Abhaengigkeiten.
  Ich brauche daraus nur Lesezugriffe: die Modellliste samt Provider und Anzeigename, den
  Refresh-Status, und fuer den Config-Selektor `PathMetadata`/`ResolvedPaths`/
  `ResolvedResource`/`PackageSource`. Melde dich hier, sobald etwas davon auf main liegt —
  dann ziehe ich die beiden Selektoren sofort nach.
- **Status**: offen (wartet auf C und B)

### A-15 Batch 4 ist fertig — nur noch sechs Module fehlen
- **Von / An**: A -> C und B
- **Datum**: 2026-08-14
- **Betrifft**: `crates/notagent/src/modes/interactive/components/`,
  `crates/notagent/src/core/tools/render_utils.rs`
- **Geliefert**:
  - `core/tools/render_utils.rs` (O-5) — die Helfer, die deine `renderCall`/`renderResult`-
    Hälften brauchen: `shorten_path`, `link_path`, `str_arg`, `replace_tabs`,
    `normalize_display_text`, `get_text_output`, `invalid_arg_text`, `render_tool_path`.
    7 eigene Tests; ich fasse `core/tools/*.rs` sonst nicht an.
  - Deine Task 10 hat `core/tasks/types.rs` gebracht — damit ist **Batch 4 komplett**:
    `tasks_browser::{TasksBrowserComponent, TasksBrowserProps, TasksFilter, visible_tasks,
    count_tasks, STOP_CONFIRM_TIMEOUT_MS}`, `tasks_panel::{TasksPanel, TasksPanelScope}` und
    `subagent_panel::{SubagentPanel, build_subagent_rows, format_elapsed, format_tokens}`.
    Die drei TS-Suiten (283 + 115 + 165 LOC, 52 Fälle) sind unverändert portiert und gruen,
    inklusive der beiden Rahmen-Tests, die jede Zeile auf exakt die vorgegebene Breite pruefen.
- **Beim Verdrahten**: die Stop-Bestaetigung des Browsers wird gepollt
  (`stop_confirm_deadline()` / `tick_stop_confirm()`), wie alle Timer dieses Ports;
  `set_props` ersetzt den kompletten Props-Satz wie `setProps` in TS.
- **Was jetzt noch fehlt** (Stand nach O-4 und O-5):
  | fehlendes Modul | Owner | schliesst auf | LOC |
  |---|---|---|---|
  | `createAllToolDefinitions` + `renderCall`/`renderResult` in den Tool-Dateien | C (Task 13) | tool-execution (+ 2 Suiten) | 377 |
  | `core/agent-session.ts` (`ParsedSkillBlock`) | C | skill-invocation-message | 55 |
  | `core/agent-session.ts` + `core/footer-data-provider.ts` (C) + `core/usage-totals.ts` (B) | C und B | footer (+ 1 Suite) | 253 |
  | `core/http-dispatcher.ts` | C | settings-selector (+ 1 Suite) | 881 |
  | `core/model-runtime.ts` | B | model-selector (+ 1 Suite) | 364 |
  | `core/package-manager.ts` | B | config-selector | 942 |
  | `utils/open-browser.ts` | C | login-dialog | 233 |
  | grok-mermaid-Ersatz | C (Task 15) | mermaid (+ 1 Suite) | 89 |
- **Status**: offen (wartet auf C und B)

### A-16 Dependency-Sweep nach Cs Task 10 und O-5: nichts Neues aufgeschlossen, dafuer zwei Ledger-Luecken geschlossen
- **Von / An**: A -> C und B
- **Datum**: 2026-08-14
- **Betrifft**: `crates/notagent/src/modes/interactive/`, `crates/notagent/PARITY.md`
- **Sweep-Ergebnis**: Nach dem Merge von `core/tasks/` (deine Task 10) und `render_utils.rs`
  (O-5) habe ich alle 47 TS-Komponentendateien gegen `main` geprueft. Stand: 34 portiert,
  4 ausgeschlossen (custom-entry, extension-editor, extension-input, index — Klasse 2),
  1 umbenannt portiert (extension-selector -> list_selector), 8 blockiert. Von den acht
  fehlenden Modulen liegt keines auf main: `core/agent-session.rs`,
  `core/footer_data_provider.rs`, `core/usage_totals.rs`, `core/http_dispatcher.rs`,
  `core/model_runtime.rs`, `core/package_manager/`, `utils/open_browser.rs`, grok-mermaid-Ersatz.
  Die Tabelle aus A-15 gilt damit unveraendert weiter.
- **Ledger-Selbstaudit (das eigentliche Ergebnis dieser Sitzung)**: Der Abgleich aller
  TS-Testdateien, die `interactive/components`, `interactive/theme`, `core/keybindings` oder
  `tools/render-utils` importieren (50 Dateien), fand zwei Suiten, die mein Ledger nicht
  gefuehrt hat und die portierbar waren:
  - `test/suite/regressions/2791-fswatch-error-crash.test.ts` — der Theme-Watcher darf an
    einem asynchronen OS-Fehler nicht sterben. Portiert nach `tests/theme_runtime.rs`
    (`survives_an_error_reported_by_the_theme_watcher`). Klasse 1: der TS-Fall emittiert ein
    `error`-Event auf dem `FSWatcher` eines Kindprozesses (ohne Listener beendet
    `EventEmitter.emit("error")` den Prozess); `notify` kennt diese Regel nicht und liefert
    den Fehler als `Err` an dieselbe Closure, der Port prueft daher die Wirkung des Fixes:
    Fehler geschluckt, aktives Theme bleibt, weitere Events verworfen, Neustart hebt den
    Fehlerzustand auf. Der `Err`-Zweig ist dafuer als `notify_theme_watcher_error()`
    oeffentlich — dasselbe Muster wie `notify_theme_directory_event` fuer den `Ok`-Zweig.
  - `test/max-thinking.test.ts` — die Theme-Haelfte („falls back to thinkingXhigh for legacy
    themes") ist nach `tests/theme_validation.rs` portiert. Die CLI-/Settings-Haelfte
    (`isValidThinkingLevel`, `SettingsManager`) gehoert dir, sie ist im Ledger als solche
    vermerkt.
  Zusaetzlich sind jetzt alle Testsuiten der acht blockierten Komponenten mit LOC und Grund
  im Ledger gefuehrt (1 751 LOC), damit beim Aufschliessen nichts uebersehen wird, und drei
  Suiten sind als C-/B-Sache zugeordnet (7153, 5596, session-info-modified-timestamp).
- **Konkreter Kontrakt fuer `tool-execution` (der guenstigste naechste Aufschluss)**: Alle 16
  `ToolDefinition`-Implementierungen liegen bei dir auf main; es fehlen nur zwei Dinge, dann
  ziehe ich `tool-execution.ts` (377) plus `tool-execution-component.test.ts` (537),
  `edit-tool-no-full-redraw.test.ts` (235) nach:
  1. Die Registry `create_all_tool_definitions(cwd, options) -> BTreeMap<ToolName, Arc<dyn ToolDefinition>>`
     (`core/tools/index.ts:318-337`).
  2. Zwei Default-Methoden am `ToolDefinition`-Trait, die 13 der 16 Tool-Dateien ueberschreiben
     (`extensions/types.ts:489-497`; der Extension-Pfad entfaellt, `tool-execution` liest nur
     die eingebaute Definition):
     ```rust
     fn render_call(&self, args: &Value, theme: &Theme, context: &ToolRenderContext)
         -> Option<Box<dyn Component>> { None }
     fn render_result(&self, result: &AgentToolResult, options: &ToolRenderResultOptions,
                      theme: &Theme, context: &ToolRenderContext) -> Option<Box<dyn Component>> { None }
     ```
     `ToolRenderContext` (`extensions/types.ts:419-444`) traegt `args`, `tool_call_id`,
     `invalidate`, `last_component`, `state`, `cwd`, `execution_started`, `args_complete`,
     `is_partial`, `expanded`, `show_images`, `is_error`. Wenn du den Typ lieber von mir
     haettest: sag Bescheid, dann lege ich ihn in `components/tool_execution.rs` an (wie schon
     `MarkdownTransformer`) und du importierst ihn von dort — dann bleibt in deinen
     Tool-Dateien nur die Renderlogik.
- **An B**: unveraendert `core/model_runtime.rs` (schliesst model-selector + 177 LOC Tests auf)
  und `core/package_manager/` (config-selector, 942). `core/usage_totals.rs` ist ein Drittel
  der Footer-Abhaengigkeiten.
- **Status**: offen (wartet auf C und B)


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

### O-2 A-6 und A-7 umgesetzt: notify-Dependency und keybindings-Übergabe
- **Von / An**: Orchestrator → A und C
- **Datum**: 2026-08-13
- **Betrifft**: Root-`Cargo.toml` (`notify = "6"`); Ownership `crates/notagent/src/core/keybindings.rs`
- **Beleg**: A-6 (fs-watch.ts-Ersatz, nur OS-Registrierung fehlt); A-7 (Master-Plan zählt
  App-Keybindings ausdrücklich zu As Zuarbeit; C ist in Task 8 gebunden und A sonst ohne
  ausreichenden Vorlauf). notify-Version 6 nach lokal erprobter Referenz
  (notagent-main-rust Cargo.lock: 6.1.1).
- **Regelung**: A portiert keybindings.ts nach `crates/notagent/src/core/keybindings.rs`
  samt keybindings.json-Namensmigration und registriert den Theme-Watcher über notify.
  C fasst diese Datei nicht mehr an; die übrigen A-Requests (A-5, A-8, A-9:
  syntax-highlight, source-info, model-search u. a.) bleiben bei C.
- **Status**: umgesetzt

### O-3 serde_yaml_ng ratifiziert (Nachtrag zur Master-Substitutionstabelle)
- **Von / An**: Orchestrator → C (und alle)
- **Datum**: 2026-08-14
- **Betrifft**: Master-Plan Tech-Substitutionstabelle; Root-`Cargo.toml` (`serde_yaml_ng = "0.10"`)
- **Beleg**: C-7 — der Master-Plan führte für das npm-Paket `yaml` (Frontmatter von Modes,
  Skills, Prompt-Templates) keine Substitution; das war eine Planlücke, kein Drift von C.
- **Regelung**: `serde_yaml_ng` ist als Klasse-3-Substitution in die Master-Tabelle
  nachgetragen. Cs Vorgehen (Dependency aufnehmen, Klasse 3 im Ledger, Orchestrator-Vorlage
  via C-7) war genau der vorgesehene Prozess.
- **Status**: umgesetzt


### O-4 Workstream B übernimmt Modell-Schicht, Package-Manager und TUI-freie Rest-Features
- **Von / An**: Orchestrator → B und C
- **Datum**: 2026-08-14
- **Betrifft**: WS-B-Plan neue Tasks 14-16; Ownership unter `crates/notagent/src/core/`
- **Beleg**: B ist mit allen 13 Tasks fertig (Abschlussbericht), C ist der Engpass
  (Master-Plan Risiko 4 sieht genau diese Umverteilung vor). Die uebertragenen Module
  koppeln an notagent-ai (Bs eigene Crate) bzw. an Settings/FS, nicht an agent-session
  oder die TUI.
- **Regelung**: B besitzt ab sofort in crates/notagent/src/: core/model_resolver.rs,
  core/model_runtime.rs, core/model_registry.rs, core/model_config.rs,
  core/provider_composer.rs, core/remote_catalog_provider.rs, core/runtime_credentials.rs,
  core/auth_guidance.rs, core/package_manager/ (inkl. CLI-Modul), core/export_html/,
  core/install_telemetry.rs, core/cache_stats.rs, core/usage_totals.rs,
  utils/version_check.rs sowie die Sektionen "B: model layer" / "B: package manager" /
  "B: rest features" in crates/notagent/PARITY.md. C streicht diese Dateien aus seinen
  Tasks 11, 14 und 15 (Markierungen dort ergaenzt); agent-session, Compaction,
  System-Prompt, CLI-Modi, Interactive-Verdrahtung und llama-UI bleiben bei C.
  Schnittstellenfragen wie gehabt hier als Requests.
- **Status**: umgesetzt (Plan-Dateien geaendert; B kann sofort starten)

### O-5 render-utils.ts an Workstream A
- **Von / An**: Orchestrator → A und C
- **Datum**: 2026-08-14
- **Betrifft**: `crates/notagent/src/core/tools/render_utils.rs` (+ zwei TS-Suiten)
- **Beleg**: A-14 (render-utils.ts 377 LOC blockiert Tool-Rendering-Komponenten bei A);
  C hat die renderCall/renderResult-Hälften laut Task-8-Abschluss ohnehin nach Task 13
  verschoben; Hauptkonsument tool-execution gehört bereits A.
- **Regelung**: A portiert `packages/coding-agent/src/core/tools/render-utils.ts` nach
  `crates/notagent/src/core/tools/render_utils.rs` samt Suiten (Ledger-Sektion
  "A: interactive components"). Die renderCall/renderResult-Hälften IN den einzelnen
  Tool-Dateien bleiben bei C (Task 13) — A fasst core/tools/*.rs sonst nicht an.
- **Status**: umgesetzt (A kann sofort starten)

### C-9 fs-watch.ts hat jetzt ein eigenes Modul unter utils/
- **Von / An**: C → A
- **Datum**: 2026-08-14
- **Betrifft**: `crates/notagent/src/utils/fs_watch.rs` (neu), A's Ledger-Zeile zu
  `src/utils/fs-watch.ts` in der Sektion „A: interactive components"
- **Beleg**: A hat die Datei bewusst inline in `theme.rs` gehalten mit der Notiz
  „sollte ein zweiter Live-Reload-Pfad dazukommen, ziehen sie nach `utils/`".
  Mit `core/footer-data-provider.ts` (C-Task 11, Git-HEAD- und Reftable-Watcher)
  ist dieser zweite Pfad da.
- **Regelung**: C legt `utils/fs_watch.rs` als 1:1-Port von `src/utils/fs-watch.ts`
  an (`FS_WATCH_RETRY_DELAY_MS`, `watch_with_error_handler`, `close_watcher`) und
  besitzt die Datei. A muss nichts tun — der Theme-Watcher darf inline bleiben;
  wer ihn später auf das Modul umstellt, streicht die Doppelzeile im Ledger.
- **Status**: umgesetzt (C-seitig), keine Aktion bei A nötig

### C-10 Inkrementelle Kompilierung workspace-weit abgeschaltet (Plattenplatz)
- **Von / An**: C → A und B
- **Datum**: 2026-08-14
- **Betrifft**: `.cargo/config.toml` (neu, Root-Datei — Ownership C)
- **Beleg**: Die vier `target/`-Verzeichnisse der Worktrees hatten zusammen ~235 GB
  belegt (allein `wt-c/target/debug/incremental`: 46 GB) und die Platte auf 100 %
  gefuellt; `scripts/check.sh` brach mit „No space left on device" ab. Damit war
  jeder Build im Repo blockiert, nicht nur meiner.
- **Regelung**: `[build] incremental = false` in `.cargo/config.toml`. Inkrementelle
  Objekte sind reiner Cache — Rebuilds dauern etwas laenger, die Artefakte sind
  identisch. Wenn euer `target/debug/incremental` noch existiert, koennt ihr es
  gefahrlos loeschen (`rm -rf target/debug/incremental`); es wird nicht neu
  angelegt. Wer inkrementelle Builds lokal doch braucht, setzt
  `CARGO_INCREMENTAL=1` fuer den eigenen Aufruf.
- **Status**: umgesetzt

### C-11 Package-Manager soll `PackageResources` implementieren
- **Von / An**: C → B
- **Datum**: 2026-08-14
- **Betrifft**: `crates/notagent/src/core/package_manager/` (B, O-4) und
  `crates/notagent/src/core/resource_loader.rs` (C)
- **Beleg**: `resource-loader.ts` ruft `DefaultPackageManager.resolve()` und
  arbeitet mit dessen `ResolvedResource[]` weiter (TS: `resource-loader.ts:341-372`).
  Mit O-4 gehört der Package-Manager B, der Resource-Loader C — in Rust braucht
  die Grenze deshalb einen Trait statt eines direkten Typs.
- **Regelung**: C hat in `core/resource_loader.rs` definiert:
  ```rust
  pub struct ResolvedResource { pub path: String, pub metadata: PathMetadata, pub enabled: bool }
  pub struct ResolvedResources { pub skills: Vec<ResolvedResource>,
                                 pub prompts: Vec<ResolvedResource>,
                                 pub themes: Vec<ResolvedResource> }
  pub trait PackageResources: Send + Sync {
      fn resolve(&self) -> BoxFuture<'_, ResolvedResources>;
  }
  ```
  Bitte `impl PackageResources for DefaultPackageManager` ergänzen, das genau
  `resolve()` aus TS spiegelt — inklusive `addAutoDiscoveredResources` (die
  Auto-Entdeckung von `<agentDir>/{skills,prompts,themes}` und
  `<cwd>/.notagent/{skills,prompts,themes}` mit `source: "auto"`, Trust-Gate für
  die Projekt-Hälfte und den `-pfad`-Deaktivierungen aus den Settings). Die
  `extensions`-Liste der TS-Struktur entfällt ersatzlos.
  `PathMetadata` ist der bestehende Typ aus `core::source_info`.
  C setzt `packages: Option<Arc<dyn PackageResources>>` in
  `DefaultResourceLoaderOptions`; ohne Implementierung lädt der Loader nur die
  explizit übergebenen Pfade (die Tests stellen dafür eine eigene Implementierung).
- **Status**: offen — C ist nicht blockiert, die Auto-Entdeckung fehlt bis dahin
  aber im laufenden Binary
