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
| 2026-08-13 | `test/wrap-ansi.test.ts` | 266 | Task 2 |
| 2026-08-13 | `test/truncate-to-width.test.ts` | 127 | Task 2 |
| 2026-08-13 | `test/tab-width.test.ts` | 88 | Task 2 |
| 2026-08-13 | `test/regression-regional-indicator-width.test.ts` | 52 | Task 2 |
| 2026-08-13 | `test/regression-overlay-cjk-boundary.test.ts` | 46 | Task 2 |
| 2026-08-13 | `src/keys.ts` | 1401 | Task 3 |
| 2026-08-13 | `test/keys.test.ts` | 633 | Task 3 |
| 2026-08-13 | `src/tui.ts:1-120` (Kontraktbereich) | 120 von 1257 | Master-Plan Task 2 (Kontrakt-Commit) |
| 2026-08-13 | `src/terminal.ts:1-140` (Kontraktbereich) | 140 von 559 | Master-Plan Task 2 (Kontrakt-Commit) |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| `src/tui.ts` | 1257 | `src/tui.rs` | Kontrakt portiert (Component/Focusable/CURSOR_MARKER); Rest offen (Task 5) | Klasse 1: `handleInput?` → Default-Methode; `isFocusable()`-Type-Guard → `Component::as_focusable()`; Referenzsemantik der TS-Objekte → `ComponentRef = Rc<RefCell<dyn Component>>` |
| `src/terminal.ts` | 559 | `src/terminal.rs` | Kontrakt portiert (`Terminal`-Trait); `ProcessTerminal` offen (Task 4) | Klasse 1: Default-Parameter von `drainInput` → `Option<u64>`; `async` Methode via `async-trait` (dyn-Kompatibilität) |

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

Kontingenz laut Plan (eigener schlanker Emulator für die benötigte
Sequenz-Teilmenge), falls eine portierte Testerwartung eine dieser Abweichungen
doch beobachtet.
