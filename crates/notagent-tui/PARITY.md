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
| 2026-08-13 | `src/utils.ts` | 1326 | Task 2 |
| 2026-08-13 | `test/virtual-terminal.ts` | 218 | `src/test_terminal.rs` (Feature `test-terminal`) | verifiziert — `tests/virtual_terminal.rs` prüft den Harness gegen 21 `@xterm/headless`-Fixtures (Viewport, Scrollback, Cursor, Resize, CSI/OSC/APC, Synchronized Output) plus Ereignisaufzeichnung, Handler-Weiterleitung und Sequenz-Helfer |
| `test/wrap-ansi.test.ts` | 266 | Task 2 |
| 2026-08-13 | `test/truncate-to-width.test.ts` | 127 | Task 2 |
| 2026-08-13 | `test/tab-width.test.ts` | 88 | Task 2 |
| 2026-08-13 | `test/regression-regional-indicator-width.test.ts` | 52 | Task 2 |
| 2026-08-13 | `test/regression-overlay-cjk-boundary.test.ts` | 46 | Task 2 |
| 2026-08-13 | `src/keys.ts` | 1401 | Task 3 |
| 2026-08-13 | `test/keys.test.ts` | 633 | Task 3 |
| 2026-08-13 | `src/tui.ts:1-120` (Kontraktbereich) | 120 von 1257 | Master-Plan Task 2 (Kontrakt-Commit) |
| 2026-08-13 | `src/terminal.ts:1-140` (Kontraktbereich) | 140 von 559 | Master-Plan Task 2 (Kontrakt-Commit) |
| 2026-08-13 | `node_modules/get-east-asian-width/{index,lookup,lookup-data,utilities}.js` | 213 | Task 2 (Referenz für `eastAsianWidth`) |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| `src/tui.ts` | 1257 | `src/tui.rs` | Kontrakt portiert (Component/Focusable/CURSOR_MARKER); Rest offen (Task 5) | Klasse 1: `handleInput?` → Default-Methode; `isFocusable()`-Type-Guard → `Component::as_focusable()`; Referenzsemantik der TS-Objekte → `ComponentRef = Rc<RefCell<dyn Component>>` |
| `src/terminal.ts` | 559 | `src/terminal.rs` | Kontrakt portiert (`Terminal`-Trait); `ProcessTerminal` offen (Task 4) | Klasse 1: Default-Parameter von `drainInput` → `Option<u64>`; `async` Methode via `async-trait` (dyn-Kompatibilität) |
| `src/utils.ts` | 1326 | `src/utils.rs` (+ generiertes `src/unicode_tables.rs`) | verifiziert | Klasse 3: `Intl.Segmenter` → `unicode-segmentation`; `get-east-asian-width` und die `\p{…}`-Klassen (inkl. `\p{RGI_Emoji}`) als generierte Tabellen aus derselben Node-/Datenquelle (Rusts `regex` kennt weder `\p{RGI_Emoji}` noch `[A--[B]]`). Klasse 1: gepoolter `AnsiCodeTracker` in `extractSegments` → lokale Instanz (kein globaler Zustand, `clear()` beim Eintritt macht das verhaltensgleich); Width-Cache als `thread_local` mit identischer FIFO-Eviktion (512); Default-Parameter `truncateToWidth(text, w)` → zusätzliche Funktion `truncate_to_width_opts` |

## Portierte Testdateien

| TS-Testdatei | LOC | Rust-Test | Status |
|---|---|---|---|
| `test/virtual-terminal.ts` | 218 | `src/test_terminal.rs` (Feature `test-terminal`) | verifiziert — `tests/virtual_terminal.rs` prüft den Harness gegen 21 `@xterm/headless`-Fixtures (Viewport, Scrollback, Cursor, Resize, CSI/OSC/APC, Synchronized Output) plus Ereignisaufzeichnung, Handler-Weiterleitung und Sequenz-Helfer |
| `test/wrap-ansi.test.ts` | 266 | `tests/wrap_ansi.rs` (19 Fälle) | verifiziert |
| `test/truncate-to-width.test.ts` | 127 | `tests/truncate_to_width.rs` (16 Fälle) | verifiziert |
| `test/regression-regional-indicator-width.test.ts` | 52 | `tests/regression_regional_indicator_width.rs` (5 Fälle) | verifiziert |
| `test/tab-width.test.ts` | 88 | `tests/tab_width.rs` (3 von 4 Fällen) | Fall 4 („keeps tab-containing overlays on one physical terminal row") rendert über `TuiMainScreen` + virtuelles Terminal → Task 6 |
| `test/regression-overlay-cjk-boundary.test.ts` | 46 | `tests/regression_overlay_cjk_boundary.rs` (2 von 4 Fällen) | Die beiden `compositeTuiLine`-Fälle → Task 5 |
| — (zusätzlich) | — | `tests/utils_oracle.rs` + `tests/fixtures/utils-oracle.json` | Differenztest gegen die TS-Implementierung: 2695 Korpusfälle × {visibleWidth, wrapTextWithAnsi ×5 Breiten, truncateToWidth ×5 (auch mit `…`+Padding), sliceWithWidth ×5 Konfigurationen, extractSegments ×4} — alle identisch. Erzeugt von `tools/gen-utils-oracle.mjs` (Master-Plan, Risiko 1) |

## Werkzeuge

| Datei | Zweck |
|---|---|
| `tools/gen-unicode-tables.mjs` | Erzeugt `src/unicode_tables.rs` aus der Node-Runtime (Unicode 17.0) und `get-east-asian-width`. Die 1494 RGI-ZWJ-Sequenzen entstehen aus `emoji-zwj-sequences.txt` (Emoji 16.0, alle 1468 Einträge gegen V8 verifiziert) plus vollständiger Paarsuche über alle 1438 Emoji-Codepoints (26 Ergänzungen aus Unicode 17). |
| `tools/gen-utils-oracle.mjs` | Erzeugt `tests/fixtures/utils-oracle.json` aus `packages/tui/src/utils.ts`. |
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
3. **Emoji-Zellbreite**: vt100 rechnet 2 (wie `graphemeWidth` der TUI),
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
