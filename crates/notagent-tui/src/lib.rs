//! Differenzielle Terminal-Rendering-Engine und Komponenten.
//!
//! 1:1-Port von `packages/tui` (siehe crates/notagent-tui/PARITY.md).
//! Das Export-Set spiegelt `packages/tui/src/index.ts`.

pub mod terminal;
pub mod tui;

pub use terminal::{InputHandler, ResizeHandler, Terminal};
pub use tui::{CURSOR_MARKER, Component, ComponentRef, Focusable};
