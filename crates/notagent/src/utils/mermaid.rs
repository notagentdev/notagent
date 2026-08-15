//! Substitute for the npm package `grok-mermaid` 0.2.2 (4 546 LOC), which
//! `modes/interactive/components/mermaid.ts` uses to draw Mermaid sources as
//! Unicode box art.
//!
//! Ownership handed to workstream A with interface request O-6; the master
//! plan lists the package under the substitutions, and its consumer is A's
//! component. The port covers the surface that consumer uses — `render`,
//! `MermaidArt`, `Span`, `Cls` — and keeps the module layout of the original
//! so the two stay comparable.
//!
//! One deviation runs through the whole port (class 3): `width-data.ts` is a
//! generated table of per-code-point widths, and its header says it comes from
//! the Rust `unicode-width` crate. The port calls that crate directly instead
//! of carrying the generated copy, and takes grapheme clusters from
//! `unicode-segmentation` where TypeScript uses `Intl.Segmenter` — both sides
//! of UAX #29.

pub mod ansi;
pub mod canvas;
pub mod graph;
pub mod labels;
pub mod layout;
pub mod layout_seq;
pub mod parse;
pub mod source_box;
pub mod types;
pub mod width;

pub use types::{Cls, MermaidArt, Span};

/// Render a Mermaid source block as Unicode box-drawing art.
///
/// Port of `render` (`grok-mermaid/src/index.ts:38-46`). Supported:
/// `graph`/`flowchart` (including `subgraph`), `stateDiagram`, `classDiagram`,
/// `erDiagram` and `sequenceDiagram`.
///
/// The diagram is laid out at whatever size it needs; `art.width` reports the
/// columns that turned out to be. Deciding what to do when that exceeds the
/// space at hand is the caller's — [`source_box::source_box`] is the usual
/// answer.
///
/// `None` means there is no art to show: blank input, a syntax error, a
/// diagram type this renderer does not draw, or one large enough that laying it
/// out is refused. [`parse::diagram_kind`] separates the middle two.
///
/// Rendering is best-effort. A flowchart keeps whatever parsed; the stricter
/// grammars additionally get one retry without their final line, which is what
/// keeps a streaming diagram on screen while its last statement is half-typed.
/// Everything given up on is listed in `art.warnings` — advisory only, never a
/// reason to withhold the art.
pub fn render(src: &str) -> Option<types::MermaidArt> {
    let src = labels::strip_controls(src);
    if src.trim().is_empty() {
        return None;
    }
    let drawn = attempt(&src)?;
    let lines = drawn.canvas.to_lines();
    Some(types::MermaidArt {
        plain: lines.plain,
        styled: lines.styled,
        width: lines.width,
        warnings: drawn.warnings,
    })
}

struct Drawn {
    canvas: canvas::Canvas,
    warnings: Vec<String>,
}

/// Draw `src`, retrying once without its last line if the grammar rejects it.
///
/// State, class, ER and sequence fail a whole diagram on one unreadable
/// statement, and while a source is streaming its last line is usually still
/// being typed — so without this a diagram alternates with the source box all
/// the way in. Only the final line is dropped, and doing so is always reported,
/// so a finished document with a bad last line still says what it lost rather
/// than quietly rendering short.
fn attempt(src: &str) -> Option<Drawn> {
    if let Some(drawn) = draw(src) {
        return Some(drawn);
    }

    let body = src.trim_end();
    let cut = body.rfind('\n')?;
    let salvaged = draw(&body[..cut])?;

    let dropped = body[cut + 1..].trim();
    let mut warnings = salvaged.warnings;
    warnings.push(format!("dropped, unreadable final line: \"{dropped}\""));
    Some(Drawn {
        canvas: salvaged.canvas,
        warnings,
    })
}

/// Dispatch on the declared diagram type; `None` means nothing was drawn.
fn draw(src: &str) -> Option<Drawn> {
    let plain = |canvas: layout::CanvasResult| {
        canvas.map(|canvas| Drawn {
            canvas,
            warnings: Vec::new(),
        })
    };

    match parse::diagram_kind(src)? {
        parse::DiagramKind::Flowchart => {
            let graph = parse::parse_graph(src)?;
            let canvas = if graph.groups.is_empty() {
                layout::layout_flowchart(&graph)
            } else {
                layout::layout_grouped(&graph)
            }?;
            Some(Drawn {
                canvas,
                warnings: graph.warnings,
            })
        }
        parse::DiagramKind::State => plain(layout::layout_flowchart(&parse::parse_state(src)?)),
        parse::DiagramKind::Class => {
            let (graph, infos) = parse::parse_class(src)?;
            plain(layout::layout_class(&graph, &infos))
        }
        parse::DiagramKind::Er => {
            let (graph, infos) = parse::parse_er(src)?;
            plain(layout::layout_class(&graph, &infos))
        }
        parse::DiagramKind::Sequence => {
            plain(layout_seq::layout_sequence(&parse::parse_sequence(src)?))
        }
    }
}
