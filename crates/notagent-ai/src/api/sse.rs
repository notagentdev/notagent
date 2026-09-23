use crate::utils::utf8_stream::Utf8StreamDecoder;

/// `ServerSentEvent { event, data, raw }`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerSentEvent {
    pub event: Option<String>,
    pub data: String,
    /// The raw lines of this event, comments included.
    pub raw: Vec<String>,
}

/// Incremental decoder; feed arbitrary chunks, collect complete events.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: String,
    event: Option<String>,
    data: Vec<String>,
    raw: Vec<String>,
    /// Bytes of a character a network chunk cut in half.
    utf8: Utf8StreamDecoder,
    /// The previous chunk ended in `\r`, so a `\n` opening the next one is
    /// the second half of that CRLF and not an empty line of its own.
    skip_leading_newline: bool,
}

impl SseDecoder {
    pub fn new() -> Self {
        SseDecoder::default()
    }

    /// `flushSseEvent(state)`
    fn flush(&mut self) -> Option<ServerSentEvent> {
        if self.event.is_none() && self.data.is_empty() {
            self.raw.clear();
            return None;
        }
        let event = ServerSentEvent {
            event: self.event.take(),
            data: self.data.join("\n"),
            raw: std::mem::take(&mut self.raw),
        };
        self.data.clear();
        Some(event)
    }

    /// `decodeSseLine(line, state)`
    fn decode_line(&mut self, line: &str) -> Option<ServerSentEvent> {
        if line.is_empty() {
            return self.flush();
        }

        self.raw.push(line.to_string());
        if line.starts_with(':') {
            return None;
        }

        let (field_name, value) = match line.find(':') {
            None => (line, ""),
            Some(index) => (&line[..index], &line[index + 1..]),
        };
        let value = value.strip_prefix(' ').unwrap_or(value);

        if field_name == "event" {
            self.event = Some(value.to_string());
        } else if field_name == "data" {
            self.data.push(value.to_string());
        }
        None
    }

    /// `consumeLine(text)` — the next line plus the rest, or `None` while incomplete.
    fn consume_line(&mut self) -> Option<String> {
        if self.skip_leading_newline && !self.buffer.is_empty() {
            self.skip_leading_newline = false;
            if self.buffer.starts_with('\n') {
                self.buffer.drain(..1);
            }
        }
        let carriage_return = self.buffer.find('\r');
        let newline = self.buffer.find('\n');
        let break_index = match (carriage_return, newline) {
            (None, None) => return None,
            (None, Some(index)) | (Some(index), None) => index,
            (Some(first), Some(second)) => first.min(second),
        };

        let mut next_index = break_index + 1;
        if self.buffer.as_bytes()[break_index] == b'\r' {
            match self.buffer.as_bytes().get(next_index) {
                Some(b'\n') => next_index += 1,
                None => self.skip_leading_newline = true,
                Some(_) => {}
            }
        }
        let line = self.buffer[..break_index].to_string();
        self.buffer.drain(..next_index);
        Some(line)
    }

    /// Feeds a chunk and returns every event it completed.
    pub fn feed(&mut self, chunk: &str) -> Vec<ServerSentEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();
        while let Some(line) = self.consume_line() {
            if let Some(event) = self.decode_line(&line) {
                events.push(event);
            }
        }
        events
    }

    /// Feeds raw bytes as they came off the wire. A multi-byte character split
    /// between two chunks is held back until it is complete.
    pub fn feed_bytes(&mut self, chunk: &[u8]) -> Vec<ServerSentEvent> {
        let text = self.utf8.decode(chunk);
        self.feed(&text)
    }

    /// Ends the stream: decodes a trailing partial line and flushes a pending event.
    pub fn finish(&mut self) -> Vec<ServerSentEvent> {
        let tail = self.utf8.finish();
        self.buffer.push_str(&tail);
        let mut events = Vec::new();
        while let Some(line) = self.consume_line() {
            if let Some(event) = self.decode_line(&line) {
                events.push(event);
            }
        }
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            if let Some(event) = self.decode_line(&line) {
                events.push(event);
            }
        }
        if let Some(event) = self.flush() {
            events.push(event);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(chunks: &[&str]) -> Vec<ServerSentEvent> {
        let mut decoder = SseDecoder::new();
        let mut events = Vec::new();
        for chunk in chunks {
            events.extend(decoder.feed(chunk));
        }
        events.extend(decoder.finish());
        events
    }

    #[test]
    fn decodes_a_simple_event() {
        let events = decode_all(&["event: message_start\ndata: {\"a\":1}\n\n"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message_start"));
        assert_eq!(events[0].data, "{\"a\":1}");
        assert_eq!(events[0].raw, ["event: message_start", "data: {\"a\":1}"]);
    }

    #[test]
    fn joins_multiple_data_lines_with_newlines() {
        let events = decode_all(&["data: first\ndata: second\n\n"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "first\nsecond");
    }

    #[test]
    fn handles_cr_lf_and_crlf_line_endings() {
        for terminator in ["\n", "\r", "\r\n"] {
            let payload = format!("event: e{terminator}data: d{terminator}{terminator}");
            let events = decode_all(&[&payload]);
            assert_eq!(events.len(), 1, "terminator {terminator:?}");
            assert_eq!(events[0].event.as_deref(), Some("e"));
            assert_eq!(events[0].data, "d");
        }
    }

    #[test]
    fn keeps_comments_in_raw_but_not_in_data() {
        let events = decode_all(&[": keep-alive\ndata: value\n\n"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "value");
        assert_eq!(events[0].raw, [": keep-alive", "data: value"]);
    }

    #[test]
    fn a_comment_only_block_yields_no_event() {
        assert_eq!(decode_all(&[": ping\n\n"]), Vec::new());
    }

    #[test]
    fn strips_exactly_one_leading_space_after_the_colon() {
        let events = decode_all(&["data:  two-spaces\n\n"]);
        assert_eq!(events[0].data, " two-spaces");
    }

    #[test]
    fn treats_a_field_without_a_colon_as_an_empty_value() {
        let events = decode_all(&["data\nevent\n\n"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "");
        assert_eq!(events[0].event.as_deref(), Some(""));
    }

    #[test]
    fn emits_a_trailing_event_without_a_closing_blank_line() {
        let events = decode_all(&["event: done\ndata: {}"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("done"));
        assert_eq!(events[0].data, "{}");
    }

    #[test]
    fn ignores_a_completely_empty_stream() {
        assert_eq!(decode_all(&[]), Vec::new());
        assert_eq!(decode_all(&[""]), Vec::new());
    }

    /// The plan asks for property tests that split the same fixture at every possible
    /// boundary; fragmentation must not change the decoded events.
    #[test]
    fn fragmentation_does_not_change_the_result() {
        let payload = "event: message_start\ndata: {\"type\":\"message_start\"}\n\n\
                       : keep-alive\n\
                       event: content_block_delta\ndata: {\"delta\":\"a\"}\ndata: {\"delta\":\"b\"}\n\n\
                       event: message_stop\ndata: {}\n\n";
        let expected = decode_all(&[payload]);
        assert_eq!(expected.len(), 3);

        for split in 1..payload.len() {
            if !payload.is_char_boundary(split) {
                continue;
            }
            let (head, tail) = payload.split_at(split);
            assert_eq!(decode_all(&[head, tail]), expected, "split at {split}");
        }

        // Byte-by-byte feeding must also agree.
        let single_chars: Vec<String> = payload.chars().map(|c| c.to_string()).collect();
        let chunks: Vec<&str> = single_chars.iter().map(String::as_str).collect();
        assert_eq!(decode_all(&chunks), expected);
    }

    #[test]
    fn a_crlf_split_between_two_chunks_is_one_line_break() {
        let payload = "event: e\r\ndata: d\r\n\r\nevent: f\r\ndata: g\r\n\r\n";
        let expected = decode_all(&[payload]);
        assert_eq!(expected.len(), 2);
        for split in 1..payload.len() {
            let (head, tail) = payload.split_at(split);
            assert_eq!(decode_all(&[head, tail]), expected, "split at {split}");
        }
    }

    #[test]
    fn a_character_split_between_two_byte_chunks_is_decoded_whole() {
        let payload = "event: e\ndata: {\"text\":\"Grüße 🙈\"}\n\n".as_bytes();
        for split in 1..payload.len() {
            let mut decoder = SseDecoder::new();
            let mut events = decoder.feed_bytes(&payload[..split]);
            events.extend(decoder.feed_bytes(&payload[split..]));
            events.extend(decoder.finish());
            assert_eq!(events.len(), 1, "split at {split}");
            assert_eq!(
                events[0].data, "{\"text\":\"Grüße 🙈\"}",
                "split at {split}"
            );
        }
    }

    #[test]
    fn fragmentation_preserves_multibyte_payloads() {
        let payload = "event: e\ndata: {\"text\":\"héllo 🙈 世界\"}\n\n";
        let expected = decode_all(&[payload]);
        assert_eq!(expected.len(), 1);
        for split in 1..payload.len() {
            if !payload.is_char_boundary(split) {
                continue;
            }
            let (head, tail) = payload.split_at(split);
            assert_eq!(decode_all(&[head, tail]), expected, "split at {split}");
        }
    }
}
