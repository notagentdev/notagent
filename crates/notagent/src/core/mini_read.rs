pub mod minify;
pub mod minify_edit;

pub use minify::{
    IndentStyle, MinifyResult, detect_indent_style, expand_indentation, fragment_has_only_comments,
    language_for_path, literal_interior_lines, minify_for_path, minify_with_map,
    normalize_fragment_for_path,
};
pub use minify_edit::{MinifiedEdit, apply_minified_edit};
