//! Port of `packages/coding-agent/src/core/mini-read/`.
//!
//! The TypeScript package splits into `index.ts` (path-based entry points),
//! `languages.ts` (extension → grammar), `parser.ts` (WASM runtime loading),
//! `minify.ts` and `minify-edit.ts`. Of those, `parser.ts` has no counterpart
//! here: grammars are linked natively, so there is no runtime to initialize, no
//! wasm asset to locate and no `loadLanguage` to await — the user directive in
//! `plans/facts/rust-minify-reference.md` is precisely to take the native
//! reference implementation instead. `languages.ts` collapses into
//! `minify::language_for_path`, which is where the reference keeps the
//! extension table, and `index.ts` collapses into the path-based functions
//! re-exported below, since without grammar loading there is nothing left for a
//! separate layer to do.

pub mod minify;
pub mod minify_edit;

pub use minify::{
    IndentStyle, MinifyResult, detect_indent_style, expand_indentation, fragment_has_only_comments,
    language_for_path, literal_interior_lines, minify_for_path, minify_with_map,
    normalize_fragment_for_path,
};
pub use minify_edit::{MinifiedEdit, apply_minified_edit};
