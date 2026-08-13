//! Differential test of the Markdown lexer against marked.
//!
//! `tools/gen-markdown-oracle.mjs` dumps the marked token stream for every
//! source of the ported test suite; the Rust lexer must reproduce it exactly.

use notagent_tui::markdown_lexer::{Token, lex};
use serde_json::Value;

/// Convert a lexer token into the shape of the oracle fixture.
fn to_json(token: &Token) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("type".into(), Value::String(token.kind.clone()));
    object.insert("raw".into(), Value::String(token.raw.clone()));
    if let Some(text) = &token.text {
        object.insert("text".into(), Value::String(text.clone()));
    }
    if let Some(depth) = token.depth {
        object.insert("depth".into(), Value::from(depth));
    }
    if let Some(lang) = &token.lang {
        object.insert("lang".into(), Value::String(lang.clone()));
    }
    if let Some(href) = &token.href {
        object.insert("href".into(), Value::String(href.clone()));
    }
    if let Some(ordered) = token.ordered {
        object.insert("ordered".into(), Value::Bool(ordered));
    }
    if token.kind == "list" {
        // marked emits an empty string for unordered lists.
        object.insert(
            "start".into(),
            token
                .start
                .map_or_else(|| Value::String(String::new()), Value::from),
        );
    }
    if let Some(loose) = token.loose {
        object.insert("loose".into(), Value::Bool(loose));
    }
    if let Some(task) = token.task {
        object.insert("task".into(), Value::Bool(task));
    }
    if let Some(checked) = token.checked {
        object.insert("checked".into(), Value::Bool(checked));
    }
    if let Some(pending) = token.pending {
        object.insert("pending".into(), Value::Bool(pending));
    }
    if let Some(tokens) = &token.tokens {
        object.insert(
            "tokens".into(),
            Value::Array(tokens.iter().map(to_json).collect()),
        );
    }
    if let Some(items) = &token.items {
        object.insert(
            "items".into(),
            Value::Array(items.iter().map(to_json).collect()),
        );
    }
    if let Some(header) = &token.header {
        object.insert(
            "header".into(),
            Value::Array(
                header
                    .iter()
                    .map(|cell| {
                        serde_json::json!({
                            "text": cell.text,
                            "tokens": cell.tokens.iter().map(to_json).collect::<Vec<_>>(),
                        })
                    })
                    .collect(),
            ),
        );
    }
    if let Some(rows) = &token.rows {
        object.insert(
            "rows".into(),
            Value::Array(
                rows.iter()
                    .map(|row| {
                        Value::Array(
                            row.iter()
                                .map(|cell| {
                                    serde_json::json!({
                                        "text": cell.text,
                                        "tokens": cell.tokens.iter().map(to_json).collect::<Vec<_>>(),
                                    })
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(object)
}

#[test]
fn reproduces_the_marked_token_stream() {
    let fixture = include_str!("fixtures/markdown-oracle.json");
    let cases: Vec<Value> = serde_json::from_str(fixture).expect("fixture parses");

    let mut mismatches: Vec<String> = Vec::new();
    for case in &cases {
        let source = case["source"].as_str().expect("source");
        let expected = &case["tokens"];
        let actual = Value::Array(
            lex(&source.replace('\t', "   "))
                .iter()
                .map(to_json)
                .collect(),
        );
        if &actual != expected {
            mismatches.push(format!(
                "source {source:?}\n  expected {}\n  actual   {}",
                serde_json::to_string(expected).expect("json"),
                serde_json::to_string(&actual).expect("json")
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "{} of {} sources mismatch:\n{}",
        mismatches.len(),
        cases.len(),
        mismatches.join("\n")
    );
}
