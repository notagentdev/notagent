use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;

use crate::terminal::{InputHandler, PumpResult, ResizeHandler, Terminal, TerminalPump};

/// Aufgezeichnetes Terminal-Ereignis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Start,
    Write(String),
    Stop,
}

struct VirtualTerminalState {
    parser: vt100::Parser,
    input_handler: Option<InputHandler>,
    resize_handler: Option<ResizeHandler>,
    columns: usize,
    rows: usize,
    events: Vec<TerminalEvent>,
    /// Events a [`VirtualTerminalPump`] hands to the render loop, plus its
    /// wake-up. The dispatch itself already happened in `send_input`/`resize`;
    /// the loop only needs to learn that it has to render again.
    pump_events: std::collections::VecDeque<PumpResult>,
    pump_notify: Rc<tokio::sync::Notify>,
}

#[derive(Clone)]
pub struct VirtualTerminal(Rc<RefCell<VirtualTerminalState>>);

const SCROLLBACK_LEN: usize = 1000;

impl VirtualTerminal {
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
            pump_events: std::collections::VecDeque::new(),
            pump_notify: Rc::new(tokio::sync::Notify::new()),
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
        self.push_pump_event(PumpResult::Input);
    }

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
        self.push_pump_event(PumpResult::Resize);
    }

    pub fn flush(&self) {}

    /// Deleted cells at the end of a line are omitted.
    pub fn get_viewport(&self) -> Vec<String> {
        let state = self.0.borrow();
        let cols = u16::try_from(state.columns).expect("columns fit u16");
        state.parser.screen().rows(0, cols).collect()
    }

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

    pub fn get_cursor_position(&self) -> (usize, usize) {
        let state = self.0.borrow();
        let (row, col) = state.parser.screen().cursor_position();
        (usize::from(col), usize::from(row))
    }

    pub fn clear(&self) {
        self.reset();
    }

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

    /// Returns all writes concatenated.
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

    pub fn clear_writes(&self) {
        self.0.borrow_mut().events.clear();
    }

    /// Pump for the render loop of [`crate::tui::run_until`].
    /// The virtual terminal dispatches input synchronously in
    /// [`Self::send_input`], so its pump only reports that something happened
    /// and wakes the loop for the next frame.
    pub fn pump_handle(&self) -> VirtualTerminalPump {
        VirtualTerminalPump(Rc::clone(&self.0))
    }

    fn push_pump_event(&self, result: PumpResult) {
        let notify = {
            let mut state = self.0.borrow_mut();
            state.pump_events.push_back(result);
            Rc::clone(&state.pump_notify)
        };
        notify.notify_waiters();
    }

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
        self.write("\x1b[?2004h");
    }

    fn stop(&mut self) {
        self.0.borrow_mut().events.push(TerminalEvent::Stop);
        self.write("\x1b[?2004l");
        {
            let mut state = self.0.borrow_mut();
            state.input_handler = None;
            state.resize_handler = None;
        }
        self.push_pump_event(PumpResult::Eof);
    }

    async fn drain_input(&mut self, _max_ms: Option<u64>, _idle_ms: Option<u64>) {}

    fn write(&mut self, data: &str) {
        let mut state = self.0.borrow_mut();
        state.events.push(TerminalEvent::Write(data.to_string()));
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
                "CSI 3J requires an empty screen because the virtual terminal only \
                 models the renderer sequence 2J+H+3J"
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

/// Pump of a [`VirtualTerminal`] (see [`VirtualTerminal::pump_handle`]).
pub struct VirtualTerminalPump(Rc<RefCell<VirtualTerminalState>>);

#[async_trait(?Send)]
impl TerminalPump for VirtualTerminalPump {
    async fn pump(&mut self) -> PumpResult {
        loop {
            let notify = Rc::clone(&self.0.borrow().pump_notify);
            let notified = notify.notified();
            tokio::pin!(notified);
            // Register before looking into the queue, otherwise an event
            notified.as_mut().enable();
            let event = self.0.borrow_mut().pump_events.pop_front();
            match event {
                Some(result) => return result,
                None => notified.await,
            }
        }
    }
}
