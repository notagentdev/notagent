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
| `src/tui.ts` | 1257 | `src/tui.rs` | verifiziert | Klasse 1: `TuiBase` (abstrakte Klasse) → `TuiCore` (geteilter Zustand) plus konkrete Renderer; Overlay-Handles halten einen `TuiCore`-Klon statt `this`-Closures; Timer rufen nicht zurück, sondern `render_deadline()` + `begin_frame()` treibt die Schleife des Renderers (Handlerausführung bleibt einsträngig wie im Node-Event-Loop); `addInputListener` gibt eine `ListenerId` statt einer Unsubscribe-Closure zurück; Promises → `async fn` mit tokio-Timeout; Fokus-Flags einer Komponente, die gerade `handle_input` ausführt (und damit `RefCell`-geliehen ist), werden bis zum Rücksprung nachgezogen — beobachtbar identisch, da niemand vorher lesen kann |
| `src/tui-main-screen.ts` | 586 | `src/tui_main_screen.rs` | verifiziert (tui-render-Suite ohne die Kitty-Bild-Fälle) | Klasse 1: geworfener `Error` des Überbreiten-Guards → `panic!` mit identischem Text (Programmierfehler, kein Kontrollfluss); Debug-Dump ohne `Math.random()`-Suffix |
| `src/terminal-image.ts` | 657 | `src/terminal_image.rs` | teilweise portiert (Capability-Detection, Zellmaße, `is_image_line`, `delete_kitty_image`); Rest mit Task 12 | Klasse 3: `execSync("tmux …")` → `std::process::Command` |
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
| `test/overlay-non-capturing.test.ts` | 1203 | `tests/overlay_non_capturing.rs` (44 Fälle) | verifiziert — komplette Fokus-Zustandsmaschine, No-op-Guards, Fokuszyklen und Renderreihenfolge |
| `test/overlay-options.test.ts` | 541 | `tests/overlay_options.rs` (24 Fälle) | verifiziert |
| `test/overlay-short-content.test.ts` | 62 | `tests/overlay_short_content.rs` (1 Fall) | verifiziert |
| `test/tui-shrink.test.ts` | 45 | `tests/tui_shrink.rs` (1 Fall) | verifiziert |
| `test/tui-overlay-style-leak.test.ts` | 81 | `tests/tui_overlay_style_leak.rs` (2 Fälle) | verifiziert |
| `test/tui-cell-size-input.test.ts` | 82 | `tests/tui_cell_size_input.rs` (2 Fälle) | verifiziert |
| `test/tui-render.test.ts` | 832 | `tests/tui_render.rs` (17 von 24 Fällen) | verifiziert; die sieben Kitty-Bild-Fälle brauchen `encodeKitty` und die `Image`-Komponente → Tasks 9/12 |
| `test/terminal.test.ts` | 300 | Task 4 |
| 2026-08-13 | `src/tui.ts` (vollständig) | 1257 | Task 5 |
| 2026-08-13 | `src/tui.ts` | 1257 | `src/tui.rs` | verifiziert | Klasse 1: `TuiBase` (abstrakte Klasse) → `TuiCore` (geteilter Zustand) plus konkrete Renderer; Overlay-Handles halten einen `TuiCore`-Klon statt `this`-Closures; Timer rufen nicht zurück, sondern `render_deadline()` + `begin_frame()` treibt die Schleife des Renderers (Handlerausführung bleibt einsträngig wie im Node-Event-Loop); `addInputListener` gibt eine `ListenerId` statt einer Unsubscribe-Closure zurück; Promises → `async fn` mit tokio-Timeout; Fokus-Flags einer Komponente, die gerade `handle_input` ausführt (und damit `RefCell`-geliehen ist), werden bis zum Rücksprung nachgezogen — beobachtbar identisch, da niemand vorher lesen kann |
| `src/tui-main-screen.ts` | 586 | `src/tui_main_screen.rs` | verifiziert (tui-render-Suite ohne die Kitty-Bild-Fälle) | Klasse 1: geworfener `Error` des Überbreiten-Guards → `panic!` mit identischem Text (Programmierfehler, kein Kontrollfluss); Debug-Dump ohne `Math.random()`-Suffix |
| `src/terminal-image.ts` | 657 | `src/terminal_image.rs` | teilweise portiert (Capability-Detection, Zellmaße, `is_image_line`, `delete_kitty_image`); Rest mit Task 12 | Klasse 3: `execSync("tmux …")` → `std::process::Command` |
| `src/terminal-colors.ts` | 73 | Task 5 (vorgezogen aus Task 12) |
| 2026-08-13 | `test/terminal-colors.test.ts` | 252 | Task 5 |
| 2026-08-13 | `src/tui-main-screen.ts` | 586 | Task 6 |
| 2026-08-13 | `test/overlay-non-capturing.test.ts` | 1203 | `tests/overlay_non_capturing.rs` (44 Fälle) | verifiziert — komplette Fokus-Zustandsmaschine, No-op-Guards, Fokuszyklen und Renderreihenfolge |
| `test/overlay-options.test.ts` | 541 | `tests/overlay_options.rs` (24 Fälle) | verifiziert |
| `test/overlay-short-content.test.ts` | 62 | `tests/overlay_short_content.rs` (1 Fall) | verifiziert |
| `test/tui-shrink.test.ts` | 45 | `tests/tui_shrink.rs` (1 Fall) | verifiziert |
| `test/tui-overlay-style-leak.test.ts` | 81 | `tests/tui_overlay_style_leak.rs` (2 Fälle) | verifiziert |
| `test/tui-cell-size-input.test.ts` | 82 | `tests/tui_cell_size_input.rs` (2 Fälle) | verifiziert |
| `test/tui-render.test.ts` | 832 | Task 6 |
| 2026-08-13 | `src/terminal-image.ts` (Capability-/Bildzeilen-Teil) | 657 | Task 5/6 |
| 2026-08-13 | `test/stdin-buffer.test.ts` | 526 | `tests/stdin_buffer.rs` (48 Fälle) | verifiziert |
| — (zusätzlich) | — | `tests/stdin_buffer_oracle.rs` + `tests/fixtures/stdin-buffer-oracle.json` | Differenztest gegen die TS-Implementierung: 1276 Chunk-Zerlegungen von 38 Eingabeströmen (Ereignisfolge und Restpuffer) — alle identisch |
| `test/terminal-colors.test.ts` | 252 | `tests/terminal_colors.rs` (9 Fälle) | verifiziert |
| `test/overlay-non-capturing.test.ts` | 1203 | `tests/overlay_non_capturing.rs` (44 Fälle) | verifiziert — komplette Fokus-Zustandsmaschine, No-op-Guards, Fokuszyklen und Renderreihenfolge |
| `test/overlay-options.test.ts` | 541 | `tests/overlay_options.rs` (24 Fälle) | verifiziert |
| `test/overlay-short-content.test.ts` | 62 | `tests/overlay_short_content.rs` (1 Fall) | verifiziert |
| `test/tui-shrink.test.ts` | 45 | `tests/tui_shrink.rs` (1 Fall) | verifiziert |
| `test/tui-overlay-style-leak.test.ts` | 81 | `tests/tui_overlay_style_leak.rs` (2 Fälle) | verifiziert |
| `test/tui-cell-size-input.test.ts` | 82 | `tests/tui_cell_size_input.rs` (2 Fälle) | verifiziert |
| `test/tui-render.test.ts` | 832 | `tests/tui_render.rs` (17 von 24 Fällen) | verifiziert; die sieben Kitty-Bild-Fälle brauchen `encodeKitty` und die `Image`-Komponente → Tasks 9/12 |
| `test/terminal.test.ts` | 300 | `tests/terminal.rs` (17 Fälle) | verifiziert |
| `test/keys.test.ts` | 633 | Task 3 |
| 2026-08-13 | `src/tui.ts:1-120` (Kontraktbereich) | 120 von 1257 | Master-Plan Task 2 (Kontrakt-Commit) |
| 2026-08-13 | `src/terminal.ts:1-140` (Kontraktbereich) | 140 von 559 | Master-Plan Task 2 (Kontrakt-Commit) |
| 2026-08-13 | `node_modules/get-east-asian-width/{index,lookup,lookup-data,utilities}.js` | 213 | Task 2 (Referenz für `eastAsianWidth`) |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| `src/tui.ts` | 1257 | `src/tui.rs` | Kontrakt portiert (Component/Focusable/CURSOR_MARKER); Rest offen (Task 5) | Klasse 1: `handleInput?` → Default-Methode; `isFocusable()`-Type-Guard → `Component::as_focusable()`; Referenzsemantik der TS-Objekte → `ComponentRef = Rc<RefCell<dyn Component>>` |
| `src/terminal.ts` | 559 | `src/terminal.rs` | Kontrakt portiert (`Terminal`-Trait); `ProcessTerminal` offen (Task 4) | Klasse 1: Default-Parameter von `drainInput` → `Option<u64>`; `async` Methode via `async-trait` (dyn-Kompatibilität) |
| `src/keys.ts` | 1401 | `src/keys.rs` | verifiziert | Klasse 1: `_kittyProtocolActive` → prozessglobales `AtomicBool`; `_lastEventType` entfällt (in TS nur geschrieben, nie gelesen); `String.fromCharCode`-Semantik (16-Bit-Truncation) als `js_from_char_code` nachgebildet; Codepoints als `i64` wegen der negativen Sentinels; das TS-Hilfsobjekt `Key` (reine Template-Literal-Typen) entfällt, KeyIds sind `&str`; `parseInt`-Überlauf ⇒ „kein Treffer" statt Gleitkomma-Codepoint (in beiden Fällen unauffindbarer Key) |
| `src/stdin-buffer.ts` | 444 | `src/stdin_buffer.rs` | verifiziert | Klasse 1: `EventEmitter` → geordnete `Vec<StdinEvent>` als Rückgabewert; `setTimeout` → `pending_timeout_ms()` + `flush_timeout()` (Timer treibt der Aufrufer, Semantik identisch); Einzelzeichen-Emission pro `char` statt pro UTF-16-Codeeinheit (lone Surrogates sind in Rust-Strings nicht darstellbar; für BMP-Eingaben identisch) |
| `src/terminal.ts` | 559 | `src/terminal.rs` | verifiziert | Klasse 1: Node-Event-Loop → `pump()` mit tokio-`select!` (stdin-Leser als eigener Thread mit Kanal, Handler laufen auf dem TUI-Strang); `process.stdout.write`-Monkey-Patching der Tests → injizierbare Ausgabe-Senke; `setTimeout`-Timer → Deadlines in `pump()`. Klasse 3: `setRawMode` → termios via libc (Flags empirisch gegen Nodes/libuv Raw-Mode auf macOS verifiziert: `~(BRKINT|ICRNL|INPCK|ISTRIP|IXON)`, `OPOST|ONLCR` bleiben, CS8 ohne CSIZE/PARENB, `~(ECHO|ICANON|IEXTEN|ISIG)`, VMIN=1, VTIME=0); `process.stdout.on("resize")` → SIGWINCH via tokio-signal; Windows-VT-Input als direkter Console-API-Aufruf statt Node-Addon |
| `src/tui.ts` | 1257 | `src/tui.rs` | verifiziert | Klasse 1: `TuiBase` (abstrakte Klasse) → `TuiCore` (geteilter Zustand) plus konkrete Renderer; Overlay-Handles halten einen `TuiCore`-Klon statt `this`-Closures; Timer rufen nicht zurück, sondern `render_deadline()` + `begin_frame()` treibt die Schleife des Renderers (Handlerausführung bleibt einsträngig wie im Node-Event-Loop); `addInputListener` gibt eine `ListenerId` statt einer Unsubscribe-Closure zurück; Promises → `async fn` mit tokio-Timeout; Fokus-Flags einer Komponente, die gerade `handle_input` ausführt (und damit `RefCell`-geliehen ist), werden bis zum Rücksprung nachgezogen — beobachtbar identisch, da niemand vorher lesen kann |
| `src/tui-main-screen.ts` | 586 | `src/tui_main_screen.rs` | verifiziert (tui-render-Suite ohne die Kitty-Bild-Fälle) | Klasse 1: geworfener `Error` des Überbreiten-Guards → `panic!` mit identischem Text (Programmierfehler, kein Kontrollfluss); Debug-Dump ohne `Math.random()`-Suffix |
| `src/terminal-image.ts` | 657 | `src/terminal_image.rs` | teilweise portiert (Capability-Detection, Zellmaße, `is_image_line`, `delete_kitty_image`); Rest mit Task 12 | Klasse 3: `execSync("tmux …")` → `std::process::Command` |
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
| `test/overlay-non-capturing.test.ts` | 1203 | `tests/overlay_non_capturing.rs` (44 Fälle) | verifiziert — komplette Fokus-Zustandsmaschine, No-op-Guards, Fokuszyklen und Renderreihenfolge |
| `test/overlay-options.test.ts` | 541 | `tests/overlay_options.rs` (24 Fälle) | verifiziert |
| `test/overlay-short-content.test.ts` | 62 | `tests/overlay_short_content.rs` (1 Fall) | verifiziert |
| `test/tui-shrink.test.ts` | 45 | `tests/tui_shrink.rs` (1 Fall) | verifiziert |
| `test/tui-overlay-style-leak.test.ts` | 81 | `tests/tui_overlay_style_leak.rs` (2 Fälle) | verifiziert |
| `test/tui-cell-size-input.test.ts` | 82 | `tests/tui_cell_size_input.rs` (2 Fälle) | verifiziert |
| `test/tui-render.test.ts` | 832 | `tests/tui_render.rs` (17 von 24 Fällen) | verifiziert; die sieben Kitty-Bild-Fälle brauchen `encodeKitty` und die `Image`-Komponente → Tasks 9/12 |
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
