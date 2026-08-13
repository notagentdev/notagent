//! Pins the jsdiff port in
//! `src/modes/interactive/components/diff/word_diff.rs` against the real
//! `diff` package (8.0.4).
//!
//! `tools/gen-diff-words-oracle.mjs` runs `Diff.diffWords` over 30 × 30 line
//! pairs from the TypeScript repo and writes
//! `tests/fixtures/diff-words-oracle.json`.

use notagent::modes::interactive::components::diff::word_diff::{Change, diff_words};

#[derive(serde::Deserialize)]
struct OracleCase {
    old: String,
    new: String,
    parts: Vec<OraclePart>,
}

#[derive(serde::Deserialize)]
struct OraclePart {
    value: String,
    added: bool,
    removed: bool,
}

#[test]
fn matches_jsdiff_for_every_oracle_case() {
    let oracle: Vec<OracleCase> =
        serde_json::from_str(include_str!("fixtures/diff-words-oracle.json"))
            .expect("oracle parses");
    assert_eq!(oracle.len(), 900);

    let mut failures = Vec::new();
    for case in &oracle {
        let expected: Vec<Change> = case
            .parts
            .iter()
            .map(|part| Change {
                value: part.value.clone(),
                added: part.added,
                removed: part.removed,
            })
            .collect();
        let actual = diff_words(&case.old, &case.new);
        if actual != expected {
            failures.push(format!(
                "{:?} -> {:?}\n  expected {expected:?}\n  actual   {actual:?}",
                case.old, case.new
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        oracle.len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
