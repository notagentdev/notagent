//! Terminal-Abstraktion.
//!
//! Port von `packages/tui/src/terminal.ts`. Dieser Commit enthält den Kontrakt
//! (`Terminal`-Trait, `packages/tui/src/terminal.ts:60-102`); `ProcessTerminal`
//! samt Raw-Mode, Kitty-Negotiation und Bracketed Paste folgt in Task 4 des
//! Workstream-A-Plans.

use async_trait::async_trait;

/// Callback für eingehende Terminaldaten (`onInput`).
///
/// Kein `Send`: der TUI-Kern läuft wie in TS einsträngig (Komponenten sind
/// `Rc<RefCell<…>>`); der stdin-Leser reicht Daten per Kanal an diesen Strang.
pub type InputHandler = Box<dyn FnMut(&str)>;
/// Callback für Größenänderungen (`onResize`).
pub type ResizeHandler = Box<dyn FnMut()>;

/// Minimales Terminal-Interface für die TUI.
///
/// Entspricht `interface Terminal` (`packages/tui/src/terminal.ts:60-102`).
/// Der Alternate Screen ist bewusst **nicht** Teil des Interfaces — der
/// Alt-Screen-Renderer schreibt die Sequenzen selbst.
#[async_trait(?Send)]
pub trait Terminal {
    /// Startet das Terminal mit Input- und Resize-Handler.
    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler);

    /// Stoppt das Terminal und stellt den Zustand wieder her.
    fn stop(&mut self);

    /// Leert stdin vor dem Beenden, damit Kitty-Key-Release-Events nicht über
    /// langsame SSH-Verbindungen in die Parent-Shell lecken.
    ///
    /// `max_ms` (Default 1000) begrenzt die Gesamtdauer, `idle_ms` (Default 50)
    /// beendet früh, wenn keine Eingabe mehr eintrifft.
    async fn drain_input(&mut self, max_ms: Option<u64>, idle_ms: Option<u64>);

    /// Schreibt Ausgabe ins Terminal.
    fn write(&mut self, data: &str);

    /// Terminalbreite in Spalten.
    fn columns(&self) -> usize;
    /// Terminalhöhe in Zeilen.
    fn rows(&self) -> usize;

    /// Ob das Kitty-Keyboard-Protokoll aktiv ist.
    fn kitty_protocol_active(&self) -> bool;

    /// Bewegt den Cursor relativ: negativ = hoch, positiv = runter.
    fn move_by(&mut self, lines: isize);

    /// Blendet den Cursor aus.
    fn hide_cursor(&mut self);
    /// Blendet den Cursor ein.
    fn show_cursor(&mut self);

    /// Löscht die aktuelle Zeile.
    fn clear_line(&mut self);
    /// Löscht vom Cursor bis zum Bildschirmende.
    fn clear_from_cursor(&mut self);
    /// Löscht den gesamten Bildschirm und setzt den Cursor auf (0,0).
    fn clear_screen(&mut self);

    /// Setzt den Fenstertitel des Terminals.
    fn set_title(&mut self, title: &str);

    /// Fortschrittsanzeige (OSC 9;4).
    fn set_progress(&mut self, active: bool);
}
