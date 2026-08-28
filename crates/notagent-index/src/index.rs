//! Symbol index - in-memory storage for code symbols

use crate::parser::{MultiParser, SupportedLanguage};
use crate::symbol::{Symbol, SymbolKind};
use anyhow::Result;
use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tree_sitter::Tree;

/// In-memory symbol index
pub struct SymbolIndex {
    /// Symbols indexed by file path
    files: DashMap<PathBuf, FileIndex>,
    /// All symbols for quick search (flattened), keyed by the file the symbol
    /// lives in plus its qualified path.
    /// The qualified path alone is not an identity. Two files that both define
    /// `fn same()` produce the same `full_path`, so one used to overwrite the
    /// other, `symbol_count()` reported one instead of two, and removing
    /// either file deleted the entry both of them shared.
    all_symbols: DashMap<(PathBuf, String), Arc<Symbol>>,
}

/// Index for a single file
#[allow(dead_code)] // tree and content_hash are reserved for incremental parsing
struct FileIndex {
    symbols: Vec<Arc<Symbol>>,
    tree: Option<Tree>,
    content_hash: u64,
}

impl SymbolIndex {
    /// Create a new empty index
    pub fn new() -> Self {
        Self {
            files: DashMap::new(),
            all_symbols: DashMap::new(),
        }
    }

    /// Index a single file (auto-detect language from extension)
    pub fn index_file(&self, path: &Path, content: &str) -> Result<Vec<Arc<Symbol>>> {
        // Check if language is supported
        let lang = SupportedLanguage::from_path(path);
        if lang.is_none() {
            // Skip unsupported files silently
            return Ok(vec![]);
        }

        let mut parser = MultiParser::new();
        let tree = parser.parse_file(content, path)?;

        let symbols = self.extract_symbols(&tree, content, path);

        // Store in file index
        let file_index = FileIndex {
            symbols: symbols.clone(),
            tree: Some(tree),
            content_hash: Self::hash_content(content),
        };

        // Update global symbol map
        for sym in &symbols {
            self.all_symbols
                .insert((sym.file_path.clone(), sym.full_path.clone()), sym.clone());
        }

        self.files.insert(path.to_path_buf(), file_index);

        Ok(symbols)
    }

    /// Index a file using an already-parsed tree (avoids double parsing)
    pub fn index_file_with_tree(
        &self,
        path: &Path,
        content: &str,
        tree: &Tree,
    ) -> Vec<Arc<Symbol>> {
        let symbols = self.extract_symbols(tree, content, path);

        // Store in file index
        let file_index = FileIndex {
            symbols: symbols.clone(),
            tree: None, // Don't store tree to avoid Send issues with rayon
            content_hash: Self::hash_content(content),
        };

        // Update global symbol map
        for sym in &symbols {
            self.all_symbols
                .insert((sym.file_path.clone(), sym.full_path.clone()), sym.clone());
        }

        self.files.insert(path.to_path_buf(), file_index);

        symbols
    }

    /// Remove a file from the index
    pub fn remove_file(&self, path: &Path) {
        if let Some((_, file_index)) = self.files.remove(path) {
            for sym in file_index.symbols {
                self.all_symbols
                    .remove(&(sym.file_path.clone(), sym.full_path.clone()));
            }
        }
    }

    /// Get all symbols in the index
    pub fn all_symbols(&self) -> Vec<Arc<Symbol>> {
        self.all_symbols
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get symbols for a specific file
    pub fn symbols_for_file(&self, path: &Path) -> Vec<Arc<Symbol>> {
        self.files
            .get(path)
            .map(|f| f.symbols.clone())
            .unwrap_or_default()
    }

    /// Get symbol count
    pub fn symbol_count(&self) -> usize {
        self.all_symbols.len()
    }

    /// Get file count
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Insert a symbol directly (for loading from cache)
    pub fn insert_symbol(&self, symbol: Arc<Symbol>) {
        self.all_symbols
            .insert((symbol.file_path.clone(), symbol.full_path.clone()), symbol);
    }

    /// Restores one file's symbols from the cache, rebuilding its per-file
    /// entry alongside the flat map.
    /// Loading symbol by symbol left `files` empty, so a cache-loaded index
    /// looked complete to a search but `remove_file` found nothing to remove
    /// — a deleted file's symbols stayed findable for the rest of the run.
    pub fn restore_file(&self, path: &Path, symbols: Vec<Arc<Symbol>>, content_hash: u64) {
        for sym in &symbols {
            self.all_symbols
                .insert((sym.file_path.clone(), sym.full_path.clone()), sym.clone());
        }
        self.files.insert(
            path.to_path_buf(),
            FileIndex {
                symbols,
                tree: None,
                content_hash,
            },
        );
    }

    /// Every file the index currently holds.
    pub fn indexed_paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|entry| entry.key().clone()).collect()
    }

    /// Extract symbols from a parsed tree
    fn extract_symbols(&self, tree: &Tree, source: &str, file_path: &Path) -> Vec<Arc<Symbol>> {
        let mut symbols = Vec::new();
        let root = tree.root_node();

        self.visit_node(root, source, file_path, None, &mut symbols);

        symbols
    }

    /// Recursively visit AST nodes and extract symbols
    fn visit_node(
        &self,
        node: tree_sitter::Node,
        source: &str,
        file_path: &Path,
        parent: Option<&str>,
        symbols: &mut Vec<Arc<Symbol>>,
    ) {
        // Check if this node is a symbol we care about
        if let Some(kind) = SymbolKind::from_node_kind(node.kind())
            && let Some(name) = self.get_symbol_name(node, source)
        {
            let mut symbol = Symbol::new(
                name.clone(),
                kind,
                file_path.to_path_buf(),
                node.start_position().row..node.end_position().row,
                node.byte_range(),
            );

            if let Some(p) = parent {
                symbol = symbol.with_parent(p.to_string());
            }

            // Extract visibility
            if let Some(vis) = self.get_visibility(node, source) {
                symbol = symbol.with_visibility(vis);
            }

            let current_name = symbol.full_path.clone();
            symbols.push(Arc::new(symbol));

            // Visit children with this as parent (for impl blocks, etc.)
            let new_parent = if kind == SymbolKind::Impl || kind == SymbolKind::Mod {
                Some(current_name.as_str())
            } else {
                parent
            };

            for child in node.children(&mut node.walk()) {
                self.visit_node(child, source, file_path, new_parent, symbols);
            }
            return;
        }

        // Continue visiting children
        for child in node.children(&mut node.walk()) {
            self.visit_node(child, source, file_path, parent, symbols);
        }
    }

    /// Get the name of a symbol from a node
    fn get_symbol_name(&self, node: tree_sitter::Node, source: &str) -> Option<String> {
        // For most nodes, look for an identifier child
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "identifier" | "type_identifier" | "generic_type" => {
                    match child.utf8_text(source.as_bytes()) {
                        Ok(text) => return Some(text.to_string()),
                        Err(e) => {
                            tracing::warn!(
                                "[index] Failed to get UTF-8 text for node {}: {}",
                                child.kind(),
                                e
                            );
                            return None;
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Get visibility modifier
    fn get_visibility(&self, node: tree_sitter::Node, source: &str) -> Option<String> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "visibility_modifier" {
                match child.utf8_text(source.as_bytes()) {
                    Ok(text) => return Some(text.to_string()),
                    Err(e) => {
                        tracing::warn!(
                            "[index] Failed to get UTF-8 text for visibility modifier: {}",
                            e
                        );
                        return None;
                    }
                }
            }
        }
        None
    }

    /// Simple content hash
    fn hash_content(content: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }
}

impl Default for SymbolIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_named_symbols_in_different_files_both_survive() {
        let index = SymbolIndex::new();
        let a = Path::new("/w/a.rs");
        let b = Path::new("/w/b.rs");
        index.index_file(a, "fn same() {}").unwrap();
        index.index_file(b, "fn same() {}").unwrap();

        assert_eq!(
            index.symbol_count(),
            2,
            "one entry per file, not one shared"
        );
        assert_eq!(index.file_count(), 2);

        // Removing one file must not take the other's symbol with it.
        index.remove_file(a);
        assert_eq!(index.symbol_count(), 1);
        assert_eq!(index.symbols_for_file(b).len(), 1);
    }

    #[test]
    fn a_cache_restored_file_can_still_be_removed() {
        let index = SymbolIndex::new();
        let path = Path::new("/w/cached.rs");
        let symbols = SymbolIndex::new()
            .index_file(path, "fn cached() {}")
            .unwrap();
        assert!(!symbols.is_empty(), "fixture yields at least one symbol");

        index.restore_file(path, symbols, 42);
        assert_eq!(index.file_count(), 1, "the per-file entry is rebuilt");
        assert!(index.symbol_count() >= 1);

        index.remove_file(path);
        assert_eq!(
            index.symbol_count(),
            0,
            "removal reaches cache-loaded symbols"
        );
    }

    #[test]
    fn test_index_simple_file() {
        let index = SymbolIndex::new();
        let source = r#"
pub fn hello() {
    println!("Hello!");
}

struct Person {
    name: String,
}

impl Person {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}
"#;
        let path = Path::new("test.rs");
        let symbols = index.index_file(path, source).unwrap();

        assert!(symbols.len() >= 3); // hello, Person, new

        // Check function was found
        let hello = symbols.iter().find(|s| s.name == "hello");
        assert!(hello.is_some());
        assert_eq!(hello.unwrap().kind, SymbolKind::Function);
    }

    #[test]
    fn test_remove_file() {
        let index = SymbolIndex::new();
        let source = "fn test() {}";
        let path = Path::new("test.rs");

        index.index_file(path, source).unwrap();
        assert_eq!(index.file_count(), 1);

        index.remove_file(path);
        assert_eq!(index.file_count(), 0);
    }
}
