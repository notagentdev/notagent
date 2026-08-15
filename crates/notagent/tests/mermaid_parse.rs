//! Tests for the ported flowchart grammar of the grok-mermaid substitute
//! (`utils::mermaid::parse`).
//!
//! Every expectation is the real library's output: `tools/gen-mermaid-parse-oracle.mjs`
//! runs grok-mermaid 0.2.2 over the corpus in `fixtures/mermaid-parse-oracle.json`
//! and records the statements, the diagram kind and the whole parsed graph.

use notagent::utils::mermaid::graph::{Dir, Head, LineKind, Shape};
use notagent::utils::mermaid::parse::{DiagramKind, diagram_kind, parse_graph, statements_of};
use serde_json::Value;

fn oracle() -> Vec<Value> {
    let raw = include_str!("fixtures/mermaid-parse-oracle.json");
    serde_json::from_str(raw).expect("parses")
}

fn kind_name(kind: Option<DiagramKind>) -> Value {
    match kind {
        Some(DiagramKind::Flowchart) => Value::from("flowchart"),
        Some(DiagramKind::State) => Value::from("state"),
        Some(DiagramKind::Class) => Value::from("class"),
        Some(DiagramKind::Er) => Value::from("er"),
        Some(DiagramKind::Sequence) => Value::from("sequence"),
        None => Value::Null,
    }
}

fn dir_name(dir: Dir) -> &'static str {
    match dir {
        Dir::Down => "down",
        Dir::Up => "up",
        Dir::Right => "right",
        Dir::Left => "left",
    }
}

fn shape_name(shape: Shape) -> &'static str {
    match shape {
        Shape::Rect => "rect",
        Shape::Round => "round",
        Shape::Diamond => "diamond",
    }
}

fn head_name(head: Head) -> &'static str {
    match head {
        Head::None => "none",
        Head::Arrow => "arrow",
        Head::Circle => "circle",
        Head::Cross => "cross",
        Head::Triangle => "triangle",
        Head::DiamondFill => "diamondFill",
        Head::DiamondOpen => "diamondOpen",
    }
}

fn line_name(line: LineKind) -> &'static str {
    match line {
        LineKind::Solid => "solid",
        LineKind::Dotted => "dotted",
        LineKind::Thick => "thick",
    }
}

/// The parsed graph in the shape the oracle prints.
fn graph_value(src: &str) -> Value {
    let Some(graph) = parse_graph(src) else {
        return Value::Null;
    };
    serde_json::json!({
        "dir": dir_name(graph.dir),
        "nodes": graph
            .nodes
            .iter()
            .map(|node| serde_json::json!({ "label": node.label, "shape": shape_name(node.shape) }))
            .collect::<Vec<_>>(),
        "edges": graph
            .edges
            .iter()
            .map(|edge| serde_json::json!({
                "from": edge.from,
                "to": edge.to,
                "label": edge.label,
                "headTo": head_name(edge.head_to),
                "headFrom": head_name(edge.head_from),
                "line": line_name(edge.line),
            }))
            .collect::<Vec<_>>(),
        "groups": graph
            .groups
            .iter()
            .map(|group| serde_json::json!({
                "id": group.id,
                "label": group.label,
                "parent": group.parent,
            }))
            .collect::<Vec<_>>(),
        "nodeGroup": graph.node_group,
        "warnings": graph.warnings,
    })
}

#[test]
fn the_statement_splitter_matches_the_library() {
    for case in oracle() {
        let src = case["src"].as_str().expect("src");
        let expected: Vec<String> =
            serde_json::from_value(case["statements"].clone()).expect("statements");
        assert_eq!(statements_of(src), expected, "statements of {src:?}");
    }
}

#[test]
fn the_diagram_kind_matches_the_library() {
    for case in oracle() {
        let src = case["src"].as_str().expect("src");
        assert_eq!(
            kind_name(diagram_kind(src)),
            case["kind"],
            "kind of {src:?}"
        );
    }
}

#[test]
fn the_flowchart_grammar_matches_the_library() {
    for case in oracle() {
        let src = case["src"].as_str().expect("src");
        assert_eq!(graph_value(src), case["graph"], "graph of {src:?}");
    }
}
