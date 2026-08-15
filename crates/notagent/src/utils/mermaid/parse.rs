//! Port of `grok-mermaid/src/parse.ts` (1 150 LOC).
//!
//! Complete: statement splitting, the diagram-kind header test and all five
//! grammars (`graph`/`flowchart`, `stateDiagram`, `classDiagram`, `erDiagram`,
//! `sequenceDiagram`).

use super::graph::{
    ClassInfo, Dir, Edge, Graph, Head, LineKind, MAX_EDGES, MAX_GROUP_DEPTH, MAX_GROUPS,
    MAX_MEMBERS, MAX_NODES, Shape, empty_class_info, parse_dir,
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

// --------------------------------------------------------------------- state

/// `stateDiagram` / `stateDiagram-v2`.
pub fn parse_state(src: &str) -> Option<Graph> {
    let statements = statements_of(src);
    let kind = header_kind(&statements)?;
    if !kind.starts_with("statediagram") {
        return None;
    }

    let mut graph = Graph::new(Dir::Down);
    let mut in_note = false;

    for st in statements.iter().skip(1) {
        if in_note {
            if ascii_lower(st) == "end note" {
                in_note = false;
            }
            continue;
        }
        let first = ascii_lower(&first_word(st));
        if first == "direction" {
            graph.dir = parse_dir(words(st).get(1).copied().unwrap_or(""));
        } else if first == "note" {
            // A single-line `note ... : text` needs no terminator.
            if !st.contains(':') {
                in_note = true;
            }
        } else if first == "state" {
            parse_state_decl(st, &mut graph)?;
        } else if matches!(
            first.as_str(),
            "classdef" | "class" | "hide" | "scale" | "}" | "--"
        ) {
            // Styling and composite-state punctuation carry no layout meaning.
        } else if st.contains("-->") {
            parse_transition(st, &mut graph)?;
        } else {
            parse_state_desc(st, &mut graph)?;
        }
        if graph.over_cap {
            return None;
        }
    }

    (!graph.nodes.is_empty()).then_some(graph)
}

/// `state "Label" as id`, `state id <<choice>>`, or `state id {`.
fn parse_state_decl(st: &str, graph: &mut Graph) -> Option<()> {
    let rest = st["state".len()..].trim();
    let rest = rest.strip_suffix('{').unwrap_or(rest).trim();
    if rest.is_empty() {
        return Some(());
    }

    if let Some(body) = rest.strip_prefix('"') {
        let close = body.find('"')?;
        let label = &body[..close];
        let after = body[close + 1..].trim();
        let id = match after.strip_prefix("as") {
            Some(id) => id.trim(),
            None => label,
        };
        return graph
            .node_label(id, &decode_html_entities(label))
            .map(|_| ());
    }

    let mut shape = Shape::Round;
    let mut id = rest;
    let mut stereotyped = false;
    if let Some(pos) = rest.find("<<") {
        let stereo = rest[pos + 2..].trim_end();
        let stereo = stereo.strip_suffix(">>").unwrap_or(stereo).trim();
        if stereo == "choice" {
            shape = Shape::Diamond;
        }
        id = rest[..pos].trim();
        stereotyped = true;
    }
    if id.is_empty() || id.chars().any(char::is_whitespace) {
        return None;
    }
    let label = stereotyped.then_some(id).map(str::to_string);
    graph.node_index(id, label.as_deref(), shape).map(|_| ())
}

/// `A --> B: label`, including chains `A --> B --> C`.
fn parse_transition(st: &str, graph: &mut Graph) -> Option<()> {
    let mut rest = st.to_string();
    let mut prev: Option<usize> = None;

    while let Some((lhs, rhs)) = rest
        .split_once("-->")
        .map(|(lhs, rhs)| (lhs.to_string(), rhs.to_string()))
    {
        let from_id = lhs.trim_end().trim_end_matches('-').trim().to_string();
        let from = match prev {
            // Mid-chain: the source is the previous target, so nothing may precede.
            Some(prev) => {
                if !from_id.is_empty() {
                    return None;
                }
                prev
            }
            None => {
                if from_id.is_empty() {
                    return None;
                }
                state_endpoint(graph, &from_id, true)?
            }
        };

        let next_arrow = rhs.find("-->");
        let (to_part_raw, tail) = match next_arrow {
            Some(at) => (&rhs[..at], rhs[at..].to_string()),
            None => (rhs.as_str(), String::new()),
        };

        let (to_part, label) = match to_part_raw.split_once(':') {
            Some((head, rest)) => (head, non_empty(decode_html_entities(rest.trim()))),
            None => (to_part_raw, None),
        };

        let to_id = to_part
            .trim_start()
            .trim_start_matches('>')
            .trim_end()
            .trim_end_matches('-')
            .trim()
            .to_string();
        if to_id.is_empty() {
            return None;
        }
        let to = state_endpoint(graph, &to_id, false)?;

        if !graph.push_edge(Edge {
            from,
            to,
            label,
            head_to: Head::Arrow,
            head_from: Head::None,
            line: LineKind::Solid,
        }) {
            return Some(());
        }
        prev = Some(to);
        rest = tail;
    }
    Some(())
}

/// `[*]` is start or end depending on which side of the arrow it sits.
fn state_endpoint(graph: &mut Graph, id: &str, is_source: bool) -> Option<usize> {
    if id == "[*]" {
        let id = if is_source { "[*]start" } else { "[*]end" };
        return graph.node_index(id, Some("●"), Shape::Round);
    }
    graph.node_index(id, None, Shape::Round)
}

/// `id: description`, or a bare state name.
fn parse_state_desc(st: &str, graph: &mut Graph) -> Option<()> {
    if let Some((id, desc)) = st.split_once(':') {
        let id = id.trim();
        let desc = desc.trim();
        if id.is_empty() || id.chars().any(char::is_whitespace) || desc.is_empty() {
            return None;
        }
        return graph
            .node_label(id, &decode_html_entities(desc))
            .map(|_| ());
    }
    if st.chars().any(char::is_whitespace) {
        return None;
    }
    graph.node_index(st, None, Shape::Round).map(|_| ())
}

// --------------------------------------------------------------------- class

/// Relation operators, longest-first so `--|>` wins over `--`.
const CLASS_OPS: [(&str, Head, Head, LineKind); 14] = [
    ("<|--", Head::Triangle, Head::None, LineKind::Solid),
    ("--|>", Head::None, Head::Triangle, LineKind::Solid),
    ("<|..", Head::Triangle, Head::None, LineKind::Dotted),
    ("..|>", Head::None, Head::Triangle, LineKind::Dotted),
    ("*--", Head::DiamondFill, Head::None, LineKind::Solid),
    ("--*", Head::None, Head::DiamondFill, LineKind::Solid),
    ("o--", Head::DiamondOpen, Head::None, LineKind::Solid),
    ("--o", Head::None, Head::DiamondOpen, LineKind::Solid),
    ("<--", Head::Arrow, Head::None, LineKind::Solid),
    ("-->", Head::None, Head::Arrow, LineKind::Solid),
    ("<..", Head::Arrow, Head::None, LineKind::Dotted),
    ("..>", Head::None, Head::Arrow, LineKind::Dotted),
    ("--", Head::None, Head::None, LineKind::Solid),
    ("..", Head::None, Head::None, LineKind::Dotted),
];

const MAX_CLASS_OP: usize = 4;

/// `classDiagram` / `classDiagram-v2`.
pub fn parse_class(src: &str) -> Option<(Graph, Vec<ClassInfo>)> {
    let statements = statements_of(src);
    let kind = header_kind(&statements)?;
    if !kind.starts_with("classdiagram") {
        return None;
    }

    let mut graph = Graph::new(Dir::Down);
    let mut infos: Vec<ClassInfo> = Vec::new();
    let mut cur_class: Option<usize> = None;

    for st in statements.iter().skip(1) {
        if let Some(cur) = cur_class {
            if st == "}" {
                cur_class = None;
            } else {
                push_member(&mut infos[cur], st);
            }
            continue;
        }

        let first = ascii_lower(&first_word(st));
        if first == "direction" {
            graph.dir = parse_dir(words(st).get(1).copied().unwrap_or(""));
            continue;
        }
        if matches!(
            first.as_str(),
            "note"
                | "callback"
                | "click"
                | "link"
                | "style"
                | "cssclass"
                | "classdef"
                | "namespace"
                | "}"
        ) {
            continue;
        }
        if first == "class" {
            let rest = st["class".len()..].trim();
            let open = rest.ends_with('{');
            let name = if open {
                rest[..rest.len() - 1].trim()
            } else {
                rest
            };
            if name.is_empty() || name.chars().any(char::is_whitespace) {
                return None;
            }
            let idx = declare_class(&mut graph, &mut infos, name)?;
            if open {
                cur_class = Some(idx);
            }
            continue;
        }

        if let Some(body) = st.strip_prefix("<<") {
            let (annotation, name) = body.split_once(">>")?;
            let name = name.trim();
            if name.is_empty() || name.chars().any(char::is_whitespace) {
                return None;
            }
            let idx = declare_class(&mut graph, &mut infos, name)?;
            infos[idx].annotation = Some(annotation.trim().to_string());
            continue;
        }

        if let Some(rel) = parse_class_relation(st) {
            let f = declare_class(&mut graph, &mut infos, &rel.from)?;
            let t = declare_class(&mut graph, &mut infos, &rel.to)?;
            if graph.edges.len() >= MAX_EDGES {
                return None;
            }
            graph.edges.push(Edge {
                from: f,
                to: t,
                label: rel.label,
                head_to: rel.head_to,
                head_from: rel.head_from,
                line: rel.line,
            });
            continue;
        }

        if let Some((id, text)) = st.split_once(':') {
            let id = id.trim();
            let text = text.trim();
            if id.is_empty() || id.chars().any(char::is_whitespace) || text.is_empty() {
                return None;
            }
            let idx = declare_class(&mut graph, &mut infos, id)?;
            push_member(&mut infos[idx], text);
            continue;
        }
        return None;
    }

    if graph.nodes.is_empty() {
        return None;
    }
    sync_infos(&graph, &mut infos);
    Some((graph, infos))
}

/// Keep `infos` aligned with `graph.nodes`.
fn sync_infos(graph: &Graph, infos: &mut Vec<ClassInfo>) {
    while infos.len() < graph.nodes.len() {
        infos.push(empty_class_info());
    }
}

/// Declare a class, keeping `infos` aligned with `graph.nodes`.
fn declare_class(graph: &mut Graph, infos: &mut Vec<ClassInfo>, name: &str) -> Option<usize> {
    let idx = graph.node_index(name, None, Shape::Rect);
    sync_infos(graph, infos);
    idx
}

/// Add a member to the attribute or method compartment, eliding past the cap.
pub fn push_member(info: &mut ClassInfo, raw: &str) {
    if let Some(body) = raw.strip_prefix("<<") {
        if let Some((annotation, _)) = body.split_once(">>") {
            info.annotation = Some(annotation.trim().to_string());
        }
        return;
    }
    let member = decode_html_entities(&display_generics(raw.trim()));
    let list = if member.contains('(') {
        &mut info.methods
    } else {
        &mut info.attrs
    };
    if list.len() < MAX_MEMBERS {
        list.push(member);
    } else if list.len() == MAX_MEMBERS {
        list.push("…".to_string());
    }
}

struct ClassRelation {
    from: String,
    to: String,
    head_from: Head,
    head_to: Head,
    line: LineKind,
    label: Option<String>,
}

fn parse_class_relation(st: &str) -> Option<ClassRelation> {
    let chars: Vec<char> = st.chars().collect();
    let mut found: Option<(usize, &str, Head, Head, LineKind)> = None;

    'outer: for pos in 0..chars.len() {
        let tail: String = chars[pos..(pos + MAX_CLASS_OP).min(chars.len())]
            .iter()
            .collect();
        for &(op, head_from, head_to, line) in &CLASS_OPS {
            if !tail.starts_with(op) {
                continue;
            }
            // `o` is also an identifier character: skip a match glued to a name.
            if op.starts_with('o') && pos > 0 && is_id_char(chars[pos - 1]) {
                continue;
            }
            let after = chars.get(pos + op.chars().count());
            if op.ends_with('o') && after.is_some_and(|&c| is_id_char(c)) {
                continue;
            }
            found = Some((pos, op, head_from, head_to, line));
            break 'outer;
        }
    }
    let (pos, op, head_from, head_to, line) = found?;

    let lhs_raw: String = chars[..pos].iter().collect();
    let lhs_raw = lhs_raw.trim().to_string();
    let rhs_raw: String = chars[pos + op.chars().count()..].iter().collect();
    let rhs_raw = rhs_raw.trim().to_string();

    let (lhs, card_from) = strip_cardinality_suffix(&lhs_raw);
    let (rhs, card_to) = strip_cardinality_prefix(&rhs_raw);

    let (to_id, rel_label) = match rhs.split_once(':') {
        Some((to_id, label)) => (
            to_id.trim().to_string(),
            non_empty(decode_html_entities(label.trim())),
        ),
        None => (rhs.trim().to_string(), None),
    };

    if lhs.is_empty()
        || to_id.is_empty()
        || lhs.chars().any(char::is_whitespace)
        || to_id.chars().any(char::is_whitespace)
    {
        return None;
    }

    let parts = [card_from, rel_label.unwrap_or_default(), card_to];
    let label = non_empty(
        parts
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<String>>()
            .join(" "),
    );
    Some(ClassRelation {
        from: lhs,
        to: to_id,
        head_from,
        head_to,
        line,
        label,
    })
}

/// `Class "1"` — a quoted cardinality trailing the left-hand name.
fn strip_cardinality_suffix(s: &str) -> (String, String) {
    let t = s.trim_end();
    if let Some(rest) = t.strip_suffix('"')
        && let Some(q) = rest.rfind('"')
    {
        return (rest[..q].trim_end().to_string(), rest[q + 1..].to_string());
    }
    (t.to_string(), String::new())
}

/// `"0..*" Class` — a quoted cardinality leading the right-hand name.
fn strip_cardinality_prefix(s: &str) -> (String, String) {
    let t = s.trim_start();
    if let Some(rest) = t.strip_prefix('"')
        && let Some(q) = rest.find('"')
    {
        return (
            rest[q + 1..].trim_start().to_string(),
            rest[..q].to_string(),
        );
    }
    (t.to_string(), String::new())
}

/// Mermaid writes generics as `List~T~`; show them as `List<T>`.
fn display_generics(s: &str) -> String {
    let mut out = String::new();
    let mut open = false;
    for c in s.chars() {
        if c == '~' {
            out.push(if open { '>' } else { '<' });
            open = !open;
        } else {
            out.push(c);
        }
    }
    out
}

// ------------------------------------------------------------------------ ER

/// `erDiagram`.
pub fn parse_er(src: &str) -> Option<(Graph, Vec<ClassInfo>)> {
    let statements = statements_of(src);
    if header_kind(&statements)? != "erdiagram" {
        return None;
    }

    let mut graph = Graph::new(Dir::Down);
    let mut infos: Vec<ClassInfo> = Vec::new();
    let mut cur_entity: Option<usize> = None;

    for st in statements.iter().skip(1) {
        if let Some(cur) = cur_entity {
            if st == "}" {
                cur_entity = None;
            } else {
                push_er_attribute(&mut infos[cur], st);
            }
            continue;
        }

        if let Some((rel, label)) = split_er_relationship(st) {
            let tokens = words(&rel);
            if tokens.len() != 3 {
                return None;
            }
            let op = parse_er_op(tokens[1])?;
            let f = er_entity(&mut graph, &mut infos, tokens[0])?;
            let t = er_entity(&mut graph, &mut infos, tokens[2])?;
            if graph.edges.len() >= MAX_EDGES {
                return None;
            }
            let rel_label = label.map(|label| clean_label(&label)).unwrap_or_default();
            let parts = [op.0, rel_label, op.1];
            graph.edges.push(Edge {
                from: f,
                to: t,
                label: non_empty(
                    parts
                        .iter()
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .collect::<Vec<String>>()
                        .join(" "),
                ),
                head_to: Head::None,
                head_from: Head::None,
                line: op.2,
            });
            continue;
        }

        let open = st.ends_with('{');
        let decl = if open {
            st[..st.len() - 1].trim()
        } else {
            st.as_str()
        };
        if decl.is_empty() || words(decl).len() != 1 {
            return None;
        }
        let idx = er_entity(&mut graph, &mut infos, decl)?;
        if open {
            cur_entity = Some(idx);
        }
    }

    if graph.nodes.is_empty() {
        return None;
    }
    sync_infos(&graph, &mut infos);
    Some((graph, infos))
}

fn er_entity(graph: &mut Graph, infos: &mut Vec<ClassInfo>, token: &str) -> Option<usize> {
    let idx = match token.find('[') {
        Some(open) => {
            let id = &token[..open];
            let label = clean_label(token[open + 1..].trim_end_matches(']'));
            if id.is_empty() || label.is_empty() {
                return None;
            }
            graph.node_label(id, &label)
        }
        None => graph.node_index(token, None, Shape::Rect),
    }?;
    sync_infos(graph, infos);
    Some(idx)
}

fn split_er_relationship(st: &str) -> Option<(String, Option<String>)> {
    let (rel, label) = match st.split_once(':') {
        Some((rel, label)) => (rel.to_string(), Some(label.trim().to_string())),
        None => (st.to_string(), None),
    };
    words(&rel)
        .iter()
        .any(|t| parse_er_op(t).is_some())
        .then_some((rel, label))
}

/// A crow's-foot operator: two cardinality glyphs around `--` or `..`.
fn parse_er_op(tok: &str) -> Option<(String, String, LineKind)> {
    if tok.len() != 6 || !tok.is_ascii() {
        return None;
    }
    let line = match &tok[2..4] {
        "--" => LineKind::Solid,
        ".." => LineKind::Dotted,
        _ => return None,
    };
    let card_l = er_card(&tok[..2])?;
    let card_r = er_card(&tok[4..6])?;
    Some((card_l.to_string(), card_r.to_string(), line))
}

fn er_card(tok: &str) -> Option<&'static str> {
    match tok {
        "|o" | "o|" => Some("0..1"),
        "||" => Some("1"),
        "}o" | "o{" => Some("0..*"),
        "}|" | "|{" => Some("1..*"),
        _ => None,
    }
}

/// ER attributes are `type name`; a trailing quoted comment is dropped.
pub fn push_er_attribute(info: &mut ClassInfo, raw: &str) {
    let mut parts: Vec<&str> = Vec::new();
    for tok in words(raw) {
        if tok.starts_with('"') {
            break;
        }
        parts.push(tok);
    }
    if parts.is_empty() {
        return;
    }
    let line = decode_html_entities(&parts.join(" "));
    if info.attrs.len() < MAX_MEMBERS {
        info.attrs.push(line);
    } else if info.attrs.len() == MAX_MEMBERS {
        info.attrs.push("…".to_string());
    }
}

// ------------------------------------------------------------------ sequence

/// Arrowhead of a sequence message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqHead {
    Arrow,
    Cross,
}

/// Message operators, longest-first so `-->>` wins over `-->`.
const SEQ_OPS: [(&str, bool, SeqHead); 8] = [
    ("-->>", true, SeqHead::Arrow),
    ("->>", false, SeqHead::Arrow),
    ("--x", true, SeqHead::Cross),
    ("-x", false, SeqHead::Cross),
    ("--)", true, SeqHead::Arrow),
    ("-)", false, SeqHead::Arrow),
    ("-->", true, SeqHead::Arrow),
    ("->", false, SeqHead::Arrow),
];

const MAX_SEQ_OP: usize = 4;

/// Where a note attaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteAnchor {
    Over { from: usize, to: usize },
    Left { at: usize },
    Right { at: usize },
}

/// One drawn row of a sequence diagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeqItem {
    Message {
        from: usize,
        to: usize,
        text: Option<String>,
        dashed: bool,
        head: SeqHead,
    },
    Note {
        anchor: NoteAnchor,
        text: String,
    },
    Divider {
        text: String,
    },
}

/// Participants and items of a sequence diagram.
#[derive(Debug, Default)]
pub struct Sequence {
    pub labels: Vec<String>,
    pub index: std::collections::HashMap<String, usize>,
    pub items: Vec<SeqItem>,
}

impl Sequence {
    /// Look up or declare a participant; `None` past the node cap.
    pub fn participant(&mut self, id: &str, label: Option<&str>) -> Option<usize> {
        if let Some(&existing) = self.index.get(id) {
            if let Some(label) = label {
                self.labels[existing] = label.to_string();
            }
            return Some(existing);
        }
        if self.labels.len() >= MAX_NODES {
            return None;
        }
        self.index.insert(id.to_string(), self.labels.len());
        self.labels.push(label.unwrap_or(id).to_string());
        Some(self.labels.len() - 1)
    }
}

/// `sequenceDiagram`.
pub fn parse_sequence(src: &str) -> Option<Sequence> {
    let statements = statements_of(src);
    if header_kind(&statements)? != "sequencediagram" {
        return None;
    }

    let mut seq = Sequence::default();
    let mut autonumber = false;
    let mut msg_count = 0usize;
    // One entry per open block; `true` when it draws a divider on `end`.
    let mut blocks: Vec<bool> = Vec::new();

    for st in statements.iter().skip(1) {
        let first = first_word(st);
        let lower = ascii_lower(&first);

        if lower == "participant" || lower == "actor" {
            let rest = st[first.len()..].trim();
            if rest.is_empty() {
                return None;
            }
            match rest.split_once(" as ") {
                Some((id, label)) => seq.participant(id.trim(), Some(&clean_label(label)))?,
                None => seq.participant(rest, None)?,
            };
            continue;
        }
        if lower == "autonumber" {
            autonumber = true;
            continue;
        }
        if matches!(
            lower.as_str(),
            "activate"
                | "deactivate"
                | "create"
                | "destroy"
                | "title"
                | "acctitle"
                | "accdescr"
                | "links"
                | "link"
                | "properties"
        ) {
            continue;
        }
        if lower == "note" {
            let (text, anchor) = parse_note_anchor(st[first.len()..].trim(), &mut seq)?;
            if seq.items.len() >= MAX_EDGES {
                return None;
            }
            seq.items.push(SeqItem::Note { anchor, text });
            continue;
        }
        if matches!(
            lower.as_str(),
            "loop" | "alt" | "opt" | "par" | "critical" | "break" | "else" | "and" | "option"
        ) {
            if matches!(lower.as_str(), "else" | "and" | "option") {
                // A continuation only divides a block that opened one.
                if blocks.last() != Some(&true) {
                    continue;
                }
            } else {
                blocks.push(true);
            }
            if seq.items.len() >= MAX_EDGES {
                return None;
            }
            seq.items.push(SeqItem::Divider {
                text: decode_html_entities(st),
            });
            continue;
        }
        if lower == "rect" || lower == "box" {
            blocks.push(false);
            continue;
        }
        if lower == "end" {
            if blocks.pop() == Some(true) {
                if seq.items.len() >= MAX_EDGES {
                    return None;
                }
                seq.items.push(SeqItem::Divider {
                    text: "end".to_string(),
                });
            }
            continue;
        }

        let msg = parse_seq_message(st, &mut seq)?;
        let mut text = msg.2;
        if autonumber {
            msg_count += 1;
            text = Some(match text {
                Some(text) => format!("{msg_count}. {text}"),
                None => format!("{msg_count}."),
            });
        }
        if seq.items.len() >= MAX_EDGES {
            return None;
        }
        seq.items.push(SeqItem::Message {
            from: msg.0,
            to: msg.1,
            text,
            dashed: msg.3,
            head: msg.4,
        });
    }

    (!seq.labels.is_empty()).then_some(seq)
}

fn parse_note_anchor(rest: &str, seq: &mut Sequence) -> Option<(String, NoteAnchor)> {
    let lower = ascii_lower(rest);
    let (kind, ids_and_text) = if lower.starts_with("over ") {
        ("over", &rest["over ".len()..])
    } else if lower.starts_with("left of ") {
        ("left", &rest["left of ".len()..])
    } else if lower.starts_with("right of ") {
        ("right", &rest["right of ".len()..])
    } else {
        return None;
    };

    let (ids, text) = ids_and_text.split_once(':')?;
    let text = decode_html_entities(text.trim());
    let parts: Vec<&str> = ids
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let first = *parts.first()?;
    let a = seq.participant(first, None)?;

    if kind != "over" {
        let anchor = if kind == "left" {
            NoteAnchor::Left { at: a }
        } else {
            NoteAnchor::Right { at: a }
        };
        return Some((text, anchor));
    }
    let mut b = a;
    if let Some(second) = parts.get(1) {
        b = seq.participant(second, None)?;
    }
    Some((
        text,
        NoteAnchor::Over {
            from: a.min(b),
            to: a.max(b),
        },
    ))
}

fn parse_seq_message(
    st: &str,
    seq: &mut Sequence,
) -> Option<(usize, usize, Option<String>, bool, SeqHead)> {
    let chars: Vec<char> = st.chars().collect();
    let mut found: Option<(usize, &str, bool, SeqHead)> = None;
    'outer: for pos in 0..chars.len() {
        let tail: String = chars[pos..(pos + MAX_SEQ_OP).min(chars.len())]
            .iter()
            .collect();
        for &(op, dashed, head) in &SEQ_OPS {
            if tail.starts_with(op) {
                found = Some((pos, op, dashed, head));
                break 'outer;
            }
        }
    }
    let (pos, op, dashed, head) = found?;

    let from_id: String = chars[..pos].iter().collect();
    let from_id = from_id.trim().to_string();
    if from_id.is_empty() {
        return None;
    }
    // `+`/`-` activate and deactivate the target; they carry no layout meaning.
    let rest: String = chars[pos + op.chars().count()..].iter().collect();
    let rest = rest.trim_start().trim_start_matches(['+', '-']).to_string();

    let (to_id, text) = match rest.split_once(':') {
        Some((to_id, text)) => (
            to_id.trim().to_string(),
            non_empty(decode_html_entities(text.trim())),
        ),
        None => (rest.trim().to_string(), None),
    };
    if to_id.is_empty() {
        return None;
    }

    let from = seq.participant(&from_id, None)?;
    let to = seq.participant(&to_id, None)?;
    Some((from, to, text, dashed, head))
}
