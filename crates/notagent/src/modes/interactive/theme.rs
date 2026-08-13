//! Theme system — port of `packages/coding-agent/src/modes/interactive/theme/`.

// `theme/theme.rs` mirrors `theme/theme.ts`: CONVENTIONS.md section 2 requires
// the Rust module path to equal the TypeScript file path.
#[allow(clippy::module_inception)]
pub mod theme;
pub mod theme_controller;
