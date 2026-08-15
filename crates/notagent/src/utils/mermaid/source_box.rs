//! The raw source in a framed box.
//!
//! Port of `grok-mermaid/src/source-box.ts` (89 LOC). What to show when
//! [`super::render`] returns `None`, or returns art too wide for the space at
//! hand. Both are the caller's call, so this is theirs to invoke — and theirs
//! to caption, since only they know whether some other view of the diagram
//! exists to point the reader at.

use super::labels::{src_lines, strip_controls};
use super::types::{Cls, MermaidArt, Span};
use super::width::{measured, string_width};

fn sat(a: usize, b: usize) -> usize {
    a.saturating_sub(b)
}

/// Frame `src` in a titled box, hard-wrapping its lines to `max_width` columns.
///
/// The result can still exceed `max_width`: the body wraps to
/// `max(8, max_width - 4)` and the ` mermaid: <kind> ` title is never
/// truncated, so a long first token sets a floor. Check `width` if it matters.
pub fn source_box(src: &str, max_width: Option<usize>) -> MermaidArt {
    let src = strip_controls(src);
    let header = src.split_whitespace().next().unwrap_or("diagram");
    let title = format!(" mermaid: {header} ");
    let limit = max_width.map(|max_width| sat(max_width, 4).max(8));

    let mut body: Vec<String> = Vec::new();
    let mut started = false;
    for line in src_lines(&src) {
        let line = line.trim_end();
        if !started && line.is_empty() {
            continue;
        }
        started = true;
        body.extend(chunk_line(line, limit));
    }

    let content_w = body
        .iter()
        .map(|line| string_width(line))
        .chain(std::iter::once(string_width(&title)))
        .max()
        .unwrap_or(0);
    let inner = content_w + 2;

    let mut plain: Vec<String> = Vec::new();
    let mut styled: Vec<Vec<Span>> = Vec::new();

    let rule = "─".repeat(sat(inner, string_width(&title)));
    plain.push(format!("╭{title}{rule}╮"));
    styled.push(vec![
        Span {
            text: "╭".to_string(),
            cls: Cls::Border,
        },
        Span {
            text: title.clone(),
            cls: Cls::Title,
        },
        Span {
            text: format!("{rule}╮"),
            cls: Cls::Border,
        },
    ]);

    for line in &body {
        let pad = " ".repeat(sat(content_w, string_width(line)));
        plain.push(format!("│ {line}{pad} │"));
        styled.push(vec![
            Span {
                text: "│ ".to_string(),
                cls: Cls::Border,
            },
            Span {
                text: line.clone(),
                cls: Cls::Text,
            },
            Span {
                text: format!("{pad} │"),
                cls: Cls::Border,
            },
        ]);
    }

    let bottom = format!("╰{}╯", "─".repeat(inner));
    plain.push(bottom.clone());
    styled.push(vec![Span {
        text: bottom,
        cls: Cls::Border,
    }]);

    MermaidArt {
        plain,
        styled,
        width: inner + 2,
        warnings: Vec::new(),
    }
}

/// Hard-break a line at `limit` columns, never splitting a wide glyph.
fn chunk_line(line: &str, limit: Option<usize>) -> Vec<String> {
    let Some(limit) = limit else {
        return vec![line.to_string()];
    };
    if string_width(line) <= limit {
        return vec![line.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0;
    for (c, cw) in measured(line) {
        if cur_w + cw > limit && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push_str(c);
        cur_w += cw;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
