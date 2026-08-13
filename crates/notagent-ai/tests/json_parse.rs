//! Differential test of `parse_streaming_json` against the TypeScript implementation.
//!
//! The fixture was generated in the TS repo from the original `json-parse.ts` chain on
//! top of the installed `partial-json` package (`/tmp/gen-partial-json-fixture.cjs`):
//! every prefix of eight representative tool-argument payloads plus malformed and
//! repair cases — 513 inputs with the exact `JSON.stringify` output of the TS chain.

use notagent_ai::utils::json_parse::{parse_streaming_json, repair_json};
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/partial-json.jsonl");

#[derive(serde::Deserialize)]
struct Case {
    input: String,
    expected: String,
}

#[test]
fn matches_typescript_for_every_fixture_input() {
    let cases: Vec<Case> = FIXTURE
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(cases.len(), 512, "unexpected fixture size");

    let mut mismatches = Vec::new();
    for case in &cases {
        let expected: Value =
            serde_json::from_str(&case.expected).expect("fixture expectation is JSON");
        let actual = parse_streaming_json(Some(&case.input));
        if actual != expected {
            mismatches.push(format!(
                "input {:?}: expected {expected}, got {actual}",
                case.input
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn empty_input_yields_empty_object() {
    assert_eq!(parse_streaming_json(None), serde_json::json!({}));
    assert_eq!(parse_streaming_json(Some("")), serde_json::json!({}));
    assert_eq!(parse_streaming_json(Some("   ")), serde_json::json!({}));
}

#[test]
fn repair_json_escapes_control_characters_and_invalid_escapes() {
    assert_eq!(
        repair_json("{\"a\": \"raw\nnewline\"}"),
        r#"{"a": "raw\nnewline"}"#
    );
    assert_eq!(
        repair_json(r#"{"a": "bad \q escape"}"#),
        r#"{"a": "bad \\q escape"}"#
    );
    assert_eq!(repair_json(r#"{"a": "keep é"}"#), r#"{"a": "keep é"}"#);
    // `u` is a valid escape character, so a short \u sequence is left untouched.
    assert_eq!(
        repair_json(r#"{"a": "short \u12"}"#),
        r#"{"a": "short \u12"}"#
    );
    // Outside strings nothing is touched.
    assert_eq!(repair_json("{\"a\": 1}"), "{\"a\": 1}");
}
