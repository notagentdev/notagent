//! Byte-exact comparison of the grok-mermaid substitute against the library.
//!
//! `tools/gen-mermaid-render-oracle.mjs` runs grok-mermaid 0.2.2 over a corpus
//! of 49 sources — every diagram kind, every routing case (self edges, back
//! edges, subgraphs, notes, dividers), the retry-without-the-last-line path and
//! the three "nothing to draw" cases — and records `render` and `sourceBox`
//! output. The port has to reproduce it cell for cell.

use notagent::utils::mermaid::render;
use notagent::utils::mermaid::source_box::source_box;
use notagent::utils::mermaid::types::{Cls, MermaidArt};
use serde_json::Value;

fn oracle() -> Vec<Value> {
    let raw = include_str!("fixtures/mermaid-render-oracle.json");
    serde_json::from_str(raw).expect("parses")
}

fn cls_name(cls: Cls) -> &'static str {
    match cls {
        Cls::Border => "border",
        Cls::Text => "text",
        Cls::Edge => "edge",
        Cls::EdgeLabel => "edgeLabel",
        Cls::Title => "title",
        Cls::None => "none",
    }
}

fn art_json(art: &MermaidArt) -> Value {
    serde_json::json!({
        "plain": art.plain,
        "styled": art
            .styled
            .iter()
            .map(|row| {
                row.iter()
                    .map(|span| serde_json::json!({ "text": span.text, "cls": cls_name(span.cls) }))
                    .collect::<Vec<Value>>()
            })
            .collect::<Vec<Vec<Value>>>(),
        "width": art.width,
        "warnings": art.warnings,
    })
}

#[test]
fn renders_every_corpus_source_exactly_like_the_library() {
    for case in oracle() {
        let src = case["src"].as_str().expect("src");
        let expected = &case["art"];
        match render(src) {
            None => assert!(
                expected.is_null(),
                "render returned nothing for {src:?}, library drew {expected}"
            ),
            Some(art) => {
                assert!(!expected.is_null(), "library drew nothing for {src:?}");
                assert_eq!(
                    art_json(&art),
                    *expected,
                    "art differs for {src:?}\n--- port ---\n{}\n--- library ---\n{}",
                    art.plain.join("\n"),
                    expected["plain"]
                        .as_array()
                        .expect("plain")
                        .iter()
                        .map(|line| line.as_str().unwrap_or_default())
                        .collect::<Vec<&str>>()
                        .join("\n"),
                );
            }
        }
    }
}

#[test]
fn source_box_frames_every_corpus_source_exactly_like_the_library() {
    for case in oracle() {
        let src = case["src"].as_str().expect("src");
        assert_eq!(
            art_json(&source_box(src, Some(40))),
            case["sourceBox"],
            "source box differs for {src:?}"
        );
    }
}
