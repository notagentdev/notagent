use anyhow::{Result, anyhow};
use std::path::Path;
use tree_sitter::{Language, Parser, Tree};

/// Supported programming languages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportedLanguage {
    Rust,
    JavaScript,
    TypeScript,
    Tsx,
    Python,
    Go,
    C,
    Cpp,
    Java,
    Ruby,
    Json,
    Css,
    Html,
    Bash,
}

impl SupportedLanguage {
    /// Get language from file extension
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "rs" => Some(Self::Rust),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "ts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "py" | "pyw" | "pyi" => Some(Self::Python),
            "go" => Some(Self::Go),
            "c" | "h" => Some(Self::C),
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Some(Self::Cpp),
            "java" => Some(Self::Java),
            "rb" | "rake" | "gemspec" => Some(Self::Ruby),
            "json" => Some(Self::Json),
            "css" | "scss" | "less" => Some(Self::Css),
            "html" | "htm" => Some(Self::Html),
            "sh" | "bash" | "zsh" => Some(Self::Bash),
            _ => None,
        }
    }

    /// Get language from file path
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(Self::from_extension)
    }

    /// Get tree-sitter language
    pub fn tree_sitter_language(&self) -> Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Css => tree_sitter_css::LANGUAGE.into(),
            Self::Html => tree_sitter_html::LANGUAGE.into(),
            Self::Bash => tree_sitter_bash::LANGUAGE.into(),
        }
    }
}

/// Multi-language parser
pub struct MultiParser {
    parser: Parser,
    current_language: Option<SupportedLanguage>,
}

impl MultiParser {
    /// Create a new multi-language parser
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            current_language: None,
        }
    }

    /// Set the current language
    pub fn set_language(&mut self, lang: SupportedLanguage) -> Result<()> {
        self.parser.set_language(&lang.tree_sitter_language())?;
        self.current_language = Some(lang);
        Ok(())
    }

    /// Parse source code for a given language
    pub fn parse(&mut self, source: &str, lang: SupportedLanguage) -> Result<Tree> {
        if self.current_language != Some(lang) {
            self.set_language(lang)?;
        }
        self.parser
            .parse(source, None)
            .ok_or_else(|| anyhow!("Failed to parse source code"))
    }

    /// Parse source code, auto-detecting language from file path
    pub fn parse_file(&mut self, source: &str, path: &Path) -> Result<Tree> {
        let lang = SupportedLanguage::from_path(path)
            .ok_or_else(|| anyhow!("Unsupported file type: {:?}", path))?;
        self.parse(source, lang)
    }
}

impl Default for MultiParser {
    fn default() -> Self {
        Self::new()
    }
}

// Keep the old RustParser for backwards compatibility
pub struct RustParser {
    parser: Parser,
}

impl RustParser {
    /// Create a new Rust parser
    pub fn new() -> Result<Self> {
        let mut parser = Parser::new();
        let language = tree_sitter_rust::LANGUAGE;
        parser.set_language(&language.into())?;
        Ok(Self { parser })
    }

    /// Parse source code into a syntax tree
    pub fn parse(&mut self, source: &str) -> Option<Tree> {
        self.parser.parse(source, None)
    }

    /// Incrementally update an existing tree after edits
    pub fn parse_with_old_tree(&mut self, source: &str, old_tree: &Tree) -> Option<Tree> {
        self.parser.parse(source, Some(old_tree))
    }

    /// Get the tree-sitter language
    pub fn language(&self) -> Language {
        tree_sitter_rust::LANGUAGE.into()
    }
}

impl Default for RustParser {
    fn default() -> Self {
        match Self::new() {
            Ok(parser) => parser,
            Err(err) => {
                tracing::error!("Failed to create Rust parser: {err}");
                Self {
                    parser: Parser::new(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_function() {
        let mut parser = RustParser::new().unwrap();
        let source = r#"
fn hello() {
    println!("Hello, world!");
}
"#;
        let tree = parser.parse(source).unwrap();
        let root = tree.root_node();
        assert_eq!(root.kind(), "source_file");
        assert!(root.child_count() > 0);
    }

    #[test]
    fn test_parse_struct() {
        let mut parser = RustParser::new().unwrap();
        let source = r#"
pub struct Person {
    name: String,
    age: u32,
}
"#;
        let tree = parser.parse(source).unwrap();
        let root = tree.root_node();
        assert_eq!(root.kind(), "source_file");
    }

    #[test]
    fn test_multi_parser_rust() {
        let mut parser = MultiParser::new();
        let source = "fn main() {}";
        let tree = parser.parse(source, SupportedLanguage::Rust).unwrap();
        assert_eq!(tree.root_node().kind(), "source_file");
    }

    #[test]
    fn test_multi_parser_javascript() {
        let mut parser = MultiParser::new();
        let source = "function hello() { return 42; }";
        let tree = parser.parse(source, SupportedLanguage::JavaScript).unwrap();
        assert_eq!(tree.root_node().kind(), "program");
    }

    #[test]
    fn test_multi_parser_python() {
        let mut parser = MultiParser::new();
        let source = "def hello():\n    return 42";
        let tree = parser.parse(source, SupportedLanguage::Python).unwrap();
        assert_eq!(tree.root_node().kind(), "module");
    }

    #[test]
    fn test_language_from_extension() {
        assert_eq!(
            SupportedLanguage::from_extension("rs"),
            Some(SupportedLanguage::Rust)
        );
        assert_eq!(
            SupportedLanguage::from_extension("ts"),
            Some(SupportedLanguage::TypeScript)
        );
        assert_eq!(
            SupportedLanguage::from_extension("tsx"),
            Some(SupportedLanguage::Tsx)
        );
        assert_eq!(
            SupportedLanguage::from_extension("py"),
            Some(SupportedLanguage::Python)
        );
        assert_eq!(
            SupportedLanguage::from_extension("go"),
            Some(SupportedLanguage::Go)
        );
        assert_eq!(SupportedLanguage::from_extension("unknown"), None);
    }
}
