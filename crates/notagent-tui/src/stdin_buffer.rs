use std::sync::LazyLock;

use regex::Regex;

const ESC: &str = "\x1b";
const DEFAULT_SEQUENCE_TIMEOUT_MS: u64 = 50;
const DEFAULT_ESCAPE_TIMEOUT_MS: u64 = 10;
const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";

/// What the buffer produces while processing input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdinEvent {
    Data(String),
    Paste(String),
}

/// Whether a candidate string is a complete escape sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequenceStatus {
    Complete,
    Incomplete,
    NotEscape,
}

fn is_complete_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with(ESC) {
        return SequenceStatus::NotEscape;
    }

    if data.chars().count() == 1 {
        return SequenceStatus::Incomplete;
    }

    let after_esc = &data[ESC.len()..];

    // CSI sequences: ESC [
    if after_esc.starts_with('[') {
        // Old-style mouse sequence: ESC [ M + 3 bytes = 6 characters total.
        if after_esc.starts_with("[M") {
            return if data.chars().count() >= 6 {
                SequenceStatus::Complete
            } else {
                SequenceStatus::Incomplete
            };
        }
        return is_complete_csi_sequence(data);
    }

    // OSC sequences: ESC ]
    if after_esc.starts_with(']') {
        return is_complete_osc_sequence(data);
    }

    // DCS sequences: ESC P ... ESC \ (includes XTVersion responses)
    if after_esc.starts_with('P') {
        return is_complete_dcs_sequence(data);
    }

    // APC sequences: ESC _ ... ESC \ (includes Kitty graphics responses)
    if after_esc.starts_with('_') {
        return is_complete_apc_sequence(data);
    }

    // SS3 sequences: ESC O followed by a single character
    if after_esc.starts_with('O') {
        return if after_esc.chars().count() >= 2 {
            SequenceStatus::Complete
        } else {
            SequenceStatus::Incomplete
        };
    }

    // Meta key sequences: ESC followed by a single character
    if after_esc.chars().count() == 1 {
        return SequenceStatus::Complete;
    }

    // Unknown escape sequence - treat as complete
    SequenceStatus::Complete
}

static SGR_MOUSE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<\d+;\d+;\d+[Mm]$").expect("valid regex"));
static DIGITS_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+$").expect("valid regex"));

/// CSI sequences end with a byte in the range 0x40-0x7E (`@`-`~`).
fn is_complete_csi_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1b[") {
        return SequenceStatus::Complete;
    }

    if data.chars().count() < 3 {
        return SequenceStatus::Incomplete;
    }

    let payload = &data["\x1b[".len()..];
    let last_char = payload.chars().next_back().expect("payload is non-empty");
    let last_char_code = u32::from(last_char);

    if (0x40..=0x7e).contains(&last_char_code) {
        // SGR mouse sequences: ESC[<B;X;Ym or ESC[<B;X;YM
        if payload.starts_with('<') {
            if SGR_MOUSE_REGEX.is_match(payload) {
                return SequenceStatus::Complete;
            }
            if last_char == 'M' || last_char == 'm' {
                let inner = &payload[1..payload.len() - last_char.len_utf8()];
                let parts: Vec<&str> = inner.split(';').collect();
                if parts.len() == 3 && parts.iter().all(|p| DIGITS_REGEX.is_match(p)) {
                    return SequenceStatus::Complete;
                }
            }
            return SequenceStatus::Incomplete;
        }

        return SequenceStatus::Complete;
    }

    SequenceStatus::Incomplete
}

/// OSC sequences end with ST (ESC \\) or BEL.
fn is_complete_osc_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1b]") {
        return SequenceStatus::Complete;
    }
    if data.ends_with("\x1b\\") || data.ends_with('\x07') {
        return SequenceStatus::Complete;
    }
    SequenceStatus::Incomplete
}

/// DCS sequences end with ST (ESC \\).
fn is_complete_dcs_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1bP") {
        return SequenceStatus::Complete;
    }
    if data.ends_with("\x1b\\") {
        return SequenceStatus::Complete;
    }
    SequenceStatus::Incomplete
}

/// APC sequences end with ST (ESC \\).
fn is_complete_apc_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1b_") {
        return SequenceStatus::Complete;
    }
    if data.ends_with("\x1b\\") {
        return SequenceStatus::Complete;
    }
    SequenceStatus::Incomplete
}

static UNMODIFIED_KITTY_PRINTABLE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\x1b\[(\d+)(?::\d*)?(?::\d+)?u$").expect("valid regex"));

fn parse_unmodified_kitty_printable_codepoint(sequence: &str) -> Option<u32> {
    let captures = UNMODIFIED_KITTY_PRINTABLE_REGEX.captures(sequence)?;
    let codepoint: u32 = captures.get(1)?.as_str().parse().ok()?;
    if codepoint >= 32 {
        Some(codepoint)
    } else {
        None
    }
}

struct ExtractedSequences {
    sequences: Vec<String>,
    remainder: String,
}

/// Split an accumulated buffer into complete sequences.
fn extract_complete_sequences(buffer: &str) -> ExtractedSequences {
    let mut sequences: Vec<String> = Vec::new();
    let mut pos = 0;

    while pos < buffer.len() {
        let remaining = &buffer[pos..];

        if remaining.starts_with(ESC) {
            // Find the end of this escape sequence, growing character by
            // ASCII, so the scan is equivalent).
            let mut seq_end = ESC.len();
            let mut consumed = false;
            loop {
                if seq_end > remaining.len() {
                    break;
                }
                let candidate = &remaining[..seq_end];
                match is_complete_sequence(candidate) {
                    SequenceStatus::Complete => {
                        // WezTerm with enable_kitty_keyboard sends the Escape key
                        // press as a raw '\x1b' byte and the release as a full
                        // Kitty CSI-u sequence, arriving as '\x1b\x1b[27;...u'.
                        // '\x1b\x1b' would look like a complete meta-key sequence
                        // and leave '[27;...u' to be typed as plain text. If the
                        // character after '\x1b\x1b' would start a new escape
                        // sequence, emit only the first ESC and restart there.
                        if candidate == "\x1b\x1b" {
                            let next_char = remaining[seq_end..].chars().next();
                            if matches!(next_char, Some('[' | ']' | 'O' | 'P' | '_')) {
                                sequences.push(ESC.to_string());
                                pos += ESC.len();
                                consumed = true;
                                break;
                            }
                        }
                        sequences.push(candidate.to_string());
                        pos += seq_end;
                        consumed = true;
                        break;
                    }
                    SequenceStatus::Incomplete => {
                        seq_end += next_char_len(remaining, seq_end);
                    }
                    SequenceStatus::NotEscape => {
                        // Should not happen when starting with ESC.
                        sequences.push(candidate.to_string());
                        pos += seq_end;
                        consumed = true;
                        break;
                    }
                }
            }

            if !consumed {
                return ExtractedSequences {
                    sequences,
                    remainder: remaining.to_string(),
                };
            }
        } else {
            // Not an escape sequence - take a single character.
            let char_len = next_char_len(remaining, 0);
            sequences.push(remaining[..char_len].to_string());
            pos += char_len;
        }
    }

    ExtractedSequences {
        sequences,
        remainder: String::new(),
    }
}

/// Length in bytes of the character at `pos`, or 1 past the end of the string.
fn next_char_len(text: &str, pos: usize) -> usize {
    text.get(pos..)
        .and_then(|rest| rest.chars().next())
        .map_or(1, char::len_utf8)
}

/// Options for [`StdinBuffer`].
#[derive(Debug, Clone, Copy, Default)]
pub struct StdinBufferOptions {
    /// Maximum time to wait for an incomplete sequence such as CSI or mouse
    /// (default: 50 ms).
    pub timeout: Option<u64>,
    /// Maximum time to wait after a lone ESC before treating it as Escape
    /// (default: 10 ms). Increase for high-latency Alt+key input (SSH).
    pub escape_timeout: Option<u64>,
}

/// Buffers stdin input and produces complete sequences.
pub struct StdinBuffer {
    buffer: String,
    timeout_ms: u64,
    escape_timeout_ms: u64,
    paste_mode: bool,
    paste_buffer: String,
    pending_kitty_printable_codepoint: Option<u32>,
}

impl StdinBuffer {
    /// New buffer with the given options.
    pub fn new(options: StdinBufferOptions) -> Self {
        Self {
            buffer: String::new(),
            timeout_ms: options.timeout.unwrap_or(DEFAULT_SEQUENCE_TIMEOUT_MS),
            escape_timeout_ms: options.escape_timeout.unwrap_or(DEFAULT_ESCAPE_TIMEOUT_MS),
            paste_mode: false,
            paste_buffer: String::new(),
            pending_kitty_printable_codepoint: None,
        }
    }

    /// single byte > 127 becomes ESC + (byte - 128).
    pub fn process_bytes(&mut self, data: &[u8]) -> Vec<StdinEvent> {
        if data.len() == 1 && data[0] > 127 {
            let byte = char::from(data[0] - 128);
            return self.process(&format!("\x1b{byte}"));
        }
        self.process(&String::from_utf8_lossy(data))
    }

    /// Feed decoded input and return the resulting events in order.
    pub fn process(&mut self, data: &str) -> Vec<StdinEvent> {
        let mut events = Vec::new();
        self.process_into(data, &mut events);
        events
    }

    fn process_into(&mut self, data: &str, events: &mut Vec<StdinEvent>) {
        if data.is_empty() && self.buffer.is_empty() {
            self.emit_data_sequence("", events);
            return;
        }

        self.buffer.push_str(data);

        if self.paste_mode {
            let buffered = std::mem::take(&mut self.buffer);
            self.paste_buffer.push_str(&buffered);
            self.finish_paste_if_complete(events);
            return;
        }

        if let Some(start_index) = self.buffer.find(BRACKETED_PASTE_START) {
            if start_index > 0 {
                let before_paste = self.buffer[..start_index].to_string();
                let result = extract_complete_sequences(&before_paste);
                for sequence in result.sequences {
                    self.emit_data_sequence(&sequence, events);
                }
            }

            self.pending_kitty_printable_codepoint = None;
            self.buffer = self.buffer[start_index + BRACKETED_PASTE_START.len()..].to_string();
            self.paste_mode = true;
            self.paste_buffer = std::mem::take(&mut self.buffer);

            self.finish_paste_if_complete(events);
            return;
        }

        let result = extract_complete_sequences(&self.buffer);
        self.buffer = result.remainder;

        for sequence in result.sequences {
            self.emit_data_sequence(&sequence, events);
        }
    }

    fn finish_paste_if_complete(&mut self, events: &mut Vec<StdinEvent>) {
        let Some(end_index) = self.paste_buffer.find(BRACKETED_PASTE_END) else {
            return;
        };
        let pasted_content = self.paste_buffer[..end_index].to_string();
        let remaining = self.paste_buffer[end_index + BRACKETED_PASTE_END.len()..].to_string();

        self.paste_mode = false;
        self.paste_buffer = String::new();
        self.pending_kitty_printable_codepoint = None;

        events.push(StdinEvent::Paste(pasted_content));

        if !remaining.is_empty() {
            self.process_into(&remaining, events);
        }
    }

    fn emit_data_sequence(&mut self, sequence: &str, events: &mut Vec<StdinEvent>) {
        let raw_codepoint = if sequence.encode_utf16().count() == 1 {
            sequence.chars().next().map(u32::from)
        } else {
            None
        };
        if raw_codepoint.is_some() && raw_codepoint == self.pending_kitty_printable_codepoint {
            self.pending_kitty_printable_codepoint = None;
            return;
        }

        self.pending_kitty_printable_codepoint =
            parse_unmodified_kitty_printable_codepoint(sequence);
        events.push(StdinEvent::Data(sequence.to_string()));
    }

    /// How long to wait before flushing an incomplete sequence, if any.
    /// this duration and calls [`Self::flush_timeout`] when it fires. A new
    /// [`Self::process`] call cancels the timer.
    pub fn pending_timeout_ms(&self) -> Option<u64> {
        if self.buffer.is_empty() {
            return None;
        }
        Some(if self.buffer == ESC {
            self.escape_timeout_ms
        } else {
            self.timeout_ms
        })
    }

    pub fn flush_timeout(&mut self) -> Vec<StdinEvent> {
        let mut events = Vec::new();
        for sequence in self.flush() {
            self.emit_data_sequence(&sequence, &mut events);
        }
        events
    }

    /// Flush the buffered remainder without emitting.
    pub fn flush(&mut self) -> Vec<String> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let sequences = vec![std::mem::take(&mut self.buffer)];
        self.pending_kitty_printable_codepoint = None;
        sequences
    }

    /// Drop all buffered state.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.paste_mode = false;
        self.paste_buffer.clear();
        self.pending_kitty_printable_codepoint = None;
    }

    /// The currently buffered, not yet complete input.
    pub fn get_buffer(&self) -> &str {
        &self.buffer
    }

    pub fn destroy(&mut self) {
        self.clear();
    }
}
