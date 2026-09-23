use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use async_trait::async_trait;

use crate::keys::set_kitty_protocol_active;
use crate::native_modifiers::{ModifierKey, is_native_modifier_pressed};
use crate::stdin_buffer::{StdinBuffer, StdinBufferOptions, StdinEvent};

pub type InputHandler = Box<dyn FnMut(&str)>;
pub type ResizeHandler = Box<dyn FnMut()>;

#[async_trait(?Send)]
pub trait Terminal {
    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler);

    fn stop(&mut self);

    async fn drain_input(&mut self, max_ms: Option<u64>, idle_ms: Option<u64>);

    /// Schreibt Ausgabe ins Terminal.
    fn write(&mut self, data: &str);

    /// Terminalbreite in Spalten.
    fn columns(&self) -> usize;
    fn rows(&self) -> usize;

    /// Screen row (0-based) of the cursor when `start` ran, if the terminal
    /// reported it. A renderer that writes below the shell output starts there.
    fn start_cursor_row(&self) -> Option<usize> {
        None
    }

    fn kitty_protocol_active(&self) -> bool;

    fn move_by(&mut self, lines: isize);

    fn hide_cursor(&mut self);
    fn show_cursor(&mut self);

    fn clear_line(&mut self);
    fn clear_from_cursor(&mut self);
    fn clear_screen(&mut self);

    fn set_title(&mut self, title: &str);

    /// Fortschrittsanzeige (OSC 9;4).
    fn set_progress(&mut self, active: bool);
}

const TERMINAL_PROGRESS_KEEPALIVE_MS: u64 = 1000;
const TERMINAL_PROGRESS_ACTIVE_SEQUENCE: &str = "\x1b]9;4;3\x07";
const TERMINAL_PROGRESS_CLEAR_SEQUENCE: &str = "\x1b]9;4;0\x07";
const NATIVE_SHIFT_ENTER_SEQUENCE: &str = "\x1b[13;2u";
const DESIRED_KITTY_KEYBOARD_PROTOCOL_FLAGS: u32 = 7;
const KEYBOARD_PROTOCOL_RESPONSE_FRAGMENT_TIMEOUT_MS: u64 = 150;
const KITTY_KEYBOARD_PROTOCOL_QUERY: &str = "\x1b[>7u\x1b[?u\x1b[c";

const DEFAULT_ESCAPE_TIMEOUT_MS: u64 = 10;
const DEFAULT_SSH_ESCAPE_TIMEOUT_MS: u64 = 100;

/// Response of the keyboard protocol negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardProtocolNegotiationSequence {
    /// `CSI ? <flags> u` — Kitty keyboard protocol flags.
    KittyFlags(u32),
    /// `CSI ? … c` — device attributes (DA1), used as the sentinel.
    DeviceAttributes,
}

/// Parse a keyboard protocol negotiation response.
pub fn parse_keyboard_protocol_negotiation_sequence(
    sequence: &str,
) -> Option<KeyboardProtocolNegotiationSequence> {
    if let Some(body) = sequence
        .strip_prefix("\x1b[?")
        .and_then(|rest| rest.strip_suffix('u'))
        && !body.is_empty()
        && body.chars().all(|c| c.is_ascii_digit())
    {
        return Some(KeyboardProtocolNegotiationSequence::KittyFlags(
            body.parse().ok()?,
        ));
    }
    if let Some(body) = sequence
        .strip_prefix("\x1b[?")
        .and_then(|rest| rest.strip_suffix('c'))
        && body.chars().all(|c| c.is_ascii_digit() || c == ';')
    {
        return Some(KeyboardProtocolNegotiationSequence::DeviceAttributes);
    }
    None
}

fn is_keyboard_protocol_negotiation_sequence_prefix(sequence: &str) -> bool {
    if sequence == "\x1b[" {
        return true;
    }
    sequence
        .strip_prefix("\x1b[?")
        .is_some_and(|body| body.chars().all(|c| c.is_ascii_digit() || c == ';'))
}

/// Whether the process runs inside Apple Terminal on macOS.
pub fn is_apple_terminal_session() -> bool {
    cfg!(target_os = "macos") && std::env::var("TERM_PROGRAM").as_deref() == Ok("Apple_Terminal")
}

/// Rewrite a bare Return to the CSI-u Shift+Enter sequence when the native
/// modifier state says Shift is held.
pub fn normalize_native_shift_enter_input(
    data: &str,
    should_detect_native_shift_enter: bool,
    is_shift_pressed: bool,
) -> String {
    if should_detect_native_shift_enter && data == "\r" && is_shift_pressed {
        return NATIVE_SHIFT_ENTER_SEQUENCE.to_string();
    }
    data.to_string()
}

/// Apple Terminal variant of [`normalize_native_shift_enter_input`].
pub fn normalize_apple_terminal_input(
    data: &str,
    is_apple_terminal: bool,
    is_shift_pressed: bool,
) -> String {
    normalize_native_shift_enter_input(data, is_apple_terminal, is_shift_pressed)
}

/// How long to wait for the rest of an escape sequence before dispatching a
/// lone ESC as the Escape key.
/// Legacy Alt+key input is ESC plus another byte, so high-latency transports
/// need a longer reassembly window.
pub fn resolve_escape_timeout_ms() -> u64 {
    resolve_escape_timeout_ms_from(&|name| std::env::var(name).ok())
}

/// [`resolve_escape_timeout_ms`] with an injectable environment (for tests).
pub fn resolve_escape_timeout_ms_from(env: &dyn Fn(&str) -> Option<String>) -> u64 {
    if let Some(configured) = env("NOTAGENT_TUI_ESC_TIMEOUT")
        && let Ok(value) = configured.trim().parse::<f64>()
        && value.is_finite()
        && value > 0.0
    {
        return value as u64;
    }
    if env("SSH_CONNECTION").is_some() || env("SSH_TTY").is_some() {
        return DEFAULT_SSH_ESCAPE_TIMEOUT_MS;
    }
    DEFAULT_ESCAPE_TIMEOUT_MS
}

/// Sink for everything the terminal writes.
enum OutputSink {
    Stdout,
    /// Only constructed by [`ProcessTerminal::with_writer`], which the
    #[cfg(feature = "test-terminal")]
    Collector(Box<dyn FnMut(&str)>),
}

/// What [`ProcessTerminal::pump`] dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpResult {
    /// Input was forwarded to the input handler.
    Input,
    /// A resize was forwarded to the resize handler.
    Resize,
    /// A buffered sequence was flushed after its timeout.
    Timeout,
    /// stdin reached end of file.
    Eof,
}

/// Real terminal on `process.stdin` / `process.stdout`.
pub struct ProcessTerminal {
    input_handler: Option<InputHandler>,
    resize_handler: Option<ResizeHandler>,
    kitty_protocol_active: bool,
    modify_other_keys_active: bool,
    keyboard_protocol_pushed: bool,
    keyboard_protocol_negotiation_buffer: String,
    keyboard_protocol_buffer_deadline: Option<Instant>,
    stdin_buffer: Option<StdinBuffer>,
    stdin_deadline: Option<Instant>,
    stdin_rx: Option<mpsc::UnboundedReceiver<Vec<u8>>>,
    #[cfg(unix)]
    resize_signal: Option<tokio::signal::unix::Signal>,
    progress_active: bool,
    progress_next_keepalive: Option<Instant>,
    write_log_path: Option<std::path::PathBuf>,
    output: OutputSink,
    saved_termios: Option<RawModeState>,
    columns_override: Option<usize>,
    rows_override: Option<usize>,
    /// Bumped by every `start()`/`stop()`; leases from an older epoch belong to
    /// a reader thread that no longer exists and are dropped instead of restored.
    pump_epoch: u64,
    start_cursor_row: Option<usize>,
    late_cursor_report_until: Option<Instant>,
}

impl Default for ProcessTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessTerminal {
    /// New terminal writing to stdout.
    pub fn new() -> Self {
        Self {
            input_handler: None,
            resize_handler: None,
            kitty_protocol_active: false,
            modify_other_keys_active: false,
            keyboard_protocol_pushed: false,
            keyboard_protocol_negotiation_buffer: String::new(),
            keyboard_protocol_buffer_deadline: None,
            stdin_buffer: None,
            stdin_deadline: None,
            stdin_rx: None,
            #[cfg(unix)]
            resize_signal: None,
            progress_active: false,
            progress_next_keepalive: None,
            write_log_path: resolve_write_log_path(),
            output: OutputSink::Stdout,
            saved_termios: None,
            columns_override: None,
            rows_override: None,
            pump_epoch: 0,
            start_cursor_row: None,
            late_cursor_report_until: None,
        }
    }

    /// Whether the Kitty keyboard protocol is active.
    pub fn kitty_protocol_active(&self) -> bool {
        self.kitty_protocol_active
    }

    /// Whether the modifyOtherKeys fallback is active.
    pub fn modify_other_keys_active(&self) -> bool {
        self.modify_other_keys_active
    }

    fn write_out(&mut self, data: &str) {
        match &mut self.output {
            OutputSink::Stdout => {
                let mut stdout = std::io::stdout();
                let _ = stdout.write_all(data.as_bytes());
                let _ = stdout.flush();
            }
            #[cfg(feature = "test-terminal")]
            OutputSink::Collector(sink) => sink(data),
        }
        if let Some(path) = &self.write_log_path
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        {
            let _ = file.write_all(data.as_bytes());
        }
    }

    /// Query the terminal for Kitty keyboard protocol support.
    /// Kitty's progressive enhancement detection requires requesting the desired
    /// flags before querying them. The trailing DA query is a sentinel supported
    /// by terminals that do not know the Kitty protocol; receiving DA before a
    /// Kitty response enables the modifyOtherKeys fallback without a startup
    /// timeout. Requested flags: 1 = disambiguate, 2 = event types,
    /// 4 = alternate keys.
    fn query_and_enable_kitty_protocol(&mut self) {
        debug_assert_eq!(DESIRED_KITTY_KEYBOARD_PROTOCOL_FLAGS, 7);
        self.stdin_buffer = Some(StdinBuffer::new(StdinBufferOptions {
            timeout: None,
            escape_timeout: Some(resolve_escape_timeout_ms()),
        }));
        self.keyboard_protocol_pushed = true;
        self.clear_keyboard_protocol_negotiation_buffer();
        self.write_out(KITTY_KEYBOARD_PROTOCOL_QUERY);
    }

    /// Feed a chunk of raw stdin data through the buffer and the negotiation.
    /// Returns the sequences destined for the input handler instead of calling
    /// terminal through the TUI (overlays hide the cursor, components write), so
    /// it must not run while the terminal is borrowed. The order of the
    /// forwarded sequences is unchanged; only their interleaving with the
    /// negotiation's own writes inside a single chunk differs.
    fn handle_stdin_chunk(&mut self, chunk: &[u8]) -> Vec<String> {
        let Some(buffer) = self.stdin_buffer.as_mut() else {
            return Vec::new();
        };
        let events = buffer.process_bytes(chunk);
        self.stdin_deadline = self
            .stdin_buffer
            .as_ref()
            .and_then(StdinBuffer::pending_timeout_ms)
            .map(|ms| Instant::now() + Duration::from_millis(ms));
        self.dispatch_events(events)
    }

    /// The reply to a start-up cursor query that timed out still arrives as
    /// input. A modified F3 shares its shape (`CSI 1 ; 5 R`), so only one
    /// report within a short window after the timeout is swallowed.
    fn consume_late_cursor_report(&mut self, sequence: &str) -> bool {
        let Some(until) = self.late_cursor_report_until else {
            return false;
        };
        if Instant::now() > until {
            self.late_cursor_report_until = None;
            return false;
        }
        let whole = find_cursor_report(sequence.as_bytes())
            .is_some_and(|(_, range)| range == (0..sequence.len()));
        if whole {
            self.late_cursor_report_until = None;
        }
        whole
    }

    fn dispatch_events(&mut self, events: Vec<StdinEvent>) -> Vec<String> {
        let mut forwards = Vec::new();
        for event in events {
            match event {
                StdinEvent::Data(sequence) => {
                    match self.read_keyboard_protocol_negotiation_sequence(&sequence, &mut forwards)
                    {
                        NegotiationRead::Pending => {
                            self.schedule_keyboard_protocol_negotiation_buffer_flush();
                        }
                        NegotiationRead::Sequence(negotiation) => {
                            self.handle_keyboard_protocol_negotiation_sequence(negotiation);
                        }
                        NegotiationRead::None => {
                            if self.consume_late_cursor_report(&sequence) {
                                continue;
                            }
                            forwards.push(normalize_forwarded_input(&sequence))
                        }
                    }
                }
                StdinEvent::Paste(content) => {
                    // Re-wrap paste content with bracketed paste markers for the
                    // existing editor handling.
                    forwards.push(format!("\x1b[200~{content}\x1b[201~"));
                }
            }
        }
        forwards
    }

    /// Hand the collected sequences to the input handler.
    /// Only for callers that own the terminal exclusively (the test helpers);
    /// the shared path forwards through [`ProcessTerminalPump`], which releases
    /// the borrow first.
    #[cfg(feature = "test-terminal")]
    fn forward_to_input_handler(&mut self, forwards: Vec<String>) {
        for sequence in forwards {
            if let Some(handler) = self.input_handler.as_mut() {
                handler(&sequence);
            }
        }
    }

    fn handle_keyboard_protocol_negotiation_sequence(
        &mut self,
        negotiation: KeyboardProtocolNegotiationSequence,
    ) {
        self.clear_keyboard_protocol_negotiation_buffer();
        match negotiation {
            KeyboardProtocolNegotiationSequence::KittyFlags(flags) => {
                if flags != 0 {
                    self.disable_modify_other_keys();
                    if !self.kitty_protocol_active {
                        self.kitty_protocol_active = true;
                        set_kitty_protocol_active(true);
                    }
                } else {
                    self.enable_modify_other_keys();
                }
            }
            KeyboardProtocolNegotiationSequence::DeviceAttributes => {
                if !self.kitty_protocol_active {
                    self.enable_modify_other_keys();
                }
            }
        }
    }

    fn read_keyboard_protocol_negotiation_sequence(
        &mut self,
        sequence: &str,
        forwards: &mut Vec<String>,
    ) -> NegotiationRead {
        if !self.keyboard_protocol_negotiation_buffer.is_empty() {
            let buffered = format!("{}{sequence}", self.keyboard_protocol_negotiation_buffer);
            if let Some(negotiation) = parse_keyboard_protocol_negotiation_sequence(&buffered) {
                self.clear_keyboard_protocol_negotiation_buffer();
                return NegotiationRead::Sequence(negotiation);
            }
            if is_keyboard_protocol_negotiation_sequence_prefix(&buffered) {
                self.set_keyboard_protocol_negotiation_buffer(&buffered);
                return NegotiationRead::Pending;
            }
            self.flush_keyboard_protocol_negotiation_buffer_as_input(forwards);
        }

        if let Some(negotiation) = parse_keyboard_protocol_negotiation_sequence(sequence) {
            return NegotiationRead::Sequence(negotiation);
        }
        if is_keyboard_protocol_negotiation_sequence_prefix(sequence) {
            self.set_keyboard_protocol_negotiation_buffer(sequence);
            return NegotiationRead::Pending;
        }
        NegotiationRead::None
    }

    fn set_keyboard_protocol_negotiation_buffer(&mut self, sequence: &str) {
        self.keyboard_protocol_buffer_deadline = None;
        self.keyboard_protocol_negotiation_buffer = sequence.to_string();
    }

    fn clear_keyboard_protocol_negotiation_buffer(&mut self) {
        self.keyboard_protocol_buffer_deadline = None;
        self.keyboard_protocol_negotiation_buffer.clear();
    }

    fn flush_keyboard_protocol_negotiation_buffer_as_input(&mut self, forwards: &mut Vec<String>) {
        if self.keyboard_protocol_negotiation_buffer.is_empty() {
            return;
        }
        let sequence = std::mem::take(&mut self.keyboard_protocol_negotiation_buffer);
        self.clear_keyboard_protocol_negotiation_buffer();
        forwards.push(normalize_forwarded_input(&sequence));
    }

    fn schedule_keyboard_protocol_negotiation_buffer_flush(&mut self) {
        if self.keyboard_protocol_negotiation_buffer.is_empty()
            || self.keyboard_protocol_buffer_deadline.is_some()
        {
            return;
        }
        self.keyboard_protocol_buffer_deadline = Some(
            Instant::now() + Duration::from_millis(KEYBOARD_PROTOCOL_RESPONSE_FRAGMENT_TIMEOUT_MS),
        );
    }

    fn enable_modify_other_keys(&mut self) {
        if self.kitty_protocol_active || self.modify_other_keys_active {
            return;
        }
        self.write_out("\x1b[>4;2m");
        self.modify_other_keys_active = true;
    }

    fn disable_modify_other_keys(&mut self) {
        if !self.modify_other_keys_active {
            return;
        }
        self.write_out("\x1b[>4;0m");
        self.modify_other_keys_active = false;
    }

    /// The timeout that has come due, in the order the pump handles them.
    pub(crate) fn due_timeout(&self, now: Instant) -> Option<DueTimeout> {
        if self.stdin_deadline.is_some_and(|deadline| deadline <= now) {
            return Some(DueTimeout::Stdin);
        }
        if self
            .keyboard_protocol_buffer_deadline
            .is_some_and(|deadline| deadline <= now)
        {
            return Some(DueTimeout::Negotiation);
        }
        if self
            .progress_next_keepalive
            .is_some_and(|deadline| deadline <= now)
        {
            return Some(DueTimeout::ProgressKeepalive);
        }
        None
    }

    /// When the earliest pending timeout fires.
    pub(crate) fn next_timeout(&self) -> Option<Instant> {
        [
            self.stdin_deadline,
            self.keyboard_protocol_buffer_deadline,
            self.progress_next_keepalive,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Flush the buffered escape sequence whose timeout elapsed.
    pub(crate) fn flush_stdin_timeout(&mut self) -> Vec<String> {
        self.stdin_deadline = None;
        let events = self
            .stdin_buffer
            .as_mut()
            .map(StdinBuffer::flush_timeout)
            .unwrap_or_default();
        self.dispatch_events(events)
    }

    /// Flush the negotiation fragment whose 150 ms window elapsed.
    pub(crate) fn flush_negotiation_timeout(&mut self) -> Vec<String> {
        self.keyboard_protocol_buffer_deadline = None;
        let mut forwards = Vec::new();
        self.flush_keyboard_protocol_negotiation_buffer_as_input(&mut forwards);
        forwards
    }

    /// Re-emit the progress sequence some terminals expire after a second.
    pub(crate) fn fire_progress_keepalive(&mut self, now: Instant) {
        self.progress_next_keepalive =
            Some(now + Duration::from_millis(TERMINAL_PROGRESS_KEEPALIVE_MS));
        self.write_out(TERMINAL_PROGRESS_ACTIVE_SEQUENCE);
    }

    /// Take the input handler out so it can run without a borrow on the
    /// terminal; the epoch guards the way back.
    pub(crate) fn take_input_handler(&mut self) -> Option<(u64, InputHandler)> {
        Some((self.pump_epoch, self.input_handler.take()?))
    }

    /// Put a handler taken by [`Self::take_input_handler`] back, unless it
    /// installed a new one or the terminal was restarted in the meantime.
    pub(crate) fn restore_input_handler(&mut self, epoch: u64, handler: InputHandler) {
        if epoch == self.pump_epoch && self.input_handler.is_none() {
            self.input_handler = Some(handler);
        }
    }

    pub(crate) fn take_resize_handler(&mut self) -> Option<(u64, ResizeHandler)> {
        Some((self.pump_epoch, self.resize_handler.take()?))
    }

    pub(crate) fn restore_resize_handler(&mut self, epoch: u64, handler: ResizeHandler) {
        if epoch == self.pump_epoch && self.resize_handler.is_none() {
            self.resize_handler = Some(handler);
        }
    }

    /// Lend the stdin channel and the SIGWINCH stream to the pump for one
    /// `await`, so no `RefCell` borrow is held across it.
    pub(crate) fn take_pump_lease(&mut self) -> Option<PumpLeaseParts> {
        let rx = self.stdin_rx.take()?;
        Some(PumpLeaseParts {
            epoch: self.pump_epoch,
            rx,
            #[cfg(unix)]
            resize_signal: self.resize_signal.take(),
        })
    }

    /// Take the lease back. A lease from before a `start()`/`stop()` is dropped:
    /// its channel belongs to a reader thread that is gone.
    pub(crate) fn restore_pump_lease(&mut self, parts: PumpLeaseParts) {
        if parts.epoch != self.pump_epoch {
            return;
        }
        self.stdin_rx = Some(parts.rx);
        #[cfg(unix)]
        {
            self.resize_signal = parts.resize_signal;
        }
    }

    /// Disable the protocols, park the input handler and hand out the stdin
    /// channel for [`drain_input`](Terminal::drain_input).
    pub(crate) fn begin_drain(&mut self) -> DrainLease {
        let should_disable_kitty_protocol =
            self.keyboard_protocol_pushed || self.kitty_protocol_active;
        self.clear_keyboard_protocol_negotiation_buffer();
        if should_disable_kitty_protocol {
            // Disable the Kitty keyboard protocol first so late key releases do
            // not generate new escape sequences.
            self.write_out("\x1b[<u");
            self.keyboard_protocol_pushed = false;
            self.kitty_protocol_active = false;
            set_kitty_protocol_active(false);
        }
        self.disable_modify_other_keys();
        DrainLease {
            epoch: self.pump_epoch,
            rx: self.stdin_rx.take(),
            input_handler: self.input_handler.take(),
        }
    }

    /// Put the channel and the input handler back after the drain.
    pub(crate) fn finish_drain(&mut self, lease: DrainLease) {
        if lease.epoch != self.pump_epoch {
            return;
        }
        self.stdin_rx = lease.rx;
        self.input_handler = lease.input_handler;
    }
}

/// What woke [`ProcessTerminalPump::pump`].
enum PumpWake {
    Chunk(Vec<u8>),
    Resize,
    Deadline,
    Eof,
}

enum NegotiationRead {
    Sequence(KeyboardProtocolNegotiationSequence),
    Pending,
    None,
}

fn resolve_write_log_path() -> Option<std::path::PathBuf> {
    let env = std::env::var("NOTAGENT_TUI_WRITE_LOG")
        .ok()
        .filter(|value| !value.is_empty())?;
    let path = std::path::PathBuf::from(&env);
    if path.is_dir() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        return Some(path.join(format!("tui-{now}-{}.log", std::process::id())));
    }
    Some(path)
}

/// Saved terminal state so `stop()` can restore it.
struct RawModeState {
    #[cfg(unix)]
    termios: libc::termios,
    #[cfg(not(unix))]
    _unused: (),
}

#[cfg(unix)]
fn enable_raw_mode() -> Option<RawModeState> {
    // Mirrors what Node's `setRawMode(true)` (libuv `uv__tty_make_raw`) does,
    // verified empirically against `stty -g` on macOS: clear
    // BRKINT|ICRNL|INPCK|ISTRIP|IXON, keep OPOST|ONLCR, CS8 without CSIZE/PARENB,
    // clear ECHO|ICANON|IEXTEN|ISIG, VMIN = 1, VTIME = 0.
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: `termios` is a valid, writable struct for the duration of the call.
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut termios) } != 0 {
        return None;
    }
    let saved = RawModeState { termios };

    let mut raw = termios;
    raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
    raw.c_oflag |= libc::ONLCR;
    raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
    raw.c_cflag |= libc::CS8;
    raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;
    // SAFETY: `raw` is a fully initialized termios struct.
    unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) };
    Some(saved)
}

#[cfg(unix)]
fn restore_raw_mode(state: &RawModeState) {
    // SAFETY: `state.termios` was read from the same descriptor.
    unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &state.termios) };
}

#[cfg(not(unix))]
fn enable_raw_mode() -> Option<RawModeState> {
    Some(RawModeState { _unused: () })
}

#[cfg(not(unix))]
fn restore_raw_mode(_state: &RawModeState) {}

/// Terminal size from the OS, or `None` when stdout is not a terminal.
#[cfg(unix)]
fn terminal_size() -> Option<(usize, usize)> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: `size` is a valid, writable winsize for the duration of the call.
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } != 0 {
        return None;
    }
    if size.ws_col == 0 || size.ws_row == 0 {
        return None;
    }
    Some((usize::from(size.ws_col), usize::from(size.ws_row)))
}

#[cfg(not(unix))]
fn terminal_size() -> Option<(usize, usize)> {
    None
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.trim().parse().ok()
}

/// Terminals answer a cursor position request within a few milliseconds; a
/// missing answer must not hold up the first frame for long.
const CURSOR_REPORT_TIMEOUT: Duration = Duration::from_millis(500);
/// How long after that timeout a reply is still expected, e.g. over SSH.
const LATE_CURSOR_REPORT_WINDOW: Duration = Duration::from_secs(3);

/// Locate a cursor position report (`ESC [ row ; col R`) and return its
/// 0-based row with the byte range it occupies.
pub(crate) fn find_cursor_report(bytes: &[u8]) -> Option<(usize, std::ops::Range<usize>)> {
    let mut start = 0;
    while let Some(offset) = bytes[start..].windows(2).position(|pair| pair == b"\x1b[") {
        let begin = start + offset;
        let body = &bytes[begin + 2..];
        let row_len = body.iter().take_while(|byte| byte.is_ascii_digit()).count();
        let after_row = &body[row_len..];
        if row_len > 0 && after_row.first() == Some(&b';') {
            let column_len = after_row[1..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            if column_len > 0 && after_row.get(1 + column_len) == Some(&b'R') {
                let row = std::str::from_utf8(&body[..row_len])
                    .ok()
                    .and_then(|row| row.parse::<usize>().ok())?;
                let end = begin + 2 + row_len + 1 + column_len + 1;
                return Some((row.saturating_sub(1), begin..end));
            }
        }
        start = begin + 1;
    }
    None
}

/// Read the reply to a `CSI 6 n` written just before. Bytes that arrive around
/// it are typed input and are returned so they still reach the input handler.
#[cfg(unix)]
fn read_cursor_report(timeout: Duration) -> (Option<usize>, Vec<u8>) {
    // SAFETY: `isatty` only inspects the descriptor.
    if unsafe { libc::isatty(libc::STDIN_FILENO) } != 1 {
        return (None, Vec::new());
    }
    let deadline = std::time::Instant::now() + timeout;
    let mut received = Vec::new();
    loop {
        if let Some((row, range)) = find_cursor_report(&received) {
            received.drain(range);
            return (Some(row), received);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return (None, received);
        }
        let mut descriptor = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = i32::try_from(remaining.as_millis())
            .unwrap_or(i32::MAX)
            .max(1);
        // SAFETY: `descriptor` is one valid pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut descriptor, 1, millis) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return (None, received);
        }
        if ready == 0 {
            return (None, received);
        }
        let mut chunk = [0u8; 256];
        // SAFETY: `chunk` is writable for its full length.
        let read =
            unsafe { libc::read(libc::STDIN_FILENO, chunk.as_mut_ptr().cast(), chunk.len()) };
        let Ok(read) = usize::try_from(read) else {
            return (None, received);
        };
        if read == 0 {
            return (None, received);
        }
        received.extend_from_slice(&chunk[..read]);
    }
}

#[cfg(not(unix))]
fn read_cursor_report(_timeout: Duration) -> (Option<usize>, Vec<u8>) {
    (None, Vec::new())
}

#[async_trait(?Send)]
impl Terminal for ProcessTerminal {
    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler) {
        self.input_handler = Some(on_input);
        self.resize_handler = Some(on_resize);

        // Save previous state and enable raw mode.
        self.saved_termios = enable_raw_mode();

        // Enable bracketed paste mode — the terminal wraps pastes in
        // \x1b[200~ … \x1b[201~.
        self.write_out("\x1b[?2004h\x1b[?1004h");

        #[cfg(unix)]
        {
            self.resize_signal =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).ok();
            // Refresh terminal dimensions — they may be stale after
            // suspend/resume, where SIGWINCH is lost while stopped.
            // SAFETY: sending SIGWINCH to our own process is always valid.
            unsafe { libc::kill(std::process::id() as libc::pid_t, libc::SIGWINCH) };
        }

        // On Windows enable ENABLE_VIRTUAL_TERMINAL_INPUT so the console sends
        // VT escape sequences (e.g. \x1b[Z for Shift+Tab) instead of raw console
        // events that lose modifier information. Must run AFTER raw mode, which
        // resets console mode flags.
        enable_windows_vt_input();

        // Only before the first reader thread exists: a reader from an earlier
        // start stays blocked in `read` after `stop()` and would take the reply.
        let mut early_input = Vec::new();
        self.start_cursor_row = None;
        if self.pump_epoch == 0 && matches!(self.output, OutputSink::Stdout) {
            self.write_out("\x1b[6n");
            let (row, pending) = read_cursor_report(CURSOR_REPORT_TIMEOUT);
            self.start_cursor_row = row;
            if row.is_none() {
                self.late_cursor_report_until = Some(Instant::now() + LATE_CURSOR_REPORT_WINDOW);
            }
            early_input = pending;
        }

        // Spawn the blocking stdin reader; the channel feeds `pump()`.
        let (tx, rx) = mpsc::unbounded_channel();
        if !early_input.is_empty() {
            let _ = tx.send(early_input);
        }
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut chunk = [0u8; 4096];
            loop {
                match stdin.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if tx.send(chunk[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        self.pump_epoch = self.pump_epoch.wrapping_add(1);
        self.stdin_rx = Some(rx);

        // Query the Kitty keyboard protocol and fall back to modifyOtherKeys when
        // DA confirms there is no Kitty response.
        self.query_and_enable_kitty_protocol();
    }

    fn stop(&mut self) {
        if self.progress_active {
            self.progress_active = false;
            self.progress_next_keepalive = None;
            self.write_out(TERMINAL_PROGRESS_CLEAR_SEQUENCE);
        }

        // Disable bracketed paste mode.
        self.write_out("\x1b[?1004l\x1b[?2004l");

        let should_disable_kitty_protocol =
            self.keyboard_protocol_pushed || self.kitty_protocol_active;
        self.clear_keyboard_protocol_negotiation_buffer();

        // Disable the Kitty keyboard protocol if drain_input() did not already.
        if should_disable_kitty_protocol {
            self.write_out("\x1b[<u");
            self.keyboard_protocol_pushed = false;
            self.kitty_protocol_active = false;
            set_kitty_protocol_active(false);
        }
        self.disable_modify_other_keys();

        if let Some(buffer) = self.stdin_buffer.as_mut() {
            buffer.destroy();
        }
        self.stdin_buffer = None;
        self.stdin_deadline = None;

        // Drop the receiver first so the reader thread stops before raw mode is
        // reset; this prevents buffered input (e.g. Ctrl+D) from being
        // re-interpreted by the parent shell.
        self.pump_epoch = self.pump_epoch.wrapping_add(1);
        self.stdin_rx = None;
        self.input_handler = None;
        self.resize_handler = None;
        #[cfg(unix)]
        {
            self.resize_signal = None;
        }

        // Restore raw mode state.
        if let Some(state) = self.saved_termios.take() {
            restore_raw_mode(&state);
        }
    }

    /// Drain stdin before exiting so Kitty key release events do not leak to the
    /// parent shell over slow SSH connections.
    async fn drain_input(&mut self, max_ms: Option<u64>, idle_ms: Option<u64>) {
        let mut lease = self.begin_drain();
        drain_stdin(&mut lease.rx, max_ms, idle_ms).await;
        self.finish_drain(lease);
    }

    fn write(&mut self, data: &str) {
        self.write_out(data);
    }

    fn columns(&self) -> usize {
        if let Some(columns) = self.columns_override {
            return columns;
        }
        terminal_size()
            .map(|(columns, _)| columns)
            .or_else(|| env_usize("COLUMNS"))
            .unwrap_or(80)
    }

    fn rows(&self) -> usize {
        if let Some(rows) = self.rows_override {
            return rows;
        }
        terminal_size()
            .map(|(_, rows)| rows)
            .or_else(|| env_usize("LINES"))
            .unwrap_or(24)
    }

    fn start_cursor_row(&self) -> Option<usize> {
        self.start_cursor_row
    }

    fn kitty_protocol_active(&self) -> bool {
        self.kitty_protocol_active
    }

    fn move_by(&mut self, lines: isize) {
        match lines.cmp(&0) {
            std::cmp::Ordering::Greater => self.write_out(&format!("\x1b[{lines}B")),
            std::cmp::Ordering::Less => self.write_out(&format!("\x1b[{}A", -lines)),
            std::cmp::Ordering::Equal => {}
        }
    }

    fn hide_cursor(&mut self) {
        self.write_out("\x1b[?25l");
    }

    fn show_cursor(&mut self) {
        self.write_out("\x1b[?25h");
    }

    fn clear_line(&mut self) {
        self.write_out("\x1b[K");
    }

    fn clear_from_cursor(&mut self) {
        self.write_out("\x1b[J");
    }

    fn clear_screen(&mut self) {
        self.write_out("\x1b[2J\x1b[H");
    }

    fn set_title(&mut self, title: &str) {
        self.write_out(&format!("\x1b]0;{title}\x07"));
    }

    fn set_progress(&mut self, active: bool) {
        if active {
            // OSC 9;4;3 — indeterminate progress, refreshed by pump().
            self.write_out(TERMINAL_PROGRESS_ACTIVE_SEQUENCE);
            self.progress_active = true;
            self.progress_next_keepalive =
                Some(Instant::now() + Duration::from_millis(TERMINAL_PROGRESS_KEEPALIVE_MS));
        } else {
            self.progress_active = false;
            self.progress_next_keepalive = None;
            // OSC 9;4;0 — clear progress.
            self.write_out(TERMINAL_PROGRESS_CLEAR_SEQUENCE);
        }
    }
}

/// On Windows add ENABLE_VIRTUAL_TERMINAL_INPUT (0x0200) to the stdin console
/// handle so the terminal sends VT sequences for modified keys (e.g. \x1b[Z for
#[cfg(windows)]
fn enable_windows_vt_input() {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
        SetConsoleMode,
    };
    // SAFETY: plain console API calls on the process' own stdin handle.
    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(handle, &mut mode) != 0 {
            SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_INPUT);
        }
    }
}

#[cfg(not(windows))]
fn enable_windows_vt_input() {}

/// A timeout that has come due.
pub(crate) enum DueTimeout {
    /// The `StdinBuffer` escape timeout.
    Stdin,
    /// The 150 ms keyboard-protocol fragment window.
    Negotiation,
    /// The OSC 9;4 progress keepalive.
    ProgressKeepalive,
}

/// Stdin channel and SIGWINCH stream on loan to the pump for one `await`.
pub(crate) struct PumpLeaseParts {
    epoch: u64,
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    #[cfg(unix)]
    resize_signal: Option<tokio::signal::unix::Signal>,
}

/// Stdin channel and input handler parked for the duration of a drain.
pub(crate) struct DrainLease {
    epoch: u64,
    rx: Option<mpsc::UnboundedReceiver<Vec<u8>>>,
    input_handler: Option<InputHandler>,
}

/// Swallow stdin until it stays idle for `idle_ms`, at most `max_ms`.
/// it lives outside `ProcessTerminal` so the shared handle can run it without
/// holding a borrow across the `await`.
async fn drain_stdin(
    rx: &mut Option<mpsc::UnboundedReceiver<Vec<u8>>>,
    max_ms: Option<u64>,
    idle_ms: Option<u64>,
) {
    let max_ms = max_ms.unwrap_or(1000);
    let idle_ms = idle_ms.unwrap_or(50);
    let end_time = Instant::now() + Duration::from_millis(max_ms);
    let Some(rx) = rx.as_mut() else {
        return;
    };
    loop {
        let now = Instant::now();
        if now >= end_time {
            break;
        }
        let idle = Duration::from_millis(idle_ms).min(end_time - now);
        match tokio::time::timeout(idle, rx.recv()).await {
            // Data arrived — keep draining.
            Ok(Some(_)) => {}
            // Channel closed or idle window elapsed.
            Ok(None) | Err(_) => break,
        }
    }
}

/// Apply the platform normalisation a sequence gets on its way to the handler.
fn normalize_forwarded_input(sequence: &str) -> String {
    let should_detect_native_shift_enter =
        sequence == "\r" && (is_apple_terminal_session() || cfg!(target_os = "windows"));
    normalize_native_shift_enter_input(
        sequence,
        should_detect_native_shift_enter,
        should_detect_native_shift_enter && is_native_modifier_pressed(ModifierKey::Shift),
    )
}

/// Drives a terminal's event loop in the caller's task.
/// SIGWINCH and the buffer timeouts to `ProcessTerminal` on its own
/// only has to hand the terminal to `TuiMainScreen` and call `start()`
/// loop, so the caller owns one; a pump is what it drives. Dispatch runs on the
/// preserved.
#[async_trait(?Send)]
pub trait TerminalPump {
    /// Wait for the next terminal event, dispatch it to the handlers installed
    /// by [`Terminal::start`] and report what happened.
    async fn pump(&mut self) -> PumpResult;
}

/// [`ProcessTerminal`] shared between the TUI, which owns it, and its pump.
/// Created by [`ProcessTerminal::into_shared`]. Every method takes the borrow
/// only for its own duration, so a handler running inside the pump can write to
pub struct SharedProcessTerminal(Rc<RefCell<ProcessTerminal>>);

impl SharedProcessTerminal {
    /// Another handle to the same terminal.
    pub fn clone_handle(&self) -> Self {
        Self(Rc::clone(&self.0))
    }

    /// Whether the modifyOtherKeys fallback is active.
    pub fn modify_other_keys_active(&self) -> bool {
        self.0.borrow().modify_other_keys_active()
    }
}

impl ProcessTerminal {
    /// Split the terminal into the handle the TUI owns and the pump that drives
    /// ```no_run
    /// # use notagent_tui::terminal::{ProcessTerminal, TerminalPump};
    /// # use notagent_tui::tui_main_screen::TuiMainScreen;
    /// # async fn run() {
    /// let (terminal, mut pump) = ProcessTerminal::new().into_shared();
    /// let mut ui = TuiMainScreen::new(Box::new(terminal));
    /// ui.start();
    /// loop {
    ///     tokio::select! {
    ///         () = ui.core().wait_until_render_due() => ui.render_pending_frame(),
    ///         _ = pump.pump() => {}
    ///     }
    /// }
    /// # }
    /// ```
    pub fn into_shared(self) -> (SharedProcessTerminal, ProcessTerminalPump) {
        let terminal = Rc::new(RefCell::new(self));
        (
            SharedProcessTerminal(Rc::clone(&terminal)),
            ProcessTerminalPump { terminal },
        )
    }
}

#[async_trait(?Send)]
impl Terminal for SharedProcessTerminal {
    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler) {
        self.0.borrow_mut().start(on_input, on_resize);
    }

    fn stop(&mut self) {
        self.0.borrow_mut().stop();
    }

    async fn drain_input(&mut self, max_ms: Option<u64>, idle_ms: Option<u64>) {
        let mut lease = self.0.borrow_mut().begin_drain();
        drain_stdin(&mut lease.rx, max_ms, idle_ms).await;
        self.0.borrow_mut().finish_drain(lease);
    }

    fn write(&mut self, data: &str) {
        self.0.borrow_mut().write(data);
    }

    fn columns(&self) -> usize {
        self.0.borrow().columns()
    }

    fn rows(&self) -> usize {
        self.0.borrow().rows()
    }

    fn start_cursor_row(&self) -> Option<usize> {
        self.0.borrow().start_cursor_row()
    }

    fn kitty_protocol_active(&self) -> bool {
        Terminal::kitty_protocol_active(&*self.0.borrow())
    }

    fn move_by(&mut self, lines: isize) {
        self.0.borrow_mut().move_by(lines);
    }

    fn hide_cursor(&mut self) {
        self.0.borrow_mut().hide_cursor();
    }

    fn show_cursor(&mut self) {
        self.0.borrow_mut().show_cursor();
    }

    fn clear_line(&mut self) {
        self.0.borrow_mut().clear_line();
    }

    fn clear_from_cursor(&mut self) {
        self.0.borrow_mut().clear_from_cursor();
    }

    fn clear_screen(&mut self) {
        self.0.borrow_mut().clear_screen();
    }

    fn set_title(&mut self, title: &str) {
        self.0.borrow_mut().set_title(title);
    }

    fn set_progress(&mut self, active: bool) {
        self.0.borrow_mut().set_progress(active);
    }
}

/// Pump of a [`SharedProcessTerminal`].
/// One `pump()` call waits for the next stdin chunk, SIGWINCH or pending
/// timeout and dispatches it. The terminal is never borrowed across an `await`:
/// the stdin channel and the SIGWINCH stream are lent out for the wait and the
/// handlers run after the borrow is released. Dropping a `pump()` future
/// (`tokio::select!` cancellation) loses nothing — the lease returns in `Drop`.
pub struct ProcessTerminalPump {
    terminal: Rc<RefCell<ProcessTerminal>>,
}

impl ProcessTerminalPump {
    fn forward_input(&self, forwards: Vec<String>) {
        if forwards.is_empty() {
            return;
        }
        let Some((epoch, mut handler)) = self.terminal.borrow_mut().take_input_handler() else {
            return;
        };
        for sequence in &forwards {
            handler(sequence);
        }
        self.terminal
            .borrow_mut()
            .restore_input_handler(epoch, handler);
    }

    fn dispatch_resize(&self) {
        let Some((epoch, mut handler)) = self.terminal.borrow_mut().take_resize_handler() else {
            return;
        };
        handler();
        self.terminal
            .borrow_mut()
            .restore_resize_handler(epoch, handler);
    }
}

#[async_trait(?Send)]
impl TerminalPump for ProcessTerminalPump {
    async fn pump(&mut self) -> PumpResult {
        loop {
            let now = Instant::now();
            let due = self.terminal.borrow().due_timeout(now);
            match due {
                Some(DueTimeout::Stdin) => {
                    let forwards = self.terminal.borrow_mut().flush_stdin_timeout();
                    self.forward_input(forwards);
                    return PumpResult::Timeout;
                }
                Some(DueTimeout::Negotiation) => {
                    let forwards = self.terminal.borrow_mut().flush_negotiation_timeout();
                    self.forward_input(forwards);
                    return PumpResult::Timeout;
                }
                Some(DueTimeout::ProgressKeepalive) => {
                    self.terminal.borrow_mut().fire_progress_keepalive(now);
                    continue;
                }
                None => {}
            }

            let next_deadline = self.terminal.borrow().next_timeout();
            let Some(mut lease) = PumpLease::take(&self.terminal) else {
                return PumpResult::Eof;
            };
            let wake = lease.wait(next_deadline).await;
            drop(lease);

            match wake {
                PumpWake::Chunk(chunk) => {
                    let forwards = self.terminal.borrow_mut().handle_stdin_chunk(&chunk);
                    self.forward_input(forwards);
                    return PumpResult::Input;
                }
                PumpWake::Deadline => continue,
                PumpWake::Resize => {
                    self.dispatch_resize();
                    return PumpResult::Resize;
                }
                PumpWake::Eof => return PumpResult::Eof,
            }
        }
    }
}

/// The stdin channel and SIGWINCH stream while the pump waits on them.
struct PumpLease<'a> {
    terminal: &'a Rc<RefCell<ProcessTerminal>>,
    parts: Option<PumpLeaseParts>,
}

impl<'a> PumpLease<'a> {
    fn take(terminal: &'a Rc<RefCell<ProcessTerminal>>) -> Option<Self> {
        let parts = terminal.borrow_mut().take_pump_lease()?;
        Some(Self {
            terminal,
            parts: Some(parts),
        })
    }

    async fn wait(&mut self, next_deadline: Option<Instant>) -> PumpWake {
        let parts = self
            .parts
            .as_mut()
            .expect("the lease is held for the whole wait");
        #[cfg(unix)]
        {
            let rx = &mut parts.rx;
            let resize = parts.resize_signal.as_mut();
            tokio::select! {
                chunk = rx.recv() => match chunk {
                    Some(chunk) => PumpWake::Chunk(chunk),
                    None => PumpWake::Eof,
                },
                Some(()) = async {
                    match resize {
                        Some(signal) => signal.recv().await,
                        None => std::future::pending().await,
                    }
                } => PumpWake::Resize,
                () = async {
                    match next_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => PumpWake::Deadline,
            }
        }
        #[cfg(not(unix))]
        {
            let rx = &mut parts.rx;
            tokio::select! {
                chunk = rx.recv() => match chunk {
                    Some(chunk) => PumpWake::Chunk(chunk),
                    None => PumpWake::Eof,
                },
                () = async {
                    match next_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => PumpWake::Deadline,
            }
        }
    }
}

impl Drop for PumpLease<'_> {
    /// Hand the lease back — also when the `pump()` future is cancelled.
    fn drop(&mut self) {
        if let Some(parts) = self.parts.take() {
            self.terminal.borrow_mut().restore_pump_lease(parts);
        }
    }
}

#[cfg(feature = "test-terminal")]
impl ProcessTerminal {
    /// Terminal whose writes go to `sink` instead of stdout.
    pub fn with_writer(sink: Box<dyn FnMut(&str)>) -> Self {
        let mut terminal = Self::new();
        terminal.output = OutputSink::Collector(sink);
        terminal.write_log_path = None;
        terminal
    }

    pub fn set_input_handler(&mut self, handler: InputHandler) {
        self.input_handler = Some(handler);
    }

    /// Run the protocol negotiation without touching stdin
    pub fn begin_keyboard_protocol_negotiation(&mut self) {
        self.query_and_enable_kitty_protocol();
    }

    pub fn feed_stdin(&mut self, data: &str) {
        let forwards = self.handle_stdin_chunk(data.as_bytes());
        self.forward_to_input_handler(forwards);
    }

    pub fn fire_stdin_timeout(&mut self) {
        let forwards = self.flush_stdin_timeout();
        self.forward_to_input_handler(forwards);
    }

    /// Fire the 150 ms negotiation fragment timeout.
    pub fn fire_negotiation_fragment_timeout(&mut self) {
        let forwards = self.flush_negotiation_timeout();
        self.forward_to_input_handler(forwards);
    }

    pub fn set_dimensions_for_tests(&mut self, columns: Option<usize>, rows: Option<usize>) {
        self.columns_override = columns;
        self.rows_override = rows;
    }
}

/// Test-only access to the shared handle and its pump.
#[cfg(feature = "test-terminal")]
impl SharedProcessTerminal {
    /// Install the input handler without going through `start()`, which would
    /// put the real stdin into raw mode.
    pub fn set_input_handler(&self, handler: InputHandler) {
        self.0.borrow_mut().set_input_handler(handler);
    }

    /// Run the protocol negotiation without touching stdin.
    pub fn begin_keyboard_protocol_negotiation(&self) {
        self.0.borrow_mut().begin_keyboard_protocol_negotiation();
    }

    /// Attach a stdin channel the test feeds instead of the reader thread.
    pub fn attach_test_stdin(&self) -> mpsc::UnboundedSender<Vec<u8>> {
        self.0.borrow_mut().attach_test_stdin()
    }
}

#[cfg(feature = "test-terminal")]
impl ProcessTerminal {
    /// Attach a stdin channel the test feeds instead of the reader thread
    pub fn attach_test_stdin(&mut self) -> mpsc::UnboundedSender<Vec<u8>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.pump_epoch = self.pump_epoch.wrapping_add(1);
        self.stdin_rx = Some(rx);
        tx
    }
}

#[cfg(feature = "test-terminal")]
impl ProcessTerminalPump {
    /// Dispatch a chunk exactly like [`TerminalPump::pump`] does, without stdin.
    pub fn feed_stdin(&mut self, data: &str) -> PumpResult {
        let forwards = self
            .terminal
            .borrow_mut()
            .handle_stdin_chunk(data.as_bytes());
        self.forward_input(forwards);
        PumpResult::Input
    }
}

#[cfg(test)]
mod tests {
    use super::{Duration, Instant, ProcessTerminal, find_cursor_report};

    #[test]
    fn one_late_cursor_report_is_swallowed_and_later_keys_pass() {
        let mut terminal = ProcessTerminal::new();
        assert!(
            !terminal.consume_late_cursor_report("\x1b[1;5R"),
            "without a timed-out query the sequence is a key"
        );
        terminal.late_cursor_report_until = Some(Instant::now() + Duration::from_secs(3));
        assert!(!terminal.consume_late_cursor_report("a"));
        assert!(terminal.consume_late_cursor_report("\x1b[7;1R"));
        assert!(
            !terminal.consume_late_cursor_report("\x1b[1;5R"),
            "only the one expected reply is swallowed"
        );
    }

    #[test]
    fn a_cursor_report_is_found_between_typed_input() {
        let bytes = b"ab\x1b[A\x1b[12;40Rcd";
        let (row, range) = find_cursor_report(bytes).expect("report present");
        assert_eq!(row, 11, "rows are reported 1-based");
        assert_eq!(&bytes[range], b"\x1b[12;40R");
    }

    #[test]
    fn an_incomplete_or_foreign_sequence_is_not_a_cursor_report() {
        assert_eq!(find_cursor_report(b"\x1b[12;40"), None);
        assert_eq!(find_cursor_report(b"\x1b[1;5A"), None);
        assert_eq!(find_cursor_report(b"\x1b[;5R"), None);
    }
}
