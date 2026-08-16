//! Symbol types extracted from source code

use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::path::PathBuf;

/// Kind of symbol extracted from code
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Mod,
    Const,
    Static,
    Type,
    Macro,
}

impl SymbolKind {
    /// Convert tree-sitter node kind to SymbolKind
    pub fn from_node_kind(kind: &str) -> Option<Self> {
        match kind {
            "function_item" => Some(SymbolKind::Function),
            "struct_item" => Some(SymbolKind::Struct),
            "enum_item" => Some(SymbolKind::Enum),
            "trait_item" => Some(SymbolKind::Trait),
            "impl_item" => Some(SymbolKind::Impl),
            "mod_item" => Some(SymbolKind::Mod),
            "const_item" => Some(SymbolKind::Const),
            "static_item" => Some(SymbolKind::Static),
            "type_item" => Some(SymbolKind::Type),
            "macro_definition" => Some(SymbolKind::Macro),
            _ => None,
        }
    }

    /// Get display name for this kind
    pub fn display_name(&self) -> &'static str {
        match self {
            SymbolKind::Function => "fn",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Impl => "impl",
            SymbolKind::Mod => "mod",
            SymbolKind::Const => "const",
            SymbolKind::Static => "static",
            SymbolKind::Type => "type",
            SymbolKind::Macro => "macro",
        }
    }
}

/// A symbol extracted from source code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// Symbol name (e.g., "parse_file")
    pub name: String,
    /// Kind of symbol
    pub kind: SymbolKind,
    /// File path where symbol is defined
    pub file_path: PathBuf,
    /// Line range in file (0-indexed)
    pub line_range: Range<usize>,
    /// Byte range in file
    pub byte_range: Range<usize>,
    /// Parent symbol path (e.g., "MyStruct" for "MyStruct::new")
    pub parent: Option<String>,
    /// Full qualified path (e.g., "crate::module::MyStruct::new")
    pub full_path: String,
    /// Documentation comment if present
    pub doc_comment: Option<String>,
    /// Visibility (pub, pub(crate), etc.)
    pub visibility: Option<String>,
}

impl Symbol {
    /// Create a new symbol
    pub fn new(
        name: String,
        kind: SymbolKind,
        file_path: PathBuf,
        line_range: Range<usize>,
        byte_range: Range<usize>,
    ) -> Self {
        let full_path = name.clone();
        Self {
            name,
            kind,
            file_path,
            line_range,
            byte_range,
            parent: None,
            full_path,
            doc_comment: None,
            visibility: None,
        }
    }

    /// Set parent and update full path
    pub fn with_parent(mut self, parent: String) -> Self {
        self.full_path = format!("{}::{}", parent, self.name);
        self.parent = Some(parent);
        self
    }

    /// Set documentation comment
    pub fn with_doc(mut self, doc: String) -> Self {
        self.doc_comment = Some(doc);
        self
    }

    /// Set visibility
    pub fn with_visibility(mut self, vis: String) -> Self {
        self.visibility = Some(vis);
        self
    }

    /// Get display string for this symbol
    pub fn display(&self) -> String {
        let vis = self.visibility.as_deref().unwrap_or("");
        let vis_prefix = if vis.is_empty() { "" } else { " " };
        format!(
            "{}{}{} {} ({}:{})",
            vis,
            vis_prefix,
            self.kind.display_name(),
            self.name,
            self.file_path.display(),
            self.line_range.start + 1
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_kind_from_node() {
        assert_eq!(
            SymbolKind::from_node_kind("function_item"),
            Some(SymbolKind::Function)
        );
        assert_eq!(
            SymbolKind::from_node_kind("struct_item"),
            Some(SymbolKind::Struct)
        );
        assert_eq!(SymbolKind::from_node_kind("unknown"), None);
    }

    #[test]
    fn test_symbol_display() {
        let sym = Symbol::new(
            "hello".to_string(),
            SymbolKind::Function,
            PathBuf::from("src/main.rs"),
            0..10,
            0..100,
        )
        .with_visibility("pub".to_string());

        assert!(sym.display().contains("pub"));
        assert!(sym.display().contains("fn hello"));
    }
}
