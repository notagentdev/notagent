//! Port of `packages/coding-agent/src/core/tools/output-accumulator.ts`.
//!
//! Tracks streaming output with bounded memory: chunks are decoded
//! incrementally, only a decoded tail is kept for snapshots, and a temp file is
//! opened once the full output has to be preserved.

use std::io::Write;
use std::path::PathBuf;

use crate::core::tools::truncate::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncatedBy, TruncationOptions, TruncationResult,
    truncate_tail,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputAccumulatorOptions {
    pub max_lines: usize,
    pub max_bytes: usize,
    pub temp_file_prefix: String,
}

impl Default for OutputAccumulatorOptions {
    fn default() -> Self {
        Self {
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
            temp_file_prefix: "notagent-output".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSnapshot {
    pub content: String,
    pub truncation: TruncationResult,
    pub full_output_path: Option<String>,
}

fn default_temp_file_path(prefix: &str) -> PathBuf {
    let id: String = (0..8)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect();
    std::env::temp_dir().join(format!("{prefix}-{id}.log"))
}

#[derive(Debug)]
pub struct OutputAccumulator {
    max_lines: usize,
    max_bytes: usize,
    max_rolling_bytes: usize,
    temp_file_prefix: String,

    /// Bytes that have not been decoded yet because they end mid-character.
    pending_bytes: Vec<u8>,
    raw_chunks: Vec<Vec<u8>>,
    tail_text: String,
    tail_bytes: usize,
    tail_starts_at_line_boundary: bool,
    total_raw_bytes: usize,
    total_decoded_bytes: usize,
    completed_lines: usize,
    total_lines: usize,
    current_line_bytes: usize,
    has_open_line: bool,
    finished: bool,

    temp_file_path: Option<PathBuf>,
    temp_file: Option<std::fs::File>,
}

impl OutputAccumulator {
    pub fn new(options: OutputAccumulatorOptions) -> Self {
        Self {
            max_lines: options.max_lines,
            max_bytes: options.max_bytes,
            max_rolling_bytes: (options.max_bytes * 2).max(1),
            temp_file_prefix: options.temp_file_prefix,
            pending_bytes: Vec::new(),
            raw_chunks: Vec::new(),
            tail_text: String::new(),
            tail_bytes: 0,
            tail_starts_at_line_boundary: true,
            total_raw_bytes: 0,
            total_decoded_bytes: 0,
            completed_lines: 0,
            total_lines: 0,
            current_line_bytes: 0,
            has_open_line: false,
            finished: false,
            temp_file_path: None,
            temp_file: None,
        }
    }

    /// Append raw bytes; panics after [`Self::finish`], as the TS version throws.
    pub fn append(&mut self, data: &[u8]) {
        assert!(
            !self.finished,
            "Cannot append to a finished output accumulator"
        );

        self.total_raw_bytes += data.len();
        let decoded = self.decode_streaming(data);
        self.append_decoded_text(&decoded);

        if self.temp_file.is_some() || self.should_use_temp_file() {
            self.ensure_temp_file();
            if let Some(file) = &mut self.temp_file {
                let _ = file.write_all(data);
            }
        } else if !data.is_empty() {
            self.raw_chunks.push(data.to_vec());
        }
    }

    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        // Flush whatever the streaming decoder still holds, like `decoder.decode()`.
        let rest = std::mem::take(&mut self.pending_bytes);
        if !rest.is_empty() {
            let decoded = String::from_utf8_lossy(&rest).into_owned();
            self.append_decoded_text(&decoded);
        }
        if self.should_use_temp_file() {
            self.ensure_temp_file();
        }
    }

    pub fn snapshot(&mut self, persist_if_truncated: bool) -> OutputSnapshot {
        let tail_truncation = truncate_tail(
            &self.snapshot_text(),
            TruncationOptions {
                max_lines: Some(self.max_lines),
                max_bytes: Some(self.max_bytes),
            },
        );
        let truncated =
            self.total_lines > self.max_lines || self.total_decoded_bytes > self.max_bytes;
        let truncated_by = if truncated {
            tail_truncation
                .truncated_by
                .or(Some(if self.total_decoded_bytes > self.max_bytes {
                    TruncatedBy::Bytes
                } else {
                    TruncatedBy::Lines
                }))
        } else {
            None
        };
        let truncation = TruncationResult {
            truncated,
            truncated_by,
            total_lines: self.total_lines,
            total_bytes: self.total_decoded_bytes,
            max_lines: self.max_lines,
            max_bytes: self.max_bytes,
            ..tail_truncation
        };

        if persist_if_truncated && truncation.truncated {
            self.ensure_temp_file();
        }

        OutputSnapshot {
            content: truncation.content.clone(),
            truncation,
            full_output_path: self
                .temp_file_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }

    pub fn close_temp_file(&mut self) {
        if let Some(mut file) = self.temp_file.take() {
            let _ = file.flush();
        }
    }

    pub fn get_last_line_bytes(&self) -> usize {
        self.current_line_bytes
    }

    pub fn full_output_path(&self) -> Option<String> {
        self.temp_file_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
    }

    /// `TextDecoder.decode(chunk, { stream: true })`: keep a trailing partial
    /// character for the next chunk.
    fn decode_streaming(&mut self, data: &[u8]) -> String {
        self.pending_bytes.extend_from_slice(data);
        let bytes = std::mem::take(&mut self.pending_bytes);
        match std::str::from_utf8(&bytes) {
            Ok(text) => text.to_owned(),
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                let decoded = String::from_utf8_lossy(&bytes[..valid_up_to]).into_owned();
                match error.error_len() {
                    // An incomplete character at the end waits for the next chunk.
                    None => {
                        self.pending_bytes.extend_from_slice(&bytes[valid_up_to..]);
                        decoded
                    }
                    // Invalid bytes become the replacement character, as in JS.
                    Some(length) => {
                        let mut decoded = decoded;
                        decoded.push('\u{FFFD}');
                        self.pending_bytes
                            .extend_from_slice(&bytes[valid_up_to + length..]);
                        let rest = std::mem::take(&mut self.pending_bytes);
                        decoded.push_str(&self.decode_streaming(&rest));
                        decoded
                    }
                }
            }
        }
    }

    fn append_decoded_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bytes = text.len();
        self.total_decoded_bytes += bytes;
        self.tail_text.push_str(text);
        self.tail_bytes += bytes;
        if self.tail_bytes > self.max_rolling_bytes * 2 {
            self.trim_tail();
        }

        let newlines = text.matches('\n').count();
        if newlines == 0 {
            self.current_line_bytes += bytes;
            self.has_open_line = true;
        } else {
            self.completed_lines += newlines;
            let tail = text.rsplit('\n').next().unwrap_or_default();
            self.current_line_bytes = tail.len();
            self.has_open_line = !tail.is_empty();
        }
        self.total_lines = self.completed_lines + usize::from(self.has_open_line);
    }

    fn trim_tail(&mut self) {
        let bytes = self.tail_text.as_bytes();
        if bytes.len() <= self.max_rolling_bytes {
            self.tail_bytes = bytes.len();
            return;
        }
        let mut start = bytes.len() - self.max_rolling_bytes;
        while start < bytes.len() && (bytes[start] & 0xc0) == 0x80 {
            start += 1;
        }
        if start != 0 {
            self.tail_starts_at_line_boundary = bytes[start - 1] == b'\n';
        }
        self.tail_text = String::from_utf8_lossy(&bytes[start..]).into_owned();
        self.tail_bytes = self.tail_text.len();
    }

    fn snapshot_text(&self) -> String {
        if self.tail_starts_at_line_boundary {
            return self.tail_text.clone();
        }
        // A trimmed tail may start mid-line; drop that partial line.
        match self.tail_text.find('\n') {
            Some(index) => self.tail_text[index + 1..].to_owned(),
            None => self.tail_text.clone(),
        }
    }

    fn should_use_temp_file(&self) -> bool {
        self.total_raw_bytes > self.max_bytes
            || self.total_decoded_bytes > self.max_bytes
            || self.total_lines > self.max_lines
    }

    fn ensure_temp_file(&mut self) {
        if self.temp_file_path.is_some() {
            return;
        }
        let path = default_temp_file_path(&self.temp_file_prefix);
        let Ok(mut file) = std::fs::File::create(&path) else {
            return;
        };
        for chunk in std::mem::take(&mut self.raw_chunks) {
            let _ = file.write_all(&chunk);
        }
        self.temp_file_path = Some(path);
        self.temp_file = Some(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accumulator(max_lines: usize, max_bytes: usize) -> OutputAccumulator {
        OutputAccumulator::new(OutputAccumulatorOptions {
            max_lines,
            max_bytes,
            temp_file_prefix: "notagent-output-test".to_owned(),
        })
    }

    #[test]
    fn keeps_short_output_untruncated() {
        let mut output = accumulator(2000, 50 * 1024);
        output.append(b"one\ntwo\n");
        output.finish();
        let snapshot = output.snapshot(false);
        assert_eq!(snapshot.content, "one\ntwo\n");
        assert!(!snapshot.truncation.truncated);
        assert_eq!(snapshot.truncation.total_lines, 2);
        assert_eq!(snapshot.truncation.total_bytes, 8);
        assert_eq!(snapshot.full_output_path, None);
    }

    #[test]
    fn counts_an_open_last_line() {
        let mut output = accumulator(2000, 50 * 1024);
        output.append(b"one\ntw");
        assert_eq!(output.get_last_line_bytes(), 2);
        output.append(b"o");
        assert_eq!(output.get_last_line_bytes(), 3);
        output.finish();
        assert_eq!(output.snapshot(false).truncation.total_lines, 2);
    }

    #[test]
    fn decodes_characters_split_across_chunks() {
        let mut output = accumulator(2000, 50 * 1024);
        let bytes = "ü".as_bytes();
        output.append(&bytes[..1]);
        output.append(&bytes[1..]);
        output.finish();
        assert_eq!(output.snapshot(false).content, "ü");
    }

    #[test]
    fn truncates_from_the_tail_and_reports_the_totals() {
        let mut output = accumulator(3, 50 * 1024);
        for index in 1..=10 {
            output.append(format!("line {index}\n").as_bytes());
        }
        output.finish();
        let snapshot = output.snapshot(false);
        assert_eq!(snapshot.content, "line 8\nline 9\nline 10");
        assert!(snapshot.truncation.truncated);
        assert_eq!(snapshot.truncation.truncated_by, Some(TruncatedBy::Lines));
        assert_eq!(snapshot.truncation.total_lines, 10);
    }

    #[test]
    fn writes_a_temp_file_once_the_output_grows_past_the_limit() {
        let mut output = accumulator(2, 16);
        output.append(b"aaaaaaaa\nbbbbbbbb\ncccccccc\n");
        output.finish();
        let snapshot = output.snapshot(false);
        let path = snapshot.full_output_path.expect("temp file");
        let written = std::fs::read_to_string(&path).expect("read");
        assert_eq!(written, "aaaaaaaa\nbbbbbbbb\ncccccccc\n");
        output.close_temp_file();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn persists_on_demand_when_the_snapshot_is_truncated() {
        let mut output = accumulator(1, 50 * 1024);
        output.append(b"one\ntwo\n");
        output.finish();
        let snapshot = output.snapshot(true);
        let path = snapshot.full_output_path.expect("temp file");
        assert!(std::path::Path::new(&path).exists());
        output.close_temp_file();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    #[should_panic(expected = "Cannot append to a finished output accumulator")]
    fn appending_after_finish_is_a_programming_error() {
        let mut output = accumulator(10, 100);
        output.finish();
        output.append(b"x");
    }
}
