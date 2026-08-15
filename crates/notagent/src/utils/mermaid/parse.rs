//! Port of the flowchart half of `grok-mermaid/src/parse.ts` (lines 1–446 of
//! 1 150): statement splitting, the diagram-kind header test and the
//! `graph`/`flowchart` grammar.
//!
//! The four stricter grammars of that file (`parseState`, `parseClass`,
//! `parseEr`, `parseSequence`) are not ported yet; `diagram_kind` already
//! recognises them, so the gap is visible rather than silent.

use super::graph::{
    Dir, Edge, Graph, Head, LineKind, MAX_GROUP_DEPTH, MAX_GROUPS, Shape, parse_dir,
};
use super::labels::{ascii_lower, clean_label, decode_html_entities, is_id_char};

// ---------------------------------------------------------------- statements

fn flush_statement(cur: &mut String, out: &mut Vec<String>) {
    let trimmed = cur.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    cur.clear();
}

/// Split one source line into statements on `;`, stopping at a `%%` comment.
///
/// Quoted spans are opaque, so a label may contain `;` and `%%`.
pub fn split_statements(line: &str, out: &mut Vec<String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if in_quotes {
            if character == '"' {
                in_quotes = false;
            }
            cur.push(character);
        } else if character == '"' {
            in_quotes = true;
            cur.push(character);
        } else if character == '%' && chars.get(index + 1) == Some(&'%') {
            break;
        } else if character == ';' {
            flush_statement(&mut cur, out);
        } else {
            cur.push(character);
        }
        index += 1;
    }
    flush_statement(&mut cur, out);
}

/// All statements in a source block, in order.
pub fn statements_of(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in super::labels::src_lines(src) {
        split_statements(&line, &mut out);
    }
    out
}

fn first_word(value: &str) -> String {
    value.split_whitespace().next().unwrap_or("").to_string()
}

fn words(value: &str) -> Vec<&str> {
    value.split_whitespace().collect()
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

/// Diagram kind from the header statement, lowercased.
fn header_kind(statements: &[String]) -> Option<String> {
    let header = statements.first()?;
    let kind = first_word(header);
    (!kind.is_empty()).then(|| ascii_lower(&kind))
}

/// A diagram type this renderer draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramKind {
    Flowchart,
    State,
    Class,
    Er,
    Sequence,
}

/// The kind of diagram `src` declares, or `None` if its header names no type
/// this renderer draws.
///
/// Reads the header only — it says nothing about whether the body parses.
pub fn diagram_kind(src: &str) -> Option<DiagramKind> {
    let kind = header_kind(&statements_of(src))?;
    if kind == "graph" || kind == "flowchart" {
        return Some(DiagramKind::Flowchart);
    }
    if kind.starts_with("statediagram") {
        return Some(DiagramKind::State);
    }
    if kind.starts_with("classdiagram") {
        return Some(DiagramKind::Class);
    }
    if kind == "erdiagram" {
        return Some(DiagramKind::Er);
    }
    if kind == "sequencediagram" {
        return Some(DiagramKind::Sequence);
    }
    None
}

// ----------------------------------------------------------------- flowchart

pub fn parse_graph(src: &str) -> Option<Graph> {
    let statements = statements_of(src);
    let kind = header_kind(&statements)?;
    if kind != "graph" && kind != "flowchart" {
        return None;
    }

    let direction = words(&statements[0])
        .get(1)
        .map_or(Dir::Down, |token| parse_dir(token));
    let mut graph = Graph::new(direction);
    let mut stack: Vec<usize> = Vec::new();

    for statement in statements.iter().skip(1) {
        match ascii_lower(&first_word(statement)).as_str() {
            "subgraph" => {
                if graph.groups.len() >= MAX_GROUPS || stack.len() >= MAX_GROUP_DEPTH {
                    return None;
                }
                let rest = statement["subgraph".len()..].trim().to_string();
                let (id, label) = parse_subgraph_decl(&rest);
                graph.groups.push(super::graph::Group {
                    id,
                    label,
                    parent: stack.last().copied(),
                });
                stack.push(graph.groups.len() - 1);
                graph.cur_group = stack.last().copied();
                continue;
            }
            "end" => {
                stack.pop();
                graph.cur_group = stack.last().copied();
                continue;
            }
            "classdef" | "class" | "style" | "linkstyle" | "click" | "direction" => continue,
            _ => {}
        }
        parse_statement(statement, &mut graph);
        if graph.over_cap {
            return None;
        }
    }

    (!graph.nodes.is_empty()).then_some(graph)
}

/// `subgraph id[Title]`, `subgraph "Title"`, or a bare title.
fn parse_subgraph_decl(rest: &str) -> (String, String) {
    if rest.starts_with('"')
        && let Some(close) = rest[1..].find('"').map(|index| index + 1)
    {
        let label = rest[1..close].to_string();
        return (label.clone(), decode_html_entities(&label));
    }
    if let Some(open) = rest.find('[') {
        let id = rest[..open].trim().to_string();
        let label = clean_label(rest[open + 1..].trim_end_matches(']').trim());
        if !id.is_empty() && !label.is_empty() {
            return (id, label);
        }
    }
    (rest.to_string(), rest.to_string())
}

/// A chain of `node link node link node ...`, each link fanning out over `&`.
///
/// Parses as far as it can and keeps the prefix, matching upstream and
/// mermaid.js. Whatever it could not read is recorded in `graph.warnings`.
fn parse_statement(statement: &str, graph: &mut Graph) {
    let chars: Vec<char> = statement.chars().collect();
    let mut index;

    let Some(head) = parse_node_group(&chars, 0, graph) else {
        graph.warnings.push(format!(
            "dropped, does not start with a node: \"{statement}\""
        ));
        return;
    };
    let mut previous = head.group;
    index = head.next;

    loop {
        index = skip_spaces(&chars, index);
        if index >= chars.len() {
            break;
        }
        let Some(link) = parse_link(&chars, index) else {
            let rest: String = chars[index..].iter().collect();
            graph
                .warnings
                .push(format!("dropped, expected a link: \"{rest}\""));
            break;
        };
        index = skip_spaces(&chars, link.next);
        let Some(target) = parse_node_group(&chars, index, graph) else {
            graph
                .warnings
                .push(format!("dropped, link has no target: \"{statement}\""));
            break;
        };
        index = target.next;
        for from in &previous {
            for to in &target.group {
                // `A <-- B` reads right-to-left: swap the endpoints so the
                // arrow that was written on the left becomes a forward head.
                let reversed = link.left == Head::Arrow && link.right != Head::Arrow;
                let pushed = graph.push_edge(Edge {
                    from: if reversed { *to } else { *from },
                    to: if reversed { *from } else { *to },
                    label: link.label.clone(),
                    head_to: if reversed { Head::Arrow } else { link.right },
                    head_from: if reversed { link.right } else { link.left },
                    line: link.line,
                });
                if !pushed {
                    return;
                }
            }
        }
        previous = target.group;
    }
}

struct NodeGroup {
    group: Vec<usize>,
    next: usize,
}

/// One or more nodes joined by `&`, which fan out into a cross product.
fn parse_node_group(chars: &[char], start: usize, graph: &mut Graph) -> Option<NodeGroup> {
    let first = parse_node(chars, start, graph)?;
    let mut group = vec![first.index];
    let mut index = first.next;
    loop {
        let position = skip_spaces(chars, index);
        if chars.get(position) != Some(&'&') {
            break;
        }
        let next = parse_node(chars, position + 1, graph)?;
        group.push(next.index);
        index = next.next;
    }
    Some(NodeGroup { group, next: index })
}

fn skip_spaces(chars: &[char], mut index: usize) -> usize {
    while index < chars.len() && (chars[index] == ' ' || chars[index] == '\t') {
        index += 1;
    }
    index
}

struct ParsedNode {
    index: usize,
    next: usize,
}

fn parse_node(chars: &[char], start: usize, graph: &mut Graph) -> Option<ParsedNode> {
    let mut index = skip_spaces(chars, start);
    let id_start = index;
    while index < chars.len() && is_id_char(chars[index]) {
        index += 1;
    }
    if index == id_start {
        return None;
    }
    let id: String = chars[id_start..index].iter().collect();

    let shaped = read_shape_at(chars, index);
    if let Some(unclosed) = shaped.unclosed.as_deref() {
        graph.warnings.push(format!(
            "node \"{id}\": label is missing its closing `{unclosed}`"
        ));
    }
    let index = graph.node_index(&id, shaped.label.as_deref(), shaped.shape)?;
    Some(ParsedNode {
        index,
        next: shaped.after,
    })
}

/// What a shape bracket yielded. `unclosed` is set when it never closed.
struct Shaped {
    shape: Shape,
    label: Option<String>,
    after: usize,
    unclosed: Option<String>,
}

/// Dispatch on the bracket following an id to pick shape and closing token.
fn read_shape_at(chars: &[char], index: usize) -> Shaped {
    let character = chars.get(index).copied();
    let next = chars.get(index + 1).copied();
    match character {
        Some('[') => match next {
            Some('[') => read_shape(chars, index + 2, "]]", Shape::Rect),
            Some('(') => read_shape(chars, index + 2, ")]", Shape::Round),
            _ => read_shape(chars, index + 1, "]", Shape::Rect),
        },
        Some('(') => match next {
            Some('(') => read_shape(chars, index + 2, "))", Shape::Round),
            Some('[') => read_shape(chars, index + 2, "])", Shape::Round),
            _ => read_shape(chars, index + 1, ")", Shape::Round),
        },
        Some('{') => match next {
            Some('{') => read_shape(chars, index + 2, "}}", Shape::Diamond),
            _ => read_shape(chars, index + 1, "}", Shape::Diamond),
        },
        Some('>') => read_shape(chars, index + 1, "]", Shape::Rect),
        _ => Shaped {
            shape: Shape::Rect,
            label: None,
            after: index,
            unclosed: None,
        },
    }
}

/// Read label text up to `closer`.
///
/// Quoting is decided by the first non-space character: inside a quoted label
/// the closer is ignored until the quote closes, so `A["a] b"]` is one node.
/// An unquoted label ends at the first closer, so `A[5" pipe]` keeps its quote.
fn read_shape(chars: &[char], start: usize, closer: &str, shape: Shape) -> Shaped {
    let closer_chars: Vec<char> = closer.chars().collect();
    let mut probe = start;
    while chars.get(probe) == Some(&' ') || chars.get(probe) == Some(&'\t') {
        probe += 1;
    }
    let quoted = chars.get(probe) == Some(&'"');

    let mut index = start;
    let mut text = String::new();
    let mut in_quotes = false;
    while index < chars.len() {
        let character = chars[index];
        if quoted && character == '"' {
            in_quotes = !in_quotes;
            text.push(character);
            index += 1;
            continue;
        }
        if !in_quotes
            && index + closer_chars.len() <= chars.len()
            && chars[index..index + closer_chars.len()] == closer_chars[..]
        {
            return Shaped {
                shape,
                label: Some(clean_label(&text)),
                after: index + closer_chars.len(),
                unclosed: None,
            };
        }
        text.push(character);
        index += 1;
    }
    // Ran off the end still looking for the closer: everything after the
    // opening bracket became label text, so any link operator was swallowed.
    Shaped {
        shape,
        label: Some(clean_label(&text)),
        after: chars.len(),
        unclosed: Some(closer.to_string()),
    }
}

fn is_link_char(character: char) -> bool {
    matches!(character, '-' | '.' | '=' | '<' | '>')
}

struct Link {
    left: Head,
    right: Head,
    line: LineKind,
    label: Option<String>,
    next: usize,
}

/// Read a link operator and its label.
///
/// Labels come in two forms: `-->|text|` and the inline `-- text -->`, the
/// latter only when the first operator carried no head.
fn parse_link(chars: &[char], start: usize) -> Option<Link> {
    let mut index = skip_spaces(chars, start);
    let mut left = Head::None;
    // A leading `o`/`x` decorates the tail, but only directly before an operator.
    if matches!(chars.get(index), Some('o') | Some('x'))
        && matches!(chars.get(index + 1), Some('-') | Some('.') | Some('='))
    {
        left = if chars[index] == 'o' {
            Head::Circle
        } else {
            Head::Cross
        };
        index += 1;
    }

    let op_start = index;
    while index < chars.len() && is_link_char(chars[index]) {
        index += 1;
    }
    if index == op_start {
        return None;
    }
    let op1: String = chars[op_start..index].iter().collect();
    if left == Head::None && op1.starts_with('<') {
        left = Head::Arrow;
    }

    let mut line = line_kind(&op1);
    let mut right = if op1.contains('>') {
        Head::Arrow
    } else {
        Head::None
    };
    if right == Head::None
        && let Some(trailing) = trailing_head(chars, index)
    {
        right = trailing.head;
        index = trailing.next;
    }

    if chars.get(index) == Some(&'|') {
        index += 1;
        let label_start = index;
        while index < chars.len() && chars[index] != '|' {
            index += 1;
        }
        let label: String = chars[label_start..index].iter().collect();
        let label = clean_label(&label);
        if chars.get(index) == Some(&'|') {
            index += 1;
        }
        return Some(Link {
            left,
            right,
            line,
            label: non_empty(label),
            next: index,
        });
    }

    if right == Head::None {
        let text_start = skip_spaces(chars, index);
        let mut probe = text_start;
        while probe < chars.len() && !is_link_char(chars[probe]) {
            probe += 1;
        }
        if probe < chars.len() && probe > text_start && chars[probe] != '<' {
            let text: String = chars[text_start..probe].iter().collect();
            let op2_start = probe;
            while probe < chars.len() && is_link_char(chars[probe]) {
                probe += 1;
            }
            let op2: String = chars[op2_start..probe].iter().collect();
            if op2.contains('>') {
                right = Head::Arrow;
            } else if let Some(trailing) = trailing_head(chars, probe) {
                right = trailing.head;
                probe = trailing.next;
            }
            if line == LineKind::Solid {
                line = line_kind(&op2);
            }
            return Some(Link {
                left,
                right,
                line,
                label: non_empty(clean_label(&text)),
                next: probe,
            });
        }
    }

    Some(Link {
        left,
        right,
        line,
        label: None,
        next: index,
    })
}

fn line_kind(op: &str) -> LineKind {
    if op.contains('=') {
        return LineKind::Thick;
    }
    if op.contains('.') {
        return LineKind::Dotted;
    }
    LineKind::Solid
}

struct TrailingHead {
    head: Head,
    next: usize,
}

/// A trailing `o`/`x` head, only when followed by a statement boundary.
fn trailing_head(chars: &[char], index: usize) -> Option<TrailingHead> {
    let head = match chars.get(index) {
        Some('o') => Head::Circle,
        Some('x') => Head::Cross,
        _ => return None,
    };
    let after = chars.get(index + 1);
    let boundary = matches!(
        after,
        None | Some(' ') | Some('\t') | Some('|') | Some('&') | Some(';')
    );
    boundary.then_some(TrailingHead {
        head,
        next: index + 1,
    })
}
