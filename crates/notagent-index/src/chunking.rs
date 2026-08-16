//! AST-based chunk extraction for BM25 indexing.

use crate::bm25::{ChunkDocument, MAX_CHUNK_BYTES, MAX_CHUNK_LINES, chunk_id};
use crate::parser::SupportedLanguage;
use anyhow::Result;
use regex::Regex;
use std::path::Path;
use tree_sitter::{Node, Tree};

#[derive(Debug, Clone)]
pub struct ChunkerOptions {
    pub max_lines: usize,
    pub max_bytes: usize,
}

impl Default for ChunkerOptions {
    fn default() -> Self {
        Self {
            max_lines: MAX_CHUNK_LINES,
            max_bytes: MAX_CHUNK_BYTES,
        }
    }
}

pub fn chunk_file(path: &Path, source: &str, tree: Option<&Tree>) -> Result<Vec<ChunkDocument>> {
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && ext.eq_ignore_ascii_case("svelte")
    {
        return Ok(chunk_svelte(path, source));
    }

    let lang = match SupportedLanguage::from_path(path) {
        Some(lang) => lang,
        None => return Ok(fallback_chunks(path, source, "text")),
    };

    let mut chunks = Vec::new();
    if let Some(tree) = tree {
        let root = tree.root_node();
        let options = ChunkerOptions::default();
        visit_nodes(lang, root, source, false, &mut chunks, &options, path);
    } else {
        return Ok(fallback_chunks(path, source, &format_language(lang)));
    }

    Ok(chunks)
}

fn visit_nodes(
    lang: SupportedLanguage,
    node: Node,
    source: &str,
    parent_is_chunk: bool,
    out: &mut Vec<ChunkDocument>,
    options: &ChunkerOptions,
    path: &Path,
) {
    let kind = node.kind();
    let is_chunk = is_chunk_node(lang, kind) && !parent_is_chunk;

    if is_chunk {
        let name = symbol_name(node, source).unwrap_or_else(|| "<anonymous>".to_string());
        let range = node.range();
        let content = slice_source(source, range.start_byte, range.end_byte);
        let base_doc = ChunkDocument {
            id: chunk_id(
                &path.to_string_lossy(),
                &format_language(lang),
                range.start_point.row + 1,
                range.end_point.row + 1,
                &name,
            ),
            path: path.to_string_lossy().to_string(),
            language: format_language(lang),
            symbol: name,
            kind: kind.to_string(),
            range_start: range.start_point.row + 1,
            range_end: range.end_point.row + 1,
            content,
            tags: Vec::new(),
        };

        split_and_push(base_doc, out, options);
        return; // do not descend into children for v1
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_nodes(
            lang,
            child,
            source,
            is_chunk || parent_is_chunk,
            out,
            options,
            path,
        );
    }
}

fn is_chunk_node(lang: SupportedLanguage, kind: &str) -> bool {
    match lang {
        SupportedLanguage::Rust => matches!(
            kind,
            "function_item" | "struct_item" | "enum_item" | "trait_item" | "impl_item" | "mod_item"
        ),
        SupportedLanguage::JavaScript | SupportedLanguage::TypeScript | SupportedLanguage::Tsx => {
            matches!(
                kind,
                "function_declaration"
                    | "class_declaration"
                    | "method_definition"
                    | "generator_function_declaration"
                    | "lexical_declaration"
                    | "variable_declaration"
            )
        }
        SupportedLanguage::Python => matches!(kind, "function_definition" | "class_definition"),
        SupportedLanguage::Java => matches!(kind, "class_declaration" | "method_declaration"),
        SupportedLanguage::Go => {
            matches!(
                kind,
                "function_declaration" | "method_declaration" | "type_declaration"
            )
        }
        _ => false,
    }
}

fn symbol_name(node: Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "type_identifier" | "property_identifier" => {
                match child.utf8_text(source.as_bytes()) {
                    Ok(text) => return Some(text.to_string()),
                    Err(e) => {
                        tracing::warn!(
                            "[index] Failed to get UTF-8 text for chunk symbol {}: {}",
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

fn slice_source(source: &str, start: usize, end: usize) -> String {
    source.get(start..end).unwrap_or("").to_string()
}

fn split_and_push(doc: ChunkDocument, out: &mut Vec<ChunkDocument>, options: &ChunkerOptions) {
    if doc.content.len() <= options.max_bytes && line_count(&doc.content) <= options.max_lines {
        out.push(doc);
        return;
    }

    let lines: Vec<&str> = doc.content.lines().collect();
    let mut start = 0usize;
    while start < lines.len() {
        let end = usize::min(start + options.max_lines, lines.len());
        let content = lines[start..end].join("\n");
        if content.is_empty() {
            break;
        }
        let mut split_doc = doc.clone();
        split_doc.content = content;
        split_doc.range_start = doc.range_start + start;
        split_doc.range_end = doc.range_start + end.saturating_sub(1);
        split_doc.id = chunk_id(
            &split_doc.path,
            &split_doc.language,
            split_doc.range_start,
            split_doc.range_end,
            &split_doc.symbol,
        );
        out.push(split_doc);
        start = end;
    }
}

fn line_count(s: &str) -> usize {
    if s.is_empty() { 0 } else { s.lines().count() }
}

fn format_language(lang: SupportedLanguage) -> String {
    match lang {
        SupportedLanguage::Rust => "rust",
        SupportedLanguage::JavaScript => "javascript",
        SupportedLanguage::TypeScript => "typescript",
        SupportedLanguage::Tsx => "tsx",
        SupportedLanguage::Python => "python",
        SupportedLanguage::Go => "go",
        SupportedLanguage::Java => "java",
        SupportedLanguage::C => "c",
        SupportedLanguage::Cpp => "cpp",
        SupportedLanguage::Ruby => "ruby",
        SupportedLanguage::Json => "json",
        SupportedLanguage::Css => "css",
        SupportedLanguage::Html => "html",
        SupportedLanguage::Bash => "bash",
    }
    .to_string()
}

fn chunk_svelte(path: &Path, source: &str) -> Vec<ChunkDocument> {
    let mut chunks = Vec::new();
    let script_re = Regex::new(r"(?is)<script([^>]*)>(.*?)</script>")
        .expect("Static svelte script regex should be valid");
    let options = ChunkerOptions::default();
    let path_str = path.to_string_lossy().to_string();

    for cap in script_re.captures_iter(source) {
        let attrs = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let content = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let lang = if attrs.to_lowercase().contains("lang=\"ts\"")
            || attrs.to_lowercase().contains("lang='ts'")
            || attrs.to_lowercase().contains("typescript")
            || attrs.to_lowercase().contains("lang=\"tsx\"")
            || attrs.to_lowercase().contains("lang='tsx'")
        {
            "typescript"
        } else {
            "javascript"
        };

        let start_byte = cap.get(2).map(|m| m.start()).unwrap_or(0);
        let end_byte = cap.get(2).map(|m| m.end()).unwrap_or(0);
        let (start_line, end_line) = byte_range_to_lines(source, start_byte, end_byte);
        let base_doc = ChunkDocument {
            id: chunk_id(&path_str, lang, start_line, end_line, "<script>"),
            path: path_str.clone(),
            language: lang.to_string(),
            symbol: "<script>".to_string(),
            kind: "svelte_script".to_string(),
            range_start: start_line,
            range_end: end_line,
            content: content.to_string(),
            tags: vec!["svelte".to_string()],
        };
        split_and_push(base_doc, &mut chunks, &options);
    }

    chunks
}

fn byte_range_to_lines(source: &str, start: usize, end: usize) -> (usize, usize) {
    let mut start_line = 1usize;
    let mut end_line = 1usize;
    for (idx, ch) in source.char_indices() {
        if ch == '\n' {
            if idx < start {
                start_line += 1;
            }
            if idx < end {
                end_line += 1;
            }
        }
    }
    (start_line, end_line.max(start_line))
}

fn fallback_chunks(path: &Path, source: &str, language: &str) -> Vec<ChunkDocument> {
    let options = ChunkerOptions::default();
    let path_str = path.to_string_lossy().to_string();
    let base_doc = ChunkDocument {
        id: chunk_id(&path_str, language, 1, line_count(source).max(1), "<file>"),
        path: path_str,
        language: language.to_string(),
        symbol: "<file>".to_string(),
        kind: "file".to_string(),
        range_start: 1,
        range_end: line_count(source).max(1),
        content: source.to_string(),
        tags: Vec::new(),
    };

    let mut out = Vec::new();
    split_and_push(base_doc, &mut out, &options);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::MultiParser;

    #[test]
    fn test_chunk_python_function() {
        let source = "def hello():\n    return 1\n";
        let mut parser = MultiParser::new();
        let tree = parser.parse(source, SupportedLanguage::Python).unwrap();
        let path = Path::new("test.py");
        let chunks = chunk_file(path, source, Some(&tree)).unwrap();
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_js_function() {
        let source = "function hello() { return 1; }\n";
        let mut parser = MultiParser::new();
        let tree = parser.parse(source, SupportedLanguage::JavaScript).unwrap();
        let path = Path::new("test.js");
        let chunks = chunk_file(path, source, Some(&tree)).unwrap();
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_rust_fn() {
        let source = "fn hello() { println!(\"hi\"); }\n";
        let mut parser = MultiParser::new();
        let tree = parser.parse(source, SupportedLanguage::Rust).unwrap();
        let path = Path::new("test.rs");
        let chunks = chunk_file(path, source, Some(&tree)).unwrap();
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_java_class() {
        let source = "class A { void run() {} }\n";
        let mut parser = MultiParser::new();
        let tree = parser.parse(source, SupportedLanguage::Java).unwrap();
        let path = Path::new("Test.java");
        let chunks = chunk_file(path, source, Some(&tree)).unwrap();
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_go_function() {
        let source = "package main\n\nfunc run() {}\n";
        let mut parser = MultiParser::new();
        let tree = parser.parse(source, SupportedLanguage::Go).unwrap();
        let path = Path::new("main.go");
        let chunks = chunk_file(path, source, Some(&tree)).unwrap();
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_svelte_script() {
        let source = "<script lang=\"ts\">\nexport const x = 1;\n</script>\n";
        let path = Path::new("Component.svelte");
        let chunks = chunk_file(path, source, None).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].language, "typescript");
    }

    #[test]
    fn test_fallback_chunks_unknown_file() {
        let source = "line1\nline2\nline3\n";
        let path = Path::new("README.xyz");
        let chunks = chunk_file(path, source, None).unwrap();
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].language, "text");
    }

    #[test]
    fn test_chunk_mixed_language_files() {
        let fixtures = vec![
            (
                SupportedLanguage::TypeScript,
                "Component.ts",
                "export const x = 1;\n",
            ),
            (
                SupportedLanguage::Python,
                "script.py",
                "def run():\n    return 1\n",
            ),
            (
                SupportedLanguage::Java,
                "App.java",
                "class App { void run() {} }\n",
            ),
            (SupportedLanguage::Rust, "lib.rs", "pub fn run() {}\n"),
            (
                SupportedLanguage::Go,
                "main.go",
                "package main\n\nfunc run() {}\n",
            ),
        ];

        let mut parser = MultiParser::new();
        for (language, filename, source) in fixtures {
            let tree = parser.parse(source, language).unwrap();
            let path = Path::new(filename);
            let chunks = chunk_file(path, source, Some(&tree)).unwrap();
            assert!(!chunks.is_empty(), "no chunks for {filename}");
        }
    }
}
