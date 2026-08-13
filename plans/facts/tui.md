# Faktenbericht: @notagent/tui — Basis für den 1:1 Rust-Port

Quelle: `/Users/dev/projects/notagent-main/packages/tui` (src/, test/, package.json). Alle Aussagen stammen aus dem Quellcode (Stand 2026-08-13).

---

## 1. Vollständige Dateiliste `src/` (39 Dateien, 16 704 LOC gesamt)

### `src/` (Root)

| Datei | LOC | Beschreibung |
|---|---|---|
| `src/tui.ts` | 1257 | Kernabstraktionen: `Component`, `Container`, `TuiBase` (abstrakt), Overlay-Stack/Fokus-Restore-Maschine, Render-Scheduling, Cursor-Marker, `compositeTuiLine`. |
| `src/tui-main-screen.ts` | 586 | `TuiMainScreen`: differentieller Renderer für Hauptschirm + Scrollback (zeilenbasiertes Diff, Kitty-Bild-Bookkeeping). |
| `src/tui-alt-screen.ts` | 1291 | `TuiAltScreen`: Alternate-Screen-Renderer mit Viewport/ScrollView, Maus-Selektion, Scrollbar-Drag, Transkript-Suche, OSC-52-Clipboard, Flash-Messages. |
| `src/utils.ts` | 1326 | Breitenberechnung (`visibleWidth`), ANSI-Parsing (`extractAnsiCode`), `AnsiCodeTracker`, Word-Wrap (`wrapTextWithAnsi`), `truncateToWidth`, `sliceByColumn`/`sliceWithWidth`, `extractSegments`, OSC-8-Handling. |
| `src/keys.ts` | 1401 | Tastatur-Matching: Kitty-CSI-u-Parser, `modifyOtherKeys`, Legacy-Sequenztabellen, `matchesKey`/`parseKey`/`decodeKittyPrintable`. |
| `src/latex.ts` | 1380 | LaTeX→Unicode-Renderer (`renderLatex`), Symboltabellen, Bruch-/Matrix-/Limit-Layout. |
| `src/autocomplete.ts` | 786 | `AutocompleteProvider`-Interface + `CombinedAutocompleteProvider` (Slash-Commands + Dateipfade via `fd`/`readdirSync`). |
| `src/terminal-image.ts` | 657 | Terminal-Capability-Detection, Kitty-/iTerm2-Bildprotokolle, PNG/JPEG/GIF/WebP-Header-Parser, OSC-8-`hyperlink()`. |
| `src/terminal.ts` | 559 | `Terminal`-Interface + `ProcessTerminal` (Raw-Mode, Kitty-Protokoll-Negotiation, Bracketed Paste, Resize, OSC 9;4 Progress). |
| `src/stdin-buffer.ts` | 444 | `StdinBuffer`: reassembliert fragmentierte Escape-Sequenzen, trennt Bracketed-Paste, Timeouts. (Basiert laut Header auf OpenTUI, MIT.) |
| `src/layout.ts` | 410 | Box-Layout-Engine für Alt-Screen: `renderLayoutFrame`, `LayoutBox`, Clipping, Scrollbar-Geometrie, Hit-Testing. |
| `src/keybindings.ts` | 320 | `KeybindingsManager`, globale Registry `TUI_KEYBINDINGS`, Konflikterkennung. |
| `src/alt-screen-search.ts` | 157 | Transkript-Suche: Korpus-Aufbau mit Zeilen/Spalten-Mapping, `AltScreenSearchComponent` (Overlay-UI). |
| `src/index.ts` | 147 | Öffentliche API (Re-Exports). |
| `src/fuzzy.ts` | 137 | `fuzzyMatch`/`fuzzyFilter` (Score-basiert, niedriger = besser). |
| `src/word-navigation.ts` | 117 | `findWordBackward`/`findWordForward` über `Intl.Segmenter` (word) + Punctuation-Regex. |
| `src/editor-component.ts` | 74 | Interface `EditorComponent` für austauschbare Editor-Implementierungen. |
| `src/terminal-colors.ts` | 73 | OSC-11-Hintergrundfarbe parsen, `CSI ? 997 ; n` Color-Scheme-Report parsen. |
| `src/native-modifiers.ts` | 66 | Lädt native Prebuilds (`darwin-modifiers.node` / `win32-console-mode.node`) für `isModifierPressed`. |
| `src/layout-node.ts` | 51 | Symbol `LAYOUT_NODE`, Typen `StackLayoutNode`/`ScrollLayoutNode`, `getLayoutNode()`. |
| `src/kill-ring.ts` | 46 | Emacs-Kill-Ring (push/peek/rotate). |
| `src/undo-stack.ts` | 28 | Generischer Undo-Stack mit `structuredClone` beim Push. |

### `src/components/`

| Datei | LOC | Beschreibung |
|---|---|---|
| `components/editor.ts` | 2363 | Mehrzeiliger Editor: Word-Wrap-Layout, Visual-Line-Navigation, Paste-Marker, History, Kill-Ring, Undo, Autocomplete-Integration, Zeichen-Jump-Modus. |
| `components/markdown.ts` | 1010 | Markdown-Renderer auf `marked` (Tokens → ANSI-Zeilen), Tabellen, Codeblöcke, LaTeX-Extension. |
| `components/input.ts` | 447 | Einzeiliges Eingabefeld mit horizontalem Scrolling, Fake-Cursor, Kill-Ring, Undo, Bracketed Paste. |
| `components/settings-list.ts` | 249 | Settings-Liste mit Wertzyklen, Submenüs, optionaler Fuzzy-Suche. |
| `components/select-list.ts` | 229 | Auswahlliste mit Zweispalten-Layout (Label/Description) und Scroll-Indikator. |
| `components/scroll-view.ts` | 216 | `ScrollView` (nur vertikal), Follow-End, Overscroll-Chaining, Scrollbar-Sichtbarkeit; exponiert `LAYOUT_NODE`. |
| `components/stack.ts` | 154 | Basisklasse `Stack`, `allocateStackSizes` (Flexbox-artig: basis/grow/shrink/min/max), `visibleStackEntries`. |
| `components/box.ts` | 137 | Container mit Padding + Hintergrundfunktion, Render-Cache mit bgFn-Sampling. |
| `components/image.ts` | 127 | Bildkomponente (Kitty/iTerm2/Text-Fallback), reserviert `rows` Zeilen. |
| `components/text.ts` | 106 | Wortumbrechender Text mit Padding und optionalem Hintergrund, Render-Cache. |
| `components/loader.ts` | 92 | Spinner-Text-Komponente mit Interval-Animation. |
| `components/truncated-text.ts` | 65 | Einzeiliger, auf Breite gekürzter Text. |
| `components/alt-screen-flash.ts` | 51 | Transiente Inverse-Video-Meldungen für den Alt-Screen. |
| `components/h-stack.ts` | 44 | Horizontales Stack-Rendering (Fallback ohne Layout-Engine, komponiert per `compositeTuiLine`). |
| `components/cancellable-loader.ts` | 40 | `Loader` + Abort, Abbruch per `tui.select.cancel`. |
| `components/v-stack.ts` | 33 | Vertikales Stack-Rendering (Fallback ohne Layout-Engine). |
| `components/spacer.ts` | 28 | N Leerzeilen. |

---

## 2. Differentielle Rendering-Engine

Es gibt **zwei** Renderer, beide erben von `TuiBase` (`src/tui.ts`) und implementieren `protected abstract doRender()`.

### 2.1 Gemeinsame Basis (`TuiBase`, `src/tui.ts`)

**Render-Pipeline-Vorstufe** (in beiden Renderern identisch aufgerufen):
1. `render(width)` → Zeilen-Array (Container konkateniert Kind-Zeilen).
2. `compositeOverlays(lines, termWidth, termHeight)` — Overlays werden **vor** dem Diff in die Zeilen einkomponiert.
3. `extractCursorPosition(lines, height)` — sucht `CURSOR_MARKER` (APC-Sequenz `ESC _ pi:c BEL`) **von unten nach oben, nur in den untersten `height` Zeilen**, berechnet Spalte via `visibleWidth(prefix)`, entfernt den Marker.
4. `applyLineResets(lines)` — für jede Nicht-Bild-Zeile: `normalizeTerminalOutput(line) + SEGMENT_RESET`, mit `SEGMENT_RESET` = SGR-Reset (CSI 0 m) + OSC-8-Link-Close.

**Scheduling / Flush** (`TuiBase`):
- `MIN_RENDER_INTERVAL_MS = 16`.
- `requestRender(force=false)`: setzt Flag, nextTick → `scheduleRender()`. `scheduleRender()` berechnet `delay = max(0, 16 - (now - lastRenderAt))` per Timer; nach dem Frame wird bei erneutem Request rekursiv neu geplant.
- `requestImmediateRender()` (privat): wird nach **Tastatureingaben** benutzt, cancelt den Timer und rendert im nextTick — auf Windows kann ein 0ms-Timeout einen vollen 16-ms-Tick kosten.
- `renderNow(force)`: synchron; `force` ruft `resetRenderState()`.
- Es gibt keinen Frame-Buffer im Sinne von Zell-Grids: **Der gesamte Zustand ist ein Zeilen-Array (Strings mit eingebetteten ANSI-Sequenzen).**

**Overlay-Layout** (`resolveOverlayLayout`): Anchor (9 Positionen), `row`/`col` absolut oder Prozent ("50%"), Margin, `width`/`minWidth`/`maxHeight` (Zahl oder Prozent), `visible(termWidth, termHeight)`, `nonCapturing`. Overlays werden nach `focusOrder` sortiert (höher = weiter vorn) und zeilenweise mit `compositeTuiLine` einkomponiert. `workingHeight = max(lines.length, termHeight, minLinesNeeded)`, `viewportStart = max(0, workingHeight - termHeight)` — Overlay-Positionen sind **bildschirmrelativ**.

`compositeTuiLine(baseLine, overlayLine, startCol, overlayWidth, totalWidth)` (exportiert): benutzt `extractSegments` (ein Durchlauf, liefert before/after inkl. vererbtem SGR-Stil) und `sliceWithWidth`, fügt `SEGMENT_RESET` vor und nach dem Overlay-Bereich ein, padded mit Leerzeichen; Bildzeilen (`isImageLine`) bleiben unverändert.

### 2.2 `TuiMainScreen` — Hauptschirm + Scrollback

**Zustand** (`TuiMainScreenRenderState`, kapselbar via `captureRenderState`/`restoreRenderState`): `previousLines` (vollständige gerenderte Zeilen des letzten Frames), `previousKittyImageIds` (Set), `previousWidth`, `previousHeight`, `cursorRow` (logisches Ende des Inhalts), `hardwareCursorRow` (tatsächliche Terminal-Cursorzeile), `maxLinesRendered` (High-Water-Mark), `previousViewportTop` (Index der obersten sichtbaren Zeile).

**Kein Hashing** — Zeilenvergleich ist reine String-Gleichheit.

**Full-Redraw-Entscheidungsbaum** (`doRender`, Reihenfolge exakt):
1. Erster Render (keine previousLines, keine Größenänderung) → `fullRender(false)` (**ohne** Clear; nimmt sauberen Schirm an).
2. `widthChanged` → `fullRender(true)`.
3. `heightChanged && !isTermuxSession()` → `fullRender(true)` (Termux ausgenommen, weil dort die Software-Tastatur die Höhe toggelt).
4. `clearOnShrink && newLines.length < maxLinesRendered && keine Overlays` → `fullRender(true)`.

`fullRender(clear)`: Begin Synchronized Output (CSI ? 2026 h), optional Kitty-Images löschen + Clear Screen/Home/Clear Scrollback (CSI 2J, CSI H, CSI 3J), dann alle Zeilen mit CRLF getrennt, End Synchronized Output (CSI ? 2026 l). Mehrzeilige Kitty-Bilder werden mit CRLF-Reservierung, Cursor-hoch (CUU), Bildzeile, Cursor-runter (CUD) platziert.

**Diff-Algorithmus (Zeilen-Diff, nicht Zell-Diff):** Lineare Suche über `max(new.len, prev.len)`: erste und letzte Zeile mit `old !== new` ergeben `firstChanged`/`lastChanged`. Bei angehängten Zeilen (`appendedLines`) fällt `firstChanged` ggf. auf `previousLines.length` und `lastChanged` auf das neue Ende; `appendStart` markiert reines Anhängen. `expandChangedRangeForKittyImages(...)` erweitert den Bereich auf ganze Kitty-Bildblöcke (Blockhöhe via `r=`-Parameter, `getKittyImageReservedRows`).

Danach vier Fälle:
- **Keine Änderung**: nur `positionHardwareCursor(...)`.
- **Nur Löschungen** (`firstChanged >= newLines.length`): Cursor ans Ende des neuen Inhalts fahren, pro überzähliger Zeile CR + Erase Line (CSI 2K) + eine Zeile runter, danach Cursor zurück (CUU). Eskaliert zu `fullRender(true)`, wenn `targetRow < prevViewportTop` oder `extraLines > height`.
- **Änderung oberhalb des Viewports** (`firstChanged < prevViewportTop`) → `fullRender(true)` (Diff kann nur Sichtbares anfassen).
- **Normalfall**: Ein einziger Buffer wird gebaut und mit **einem** `terminal.write(buffer)` geflusht:
  - Begin Synchronized Output; `deleteChangedKittyImages(firstChanged, lastChanged)`.
  - **Scrollen**, falls `moveTargetRow > prevViewportBottom`: Cursor auf letzte Bildschirmzeile (CUD), dann CRLF-Wiederholung (`scroll`-mal) — es werden **keine DECSTBM-Scroll-Regionen** verwendet, gescrollt wird ausschließlich durch Newlines am unteren Rand.
  - Cursorbewegung relativ: CUD/CUU, dann CR bzw. CRLF bei `appendStart`.
  - Pro geänderter Zeile `firstChanged..min(lastChanged, newLines.length-1)`: CRLF als Trenner, Erase Line (CSI 2K), dann die Zeile. Kitty-Bildblöcke werden vorher zeilenweise gelöscht; wenn ein Bildblock über den Viewport hinausragen würde → `fullRender(true)`.
  - **Guard**: `visibleWidth(line) > width` → Crash-Log nach `<logDir>/notagent-crash.log`, `stop()`, Exception (`src/tui-main-screen.ts:447-474`).
  - Falls vorher mehr Zeilen existierten: CRLF + Erase Line je Zeile, dann Cursor zurück (CUU um `extraLines`).
  - End Synchronized Output.

**Hardware-Cursor** (`positionHardwareCursor`): relative Zeilenbewegung (CUU/CUD) + absolute Spalte (CHA, CSI n G), danach Cursor show/hide (CSI ? 25 h/l) je nach `getShowHardwareCursor()`.

**Viewport/History**: Der Hauptschirm hat keinen eigenen Viewport-Puffer — die Terminal-Scrollback ist die History. `previousViewportTop` wird als `max(prevViewportTop, finalCursorRow - height + 1)` fortgeschrieben; `maxLinesRendered` wächst monoton (Reset nur bei Clear).

**Debug-Hooks**: `NOTAGENT_DEBUG_REDRAW=1` loggt Redraw-Gründe nach `<logDir>/notagent-debug.log`; `NOTAGENT_TUI_DEBUG=1` schreibt pro Frame ein Dump-File nach `/tmp/tui/render-*.log`.

### 2.3 `TuiAltScreen` — Alternate Screen

**Zustand**: `previousScreen` (genau `height` Zeilen), `previousScreenWidth/Height`, `currentLayout: LayoutFrame`, `uploadedKittyImages` (Map id→CachedKittyImage), `lastDocument`.

**Pipeline** (`doRender`):
1. `renderLayoutFrame(root, width, height, requestRender)` → `LayoutFrame` mit `lines` (Länge = height).
2. `refreshSearch(nextLayout)`; wenn dabei gescrollt wurde, wird das Layout **neu** gerechnet.
3. Entfernen führender OSC-133-Zonen-Marker.
4. `applySearchHighlights` → `compositeOverlays` → Zuschnitt auf die **letzten** height Zeilen → `applySelection` → `compositeFlashes`.
5. `extractCursorPosition`, `applyLineResets`, harte Breitenklemme per `sliceByColumn(line, 0, width, true)`.

**Diff**: `fullRedraw = kein previousScreen || Breite/Höhe geändert`. Zusätzlich `imagesNeedRedraw` wenn sich irgendeine geänderte Zeile mit Bildzeilen überschneidet → kompletter Schirm neu. Sonst: **absolute Cursorpositionierung (CUP: CSI row;1 H) + Erase Line pro geänderter Zeile**, kein Scrolling, kein Zell-Diff.

**Emittierte Sequenzen (Konstanten in `tui-alt-screen.ts`):** Enter/Exit Alt Screen (CSI ? 1049 h/l), Autowrap aus/ein (CSI ? 7 l/h), Maus-Modi (CSI ? 1000/1002/1003/1004/1006 h — unter tmux/zellij/screen ohne 1003), Synchronized Output (CSI ? 2026 h/l), beim Start Clear+Home+Cursor-Hide, beim Beenden ohne `preserveScreen`: Alt-Screen verlassen und das Dokument in die normale Scrollback schreiben, Clipboard via OSC 52 (base64).

**Viewport/Overflow/History**: vollständig durch `ScrollView` + Layout-Engine verwaltet (nicht durch Terminal-Scrollback). `ScrollView` hält `currentScrollTop`, `contentHeight`, `currentViewportHeight`, `followingEnd`; `updateLayout()` klemmt `scrollTop` auf `[0, contentHeight - viewportHeight]` und hält bei follow:"end" am Ende fest. `PAGE_SCROLL_OVERLAP = 4`.

### 2.4 Kitty-Bild-Verwaltung
- Main-Screen: `previousKittyImageIds`, `deleteKittyImage(id)` (APC G a=d,d=I,i=id).
- Alt-Screen: `prepareKittyScreen()` ersetzt bereits übertragene Bilder durch reine Placement-Kommandos; LRU-Eviction bei >16 Offscreen-Bildern bzw. >32 MiB Transmission- oder >64 MiB geschätzten Decoded-Bytes. Delete-All-Placements und Delete-All-Images als eigene Kommandos.

---

## 3. Komponentenmodell

### 3.1 Interface
- `Component`: `render(width) -> Zeilen`, optional `handleInput(data)`, optional `wantsKeyRelease` (Kitty Release-Events), `invalidate()`.
- `Focusable`: Feld `focused` (Guard `isFocusable()`).
- **Kein Lifecycle im klassischen Sinn**: kein mount/unmount/update. Konstruktion → `addChild` → `render(width)` pro Frame → optional `handleInput` bei Fokus → `invalidate()` (Cache-Verwerfung, rekursiv) → `removeChild`.
- Fokus zentral in `TuiBase.setFocus()`; fokussierte Komponente emittiert `CURSOR_MARKER` an der Cursorposition.
- Key-Release-Events werden gefiltert, außer `wantsKeyRelease === true`.
- Caching ist komponentenlokal (Text, Markdown, Box, Image cachen `{text, width} → lines`).

### 3.2 Komponentenliste
`Box`, `CancellableLoader`, `Editor`, `HStack`, `Image`, `Input`, `Loader`, `Markdown`, `ScrollView`, `SelectList`, `SettingsList`, `Spacer`, `Text`, `TruncatedText`, `VStack` (+ intern: `AltScreenFlashContainer`, `AltScreenSearchComponent`, `Stack` als abstrakte Basis).

### 3.3 Layout
Zwei Wege:

**(a) Ohne Layout-Engine** (Main-Screen bzw. `VStack.render`/`HStack.render`): Komponenten geben Zeilen zurück, `Container.render` konkateniert. `HStack` komponiert Spalten per `compositeTuiLine`.

**(b) Layout-Engine** (`src/layout.ts`, nur `TuiAltScreen`): Komponenten mit `[LAYOUT_NODE]()` melden Knotentyp `vstack | hstack | scroll`.
- `LayoutBox`: component, rect (x,y,width,height), clip, children, parent, lines, lineOffset, scrollView, scrollContentLines, layer.
- `LayoutContext.renderCache`: Memoisierung `render(width)` pro Frame; `measureHeight` = Zeilenanzahl, `measureWidth` = max sichtbare Breite.
- Größenverteilung: `allocateStackSizes(entries, intrinsicSizes, availableSize, gap)` in `components/stack.ts` — Flexbox-artig mit basis|"auto", grow, shrink, minSize, maxSize; Verteilung gewichtsproportional in Schleife (Shrink-Gewicht = shrink × max(1, size)).
- `align: stretch | start | center | end` (nur HStack-Querachse).
- Clipping: `clip = intersect(parentClip, rect)`, rekursiv.
- `paintBox` schreibt Zeilen in ein Screen-Array; Fast-Path: volle Breite auf unberührte Zeile → Referenzzuweisung statt `compositeTuiLine`.
- Scrollbar: Thumb-Höhe = round(trackHeight²/contentHeight), min 2; letzte Spalte der Box.
- Hit-Testing: `getScrollViewsAt(frame, x, y)` (tiefste zuerst), `getScrollViewBox`.

### 3.4 Text-Breite / Wrapping / Segmentierung (`src/utils.ts`)
- **Grapheme/Wort-Segmentierung**: `Intl.Segmenter` (grapheme + word), geteilte Instanzen.
- **Zellbreite** `graphemeWidth(segment)`:
  - Tab → **3** (fest!)
  - Spacing Marks (minus drei Ausnahmen, plus Liste burmesisch/tibetisch) → Codepoint-Anzahl
  - Zero-Width (Default_Ignorable | Control | Mark | Surrogate) → 0
  - RGI-Emoji → 2; Regional Indicators U+1F1E6..U+1F1FF → 2 (auch isoliert)
  - sonst East-Asian-Width aus `get-east-asian-width`, Nachkorrektur für Halfwidth/Fullwidth-Formen U+FF00..U+FFEF, Thai/Lao AM U+0E33/U+0EB3.
- `visibleWidth(str)`: Fast-Path für reines druckbares ASCII (Länge); sonst Tab→3 Spaces, ANSI/OSC/APC strippen, Graphem-Summe. **Cache**: Map, `WIDTH_CACHE_SIZE = 512`, FIFO-Eviction.
- **ANSI-Parsing** `extractAnsiCode(str, pos)`: CSI = nur Finalbytes `m G K H J`!, OSC = BEL oder ST, APC = BEL oder ST.
- **Wrapping** `wrapTextWithAnsi(text, width)`: Splitten an CR/LF/CRLF, pro Zeile `wrapSingleLine`. Tokenisierung: Runs aus Space/Word; CJK-Zeichen (Han/Hiragana/Katakana/Hangul/Bopomofo) als Einzeltokens. Überlange Tokens brechen graphemweise. Stil über `AnsiCodeTracker` über Zeilengrenzen (aktive Codes am Zeilenanfang, Underline-Off + OSC-8-Close am Zeilenende). Ergebniszeilen **nicht** gepaddet, trimEnd.
- `truncateToWidth(text, maxWidth, ellipsis="...", pad=false)`, `sliceByColumn`/`sliceWithWidth(line, startCol, length, strict)`, `extractSegments(...)` (Overlay-Komposition, gepoolter `AnsiCodeTracker` — globaler wiederverwendeter Zustand, nicht thread-safe).
- Der **Editor** hat einen **eigenen** Wrapper `wordWrapLine(line, maxWidth, preSegmented?)` in `components/editor.ts` mit `TextChunk {text, startIndex, endIndex}` (Positionsmapping für Cursor) und Paste-Markern als atomaren Segmenten.
- `normalizeTerminalOutput(str)`: Thai/Lao AM → Kompatibilitäts-Dekomposition; Tabs außerhalb von Escape-Sequenzen → 3 Spaces.

---

## 4. Input-Handling

### 4.1 Byte-Strom → Sequenzen (`src/stdin-buffer.ts`)
`StdinBuffer` (EventEmitter, Events `data`, `paste`).
- `extractCompleteSequences(buffer)`: CSI (Finalbyte 0x40..0x7E, Sondervalidierung SGR-Maus), OSC (BEL/ST), DCS, APC, SS3 (ESC O + 1 Byte), Meta (ESC + 1 Byte), Legacy-Maus (ESC [ M + 3 Bytes).
- Spezialfall doppeltes ESC gefolgt von `[`/`]`/`O`/`P`/`_` → nur erstes ESC emittieren (WezTerm-Kitty-Bug).
- Timeouts: `DEFAULT_SEQUENCE_TIMEOUT_MS = 50`, `DEFAULT_ESCAPE_TIMEOUT_MS = 10`. Lone ESC nutzt Escape-Timeout.
- Bracketed Paste (CSI 200~ / 201~) wird herausgeschnitten und als paste-Event emittiert; `ProcessTerminal` verpackt es wieder mit Markern für die Komponenten.
- Dedup: unterdrückt rohes Zeichen direkt nach unmodifizierter Kitty-CSI-u-Sequenz.

### 4.2 Protokoll-Negotiation (`src/terminal.ts`)
- Query beim Start: Kitty-Flags 7 (disambiguate|event types|alternate keys) pushen + Query + DA1 als Sentinel.
- Antwort CSI ? flags u → Kitty aktiv, modifyOtherKeys aus. DA-Antwort zuerst → Fallback modifyOtherKeys (CSI > 4;2 m). Deaktivieren: CSI < u bzw. CSI > 4;0 m.
- Fragmentierte Antworten gepuffert (150 ms).
- ESC-Timeout: `NOTAGENT_TUI_ESC_TIMEOUT` > (SSH: 100 ms) > 10 ms.

### 4.3 Key-Matching (`src/keys.ts`)
- `matchesKey(data, keyId)` mit KeyIds wie "ctrl+shift+p"; Modifier-Bits shift=1, alt=2, ctrl=4, super=8; CapsLock/NumLock-Bits maskiert.
- Kitty-CSI-u-Parser (codepoint, shiftedKey, baseLayoutKey, modifier, eventType 1=press/2=repeat/3=release); Pfeiltasten, Funktionstasten, Home/End mit Modifier+Event-Varianten.
- modifyOtherKeys-Parser (CSI 27;mod;code ~).
- Legacy-Tabellen: `LEGACY_KEY_SEQUENCES`, `LEGACY_SHIFT_SEQUENCES`, `LEGACY_CTRL_SEQUENCES`, `LEGACY_SEQUENCE_KEY_IDS`.
- Numpad-Normalisierung: `KITTY_FUNCTIONAL_KEY_EQUIVALENTS` (57399…57426).
- Nicht-lateinische Layouts: Fallback auf `baseLayoutKey` nur wenn Codepoint kein a–z und kein bekanntes Symbol (verhindert Fehlmatches bei Dvorak/Colemak).
- `isKeyRelease`/`isKeyRepeat` arbeiten **substring-basiert** (Suche nach ":3u" usw.), mit Ausnahme für Bracketed-Paste-Inhalte.
- `decodeKittyPrintable`/`decodePrintableKey` erzeugen druckbare Zeichen (nur Modifier ∈ {none, shift}).
- Windows-Terminal-Heuristik: Backspace-Byte 0x08 als ctrl+backspace vs. backspace.

### 4.4 Maus (nur `TuiAltScreen`)
- SGR-Parser (CSI < b;x;y M/m); Wheel: Bit 64, Richtung button&3; Legacy-6-Byte-Form; Drag: Bit 32.
- Features: Wheel-Routing durch verschachtelte ScrollViews (overscroll chain|contain), Scrollbar-Thumb-Drag, Text-Selektion mit Zeichen/Wort/Zeilen-Granularität (Doppel-/Dreifachklick, 500 ms, Zyklus count%3+1), Auto-Scroll beim Ziehen (50 ms Interval), OSC-8-Link-Klick, Rechtsklick-Paste (nur win32).
- Focus-Events: CSI I / CSI O (DECSET 1004) — FOCUS_OUT verwirft die Selektion.

### 4.5 Bracketed Paste
`ProcessTerminal.start()` aktiviert (CSI ? 2004 h), `stop()` deaktiviert. Editor ersetzt große Pastes durch Marker `[paste #N +M lines]` / `[paste #N K chars]`, expandiert bei Submit.

### 4.6 Resize
stdout-resize-Event → `requestRender()`. Beim Start auf Nicht-Windows SIGWINCH an sich selbst (Refresh nach Suspend/Resume). Größenänderung → Full-Redraw (Breite immer; Höhe außer Termux).

### 4.7 Terminal-Queries im Input-Pfad (`TuiBase.handleTerminalInput`)
Reihenfolge: OSC-11-Antwort → Color-Scheme-Report (CSI ? 997;1|2 n) → registrierte `TuiInputListener` (können konsumieren/transformieren) → Zellgröße (CSI 6;h;w t) → globaler Debug-Key shift+ctrl+d → Overlay-Fokus-Reparatur → `focusedComponent.handleInput(data)` + Immediate-Render.
Queries: Zellgröße (CSI 16 t), Hintergrundfarbe (OSC 11), Color Scheme (CSI ? 996 n), Notifications (CSI ? 2031 h/l).

---

## 5. Externe Abhängigkeiten

**dependencies**: `get-east-asian-width` 1.6.0 (nur `utils.ts` Breitenberechnung; Rust: `unicode-width` oder eigene EAW-Tabelle), `marked` 18.0.5 (`components/markdown.ts` Lexer; re-exportiert; Rust: eigener Markdown-Parser oder pulldown-cmark mit angepasster Tokenisierung).

**devDependencies**: `@xterm/headless` 5.5.0 (virtuelles Terminal für Tests), `chalk` (nur Tests/Demos).

**Native Addons** (im Paket): darwin-modifiers.node (`isModifierPressed`), win32-console-mode.node (`enableVirtualTerminalInput` + `isModifierPressed`). Quellen in `native/darwin/src/`, `native/win32/src/`.

**Web-APIs, die im Rust-Port ersetzt werden müssen**: `Intl.Segmenter` (grapheme + word → unicode-segmentation), RGI-Emoji-Regex, structuredClone, AbortController, base64.

---

## 6. Öffentliche API (`src/index.ts`)

Autocomplete (`AutocompleteItem/Provider/Suggestions`, `CombinedAutocompleteProvider`, `SlashCommand`), alle Komponenten (+ Options/Theme-Typen), `EditorComponent`, Fuzzy (`fuzzyMatch/fuzzyFilter`), Keybindings (`KeybindingsManager`, `TUI_KEYBINDINGS`, …), Keys (`matchesKey`, `parseKey`, `isKeyRelease/Repeat`, `decodeKittyPrintable`, Kitty-Aktivierungs-Flags), LaTeX (`renderLatex`), `StdinBuffer`, `ProcessTerminal`/`Terminal`, Terminal-Farben (OSC-11/Color-Scheme-Parser), Terminal-Bilder (Kitty/iTerm2-Encoding, Capability-Detection, Dimension-Parser für PNG/JPEG/GIF/WebP, `hyperlink`), TUI-Kern (`Component`, `Container`, `CURSOR_MARKER`, `compositeTuiLine`, Overlay-Typen, `TUI`, `ViewportTUI`), Renderer (`TuiAltScreen`, `TuiMainScreen` + RenderState), Utils (`visibleWidth`, `truncateToWidth`, `wrapTextWithAnsi`, `sliceByColumn`, `stripTerminalSequences`, `getOsc8LinkAtColumn`).

Intern zentral (nicht exportiert, für Port nötig): `layout.ts` (renderLayoutFrame, LayoutFrame, LayoutBox), `layout-node.ts`, `components/stack.ts` (allocateStackSizes), `alt-screen-search.ts`, `word-navigation.ts`, `kill-ring.ts`, `undo-stack.ts`, `native-modifiers.ts`, utils-Interna (extractSegments, sliceWithWidth, getGraphemeCellRange, normalizeTerminalOutput, extractAnsiCode, applyBackgroundToLine, Segmenter, cjkBreakRegex, PUNCTUATION_REGEX).

---

## 7. Terminal-Abstraktionen

`interface Terminal` (`src/terminal.ts:60`): start(onInput, onResize) / stop() / drainInput(maxMs=1000, idleMs=50) / write(data) / columns / rows / kittyProtocolActive / moveBy(lines) / hideCursor() / showCursor() / clearLine() / clearFromCursor() / clearScreen() / setTitle(title) / setProgress(active).

`ProcessTerminal`:
- **Raw Mode**: setRawMode(true) (alter Zustand gesichert/restauriert), utf8, resume; in stop() stdin pausieren **vor** Raw-Mode-Reset (verhindert dass gepuffertes Ctrl+D die Parent-Shell schließt).
- **Writer**: direkt stdout; optionales Write-Log via `NOTAGENT_TUI_WRITE_LOG`.
- **Dimensionen**: stdout.columns || COLUMNS || 80; rows || LINES || 24.
- **Alternate Screen ist NICHT Teil des Terminal-Interfaces** — TuiAltScreen schreibt die Sequenzen selbst.
- **Titel**: OSC 0. **Progress**: OSC 9;4 mit 1s-Keepalive.
- **Windows**: ENABLE_VIRTUAL_TERMINAL_INPUT via natives Addon **nach** setRawMode.
- **macOS/Windows Shift+Enter**: rohes CR → CSI 13;2 u wenn Apple Terminal bzw. win32 und nativer Shift gedrückt.
- **drainInput()**: deaktiviert erst Kitty/modifyOtherKeys, drainiert stdin bis Ruhe — verhindert Key-Release-Leaks in die Parent-Shell über langsames SSH.

**Env-Variablen**: NOTAGENT_HARDWARE_CURSOR, NOTAGENT_CLEAR_ON_SHRINK, NOTAGENT_DEBUG_REDRAW, NOTAGENT_TUI_DEBUG, NOTAGENT_TUI_WRITE_LOG, NOTAGENT_TUI_ESC_TIMEOUT, NOTAGENT_CODING_AGENT_DIR; Capability-Detection liest TERM, TERM_PROGRAM, TERMINAL_EMULATOR, COLORTERM, TMUX, ZELLIJ, STY, KITTY_WINDOW_ID, GHOSTTY_RESOURCES_DIR, WEZTERM_PANE, WARP_*, ITERM_SESSION_ID, WT_SESSION, TERMUX_VERSION, SSH_*.

---

## 8. Testing

- **Framework**: node:test + node:assert (kein Vitest in diesem Paket!). TS direkt via Type-Stripping.
- **Ansatz**: **Virtuelles Terminal** — `test/virtual-terminal.ts` (218 LOC) implementiert `Terminal` auf `@xterm/headless`: sendInput, resize, flush, getViewport (sichtbare Zeilen), getScrollBuffer (inkl. Scrollback), getCursorPosition, waitForRender (nextTick + 20ms + flush, passend zum 16-ms-Throttle).
- Tests assertieren **das tatsächlich vom Emulator gerenderte Bild**, nicht nur emittierte Bytes. Für Byte-Level: RecordingTerminal/LoggingVirtualTerminal-Subklassen.
- Reine Unit-Tests: keys, stdin-buffer, wrap-ansi, truncate-to-width, fuzzy, latex, layout, word-navigation, terminal-colors, keybindings, tab-width.
- Regressionstests explizit benannt (regional-indicator-width, overlay-cjk-boundary, isimageline-startswith).
- Umfang Tests: 39 Dateien, ~16 690 LOC (≈1:1 zu src). Größte Suiten: editor (4152), markdown (1667), tui-alt-screen (1267), overlay-non-capturing (1203), tui-render (832).

---

## Wichtigste Port-Fallstricke (aus dem Code)

1. **Zeilen sind Strings mit eingebetteten ANSI-Sequenzen** — kein Zell-/Attribut-Buffer. Der Rust-Port muss diese Repräsentation beibehalten (dann sind extractAnsiCode, AnsiCodeTracker, sliceWithWidth, extractSegments 1:1 nötig), sonst ändert sich das Diff-Verhalten beobachtbar.
2. **extractAnsiCode akzeptiert bei CSI nur die Finalbytes m G K H J** — andere CSI-Sequenzen werden nicht als Sequenz erkannt und flössen in die Breitenberechnung ein.
3. **Tab = 3 Zellen**, überall.
4. Main-Screen-Renderer **wirft Exception** (nach Crash-Log + stop()), wenn eine Zeile breiter als das Terminal ist.
5. visibleWidth-Cache: Map mit FIFO-Eviction bei 512; gepoolter StyleTracker in extractSegments ist globaler Zustand (nicht thread-safe → in Rust anders lösen, Verhalten beibehalten).
6. isKeyRelease/isKeyRepeat sind substring-basiert, nicht parser-basiert.
7. Fokus/Overlay-Restore ist eine eigene Zustandsmaschine (inactive | eligible | blocked mit resume restore-overlay | focus-target) — `src/tui.ts:196-206, 423-530`.
