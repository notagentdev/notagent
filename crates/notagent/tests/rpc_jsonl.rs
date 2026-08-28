use notagent::modes::rpc::jsonl::{JsonlLineSplitter, serialize_json_line};
use serde_json::json;

fn collect(chunks: &[&[u8]], end: bool) -> Vec<String> {
    let mut splitter = JsonlLineSplitter::new();
    let mut lines: Vec<String> = Vec::new();
    for chunk in chunks {
        splitter.push(chunk, |line| lines.push(line));
    }
    if end {
        splitter.end(|line| lines.push(line));
    }
    lines
}

#[test]
fn serializes_strict_jsonl_records_without_escaping_unicode_separators() {
    let line = serialize_json_line(&json!({ "text": "a\u{2028}b\u{2029}c" }));

    assert!(line.contains("a\u{2028}b\u{2029}c"));
    assert!(line.ends_with('\n'));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(line.trim()).expect("json"),
        json!({ "text": "a\u{2028}b\u{2029}c" })
    );
}

#[test]
fn splits_on_lf_only_and_preserves_u2028_u2029_inside_payloads() {
    let record = serialize_json_line(&json!({ "text": "a\u{2028}b\u{2029}c" }));

    let lines = collect(&[record.as_bytes()], true);

    assert_eq!(lines.len(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&lines[0]).expect("json"),
        json!({ "text": "a\u{2028}b\u{2029}c" })
    );
}

#[test]
fn handles_crlf_delimited_input() {
    let lines = collect(&[b"{\"a\":1}\r\n{\"b\":2}\r\n"], true);

    assert_eq!(lines, vec!["{\"a\":1}".to_owned(), "{\"b\":2}".to_owned()]);
}

#[test]
fn emits_a_final_line_without_trailing_lf() {
    let lines = collect(&[b"{\"a\":1}"], true);

    assert_eq!(lines, vec!["{\"a\":1}".to_owned()]);
}

/// sequence split across two chunks, and so does the splitter.
#[test]
fn decodes_a_multibyte_character_split_across_chunks() {
    let text = "ä".as_bytes().to_vec();
    let lines = collect(&[&text[..1], &text[1..], b"\n"], false);

    assert_eq!(lines, vec!["ä".to_owned()]);
}
