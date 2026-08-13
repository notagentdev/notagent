//! Terminal-Abstraktion.
//!
//! Port von `packages/tui/src/terminal.ts`. Dieser Commit enthält den Kontrakt
//! (`Terminal`-Trait, `packages/tui/src/terminal.ts:60-102`); `ProcessTerminal`
//! samt Raw-Mode, Kitty-Negotiation und Bracketed Paste folgt in Task 4 des
//! Workstream-A-Plans.

use std::io::{Read, Write};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use async_trait::async_trait;

use crate::keys::set_kitty_protocol_active;
use crate::native_modifiers::{ModifierKey, is_native_modifier_pressed};
use crate::stdin_buffer::{StdinBuffer, StdinBufferOptions, StdinEvent};

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
///
/// Legacy Alt+key input is ESC plus another byte, so high-latency transports
/// need a longer reassembly window.
pub fn resolve_escape_timeout_ms() -> u64 {
    resolve_escape_timeout_ms_from(&|name| std::env::var(name).ok())
}

/// [`resolve_escape_timeout_ms`] with an injectable environment (for tests).
pub fn resolve_escape_timeout_ms_from(env: &dyn Fn(&str) -> Option<String>) -> u64 {
    // TS: `Number(env.NOTAGENT_TUI_ESC_TIMEOUT)`; empty, NaN and values <= 0 fall through.
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
///
/// The TS tests monkey-patch `process.stdout.write`; the Rust port injects the
/// sink instead (deviation class 1).
enum OutputSink {
    Stdout,
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
///
/// Port of `class ProcessTerminal` (`packages/tui/src/terminal.ts:123-559`).
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
    ///
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
    fn handle_stdin_chunk(&mut self, chunk: &[u8]) {
        let Some(buffer) = self.stdin_buffer.as_mut() else {
            return;
        };
        let events = buffer.process_bytes(chunk);
        self.stdin_deadline = self
            .stdin_buffer
            .as_ref()
            .and_then(StdinBuffer::pending_timeout_ms)
            .map(|ms| Instant::now() + Duration::from_millis(ms));
        self.dispatch_events(events);
    }

    fn dispatch_events(&mut self, events: Vec<StdinEvent>) {
        for event in events {
            match event {
                StdinEvent::Data(sequence) => {
                    match self.read_keyboard_protocol_negotiation_sequence(&sequence) {
                        NegotiationRead::Pending => {
                            self.schedule_keyboard_protocol_negotiation_buffer_flush();
                        }
                        NegotiationRead::Sequence(negotiation) => {
                            self.handle_keyboard_protocol_negotiation_sequence(negotiation);
                        }
                        NegotiationRead::None => self.forward_input_sequence(&sequence),
                    }
                }
                StdinEvent::Paste(content) => {
                    // Re-wrap paste content with bracketed paste markers for the
                    // existing editor handling.
                    if let Some(handler) = self.input_handler.as_mut() {
                        handler(&format!("\x1b[200~{content}\x1b[201~"));
                    }
                }
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

    fn read_keyboard_protocol_negotiation_sequence(&mut self, sequence: &str) -> NegotiationRead {
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
            self.flush_keyboard_protocol_negotiation_buffer_as_input();
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

    fn flush_keyboard_protocol_negotiation_buffer_as_input(&mut self) {
        if self.keyboard_protocol_negotiation_buffer.is_empty() {
            return;
        }
        let sequence = std::mem::take(&mut self.keyboard_protocol_negotiation_buffer);
        self.clear_keyboard_protocol_negotiation_buffer();
        self.forward_input_sequence(&sequence);
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

    fn forward_input_sequence(&mut self, sequence: &str) {
        let should_detect_native_shift_enter =
            sequence == "\r" && (is_apple_terminal_session() || cfg!(target_os = "windows"));
        let input = normalize_native_shift_enter_input(
            sequence,
            should_detect_native_shift_enter,
            should_detect_native_shift_enter && is_native_modifier_pressed(ModifierKey::Shift),
        );
        if let Some(handler) = self.input_handler.as_mut() {
            handler(&input);
        }
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

    /// Wait for the next stdin chunk, resize or pending timeout and dispatch it.
    ///
    /// Replaces the Node event loop: handlers run on the caller's thread, so the
    /// single-threaded component model of the TS version is preserved. The
    /// blocking stdin read lives on its own thread and feeds a channel.
    pub async fn pump(&mut self) -> PumpResult {
        loop {
            let now = Instant::now();
            if let Some(deadline) = self.stdin_deadline
                && deadline <= now
            {
                self.stdin_deadline = None;
                let events = self
                    .stdin_buffer
                    .as_mut()
                    .map(StdinBuffer::flush_timeout)
                    .unwrap_or_default();
                self.dispatch_events(events);
                return PumpResult::Timeout;
            }
            if let Some(deadline) = self.keyboard_protocol_buffer_deadline
                && deadline <= now
            {
                self.keyboard_protocol_buffer_deadline = None;
                self.flush_keyboard_protocol_negotiation_buffer_as_input();
                return PumpResult::Timeout;
            }
            if let Some(deadline) = self.progress_next_keepalive
                && deadline <= now
            {
                self.progress_next_keepalive =
                    Some(now + Duration::from_millis(TERMINAL_PROGRESS_KEEPALIVE_MS));
                self.write_out(TERMINAL_PROGRESS_ACTIVE_SEQUENCE);
                continue;
            }

            let next_deadline = [
                self.stdin_deadline,
                self.keyboard_protocol_buffer_deadline,
                self.progress_next_keepalive,
            ]
            .into_iter()
            .flatten()
            .min();

            let Some(rx) = self.stdin_rx.as_mut() else {
                return PumpResult::Eof;
            };

            #[cfg(unix)]
            let wake = {
                let resize = self.resize_signal.as_mut();
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
            };
            #[cfg(not(unix))]
            let wake = tokio::select! {
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
            };

            match wake {
                PumpWake::Chunk(chunk) => {
                    self.handle_stdin_chunk(&chunk);
                    return PumpResult::Input;
                }
                PumpWake::Deadline => continue,
                PumpWake::Resize => {
                    if let Some(handler) = self.resize_handler.as_mut() {
                        handler();
                    }
                    return PumpResult::Resize;
                }
                PumpWake::Eof => return PumpResult::Eof,
            }
        }
    }
}

/// What woke [`ProcessTerminal::pump`].
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
        // A directory gets a timestamped file, like the TS version.
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

#[async_trait(?Send)]
impl Terminal for ProcessTerminal {
    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler) {
        self.input_handler = Some(on_input);
        self.resize_handler = Some(on_resize);

        // Save previous state and enable raw mode.
        self.saved_termios = enable_raw_mode();

        // Enable bracketed paste mode — the terminal wraps pastes in
        // \x1b[200~ … \x1b[201~.
        self.write_out("\x1b[?2004h");

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

        // Spawn the blocking stdin reader; the channel feeds `pump()`.
        let (tx, rx) = mpsc::unbounded_channel();
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
        self.write_out("\x1b[?2004l");

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
        let max_ms = max_ms.unwrap_or(1000);
        let idle_ms = idle_ms.unwrap_or(50);

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

        let previous_handler = self.input_handler.take();

        let end_time = Instant::now() + Duration::from_millis(max_ms);
        if let Some(rx) = self.stdin_rx.as_mut() {
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

        self.input_handler = previous_handler;
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
/// Shift+Tab). Port of `native/win32/src/win32-console-mode.c`.
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

/// Test-only access to the parts the TS suite drives through monkey-patching.
#[cfg(feature = "test-terminal")]
impl ProcessTerminal {
    /// Terminal whose writes go to `sink` instead of stdout.
    pub fn with_writer(sink: Box<dyn FnMut(&str)>) -> Self {
        let mut terminal = Self::new();
        terminal.output = OutputSink::Collector(sink);
        terminal.write_log_path = None;
        terminal
    }

    /// Install the input handler (TS: private `inputHandler` field).
    pub fn set_input_handler(&mut self, handler: InputHandler) {
        self.input_handler = Some(handler);
    }

    /// Run the protocol negotiation without touching stdin
    /// (TS: private `queryAndEnableKittyProtocol()`).
    pub fn begin_keyboard_protocol_negotiation(&mut self) {
        self.query_and_enable_kitty_protocol();
    }

    /// Feed data as if it came from stdin (TS: the captured `data` handler).
    pub fn feed_stdin(&mut self, data: &str) {
        self.handle_stdin_chunk(data.as_bytes());
    }

    /// Fire the pending StdinBuffer timeout (TS: `mock.timers.tick`).
    pub fn fire_stdin_timeout(&mut self) {
        self.stdin_deadline = None;
        let events = self
            .stdin_buffer
            .as_mut()
            .map(StdinBuffer::flush_timeout)
            .unwrap_or_default();
        self.dispatch_events(events);
    }

    /// Fire the 150 ms negotiation fragment timeout.
    pub fn fire_negotiation_fragment_timeout(&mut self) {
        self.keyboard_protocol_buffer_deadline = None;
        self.flush_keyboard_protocol_negotiation_buffer_as_input();
    }

    /// Pin the reported dimensions (TS test overrides `process.stdout.columns`).
    pub fn set_dimensions_for_tests(&mut self, columns: Option<usize>, rows: Option<usize>) {
        self.columns_override = columns;
        self.rows_override = rows;
    }
}
