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

pub mod canvas;
pub mod graph;
pub mod labels;
pub mod width;

pub mod parse;
pub mod types;

pub use types::{Cls, MermaidArt, Span};
