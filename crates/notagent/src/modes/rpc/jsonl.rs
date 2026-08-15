//! Port of `packages/coding-agent/src/modes/rpc/jsonl.ts`.
//!
//! Strict JSONL framing: records are separated by LF and by nothing else.
//! U+2028 and U+2029 are legal inside a JSON string, and a reader that treats
//! them as line breaks would split a record in half — which is why this does not
//! use a general-purpose line reader.
//!
//! Deviation (class 3): TypeScript attaches to a Node stream and returns a
//! detach function. Rust has no such stream, so the splitter is a value the
//! caller feeds chunks to; the caller owns the read loop and therefore its own
//! detaching. The decoding of a UTF-8 sequence split across two chunks is the
//! job Node's `StringDecoder` did.

use serde_json::Value;

/// One record, LF-terminated.
pub fn serialize_json_line(value: &Value) -> String {
    format!("{value}\n")
}

/// Splits a byte stream into JSONL records.
#[derive(Default)]
pub struct JsonlLineSplitter {
    /// Bytes of a UTF-8 sequence that the last chunk cut in half.
    pending_bytes: Vec<u8>,
    buffer: String,
}

impl JsonlLineSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one chunk, calling `on_line` for every completed record.
    pub fn push(&mut self, chunk: &[u8], mut on_line: impl FnMut(String)) {
        self.pending_bytes.extend_from_slice(chunk);
        let decoded = self.decode_pending();
        self.buffer.push_str(&decoded);
        self.drain(&mut on_line);
    }

    /// Feeds the end of the stream: a trailing record without LF still counts.
    pub fn end(&mut self, mut on_line: impl FnMut(String)) {
        if !self.pending_bytes.is_empty() {
            // Whatever is left is not a complete sequence; Node's decoder emits
            // one replacement character per undecodable byte.
            let leftover = String::from_utf8_lossy(&self.pending_bytes).into_owned();
            self.pending_bytes.clear();
            self.buffer.push_str(&leftover);
        }
        self.drain(&mut on_line);
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            on_line(strip_carriage_return(line));
        }
    }

    /// Decodes as much of the pending bytes as forms complete characters.
    fn decode_pending(&mut self) -> String {
        let mut decoded = String::new();
        loop {
            match std::str::from_utf8(&self.pending_bytes) {
                Ok(text) => {
                    decoded.push_str(text);
                    self.pending_bytes.clear();
                    return decoded;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    decoded.push_str(
                        std::str::from_utf8(&self.pending_bytes[..valid_up_to])
                            .expect("valid prefix"),
                    );
                    match error.error_len() {
                        // Invalid sequence — consume it as one replacement char.
                        Some(invalid_len) => {
                            decoded.push('\u{fffd}');
                            self.pending_bytes.drain(..valid_up_to + invalid_len);
                        }
                        // Incomplete tail — wait for the next chunk.
                        None => {
                            self.pending_bytes.drain(..valid_up_to);
                            return decoded;
                        }
                    }
                }
            }
        }
    }

    fn drain(&mut self, on_line: &mut impl FnMut(String)) {
        while let Some(newline_index) = self.buffer.find('\n') {
            let line: String = self.buffer[..newline_index].to_owned();
            self.buffer = self.buffer[newline_index + 1..].to_owned();
            on_line(strip_carriage_return(line));
        }
    }
}

fn strip_carriage_return(line: String) -> String {
    match line.strip_suffix('\r') {
        Some(stripped) => stripped.to_owned(),
        None => line,
    }
}
