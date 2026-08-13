//! Kernabstraktionen des Komponentenmodells.
//!
//! Port von `packages/tui/src/tui.ts`. Dieser Commit enthält den Kontrakt
//! (Component/Focusable/CURSOR_MARKER) — Container, `TuiBase`, Overlay-Stack
//! und Render-Scheduling folgen in Task 5 des Workstream-A-Plans.

use std::cell::RefCell;
use std::rc::Rc;

/// Geteilte Referenz auf eine Komponente.
///
/// In TS hält sowohl der Container als auch der Aufrufer dieselbe
/// Komponenten-Instanz und mutiert sie (`editor.setText(...)` nach
/// `tui.addChild(editor)`). `Rc<RefCell<…>>` bildet diese Referenzsemantik
/// ab; der TUI-Kern ist wie in TS einsträngig (Abweichungsklasse 1).
pub type ComponentRef = Rc<RefCell<dyn Component>>;

/// Komponenten-Interface — jede Komponente implementiert es.
///
/// Entspricht `interface Component` (`packages/tui/src/tui.ts:23-46`).
pub trait Component {
    /// Rendert die Komponente für die gegebene Viewport-Breite.
    ///
    /// Rückgabe: ein String je Zeile (mit eingebetteten ANSI-Sequenzen).
    fn render(&mut self, width: usize) -> Vec<String>;

    /// Optionaler Handler für Tastatureingaben, wenn die Komponente Fokus hat.
    ///
    /// In TS ist `handleInput` optional; die Default-Implementierung hier ist
    /// verhaltensgleich zum Fehlen der Methode (Abweichungsklasse 1).
    fn handle_input(&mut self, data: &str) {
        let _ = data;
    }

    /// Wenn `true`, erhält die Komponente Key-Release-Events (Kitty-Protokoll).
    /// Default ist `false` — Release-Events werden herausgefiltert.
    fn wants_key_release(&self) -> bool {
        false
    }

    /// Verwirft zwischengespeicherten Renderzustand.
    fn invalidate(&mut self);

    /// Ersetzt den TS-Type-Guard `isFocusable()` (`"focused" in component`).
    ///
    /// Fokussierbare Komponenten geben hier `Some(self)` zurück.
    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        None
    }
}

/// Komponenten, die Fokus erhalten und einen Hardware-Cursor anzeigen können.
///
/// Bei Fokus emittiert die Komponente [`CURSOR_MARKER`] an der Cursorposition;
/// die TUI findet den Marker, entfernt ihn und positioniert den Hardware-Cursor
/// dort (wichtig für IME-Kandidatenfenster).
///
/// Entspricht `interface Focusable` (`packages/tui/src/tui.ts:57-63`).
pub trait Focusable {
    /// Wird von der TUI bei Fokuswechsel gesetzt.
    fn focused(&self) -> bool;
    /// Setzt den Fokuszustand; die Komponente emittiert dann [`CURSOR_MARKER`].
    fn set_focused(&mut self, focused: bool);
}

/// Cursor-Positionsmarker — APC-Sequenz (Application Program Command).
///
/// Nullbreite Escape-Sequenz, die Terminals ignorieren. Komponenten emittieren
/// sie bei Fokus an der Cursorposition; die TUI entfernt sie vor der Ausgabe.
///
/// `packages/tui/src/tui.ts:77`.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";
