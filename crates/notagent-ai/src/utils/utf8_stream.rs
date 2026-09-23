/// Decodes UTF-8 that arrives in arbitrary chunks.
/// A network or pipe read ends wherever the transport cut it, which is often
/// inside a multi-byte character. Decoding each chunk on its own turns both
/// halves of that character into U+FFFD; this keeps the unfinished tail until
/// the next chunk completes it.
#[derive(Debug, Default, Clone)]
pub struct Utf8StreamDecoder {
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes everything complete so far. Invalid bytes become U+FFFD; an
    /// incomplete character at the end waits for the next call.
    pub fn decode(&mut self, chunk: &[u8]) -> String {
        let mut bytes = std::mem::take(&mut self.pending);
        bytes.extend_from_slice(chunk);
        let mut decoded = String::with_capacity(bytes.len());
        let mut rest: &[u8] = &bytes;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    decoded.push_str(text);
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    // The prefix up to `valid_up_to` is valid by definition.
                    decoded.push_str(&String::from_utf8_lossy(&rest[..valid_up_to]));
                    match error.error_len() {
                        None => {
                            self.pending.extend_from_slice(&rest[valid_up_to..]);
                            break;
                        }
                        Some(length) => {
                            decoded.push('\u{FFFD}');
                            rest = &rest[valid_up_to + length..];
                        }
                    }
                }
            }
        }
        decoded
    }

    /// Ends the stream: a character that never completed becomes U+FFFD.
    pub fn finish(&mut self) -> String {
        let pending = std::mem::take(&mut self.pending);
        String::from_utf8_lossy(&pending).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_split_across_chunks_survives() {
        let text = "Grüße 🙈 世界";
        let bytes = text.as_bytes();
        for split in 1..bytes.len() {
            let mut decoder = Utf8StreamDecoder::new();
            let mut decoded = decoder.decode(&bytes[..split]);
            decoded.push_str(&decoder.decode(&bytes[split..]));
            decoded.push_str(&decoder.finish());
            assert_eq!(decoded, text, "split at byte {split}");
        }
    }

    #[test]
    fn byte_by_byte_feeding_decodes_the_whole_text() {
        let text = "ä€😀";
        let mut decoder = Utf8StreamDecoder::new();
        let decoded: String = text
            .as_bytes()
            .iter()
            .map(|byte| decoder.decode(&[*byte]))
            .collect();
        assert_eq!(decoded, text);
    }

    #[test]
    fn invalid_bytes_become_replacement_characters_and_decoding_continues() {
        let mut decoder = Utf8StreamDecoder::new();
        assert_eq!(decoder.decode(b"a\xffb"), "a\u{FFFD}b");
        assert_eq!(decoder.decode(b"\xc3"), "");
        assert_eq!(decoder.finish(), "\u{FFFD}");
    }
}
