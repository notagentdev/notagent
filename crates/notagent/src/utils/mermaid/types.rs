//! Port of `grok-mermaid/src/types.ts` (43 LOC).

/// Semantic class of a run of cells. The renderer never knows about colour;
/// consumers map these to their own theme.
///
/// - `Border`     box outlines, subgraph frames, compartment rules
/// - `Text`       node / participant / compartment labels
/// - `Edge`       connector lines and arrowheads
/// - `EdgeLabel`  text sitting on an edge
/// - `Title`      the `mermaid: <kind>` header of a source box
/// - `None`       blank filler
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cls {
    Border,
    Text,
    Edge,
    EdgeLabel,
    Title,
    None,
}

/// A run of adjacent cells sharing one semantic class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub cls: Cls,
}

/// A rendered diagram. `plain[i]` and `styled[i]` describe the same row:
/// `plain` is right-trimmed for display width and copy/paste, `styled` keeps
/// the run structure needed to colour it.
///
/// `width` is the display columns the widest row needs — the number to compare
/// against the space you have.
///
/// `warnings` lists source the flowchart grammar could not read and dropped.
/// They are advisory: the art is the best drawing of the source either way.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MermaidArt {
    pub plain: Vec<String>,
    pub styled: Vec<Vec<Span>>,
    pub width: usize,
    pub warnings: Vec<String>,
}
