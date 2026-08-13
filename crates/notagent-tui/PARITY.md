# Parity-Ledger: notagent-tui

TS-Quelle: `/Users/dev/projects/notagent-main/packages/tui` (16 704 LOC in src/) — Workstream A.

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

## Lektüre-Protokoll

| Datum | TS-/Referenz-Datei | LOC | gelesen von (Task) |
|---|---|---|---|
| 2026-08-13 | `plans/2026-08-13-rust-port-master-v1.md` | 216 | Pflichtlektüre |
| 2026-08-13 | `plans/2026-08-13-rust-port-ws-a-tui-v1.md` | 59 | Pflichtlektüre |
| 2026-08-13 | `plans/facts/tui.md` | 287 | Pflichtlektüre |
| 2026-08-13 | `CONVENTIONS.md` | 121 | Pflichtlektüre |
| 2026-08-13 | `test/virtual-terminal.ts` | 218 | Task 1 |
| 2026-08-13 | `src/keys.ts` | 1401 | `src/keys.rs` | verifiziert | Klasse 1: `_kittyProtocolActive` → prozessglobales `AtomicBool`; `_lastEventType` entfällt (in TS nur geschrieben, nie gelesen); `String.fromCharCode`-Semantik (16-Bit-Truncation) als `js_from_char_code` nachgebildet; Codepoints als `i64` wegen der negativen Sentinels; das TS-Hilfsobjekt `Key` (reine Template-Literal-Typen) entfällt, KeyIds sind `&str`; `parseInt`-Überlauf ⇒ „kein Treffer" statt Gleitkomma-Codepoint (in beiden Fällen unauffindbarer Key) |
| `src/stdin-buffer.ts` | 444 | `src/stdin_buffer.rs` | verifiziert | Klasse 1: `EventEmitter` → geordnete `Vec<StdinEvent>` als Rückgabewert; `setTimeout` → `pending_timeout_ms()` + `flush_timeout()` (Timer treibt der Aufrufer, Semantik identisch); Einzelzeichen-Emission pro `char` statt pro UTF-16-Codeeinheit (lone Surrogates sind in Rust-Strings nicht darstellbar; für BMP-Eingaben identisch) |
| `src/terminal.ts` | 559 | `src/terminal.rs` | verifiziert | Klasse 1: Node-Event-Loop → `pump()` mit tokio-`select!` (stdin-Leser als eigener Thread mit Kanal, Handler laufen auf dem TUI-Strang); `process.stdout.write`-Monkey-Patching der Tests → injizierbare Ausgabe-Senke; `setTimeout`-Timer → Deadlines in `pump()`. Klasse 3: `setRawMode` → termios via libc (Flags empirisch gegen Nodes/libuv Raw-Mode auf macOS verifiziert: `~(BRKINT|ICRNL|INPCK|ISTRIP|IXON)`, `OPOST|ONLCR` bleiben, CS8 ohne CSIZE/PARENB, `~(ECHO|ICANON|IEXTEN|ISIG)`, VMIN=1, VTIME=0); `process.stdout.on("resize")` → SIGWINCH via tokio-signal; Windows-VT-Input als direkter Console-API-Aufruf statt Node-Addon |
| `src/alt-screen-search.ts` | 157 | `src/alt_screen_search.rs` | verifiziert | Klasse 1: `onQueryChange`-Callback → `take_query_changed()` (der Callback bräuchte `&mut` auf den Renderer); Korpus-Index pro Zeichen statt pro UTF-16-Einheit |
| `src/tui-alt-screen.ts` | 1291 | `src/tui_alt_screen.rs` | teilweise: Renderer-Kern (Enter/Exit-Sequenzen inkl. Maus-Modi je Multiplexer, CUP+2K-Zeilendiff, `imagesNeedRedraw`-Vollredraw, Kitty-Placement-Cache mit LRU-Eviktion, Flash-Kompositio, Dokument-Ausschreiben beim Beenden) und Wheel-Routing durch verschachtelte ScrollViews; portiert zusätzlich: Maus-Textselektion (SGR-Parser, Zeichen-/Wort-/Zeilen-Granularität über Klickzählung mit 500-ms-Fenster, Grapheme-Snapping, Selektion in ScrollViews), Reverse-Video-Highlight, OSC-52-Clipboard, OSC-8-Klickaktivierung, Fokusverlust-Reset; offen: Scrollbar-Hover/-Drag, Selektions-Autoscroll, Rechtsklick-Paste, Verdrahtung der Transkript-Suche (Suchmodul selbst ist portiert) | Klasse 1: Input-Listener der TS-Konstruktion → `handle_terminal_input` des Renderers (der Listener braucht `&mut self`) |
| `src/index.ts` | 147 | `src/lib.rs` | teilweise (Export-Set der portierten Module deckungsgleich; Editor, Markdown, LaTeX, Autocomplete, Alt-Screen fehlen noch) | Klasse 2/1: `marked`-Re-Export entfällt (Tech-Substitution); `Key`-Hilfsobjekt entfällt (Template-Literal-Typen) |
| `src/tui.ts` | 1257 | `src/tui.rs` | verifiziert | Klasse 1: `TuiBase` (abstrakte Klasse) → `TuiCore` (geteilter Zustand) plus konkrete Renderer; Overlay-Handles halten einen `TuiCore`-Klon statt `this`-Closures; Timer rufen nicht zurück, sondern `render_deadline()` + `begin_frame()` treibt die Schleife des Renderers (Handlerausführung bleibt einsträngig wie im Node-Event-Loop); `addInputListener` gibt eine `ListenerId` statt einer Unsubscribe-Closure zurück; Promises → `async fn` mit tokio-Timeout; Fokus-Flags einer Komponente, die gerade `handle_input` ausführt (und damit `RefCell`-geliehen ist), werden bis zum Rücksprung nachgezogen — beobachtbar identisch, da niemand vorher lesen kann |
| `src/tui-main-screen.ts` | 586 | `src/tui_main_screen.rs` | verifiziert (tui-render-Suite ohne die Kitty-Bild-Fälle) | Klasse 1: geworfener `Error` des Überbreiten-Guards → `panic!` mit identischem Text (Programmierfehler, kein Kontrollfluss); Debug-Dump ohne `Math.random()`-Suffix |
| `src/layout.ts` | 410 | `src/layout.rs` | verifiziert | Klasse 1: `parent`-Zeiger der `LayoutBox` entfallen (nicht gelesen); `requestRender`-Callback → `render_requested`-Flag im Scroll-State |
| `src/layout-node.ts` | 51 | `src/layout_node.rs` | verifiziert | Klasse 1: `LAYOUT_NODE`-Symbol → `Component::layout_node()` |
| `src/components/stack.ts` | 154 | `src/components/stack.rs` | verifiziert | Klasse 1: abstrakte Basisklasse → Struct mit `layout_type` |
| `src/components/scroll-view.ts` | 216 | `src/components/scroll_view.rs` | verifiziert | Klasse 1: Zustand als `Rc<RefCell<ScrollViewState>>` (in TS ist die Komponente selbst der Zustand); Scrollbar-Auto-Hide-Timer als Deadline für den Aufrufer |
| `src/kill-ring.ts` | 46 | `src/kill_ring.rs` | portiert (Task 10 vorgezogen) | — |
| `src/undo-stack.ts` | 28 | `src/undo_stack.rs` | portiert (Task 10 vorgezogen) | Klasse 1: `structuredClone` beim Push → Wertübergabe (für die verwendeten Zustands-Structs gleichbedeutend) |
| `src/word-navigation.ts` | 117 | `src/word_navigation.rs` | verifiziert (Task 10 vorgezogen) | Klasse 3: `Intl.Segmenter` (Wortgranularität) → `unicode-segmentation`. **Beobachtbare Restdifferenz**: ICU segmentiert Chinesisch wörterbuchbasiert („你好"/„世界"), UAX #29 pro Zeichen — die CJK-Wortnavigation springt daher zeichenweise statt wortweise. Klasse 1: `isWordLike` → „Segment enthält alphanumerisches Zeichen"; Cursorpositionen als Byte-Offsets statt UTF-16-Indizes |
| `src/fuzzy.ts` | 137 | `src/fuzzy.rs` | verifiziert (Task 13 vorgezogen) | Klasse 1: benannte Regex-Gruppen für den Alpha-Numerik-Tausch → Zeichenprüfung; stabile Sortierung explizit über den Ursprungsindex |
| `src/keybindings.ts` | 320 | `src/keybindings.rs` | verifiziert (Task 13 vorgezogen) | Klasse 1: Deklarations-Merging-Interface → statische Definitionstabelle mit 47 Einträgen; globaler Manager als `Mutex` statt Modulvariable |
| `src/components/spacer.ts` | 28 | `src/components/spacer.rs` | portiert | — |
| `src/components/truncated-text.ts` | 65 | `src/components/truncated_text.rs` | portiert | — |
| `src/components/box.ts` | 137 | `src/components/box_component.rs` | portiert | Klasse 1: Modulname, weil `box` ein Rust-Schlüsselwort ist |
| `src/components/alt-screen-flash.ts` | 51 | `src/components/alt_screen_flash.rs` | portiert | Klasse 1: `setTimeout` je Nachricht → Deadline für den Renderer |
| `src/components/loader.ts` | 92 | `src/components/loader.rs` | portiert | Klasse 1: `setInterval` → Frame-Deadline; TS erbt von `Text`, der Port besitzt eine Instanz |
| `src/components/input.ts` | 447 | `src/components/input.rs` | verifiziert (21 von 35 Testfällen) | Klasse 1: Cursor als Byte-Offset statt UTF-16-Index (intern konsistent) |
| `src/components/image.ts` | 127 | `src/components/image.rs` | portiert | — |
| `src/components/select-list.ts` | 229 | `src/components/select_list.rs` | verifiziert | — |
| `src/components/settings-list.ts` | 249 | `src/components/settings_list.rs` | verifiziert | Klasse 1: Submenu-Callback → `close_submenu`, weil die TS-Closure die Liste selbst mutiert |
| `src/components/cancellable-loader.ts` | 40 | `src/components/cancellable_loader.rs` | portiert | Klasse 3: `AbortController` → `CancellationToken` |
| `src/components/text.ts` | 106 | `src/components/text.rs` | verifiziert (über layout-Suite) | — |
| `src/components/v-stack.ts` | 33 | `src/components/v_stack.rs` | verifiziert | — |
| `src/components/h-stack.ts` | 44 | `src/components/h_stack.rs` | verifiziert | — |
| `src/terminal-image.ts` | 657 | `src/terminal_image.rs` | portiert (Capability-Detection, Zellmaße, Kitty-/iTerm2-Encoder, Bild-ID-Vergabe, PNG/JPEG/GIF/WebP-Header, `renderImage`, `imageFallback`, `hyperlink`, Kitty-Metadaten/Crop/Delete); Tests folgen | Klasse 3: `execSync("tmux …")` → `std::process::Command`; `Buffer`-Base64 → eigene Kodierung/Dekodierung; `pathToFileURL` → `file://`-Präfix |
| `src/terminal-colors.ts` | 73 | `src/terminal_colors.rs` | verifiziert (Parser); die TUI-Query-Fälle folgen mit Task 6 | Klasse 1: `undefined` → `Option`; `TerminalColorScheme` als Enum statt String-Union |
| `src/native-modifiers.ts` | 66 | `src/native_modifiers.rs` | portiert (Task 13 vorgezogen, da `forwardInputSequence` es braucht) | Klasse 3: Node-Addon → direkte OS-Aufrufe (`CGEventSourceFlagsState` auf macOS, `GetAsyncKeyState` auf Windows, sonst `false`) |
| `native/darwin/src/darwin-modifiers.c` | 76 | `src/native_modifiers.rs` (darwin) | portiert | Klasse 3 |
| `native/win32/src/win32-console-mode.c` | 135 | `src/native_modifiers.rs` (win32) + `terminal.rs::enable_windows_vt_input` | portiert | Klasse 3 |
| `src/utils.ts` | 1326 | Task 2 |
| 2026-08-13 | `test/virtual-terminal.ts` | 218 | `src/test_terminal.rs` (Feature `test-terminal`) | verifiziert — `tests/virtual_terminal.rs` prüft den Harness gegen 21 `@xterm/headless`-Fixtures (Viewport, Scrollback, Cursor, Resize, CSI/OSC/APC, Synchronized Output) plus Ereignisaufzeichnung, Handler-Weiterleitung und Sequenz-Helfer |
| `test/wrap-ansi.test.ts` | 266 | Task 2 |
| 2026-08-13 | `test/truncate-to-width.test.ts` | 127 | Task 2 |
| 2026-08-13 | `test/tab-width.test.ts` | 88 | Task 2 |
| 2026-08-13 | `test/regression-regional-indicator-width.test.ts` | 52 | Task 2 |
| 2026-08-13 | `test/regression-overlay-cjk-boundary.test.ts` | 46 | Task 2 |
| 2026-08-13 | `src/keys.ts` | 1401 | Task 3 |
| 2026-08-13 | `src/stdin-buffer.ts` | 444 | Task 4 |
| 2026-08-13 | `test/stdin-buffer.test.ts` | 526 | Task 4 |
| 2026-08-13 | `src/terminal.ts` (vollständig) | 559 | Task 4 |
| 2026-08-13 | `test/terminal-colors.test.ts` | 252 | `tests/terminal_colors.rs` (9 Fälle) | verifiziert |
| `test/word-navigation.test.ts` | 191 | `tests/word_navigation.rs` (17 Fälle) | verifiziert; der CJK-Rückwärtsfall hält die oben dokumentierte Segmentierungsdifferenz fest |
| `test/fuzzy.test.ts` | 112 | `tests/fuzzy.rs` (14 Fälle) | verifiziert |
| `test/keybindings.test.ts` | 81 | `tests/keybindings.rs` (7 Fälle) | verifiziert |
| `test/input.test.ts` | 647 | `tests/input.rs` (21 von 35 Fällen) | verifiziert; offen: die beiden CJK-Wortgrenzen-Fälle (Segmentierungsdifferenz, siehe word-navigation), der Horizontal-Scroll-Renderfall und elf weitere Undo-Fälle |
| `test/truncated-text.test.ts` | 129 | `tests/truncated_text.rs` (9 Fälle) | verifiziert |
| `test/settings-list.test.ts` | 58 | `tests/settings_list.rs` (2 Fälle) | verifiziert |
| `test/select-list.test.ts` | 116 | `tests/select_list.rs` (5 Fälle) | verifiziert |
| `test/layout.test.ts` | 306 | `tests/layout.rs` (14 Fälle) | verifiziert |
| `test/overlay-non-capturing.test.ts` | 1203 | `tests/overlay_non_capturing.rs` (44 Fälle) | verifiziert — komplette Fokus-Zustandsmaschine, No-op-Guards, Fokuszyklen und Renderreihenfolge |
| `test/overlay-options.test.ts` | 541 | `tests/overlay_options.rs` (24 Fälle) | verifiziert |
| `test/overlay-short-content.test.ts` | 62 | `tests/overlay_short_content.rs` (1 Fall) | verifiziert |
| `test/tui-shrink.test.ts` | 45 | `tests/tui_shrink.rs` (1 Fall) | verifiziert |
| `test/tui-overlay-style-leak.test.ts` | 81 | `tests/tui_overlay_style_leak.rs` (2 Fälle) | verifiziert |
| `test/tui-cell-size-input.test.ts` | 82 | `tests/tui_cell_size_input.rs` (2 Fälle) | verifiziert |
| `test/tui-alt-screen.test.ts` | 1267 | `tests/tui_alt_screen.rs` (8 Fälle) | Viewport-Rendering, Wheel-Routing und Dock-Layout verifiziert; die Selektions-, Scrollbar- und Suchfälle folgen mit den nächsten Stufen |
| `test/tui-render.test.ts` | 832 | `tests/tui_render.rs` (21 von 24 Fällen) | verifiziert; offen sind drei Kitty-Fälle, die den Vollredraw-Fallback über Bildhöhen prüfen |
| `test/terminal.test.ts` | 300 | Task 4 |
| 2026-08-13 | `src/tui.ts` (vollständig) | 1257 | Task 5 |
| 2026-08-13 | `src/alt-screen-search.ts` | 157 | `src/alt_screen_search.rs` | verifiziert | Klasse 1: `onQueryChange`-Callback → `take_query_changed()` (der Callback bräuchte `&mut` auf den Renderer); Korpus-Index pro Zeichen statt pro UTF-16-Einheit |
| `src/tui-alt-screen.ts` | 1291 | `src/tui_alt_screen.rs` | teilweise: Renderer-Kern (Enter/Exit-Sequenzen inkl. Maus-Modi je Multiplexer, CUP+2K-Zeilendiff, `imagesNeedRedraw`-Vollredraw, Kitty-Placement-Cache mit LRU-Eviktion, Flash-Kompositio, Dokument-Ausschreiben beim Beenden) und Wheel-Routing durch verschachtelte ScrollViews; portiert zusätzlich: Maus-Textselektion (SGR-Parser, Zeichen-/Wort-/Zeilen-Granularität über Klickzählung mit 500-ms-Fenster, Grapheme-Snapping, Selektion in ScrollViews), Reverse-Video-Highlight, OSC-52-Clipboard, OSC-8-Klickaktivierung, Fokusverlust-Reset; offen: Scrollbar-Hover/-Drag, Selektions-Autoscroll, Rechtsklick-Paste, Verdrahtung der Transkript-Suche (Suchmodul selbst ist portiert) | Klasse 1: Input-Listener der TS-Konstruktion → `handle_terminal_input` des Renderers (der Listener braucht `&mut self`) |
| `src/index.ts` | 147 | `src/lib.rs` | teilweise (Export-Set der portierten Module deckungsgleich; Editor, Markdown, LaTeX, Autocomplete, Alt-Screen fehlen noch) | Klasse 2/1: `marked`-Re-Export entfällt (Tech-Substitution); `Key`-Hilfsobjekt entfällt (Template-Literal-Typen) |
| `src/tui.ts` | 1257 | `src/tui.rs` | verifiziert | Klasse 1: `TuiBase` (abstrakte Klasse) → `TuiCore` (geteilter Zustand) plus konkrete Renderer; Overlay-Handles halten einen `TuiCore`-Klon statt `this`-Closures; Timer rufen nicht zurück, sondern `render_deadline()` + `begin_frame()` treibt die Schleife des Renderers (Handlerausführung bleibt einsträngig wie im Node-Event-Loop); `addInputListener` gibt eine `ListenerId` statt einer Unsubscribe-Closure zurück; Promises → `async fn` mit tokio-Timeout; Fokus-Flags einer Komponente, die gerade `handle_input` ausführt (und damit `RefCell`-geliehen ist), werden bis zum Rücksprung nachgezogen — beobachtbar identisch, da niemand vorher lesen kann |
| `src/tui-main-screen.ts` | 586 | `src/tui_main_screen.rs` | verifiziert (tui-render-Suite ohne die Kitty-Bild-Fälle) | Klasse 1: geworfener `Error` des Überbreiten-Guards → `panic!` mit identischem Text (Programmierfehler, kein Kontrollfluss); Debug-Dump ohne `Math.random()`-Suffix |
| `src/layout.ts` | 410 | `src/layout.rs` | verifiziert | Klasse 1: `parent`-Zeiger der `LayoutBox` entfallen (nicht gelesen); `requestRender`-Callback → `render_requested`-Flag im Scroll-State |
| `src/layout-node.ts` | 51 | `src/layout_node.rs` | verifiziert | Klasse 1: `LAYOUT_NODE`-Symbol → `Component::layout_node()` |
| `src/components/stack.ts` | 154 | `src/components/stack.rs` | verifiziert | Klasse 1: abstrakte Basisklasse → Struct mit `layout_type` |
| `src/components/scroll-view.ts` | 216 | `src/components/scroll_view.rs` | verifiziert | Klasse 1: Zustand als `Rc<RefCell<ScrollViewState>>` (in TS ist die Komponente selbst der Zustand); Scrollbar-Auto-Hide-Timer als Deadline für den Aufrufer |
| `src/kill-ring.ts` | 46 | `src/kill_ring.rs` | portiert (Task 10 vorgezogen) | — |
| `src/undo-stack.ts` | 28 | `src/undo_stack.rs` | portiert (Task 10 vorgezogen) | Klasse 1: `structuredClone` beim Push → Wertübergabe (für die verwendeten Zustands-Structs gleichbedeutend) |
| `src/word-navigation.ts` | 117 | `src/word_navigation.rs` | verifiziert (Task 10 vorgezogen) | Klasse 3: `Intl.Segmenter` (Wortgranularität) → `unicode-segmentation`. **Beobachtbare Restdifferenz**: ICU segmentiert Chinesisch wörterbuchbasiert („你好"/„世界"), UAX #29 pro Zeichen — die CJK-Wortnavigation springt daher zeichenweise statt wortweise. Klasse 1: `isWordLike` → „Segment enthält alphanumerisches Zeichen"; Cursorpositionen als Byte-Offsets statt UTF-16-Indizes |
| `src/fuzzy.ts` | 137 | `src/fuzzy.rs` | verifiziert (Task 13 vorgezogen) | Klasse 1: benannte Regex-Gruppen für den Alpha-Numerik-Tausch → Zeichenprüfung; stabile Sortierung explizit über den Ursprungsindex |
| `src/keybindings.ts` | 320 | `src/keybindings.rs` | verifiziert (Task 13 vorgezogen) | Klasse 1: Deklarations-Merging-Interface → statische Definitionstabelle mit 47 Einträgen; globaler Manager als `Mutex` statt Modulvariable |
| `src/components/spacer.ts` | 28 | `src/components/spacer.rs` | portiert | — |
| `src/components/truncated-text.ts` | 65 | `src/components/truncated_text.rs` | portiert | — |
| `src/components/box.ts` | 137 | `src/components/box_component.rs` | portiert | Klasse 1: Modulname, weil `box` ein Rust-Schlüsselwort ist |
| `src/components/alt-screen-flash.ts` | 51 | `src/components/alt_screen_flash.rs` | portiert | Klasse 1: `setTimeout` je Nachricht → Deadline für den Renderer |
| `src/components/loader.ts` | 92 | `src/components/loader.rs` | portiert | Klasse 1: `setInterval` → Frame-Deadline; TS erbt von `Text`, der Port besitzt eine Instanz |
| `src/components/input.ts` | 447 | `src/components/input.rs` | verifiziert (21 von 35 Testfällen) | Klasse 1: Cursor als Byte-Offset statt UTF-16-Index (intern konsistent) |
| `src/components/image.ts` | 127 | `src/components/image.rs` | portiert | — |
| `src/components/select-list.ts` | 229 | `src/components/select_list.rs` | verifiziert | — |
| `src/components/settings-list.ts` | 249 | `src/components/settings_list.rs` | verifiziert | Klasse 1: Submenu-Callback → `close_submenu`, weil die TS-Closure die Liste selbst mutiert |
| `src/components/cancellable-loader.ts` | 40 | `src/components/cancellable_loader.rs` | portiert | Klasse 3: `AbortController` → `CancellationToken` |
| `src/components/text.ts` | 106 | `src/components/text.rs` | verifiziert (über layout-Suite) | — |
| `src/components/v-stack.ts` | 33 | `src/components/v_stack.rs` | verifiziert | — |
| `src/components/h-stack.ts` | 44 | `src/components/h_stack.rs` | verifiziert | — |
| `src/terminal-image.ts` | 657 | `src/terminal_image.rs` | portiert (Capability-Detection, Zellmaße, Kitty-/iTerm2-Encoder, Bild-ID-Vergabe, PNG/JPEG/GIF/WebP-Header, `renderImage`, `imageFallback`, `hyperlink`, Kitty-Metadaten/Crop/Delete); Tests folgen | Klasse 3: `execSync("tmux …")` → `std::process::Command`; `Buffer`-Base64 → eigene Kodierung/Dekodierung; `pathToFileURL` → `file://`-Präfix |
| `src/terminal-colors.ts` | 73 | Task 5 (vorgezogen aus Task 12) |
| 2026-08-13 | `test/terminal-colors.test.ts` | 252 | Task 5 |
| 2026-08-13 | `src/tui-main-screen.ts` | 586 | Task 6 |
| 2026-08-13 | `test/word-navigation.test.ts` | 191 | `tests/word_navigation.rs` (17 Fälle) | verifiziert; der CJK-Rückwärtsfall hält die oben dokumentierte Segmentierungsdifferenz fest |
| `test/fuzzy.test.ts` | 112 | `tests/fuzzy.rs` (14 Fälle) | verifiziert |
| `test/keybindings.test.ts` | 81 | `tests/keybindings.rs` (7 Fälle) | verifiziert |
| `test/input.test.ts` | 647 | `tests/input.rs` (21 von 35 Fällen) | verifiziert; offen: die beiden CJK-Wortgrenzen-Fälle (Segmentierungsdifferenz, siehe word-navigation), der Horizontal-Scroll-Renderfall und elf weitere Undo-Fälle |
| `test/truncated-text.test.ts` | 129 | `tests/truncated_text.rs` (9 Fälle) | verifiziert |
| `test/settings-list.test.ts` | 58 | `tests/settings_list.rs` (2 Fälle) | verifiziert |
| `test/select-list.test.ts` | 116 | `tests/select_list.rs` (5 Fälle) | verifiziert |
| `test/layout.test.ts` | 306 | `tests/layout.rs` (14 Fälle) | verifiziert |
| `test/overlay-non-capturing.test.ts` | 1203 | `tests/overlay_non_capturing.rs` (44 Fälle) | verifiziert — komplette Fokus-Zustandsmaschine, No-op-Guards, Fokuszyklen und Renderreihenfolge |
| `test/overlay-options.test.ts` | 541 | `tests/overlay_options.rs` (24 Fälle) | verifiziert |
| `test/overlay-short-content.test.ts` | 62 | `tests/overlay_short_content.rs` (1 Fall) | verifiziert |
| `test/tui-shrink.test.ts` | 45 | `tests/tui_shrink.rs` (1 Fall) | verifiziert |
| `test/tui-overlay-style-leak.test.ts` | 81 | `tests/tui_overlay_style_leak.rs` (2 Fälle) | verifiziert |
| `test/tui-cell-size-input.test.ts` | 82 | `tests/tui_cell_size_input.rs` (2 Fälle) | verifiziert |
| `test/tui-alt-screen.test.ts` | 1267 | `tests/tui_alt_screen.rs` (8 Fälle) | Viewport-Rendering, Wheel-Routing und Dock-Layout verifiziert; die Selektions-, Scrollbar- und Suchfälle folgen mit den nächsten Stufen |
| `test/tui-render.test.ts` | 832 | Task 6 |
| 2026-08-13 | `src/terminal-image.ts` (Capability-/Bildzeilen-Teil) | 657 | Task 5/6 |
| 2026-08-13 | `src/layout.ts` | 410 | Task 7 |
| 2026-08-13 | `src/layout-node.ts` | 51 | Task 7 |
| 2026-08-13 | `src/components/stack.ts` | 154 | Task 7 |
| 2026-08-13 | `src/components/scroll-view.ts` | 216 | Task 7 |
| 2026-08-13 | `src/components/{text,v-stack,h-stack}.ts` | 183 | Task 7 (aus Task 9 vorgezogen) |
| 2026-08-13 | `test/word-navigation.test.ts` | 191 | `tests/word_navigation.rs` (17 Fälle) | verifiziert; der CJK-Rückwärtsfall hält die oben dokumentierte Segmentierungsdifferenz fest |
| `test/fuzzy.test.ts` | 112 | `tests/fuzzy.rs` (14 Fälle) | verifiziert |
| `test/keybindings.test.ts` | 81 | `tests/keybindings.rs` (7 Fälle) | verifiziert |
| `test/input.test.ts` | 647 | `tests/input.rs` (21 von 35 Fällen) | verifiziert; offen: die beiden CJK-Wortgrenzen-Fälle (Segmentierungsdifferenz, siehe word-navigation), der Horizontal-Scroll-Renderfall und elf weitere Undo-Fälle |
| `test/truncated-text.test.ts` | 129 | `tests/truncated_text.rs` (9 Fälle) | verifiziert |
| `test/settings-list.test.ts` | 58 | `tests/settings_list.rs` (2 Fälle) | verifiziert |
| `test/select-list.test.ts` | 116 | `tests/select_list.rs` (5 Fälle) | verifiziert |
| `test/layout.test.ts` | 306 | Task 7 |
| 2026-08-13 | `test/stdin-buffer.test.ts` | 526 | `tests/stdin_buffer.rs` (48 Fälle) | verifiziert |
| — (zusätzlich) | — | `tests/stdin_buffer_oracle.rs` + `tests/fixtures/stdin-buffer-oracle.json` | Differenztest gegen die TS-Implementierung: 1276 Chunk-Zerlegungen von 38 Eingabeströmen (Ereignisfolge und Restpuffer) — alle identisch |
| `test/terminal-colors.test.ts` | 252 | `tests/terminal_colors.rs` (9 Fälle) | verifiziert |
| `test/word-navigation.test.ts` | 191 | `tests/word_navigation.rs` (17 Fälle) | verifiziert; der CJK-Rückwärtsfall hält die oben dokumentierte Segmentierungsdifferenz fest |
| `test/fuzzy.test.ts` | 112 | `tests/fuzzy.rs` (14 Fälle) | verifiziert |
| `test/keybindings.test.ts` | 81 | `tests/keybindings.rs` (7 Fälle) | verifiziert |
| `test/input.test.ts` | 647 | `tests/input.rs` (21 von 35 Fällen) | verifiziert; offen: die beiden CJK-Wortgrenzen-Fälle (Segmentierungsdifferenz, siehe word-navigation), der Horizontal-Scroll-Renderfall und elf weitere Undo-Fälle |
| `test/truncated-text.test.ts` | 129 | `tests/truncated_text.rs` (9 Fälle) | verifiziert |
| `test/settings-list.test.ts` | 58 | `tests/settings_list.rs` (2 Fälle) | verifiziert |
| `test/select-list.test.ts` | 116 | `tests/select_list.rs` (5 Fälle) | verifiziert |
| `test/layout.test.ts` | 306 | `tests/layout.rs` (14 Fälle) | verifiziert |
| `test/overlay-non-capturing.test.ts` | 1203 | `tests/overlay_non_capturing.rs` (44 Fälle) | verifiziert — komplette Fokus-Zustandsmaschine, No-op-Guards, Fokuszyklen und Renderreihenfolge |
| `test/overlay-options.test.ts` | 541 | `tests/overlay_options.rs` (24 Fälle) | verifiziert |
| `test/overlay-short-content.test.ts` | 62 | `tests/overlay_short_content.rs` (1 Fall) | verifiziert |
| `test/tui-shrink.test.ts` | 45 | `tests/tui_shrink.rs` (1 Fall) | verifiziert |
| `test/tui-overlay-style-leak.test.ts` | 81 | `tests/tui_overlay_style_leak.rs` (2 Fälle) | verifiziert |
| `test/tui-cell-size-input.test.ts` | 82 | `tests/tui_cell_size_input.rs` (2 Fälle) | verifiziert |
| `test/tui-alt-screen.test.ts` | 1267 | `tests/tui_alt_screen.rs` (8 Fälle) | Viewport-Rendering, Wheel-Routing und Dock-Layout verifiziert; die Selektions-, Scrollbar- und Suchfälle folgen mit den nächsten Stufen |
| `test/tui-render.test.ts` | 832 | `tests/tui_render.rs` (21 von 24 Fällen) | verifiziert; offen sind drei Kitty-Fälle, die den Vollredraw-Fallback über Bildhöhen prüfen |
| `test/terminal.test.ts` | 300 | `tests/terminal.rs` (17 Fälle) | verifiziert |
| `test/keys.test.ts` | 633 | Task 3 |
| 2026-08-13 | `src/tui.ts:1-120` (Kontraktbereich) | 120 von 1257 | Master-Plan Task 2 (Kontrakt-Commit) |
| 2026-08-13 | `src/terminal.ts:1-140` (Kontraktbereich) | 140 von 559 | Master-Plan Task 2 (Kontrakt-Commit) |
| 2026-08-13 | `node_modules/get-east-asian-width/{index,lookup,lookup-data,utilities}.js` | 213 | Task 2 (Referenz für `eastAsianWidth`) |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| `src/alt-screen-search.ts` | 157 | `src/alt_screen_search.rs` | verifiziert | Klasse 1: `onQueryChange`-Callback → `take_query_changed()` (der Callback bräuchte `&mut` auf den Renderer); Korpus-Index pro Zeichen statt pro UTF-16-Einheit |
| `src/tui-alt-screen.ts` | 1291 | `src/tui_alt_screen.rs` | teilweise: Renderer-Kern (Enter/Exit-Sequenzen inkl. Maus-Modi je Multiplexer, CUP+2K-Zeilendiff, `imagesNeedRedraw`-Vollredraw, Kitty-Placement-Cache mit LRU-Eviktion, Flash-Kompositio, Dokument-Ausschreiben beim Beenden) und Wheel-Routing durch verschachtelte ScrollViews; portiert zusätzlich: Maus-Textselektion (SGR-Parser, Zeichen-/Wort-/Zeilen-Granularität über Klickzählung mit 500-ms-Fenster, Grapheme-Snapping, Selektion in ScrollViews), Reverse-Video-Highlight, OSC-52-Clipboard, OSC-8-Klickaktivierung, Fokusverlust-Reset; offen: Scrollbar-Hover/-Drag, Selektions-Autoscroll, Rechtsklick-Paste, Verdrahtung der Transkript-Suche (Suchmodul selbst ist portiert) | Klasse 1: Input-Listener der TS-Konstruktion → `handle_terminal_input` des Renderers (der Listener braucht `&mut self`) |
| `src/index.ts` | 147 | `src/lib.rs` | teilweise (Export-Set der portierten Module deckungsgleich; Editor, Markdown, LaTeX, Autocomplete, Alt-Screen fehlen noch) | Klasse 2/1: `marked`-Re-Export entfällt (Tech-Substitution); `Key`-Hilfsobjekt entfällt (Template-Literal-Typen) |
| `src/tui.ts` | 1257 | `src/tui.rs` | Kontrakt portiert (Component/Focusable/CURSOR_MARKER); Rest offen (Task 5) | Klasse 1: `handleInput?` → Default-Methode; `isFocusable()`-Type-Guard → `Component::as_focusable()`; Referenzsemantik der TS-Objekte → `ComponentRef = Rc<RefCell<dyn Component>>` |
| `src/terminal.ts` | 559 | `src/terminal.rs` | Kontrakt portiert (`Terminal`-Trait); `ProcessTerminal` offen (Task 4) | Klasse 1: Default-Parameter von `drainInput` → `Option<u64>`; `async` Methode via `async-trait` (dyn-Kompatibilität) |
| `src/keys.ts` | 1401 | `src/keys.rs` | verifiziert | Klasse 1: `_kittyProtocolActive` → prozessglobales `AtomicBool`; `_lastEventType` entfällt (in TS nur geschrieben, nie gelesen); `String.fromCharCode`-Semantik (16-Bit-Truncation) als `js_from_char_code` nachgebildet; Codepoints als `i64` wegen der negativen Sentinels; das TS-Hilfsobjekt `Key` (reine Template-Literal-Typen) entfällt, KeyIds sind `&str`; `parseInt`-Überlauf ⇒ „kein Treffer" statt Gleitkomma-Codepoint (in beiden Fällen unauffindbarer Key) |
| `src/stdin-buffer.ts` | 444 | `src/stdin_buffer.rs` | verifiziert | Klasse 1: `EventEmitter` → geordnete `Vec<StdinEvent>` als Rückgabewert; `setTimeout` → `pending_timeout_ms()` + `flush_timeout()` (Timer treibt der Aufrufer, Semantik identisch); Einzelzeichen-Emission pro `char` statt pro UTF-16-Codeeinheit (lone Surrogates sind in Rust-Strings nicht darstellbar; für BMP-Eingaben identisch) |
| `src/terminal.ts` | 559 | `src/terminal.rs` | verifiziert | Klasse 1: Node-Event-Loop → `pump()` mit tokio-`select!` (stdin-Leser als eigener Thread mit Kanal, Handler laufen auf dem TUI-Strang); `process.stdout.write`-Monkey-Patching der Tests → injizierbare Ausgabe-Senke; `setTimeout`-Timer → Deadlines in `pump()`. Klasse 3: `setRawMode` → termios via libc (Flags empirisch gegen Nodes/libuv Raw-Mode auf macOS verifiziert: `~(BRKINT|ICRNL|INPCK|ISTRIP|IXON)`, `OPOST|ONLCR` bleiben, CS8 ohne CSIZE/PARENB, `~(ECHO|ICANON|IEXTEN|ISIG)`, VMIN=1, VTIME=0); `process.stdout.on("resize")` → SIGWINCH via tokio-signal; Windows-VT-Input als direkter Console-API-Aufruf statt Node-Addon |
| `src/alt-screen-search.ts` | 157 | `src/alt_screen_search.rs` | verifiziert | Klasse 1: `onQueryChange`-Callback → `take_query_changed()` (der Callback bräuchte `&mut` auf den Renderer); Korpus-Index pro Zeichen statt pro UTF-16-Einheit |
| `src/tui-alt-screen.ts` | 1291 | `src/tui_alt_screen.rs` | teilweise: Renderer-Kern (Enter/Exit-Sequenzen inkl. Maus-Modi je Multiplexer, CUP+2K-Zeilendiff, `imagesNeedRedraw`-Vollredraw, Kitty-Placement-Cache mit LRU-Eviktion, Flash-Kompositio, Dokument-Ausschreiben beim Beenden) und Wheel-Routing durch verschachtelte ScrollViews; portiert zusätzlich: Maus-Textselektion (SGR-Parser, Zeichen-/Wort-/Zeilen-Granularität über Klickzählung mit 500-ms-Fenster, Grapheme-Snapping, Selektion in ScrollViews), Reverse-Video-Highlight, OSC-52-Clipboard, OSC-8-Klickaktivierung, Fokusverlust-Reset; offen: Scrollbar-Hover/-Drag, Selektions-Autoscroll, Rechtsklick-Paste, Verdrahtung der Transkript-Suche (Suchmodul selbst ist portiert) | Klasse 1: Input-Listener der TS-Konstruktion → `handle_terminal_input` des Renderers (der Listener braucht `&mut self`) |
| `src/index.ts` | 147 | `src/lib.rs` | teilweise (Export-Set der portierten Module deckungsgleich; Editor, Markdown, LaTeX, Autocomplete, Alt-Screen fehlen noch) | Klasse 2/1: `marked`-Re-Export entfällt (Tech-Substitution); `Key`-Hilfsobjekt entfällt (Template-Literal-Typen) |
| `src/tui.ts` | 1257 | `src/tui.rs` | verifiziert | Klasse 1: `TuiBase` (abstrakte Klasse) → `TuiCore` (geteilter Zustand) plus konkrete Renderer; Overlay-Handles halten einen `TuiCore`-Klon statt `this`-Closures; Timer rufen nicht zurück, sondern `render_deadline()` + `begin_frame()` treibt die Schleife des Renderers (Handlerausführung bleibt einsträngig wie im Node-Event-Loop); `addInputListener` gibt eine `ListenerId` statt einer Unsubscribe-Closure zurück; Promises → `async fn` mit tokio-Timeout; Fokus-Flags einer Komponente, die gerade `handle_input` ausführt (und damit `RefCell`-geliehen ist), werden bis zum Rücksprung nachgezogen — beobachtbar identisch, da niemand vorher lesen kann |
| `src/tui-main-screen.ts` | 586 | `src/tui_main_screen.rs` | verifiziert (tui-render-Suite ohne die Kitty-Bild-Fälle) | Klasse 1: geworfener `Error` des Überbreiten-Guards → `panic!` mit identischem Text (Programmierfehler, kein Kontrollfluss); Debug-Dump ohne `Math.random()`-Suffix |
| `src/layout.ts` | 410 | `src/layout.rs` | verifiziert | Klasse 1: `parent`-Zeiger der `LayoutBox` entfallen (nicht gelesen); `requestRender`-Callback → `render_requested`-Flag im Scroll-State |
| `src/layout-node.ts` | 51 | `src/layout_node.rs` | verifiziert | Klasse 1: `LAYOUT_NODE`-Symbol → `Component::layout_node()` |
| `src/components/stack.ts` | 154 | `src/components/stack.rs` | verifiziert | Klasse 1: abstrakte Basisklasse → Struct mit `layout_type` |
| `src/components/scroll-view.ts` | 216 | `src/components/scroll_view.rs` | verifiziert | Klasse 1: Zustand als `Rc<RefCell<ScrollViewState>>` (in TS ist die Komponente selbst der Zustand); Scrollbar-Auto-Hide-Timer als Deadline für den Aufrufer |
| `src/kill-ring.ts` | 46 | `src/kill_ring.rs` | portiert (Task 10 vorgezogen) | — |
| `src/undo-stack.ts` | 28 | `src/undo_stack.rs` | portiert (Task 10 vorgezogen) | Klasse 1: `structuredClone` beim Push → Wertübergabe (für die verwendeten Zustands-Structs gleichbedeutend) |
| `src/word-navigation.ts` | 117 | `src/word_navigation.rs` | verifiziert (Task 10 vorgezogen) | Klasse 3: `Intl.Segmenter` (Wortgranularität) → `unicode-segmentation`. **Beobachtbare Restdifferenz**: ICU segmentiert Chinesisch wörterbuchbasiert („你好"/„世界"), UAX #29 pro Zeichen — die CJK-Wortnavigation springt daher zeichenweise statt wortweise. Klasse 1: `isWordLike` → „Segment enthält alphanumerisches Zeichen"; Cursorpositionen als Byte-Offsets statt UTF-16-Indizes |
| `src/fuzzy.ts` | 137 | `src/fuzzy.rs` | verifiziert (Task 13 vorgezogen) | Klasse 1: benannte Regex-Gruppen für den Alpha-Numerik-Tausch → Zeichenprüfung; stabile Sortierung explizit über den Ursprungsindex |
| `src/keybindings.ts` | 320 | `src/keybindings.rs` | verifiziert (Task 13 vorgezogen) | Klasse 1: Deklarations-Merging-Interface → statische Definitionstabelle mit 47 Einträgen; globaler Manager als `Mutex` statt Modulvariable |
| `src/components/spacer.ts` | 28 | `src/components/spacer.rs` | portiert | — |
| `src/components/truncated-text.ts` | 65 | `src/components/truncated_text.rs` | portiert | — |
| `src/components/box.ts` | 137 | `src/components/box_component.rs` | portiert | Klasse 1: Modulname, weil `box` ein Rust-Schlüsselwort ist |
| `src/components/alt-screen-flash.ts` | 51 | `src/components/alt_screen_flash.rs` | portiert | Klasse 1: `setTimeout` je Nachricht → Deadline für den Renderer |
| `src/components/loader.ts` | 92 | `src/components/loader.rs` | portiert | Klasse 1: `setInterval` → Frame-Deadline; TS erbt von `Text`, der Port besitzt eine Instanz |
| `src/components/input.ts` | 447 | `src/components/input.rs` | verifiziert (21 von 35 Testfällen) | Klasse 1: Cursor als Byte-Offset statt UTF-16-Index (intern konsistent) |
| `src/components/image.ts` | 127 | `src/components/image.rs` | portiert | — |
| `src/components/select-list.ts` | 229 | `src/components/select_list.rs` | verifiziert | — |
| `src/components/settings-list.ts` | 249 | `src/components/settings_list.rs` | verifiziert | Klasse 1: Submenu-Callback → `close_submenu`, weil die TS-Closure die Liste selbst mutiert |
| `src/components/cancellable-loader.ts` | 40 | `src/components/cancellable_loader.rs` | portiert | Klasse 3: `AbortController` → `CancellationToken` |
| `src/components/text.ts` | 106 | `src/components/text.rs` | verifiziert (über layout-Suite) | — |
| `src/components/v-stack.ts` | 33 | `src/components/v_stack.rs` | verifiziert | — |
| `src/components/h-stack.ts` | 44 | `src/components/h_stack.rs` | verifiziert | — |
| `src/terminal-image.ts` | 657 | `src/terminal_image.rs` | portiert (Capability-Detection, Zellmaße, Kitty-/iTerm2-Encoder, Bild-ID-Vergabe, PNG/JPEG/GIF/WebP-Header, `renderImage`, `imageFallback`, `hyperlink`, Kitty-Metadaten/Crop/Delete); Tests folgen | Klasse 3: `execSync("tmux …")` → `std::process::Command`; `Buffer`-Base64 → eigene Kodierung/Dekodierung; `pathToFileURL` → `file://`-Präfix |
| `src/terminal-colors.ts` | 73 | `src/terminal_colors.rs` | verifiziert (Parser); die TUI-Query-Fälle folgen mit Task 6 | Klasse 1: `undefined` → `Option`; `TerminalColorScheme` als Enum statt String-Union |
| `src/native-modifiers.ts` | 66 | `src/native_modifiers.rs` | portiert (Task 13 vorgezogen, da `forwardInputSequence` es braucht) | Klasse 3: Node-Addon → direkte OS-Aufrufe (`CGEventSourceFlagsState` auf macOS, `GetAsyncKeyState` auf Windows, sonst `false`) |
| `native/darwin/src/darwin-modifiers.c` | 76 | `src/native_modifiers.rs` (darwin) | portiert | Klasse 3 |
| `native/win32/src/win32-console-mode.c` | 135 | `src/native_modifiers.rs` (win32) + `terminal.rs::enable_windows_vt_input` | portiert | Klasse 3 |
| `src/utils.ts` | 1326 | `src/utils.rs` (+ generiertes `src/unicode_tables.rs`) | verifiziert | Klasse 3: `Intl.Segmenter` → `unicode-segmentation`; `get-east-asian-width` und die `\p{…}`-Klassen (inkl. `\p{RGI_Emoji}`) als generierte Tabellen aus derselben Node-/Datenquelle (Rusts `regex` kennt weder `\p{RGI_Emoji}` noch `[A--[B]]`). Klasse 1: gepoolter `AnsiCodeTracker` in `extractSegments` → lokale Instanz (kein globaler Zustand, `clear()` beim Eintritt macht das verhaltensgleich); Width-Cache als `thread_local` mit identischer FIFO-Eviktion (512); Default-Parameter `truncateToWidth(text, w)` → zusätzliche Funktion `truncate_to_width_opts` |

## Portierte Testdateien

| TS-Testdatei | LOC | Rust-Test | Status |
|---|---|---|---|
| `test/virtual-terminal.ts` | 218 | `src/test_terminal.rs` (Feature `test-terminal`) | verifiziert — `tests/virtual_terminal.rs` prüft den Harness gegen 21 `@xterm/headless`-Fixtures (Viewport, Scrollback, Cursor, Resize, CSI/OSC/APC, Synchronized Output) plus Ereignisaufzeichnung, Handler-Weiterleitung und Sequenz-Helfer |
| `test/wrap-ansi.test.ts` | 266 | `tests/wrap_ansi.rs` (19 Fälle) | verifiziert |
| `test/truncate-to-width.test.ts` | 127 | `tests/truncate_to_width.rs` (16 Fälle) | verifiziert |
| `test/regression-regional-indicator-width.test.ts` | 52 | `tests/regression_regional_indicator_width.rs` (5 Fälle) | verifiziert |
| `test/tab-width.test.ts` | 88 | `tests/tab_width.rs` (4 Fälle) | verifiziert |
| `test/regression-overlay-cjk-boundary.test.ts` | 46 | `tests/regression_overlay_cjk_boundary.rs` (4 Fälle) | verifiziert |
| `test/stdin-buffer.test.ts` | 526 | `tests/stdin_buffer.rs` (48 Fälle) | verifiziert |
| — (zusätzlich) | — | `tests/stdin_buffer_oracle.rs` + `tests/fixtures/stdin-buffer-oracle.json` | Differenztest gegen die TS-Implementierung: 1276 Chunk-Zerlegungen von 38 Eingabeströmen (Ereignisfolge und Restpuffer) — alle identisch |
| `test/terminal-colors.test.ts` | 252 | `tests/terminal_colors.rs` (9 Fälle) | verifiziert |
| `test/word-navigation.test.ts` | 191 | `tests/word_navigation.rs` (17 Fälle) | verifiziert; der CJK-Rückwärtsfall hält die oben dokumentierte Segmentierungsdifferenz fest |
| `test/fuzzy.test.ts` | 112 | `tests/fuzzy.rs` (14 Fälle) | verifiziert |
| `test/keybindings.test.ts` | 81 | `tests/keybindings.rs` (7 Fälle) | verifiziert |
| `test/input.test.ts` | 647 | `tests/input.rs` (21 von 35 Fällen) | verifiziert; offen: die beiden CJK-Wortgrenzen-Fälle (Segmentierungsdifferenz, siehe word-navigation), der Horizontal-Scroll-Renderfall und elf weitere Undo-Fälle |
| `test/truncated-text.test.ts` | 129 | `tests/truncated_text.rs` (9 Fälle) | verifiziert |
| `test/settings-list.test.ts` | 58 | `tests/settings_list.rs` (2 Fälle) | verifiziert |
| `test/select-list.test.ts` | 116 | `tests/select_list.rs` (5 Fälle) | verifiziert |
| `test/layout.test.ts` | 306 | `tests/layout.rs` (14 Fälle) | verifiziert |
| `test/overlay-non-capturing.test.ts` | 1203 | `tests/overlay_non_capturing.rs` (44 Fälle) | verifiziert — komplette Fokus-Zustandsmaschine, No-op-Guards, Fokuszyklen und Renderreihenfolge |
| `test/overlay-options.test.ts` | 541 | `tests/overlay_options.rs` (24 Fälle) | verifiziert |
| `test/overlay-short-content.test.ts` | 62 | `tests/overlay_short_content.rs` (1 Fall) | verifiziert |
| `test/tui-shrink.test.ts` | 45 | `tests/tui_shrink.rs` (1 Fall) | verifiziert |
| `test/tui-overlay-style-leak.test.ts` | 81 | `tests/tui_overlay_style_leak.rs` (2 Fälle) | verifiziert |
| `test/tui-cell-size-input.test.ts` | 82 | `tests/tui_cell_size_input.rs` (2 Fälle) | verifiziert |
| `test/tui-alt-screen.test.ts` | 1267 | `tests/tui_alt_screen.rs` (8 Fälle) | Viewport-Rendering, Wheel-Routing und Dock-Layout verifiziert; die Selektions-, Scrollbar- und Suchfälle folgen mit den nächsten Stufen |
| `test/tui-render.test.ts` | 832 | `tests/tui_render.rs` (21 von 24 Fällen) | verifiziert; offen sind drei Kitty-Fälle, die den Vollredraw-Fallback über Bildhöhen prüfen |
| `test/terminal.test.ts` | 300 | `tests/terminal.rs` (17 Fälle) | verifiziert |
| `test/keys.test.ts` | 633 | `tests/keys.rs` (57 Fälle) | verifiziert |
| — (zusätzlich) | — | `tests/keys_oracle.rs` + `tests/fixtures/keys-oracle.json` | Differenztest gegen die TS-Implementierung: 1611 Eingabesequenzen × 637 KeyIds × beide Kitty-Zustände (≈ 2 Mio. `matchesKey`-Vergleiche) plus `parseKey`, `isKeyRelease`, `isKeyRepeat`, `decodeKittyPrintable`, `decodePrintableKey` — alle identisch |
| — (zusätzlich) | — | `tests/utils_oracle.rs` + `tests/fixtures/utils-oracle.json` | Differenztest gegen die TS-Implementierung: 2695 Korpusfälle × {visibleWidth, wrapTextWithAnsi ×5 Breiten, truncateToWidth ×5 (auch mit `…`+Padding), sliceWithWidth ×5 Konfigurationen, extractSegments ×4} — alle identisch. Erzeugt von `tools/gen-utils-oracle.mjs` (Master-Plan, Risiko 1) |

## Werkzeuge

| Datei | Zweck |
|---|---|
| `tools/gen-unicode-tables.mjs` | Erzeugt `src/unicode_tables.rs` aus der Node-Runtime (Unicode 17.0) und `get-east-asian-width`. Die 1494 RGI-ZWJ-Sequenzen entstehen aus `emoji-zwj-sequences.txt` (Emoji 16.0, alle 1468 Einträge gegen V8 verifiziert) plus vollständiger Paarsuche über alle 1438 Emoji-Codepoints (26 Ergänzungen aus Unicode 17). |
| `tools/gen-utils-oracle.mjs` | Erzeugt `tests/fixtures/utils-oracle.json` aus `packages/tui/src/utils.ts`. |
| `tools/gen-stdin-buffer-oracle.mjs` | Erzeugt `tests/fixtures/stdin-buffer-oracle.json` aus `packages/tui/src/stdin-buffer.ts`. |
| `tools/gen-keys-oracle.mjs` | Erzeugt `tests/fixtures/keys-oracle.json` aus `packages/tui/src/keys.ts`. |
| `tools/gen-virtual-terminal-oracle.mjs` | Erzeugt `tests/fixtures/virtual-terminal-oracle.json` aus `@xterm/headless` 5.5.0 — 23 Szenarien mit genau den Sequenzen, die beide Renderer emittieren. |

## Stand der Plan-Tasks

| Task | Stand |
|---|---|
| 1 Testinfrastruktur | fertig |
| 2 utils | fertig |
| 3 keys | fertig |
| 4 stdin-buffer + terminal | fertig (native-modifiers aus Task 13 vorgezogen) |
| 5 TUI-Kern | fertig |
| 6 Main-Screen-Renderer | fertig (3 Kitty-Vollredraw-Testfälle offen) |
| 7 Layout-Engine + ScrollView | fertig (Text/VStack/HStack aus Task 9 vorgezogen; Testsuite vollständig) |
| 8 Alt-Screen-Renderer | teilweise: Renderer-Kern und Wheel-Routing portiert; `alt-screen-search.ts` portiert; Selektion, Scrollbar-Drag, OSC-52 und die Suchverdrahtung offen |
| 9 Basis-Komponenten | fertig portiert (spacer, truncated-text, box, alt-screen-flash, loader, cancellable-loader, select-list, settings-list, input, image); offene Testfälle: 14 der 35 input-Fälle |
| 10 Editor | teilweise: kill-ring, undo-stack, word-navigation portiert; editor.ts offen |
| 11 Markdown + LaTeX | offen |
| 12 Terminal-Bilder | Modul vollständig portiert; Testsuite (632 LOC) offen |
| 13 Autocomplete/Fuzzy/Keybindings/native | teilweise: fuzzy, keybindings, native-modifiers portiert; autocomplete offen |
| 14 Öffentliche API + Ledger-Abschluss | teilweise: `lib.rs` spiegelt das Export-Set der portierten Module; Abschluss nach Tasks 8/10/11/13 |
| 15 App-TUI-Schicht (ab G2) | offen |

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| | |

## Entscheidungen

### VT-Emulator für die Testinfrastruktur (Task 1)

**Entscheidung: `vt100` 0.16** als Emulatorkern hinter einem eigenen
`VirtualTerminal`-Harness (API-gleich zu `test/virtual-terminal.ts`).
Abweichungsklasse 3 (Tech-Substitution laut Master-Tabelle
"@xterm/headless → Rust-VT-Emulator-Crate").

Empirische Grundlage: 23 Szenarien (genau die Sequenzen, die beide Renderer
emittieren) wurden mit `@xterm/headless` 5.5.0 aus dem TS-Repo als Fixtures
erzeugt und gegen die Kandidaten geprüft — 69 Prüfpunkte (Viewport, Scrollback,
Cursorposition), vt100: 64 grün.

| Hartes Kriterium (Plan Task 1) | vt100 0.16 | avt 0.18 |
|---|---|---|
| Sichtbarer Viewport als Zeilen | ja (`Screen::rows`) | ja (`Vt::view`) |
| Scrollback abrufbar | ja (`set_scrollback` + `rows`) | ja (`Vt::lines`) |
| Cursorposition | ja | ja |
| Resize zur Laufzeit | ja (`Screen::set_size`) | ja |
| CSI/OSC/APC inkl. Synchronized-Output-Passthrough | ja (2026h/l ignoriert, APC/OSC verschluckt) | ja |
| **Zellsemantik von `translateToString(true)`** | **ja** (`Cell::has_contents()`: geschriebenes Leerzeichen bleibt, gelöschte/nie beschriebene Zelle fällt weg) | **nein** (`Cell::blank` = `' '` + Default-Pen, `is_default()` kann beides nicht unterscheiden) |

Die Zellsemantik ist entscheidend, weil die TS-Erwartungswerte sie voraussetzen
(`test/tab-width.test.ts:81` erwartet `"base 0          "` mit geschriebenen
Leerzeichen, gelöschte Zeilen dagegen `""`). `wezterm-term` ist auf crates.io nur
als Fork (`tattoy-wezterm-term`) verfügbar und entfällt damit.

Dokumentierte Abweichungen von `@xterm/headless` (mit Nachweis, dass die
portierten Tests sie nicht beobachten):

1. **CSI 3J (Scrollback löschen)** wird von vt100 ignoriert. Der Harness fängt die
   Sequenz im Schreibstrom ab und ersetzt den Parser (semantisch exakt, weil
   `fullRender(clear)` immer `2J`+`H`+`3J` zusammen emittiert).
2. **DECAWM (CSI ?7l)** wird von vt100 ignoriert. Nicht beobachtbar: der
   Alt-Screen positioniert vor jeder Zeile absolut (CUP), der Main-Screen
   garantiert per Überbreiten-Guard Zeilen ≤ Terminalbreite.
3. **Resize-Verankerung**: `@xterm/headless` verankert beim Resize unten
   (Verkleinern schiebt obere Zeilen in den Scrollback, Vergrößern holt sie
   zurück), `vt100` nicht. Der Harness baut den Puffer beim Resize aus dem
   bisherigen Text neu auf und bildet das Verhalten damit nach; Attribute gehen
   dabei verloren (der Harness liest nur Text).
4. **Emoji-Zellbreite**: vt100 rechnet 2 (wie `graphemeWidth` der TUI),
   `@xterm/headless` mit Unicode-V6-Tabellen 1. vt100 stimmt damit mit dem
   Breitenmodell der App überein; `translateToString` liefert in beiden Fällen
   dieselbe Zeichenfolge.

Umsetzung: `src/test_terminal.rs` (Feature `test-terminal`, hält `vt100` aus
Produktivbuilds). CSI 3J wird im Schreibstrom abgefangen; die Vorbedingung
(leerer Schirm, weil `fullRender(clear)` immer `2J`+`H`+`3J` zusammen emittiert)
prüft der Harness zur Laufzeit per Assertion, damit die Abweichung nicht
stillschweigend greift. Die drei TS-Unterklassen (`RecordingTerminal`,
`LoggingVirtualTerminal`, `CapturingVirtualTerminal`) fallen mangels Vererbung
zu einer eingebauten Ereignisaufzeichnung zusammen (Abweichungsklasse 1).

Kontingenz laut Plan (eigener schlanker Emulator für die benötigte
Sequenz-Teilmenge), falls eine portierte Testerwartung eine dieser Abweichungen
doch beobachtet.
