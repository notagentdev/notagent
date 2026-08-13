//! Virtuelles Terminal für Tests.
//!
//! Port von `packages/tui/test/virtual-terminal.ts` (218 LOC). Die TS-Vorlage
//! implementiert `Terminal` auf `@xterm/headless`; hier übernimmt das
//! [`vt100`]-Crate die Emulation (Master-Tabelle: „@xterm/headless →
//! Rust-VT-Emulator-Crate"; Auswahlbegründung in `PARITY.md`).
//!
//! Das Modul liegt hinter dem Feature `test-terminal`, damit `vt100` nicht in
//! Produktivbuilds landet. Workstream C aktiviert es für die
//! Interactive-Mode-E2E-Szenarien aus Gate G3:
//! `notagent-tui = { workspace = true, features = ["test-terminal"] }`.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;

use crate::terminal::{InputHandler, ResizeHandler, Terminal};

/// Aufgezeichnetes Terminal-Ereignis.
///
/// Ersetzt die Unterklassen der TS-Suite (`RecordingTerminal`,
/// `LoggingVirtualTerminal`, `CapturingVirtualTerminal`), die in Rust mangels
/// Vererbung zu einer eingebauten Aufzeichnung zusammenfallen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    /// `start()` wurde aufgerufen.
    Start,
    /// `write(data)` wurde aufgerufen.
    Write(String),
    /// `stop()` wurde aufgerufen.
    Stop,
}

struct VirtualTerminalState {
    parser: vt100::Parser,
    input_handler: Option<InputHandler>,
    resize_handler: Option<ResizeHandler>,
    columns: usize,
    rows: usize,
    events: Vec<TerminalEvent>,
}

/// Virtuelles Terminal auf Basis von `vt100`.
///
/// Wie in TS teilen Test und TUI **dieselbe** Instanz: [`Clone`] liefert einen
/// weiteren Griff auf denselben Zustand.
#[derive(Clone)]
pub struct VirtualTerminal(Rc<RefCell<VirtualTerminalState>>);

/// Scrollback-Tiefe des Emulators (xterm.js nutzt ebenfalls einen begrenzten Puffer).
const SCROLLBACK_LEN: usize = 1000;

impl VirtualTerminal {
    /// Neues virtuelles Terminal mit `columns` × `rows` Zellen.
    pub fn new(columns: usize, rows: usize) -> Self {
        Self(Rc::new(RefCell::new(VirtualTerminalState {
            parser: vt100::Parser::new(
                u16::try_from(rows).expect("rows fit u16"),
                u16::try_from(columns).expect("columns fit u16"),
                SCROLLBACK_LEN,
            ),
            input_handler: None,
            resize_handler: None,
            columns,
            rows,
            events: Vec::new(),
        })))
    }

    /// Simuliert Tastatureingabe.
    pub fn send_input(&self, data: &str) {
        let handler = {
            let mut state = self.0.borrow_mut();
            state.input_handler.take()
        };
        if let Some(mut handler) = handler {
            handler(data);
            let mut state = self.0.borrow_mut();
            if state.input_handler.is_none() {
                state.input_handler = Some(handler);
            }
        }
    }

    /// Ändert die Terminalgröße und ruft den Resize-Handler.
    ///
    /// `@xterm/headless` verankert beim Resize unten: beim Verkleinern wandern
    /// obere Zeilen in den Scrollback, beim Vergrößern kommen sie zurück (siehe
    /// PARITY.md). `vt100` kennt das nicht, deshalb wird der Puffer hier mit dem
    /// bisherigen Text neu aufgebaut. Attribute gehen dabei verloren; der
    /// Harness liest ohnehin nur Text.
    pub fn resize(&self, columns: usize, rows: usize) {
        let content = {
            let mut lines = self.get_scroll_buffer();
            while lines.last().is_some_and(|line| line.is_empty()) {
                lines.pop();
            }
            lines
        };
        let handler = {
            let mut state = self.0.borrow_mut();
            state.columns = columns;
            state.rows = rows;
            state.parser = vt100::Parser::new(
                u16::try_from(rows).expect("rows fit u16"),
                u16::try_from(columns).expect("columns fit u16"),
                SCROLLBACK_LEN,
            );
            state.parser.process(content.join("\r\n").as_bytes());
            state.resize_handler.take()
        };
        if let Some(mut handler) = handler {
            handler();
            let mut state = self.0.borrow_mut();
            if state.resize_handler.is_none() {
                state.resize_handler = Some(handler);
            }
        }
    }

    /// Wartet, bis alle Schreibvorgänge verarbeitet sind.
    ///
    /// `vt100` verarbeitet synchron; die Methode existiert für die
    /// Strukturgleichheit zur TS-Vorlage.
    pub fn flush(&self) {}

    /// Sichtbarer Viewport — wie `line.translateToString(true)` in xterm.js:
    /// geschriebene Leerzeichen bleiben erhalten, nie beschriebene oder
    /// gelöschte Zellen am Zeilenende fallen weg.
    pub fn get_viewport(&self) -> Vec<String> {
        let state = self.0.borrow();
        let cols = u16::try_from(state.columns).expect("columns fit u16");
        state.parser.screen().rows(0, cols).collect()
    }

    /// Wie [`Self::get_viewport`], als Convenience nach einem Schreibvorgang.
    pub fn flush_and_get_viewport(&self) -> Vec<String> {
        self.flush();
        self.get_viewport()
    }

    /// Gesamter Puffer inklusive Scrollback.
    pub fn get_scroll_buffer(&self) -> Vec<String> {
        let mut state = self.0.borrow_mut();
        let cols = u16::try_from(state.columns).expect("columns fit u16");
        state.parser.screen_mut().set_scrollback(usize::MAX);
        let max = state.parser.screen().scrollback();
        let mut out: Vec<String> = Vec::new();
        let mut offset = max;
        loop {
            state.parser.screen_mut().set_scrollback(offset);
            let rows: Vec<String> = state.parser.screen().rows(0, cols).collect();
            if offset == 0 {
                out.extend(rows);
                break;
            }
            let take = offset.min(rows.len());
            out.extend(rows.into_iter().take(take));
            offset -= take;
        }
        state.parser.screen_mut().set_scrollback(0);
        out
    }

    /// Ob die Zelle kursiv gesetzt ist (TS-Tests lesen `cell.isItalic()`).
    pub fn get_cell_italic(&self, row: usize, col: usize) -> bool {
        let state = self.0.borrow();
        state
            .parser
            .screen()
            .cell(
                u16::try_from(row).expect("row fits u16"),
                u16::try_from(col).expect("col fits u16"),
            )
            .is_some_and(vt100::Cell::italic)
    }

    /// Cursorposition als `(x, y)` — Spalte und Zeile im Viewport.
    pub fn get_cursor_position(&self) -> (usize, usize) {
        let state = self.0.borrow();
        let (row, col) = state.parser.screen().cursor_position();
        (usize::from(col), usize::from(row))
    }

    /// Leert den sichtbaren Bereich (entspricht `xterm.clear()`).
    pub fn clear(&self) {
        self.reset();
    }

    /// Setzt den Emulator vollständig zurück (entspricht `xterm.reset()`).
    pub fn reset(&self) {
        let mut state = self.0.borrow_mut();
        let (columns, rows) = (state.columns, state.rows);
        state.parser = vt100::Parser::new(
            u16::try_from(rows).expect("rows fit u16"),
            u16::try_from(columns).expect("columns fit u16"),
            SCROLLBACK_LEN,
        );
    }

    /// Alle aufgezeichneten Ereignisse (`RecordingTerminal.events`).
    pub fn events(&self) -> Vec<TerminalEvent> {
        self.0.borrow().events.clone()
    }

    /// Alle Schreibvorgänge aneinandergehängt
    /// (`LoggingVirtualTerminal.getWrites()`, `CapturingVirtualTerminal.getOutput()`).
    pub fn get_writes(&self) -> String {
        self.0
            .borrow()
            .events
            .iter()
            .filter_map(|event| match event {
                TerminalEvent::Write(data) => Some(data.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Verwirft die aufgezeichneten Ereignisse (`clearWrites()`).
    pub fn clear_writes(&self) {
        self.0.borrow_mut().events.clear();
    }

    /// Wartet, bis die gedrosselte Render-Pipeline der TUI durchgelaufen ist
    /// (16-ms-Throttle; die TS-Vorlage wartet nextTick + 20 ms + flush).
    pub async fn wait_for_render(&self) {
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        self.flush();
    }
}

#[async_trait(?Send)]
impl Terminal for VirtualTerminal {
    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler) {
        {
            let mut state = self.0.borrow_mut();
            state.events.push(TerminalEvent::Start);
            state.input_handler = Some(on_input);
            state.resize_handler = Some(on_resize);
        }
        // Bracketed Paste aktivieren — wie ProcessTerminal.
        self.write("\x1b[?2004h");
    }

    fn stop(&mut self) {
        self.0.borrow_mut().events.push(TerminalEvent::Stop);
        self.write("\x1b[?2004l");
        let mut state = self.0.borrow_mut();
        state.input_handler = None;
        state.resize_handler = None;
    }

    async fn drain_input(&mut self, _max_ms: Option<u64>, _idle_ms: Option<u64>) {
        // Kein stdin im virtuellen Terminal.
    }

    fn write(&mut self, data: &str) {
        let mut state = self.0.borrow_mut();
        state.events.push(TerminalEvent::Write(data.to_string()));
        // `vt100` kennt CSI 3J (Scrollback löschen) nicht. Der Renderer emittiert
        // die Sequenz ausschließlich in `fullRender(clear)` unmittelbar nach
        // CSI 2J + CSI H, der Schirm ist an der Stelle also bereits leer — ein
        // frischer Parser ist dort semantisch exakt. Die Vorbedingung wird
        // geprüft, damit die Abweichung nicht stillschweigend greift.
        let mut rest = data;
        while let Some(index) = rest.find("\x1b[3J") {
            let (head, tail) = rest.split_at(index);
            state.parser.process(head.as_bytes());
            let cols = u16::try_from(state.columns).expect("columns fit u16");
            let non_empty = state
                .parser
                .screen()
                .rows(0, cols)
                .any(|row| !row.is_empty());
            assert!(
                !non_empty,
                "CSI 3J mit nicht leerem Schirm: das virtuelle Terminal bildet nur \
                 die Renderer-Sequenz 2J+H+3J ab (siehe PARITY.md)"
            );
            let rows = u16::try_from(state.rows).expect("rows fit u16");
            state.parser = vt100::Parser::new(rows, cols, SCROLLBACK_LEN);
            rest = &tail["\x1b[3J".len()..];
        }
        state.parser.process(rest.as_bytes());
    }

    fn columns(&self) -> usize {
        self.0.borrow().columns
    }

    fn rows(&self) -> usize {
        self.0.borrow().rows
    }

    fn kitty_protocol_active(&self) -> bool {
        // Das virtuelle Terminal meldet das Kitty-Protokoll immer als aktiv.
        true
    }

    fn move_by(&mut self, lines: isize) {
        match lines.cmp(&0) {
            std::cmp::Ordering::Greater => self.write(&format!("\x1b[{lines}B")),
            std::cmp::Ordering::Less => self.write(&format!("\x1b[{}A", -lines)),
            std::cmp::Ordering::Equal => {}
        }
    }

    fn hide_cursor(&mut self) {
        self.write("\x1b[?25l");
    }

    fn show_cursor(&mut self) {
        self.write("\x1b[?25h");
    }

    fn clear_line(&mut self) {
        self.write("\x1b[K");
    }

    fn clear_from_cursor(&mut self) {
        self.write("\x1b[J");
    }

    fn clear_screen(&mut self) {
        self.write("\x1b[2J\x1b[H");
    }

    fn set_title(&mut self, title: &str) {
        self.write(&format!("\x1b]0;{title}\x07"));
    }

    fn set_progress(&mut self, _active: bool) {}
}
