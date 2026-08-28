pub const DEFAULT_MAX_LINES: usize = 2000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;
/// Max chars per grep match line.
pub const GREP_MAX_LINE_LENGTH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TruncationResult {
    pub content: String,
    pub truncated: bool,
    pub truncated_by: Option<TruncatedBy>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
    /// Only set by the tail-truncation edge case.
    pub last_line_partial: bool,
    /// Set when the first line alone exceeds the byte limit.
    pub first_line_exceeds_limit: bool,
    pub max_lines: usize,
    pub max_bytes: usize,
}

/// The `truncation` field of a tool result's details.
/// here, so the renderers read the field back instead of holding a reference to
/// the struct the tool produced.
pub fn truncation_from_details(details: Option<&serde_json::Value>) -> Option<TruncationResult> {
    serde_json::from_value(details?.get("truncation")?.clone()).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TruncationOptions {
    pub max_lines: Option<usize>,
    pub max_bytes: Option<usize>,
}

impl TruncationOptions {
    fn resolve(self) -> (usize, usize) {
        (
            self.max_lines.unwrap_or(DEFAULT_MAX_LINES),
            self.max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
        )
    }
}

fn split_lines_for_counting(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Format bytes as a human-readable size.
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn untruncated(
    content: &str,
    total_lines: usize,
    total_bytes: usize,
    limits: (usize, usize),
) -> TruncationResult {
    TruncationResult {
        content: content.to_owned(),
        truncated: false,
        truncated_by: None,
        total_lines,
        total_bytes,
        output_lines: total_lines,
        output_bytes: total_bytes,
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines: limits.0,
        max_bytes: limits.1,
    }
}

/// Truncate content from the head (keep the first N lines/bytes).
pub fn truncate_head(content: &str, options: TruncationOptions) -> TruncationResult {
    let (max_lines, max_bytes) = options.resolve();
    let total_bytes = content.len();
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return untruncated(content, total_lines, total_bytes, (max_lines, max_bytes));
    }

    // A first line that alone exceeds the byte limit yields no content at all.
    if lines[0].len() > max_bytes {
        return TruncationResult {
            content: String::new(),
            truncated: true,
            truncated_by: Some(TruncatedBy::Bytes),
            total_lines,
            total_bytes,
            output_lines: 0,
            output_bytes: 0,
            last_line_partial: false,
            first_line_exceeds_limit: true,
            max_lines,
            max_bytes,
        };
    }

    let mut output_lines: Vec<&str> = Vec::new();
    let mut output_bytes_count = 0usize;
    let mut truncated_by = TruncatedBy::Lines;
    for (index, line) in lines.iter().enumerate().take(max_lines) {
        let line_bytes = line.len() + usize::from(index > 0);
        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        output_lines.push(line);
        output_bytes_count += line_bytes;
    }
    if output_lines.len() >= max_lines && output_bytes_count <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }

    let output_content = output_lines.join("\n");
    TruncationResult {
        output_bytes: output_content.len(),
        output_lines: output_lines.len(),
        content: output_content,
        truncated: true,
        truncated_by: Some(truncated_by),
        total_lines,
        total_bytes,
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

/// Truncate content from the tail (keep the last N lines/bytes).
/// May return a partial first line when the last line exceeds the byte limit.
pub fn truncate_tail(content: &str, options: TruncationOptions) -> TruncationResult {
    let (max_lines, max_bytes) = options.resolve();
    let total_bytes = content.len();
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return untruncated(content, total_lines, total_bytes, (max_lines, max_bytes));
    }

    let mut output_lines: Vec<String> = Vec::new();
    let mut output_bytes_count = 0usize;
    let mut truncated_by = TruncatedBy::Lines;
    let mut last_line_partial = false;
    for line in lines.iter().rev() {
        if output_lines.len() >= max_lines {
            break;
        }
        let line_bytes = line.len() + usize::from(!output_lines.is_empty());
        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            // Nothing fit yet, so keep the end of this line.
            if output_lines.is_empty() {
                let truncated_line = truncate_string_to_bytes_from_end(line, max_bytes);
                output_bytes_count = truncated_line.len();
                output_lines.insert(0, truncated_line);
                last_line_partial = true;
            }
            break;
        }
        output_lines.insert(0, (*line).to_owned());
        output_bytes_count += line_bytes;
    }
    if output_lines.len() >= max_lines && output_bytes_count <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }

    let output_content = output_lines.join("\n");
    TruncationResult {
        output_bytes: output_content.len(),
        output_lines: output_lines.len(),
        content: output_content,
        truncated: true,
        truncated_by: Some(truncated_by),
        total_lines,
        total_bytes,
        last_line_partial,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

/// Truncate a string to a byte limit, counted from the end.
fn truncate_string_to_bytes_from_end(value: &str, max_bytes: usize) -> String {
    let bytes = value.as_bytes();
    if bytes.len() <= max_bytes {
        return value.to_owned();
    }
    let mut start = bytes.len() - max_bytes;
    // Move to the start of a UTF-8 character.
    while start < bytes.len() && (bytes[start] & 0xc0) == 0x80 {
        start += 1;
    }
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncatedLine {
    pub text: String,
    pub was_truncated: bool,
}

/// Truncate a single line to `max_chars`, adding a `[truncated]` suffix.
/// Deviation (class 1): JS counts UTF-16 code units. The cut here lands on a
/// character boundary, so a cut that would split a surrogate pair keeps one
/// character less instead of producing a lone surrogate.
pub fn truncate_line(line: &str, max_chars: usize) -> TruncatedLine {
    if line.encode_utf16().count() <= max_chars {
        return TruncatedLine {
            text: line.to_owned(),
            was_truncated: false,
        };
    }
    let mut units = 0usize;
    let mut end = line.len();
    for (index, character) in line.char_indices() {
        let width = character.len_utf16();
        if units + width > max_chars {
            end = index;
            break;
        }
        units += width;
    }
    TruncatedLine {
        text: format!("{}... [truncated]", &line[..end]),
        was_truncated: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(content: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
        truncate_head(
            content,
            TruncationOptions {
                max_lines: Some(max_lines),
                max_bytes: Some(max_bytes),
            },
        )
    }

    fn tail(content: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
        truncate_tail(
            content,
            TruncationOptions {
                max_lines: Some(max_lines),
                max_bytes: Some(max_bytes),
            },
        )
    }

    #[test]
    fn counts_lines_without_a_trailing_empty_line() {
        let result = truncate_head("a\nb\nc\n", TruncationOptions::default());
        assert!(!result.truncated);
        assert_eq!(result.total_lines, 3);
        assert_eq!(result.total_bytes, 6);
        assert_eq!(result.content, "a\nb\nc\n");

        let result = truncate_head("", TruncationOptions::default());
        assert_eq!(result.total_lines, 0);
        assert_eq!(result.content, "");
    }

    #[test]
    fn head_truncation_stops_at_the_line_limit() {
        let content = "1\n2\n3\n4\n5\n";
        let result = head(content, 3, 1024);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, Some(TruncatedBy::Lines));
        assert_eq!(result.content, "1\n2\n3");
        assert_eq!(result.output_lines, 3);
        assert_eq!(result.total_lines, 5);
    }

    #[test]
    fn head_truncation_stops_at_the_byte_limit_on_a_line_boundary() {
        let content = "aaaa\nbbbb\ncccc\n";
        let result = head(content, 100, 9);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
        assert_eq!(result.content, "aaaa\nbbbb");
        assert_eq!(result.output_bytes, 9);
    }

    #[test]
    fn head_truncation_reports_a_first_line_over_the_byte_limit() {
        let result = head("aaaaaaaaaa\nb\n", 100, 5);
        assert!(result.truncated);
        assert!(result.first_line_exceeds_limit);
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
        assert_eq!(result.content, "");
        assert_eq!(result.output_lines, 0);
    }

    #[test]
    fn tail_truncation_keeps_the_last_lines() {
        let result = tail("1\n2\n3\n4\n5\n", 2, 1024);
        assert_eq!(result.content, "4\n5");
        assert_eq!(result.truncated_by, Some(TruncatedBy::Lines));
        assert!(!result.last_line_partial);
    }

    #[test]
    fn tail_truncation_may_cut_inside_the_last_line() {
        let result = tail("short\nAAAAAAAAAA", 100, 4);
        assert!(result.last_line_partial);
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
        assert_eq!(result.content, "AAAA");
    }

    #[test]
    fn tail_truncation_keeps_utf8_characters_intact() {
        // "äöü" is six bytes; a five-byte window must not split a character.
        let result = tail("x\näöü", 100, 5);
        assert!(result.last_line_partial);
        assert_eq!(result.content, "öü");
    }

    #[test]
    fn formats_human_readable_sizes() {
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(1536), "1.5KB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0MB");
    }

    #[test]
    fn truncates_long_grep_lines() {
        let line = "x".repeat(GREP_MAX_LINE_LENGTH + 10);
        let result = truncate_line(&line, GREP_MAX_LINE_LENGTH);
        assert!(result.was_truncated);
        assert_eq!(
            result.text,
            format!("{}... [truncated]", "x".repeat(GREP_MAX_LINE_LENGTH))
        );

        let result = truncate_line("short", GREP_MAX_LINE_LENGTH);
        assert!(!result.was_truncated);
        assert_eq!(result.text, "short");
    }

    #[test]
    fn grep_line_truncation_counts_utf16_units() {
        // Each emoji is two UTF-16 units, so five of them fill a limit of ten.
        let line = "🎉".repeat(8);
        let result = truncate_line(&line, 10);
        assert!(result.was_truncated);
        assert_eq!(result.text, format!("{}... [truncated]", "🎉".repeat(5)));
    }
}
