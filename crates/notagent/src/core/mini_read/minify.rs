use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;

use tree_sitter::{Language, Node, Parser};

/// Resolves a tree-sitter grammar from a file extension. Returns `None` for
/// unsupported languages, in which case the caller should fall back to the
/// raw file content.
fn language_for_extension(ext: &str) -> Option<Language> {
    let lang = match ext {
        "rs" => tree_sitter_rust::LANGUAGE,
        "py" | "pyi" => tree_sitter_python::LANGUAGE,
        "js" | "mjs" | "cjs" | "jsx" => tree_sitter_javascript::LANGUAGE,
        "ts" | "mts" | "cts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX,
        "go" => tree_sitter_go::LANGUAGE,
        "java" => tree_sitter_java::LANGUAGE,
        "c" | "h" => tree_sitter_c::LANGUAGE,
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => tree_sitter_cpp::LANGUAGE,
        "rb" => tree_sitter_ruby::LANGUAGE,
        "sh" | "bash" | "zsh" => tree_sitter_bash::LANGUAGE,
        "css" => tree_sitter_css::LANGUAGE,
        "html" | "htm" => tree_sitter_html::LANGUAGE,
        "json" | "jsonc" => tree_sitter_json::LANGUAGE,
        _ => return None,
    };
    Some(lang.into())
}

/// Minification result enriched with the data needed to translate edits
/// expressed in minified space back onto the original source.
pub struct MinifyResult {
    /// The minified rendering, identical to what `minify_for_path` returns.
    pub text: String,
    /// Source byte offset for every byte of `text`. Bytes copied from the
    /// source map to their exact origin; synthetic bytes (indentation,
    /// collapsed whitespace runs, line separators) map to the start of the
    /// source region they stand in for.
    pub src_map: Vec<usize>,
    /// Sorted distinct indentation widths of the source, in columns. The
    /// minified depth `d` corresponds to `indent_widths[d]` — the inverse of
    /// the monotone re-indentation applied during rendering.
    pub indent_widths: Vec<usize>,
    /// Byte ranges of comment nodes that were removed (empty when
    /// `keep_comments` is true).
    pub comments: Vec<Range<usize>>,
}

/// Minifies `source` according to the grammar selected by the extension of
/// `path`. When `keep_comments` is true, comments survive minification
/// (whitespace around and inside them is still compacted like any other
/// token). Returns `None` when the language is unsupported or parsing fails
/// entirely; callers should then present the raw content instead.
pub fn minify_for_path(path: &Path, source: &str, keep_comments: bool) -> Option<String> {
    minify_with_map(path, source, keep_comments).map(|result| result.text)
}

/// Normalizes an edit fragment into the same compact representation used by
/// [`minify_for_path`]. When the fragment cannot be parsed by the target
/// language, only safe outer whitespace normalization is applied.
pub fn normalize_fragment_for_path(
    path: &Path,
    fragment: &str,
    keep_comments: bool,
) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let language = language_for_extension(&ext)?;
    match minify_source(fragment, &language, keep_comments) {
        Some(result) if !result.text.trim().is_empty() => Some(result.text),
        // Parsed but empty under this comment setting — e.g. a comment-only
        // fragment with keep_comments=false. There is no code-space form to
        // match, so report no normalization rather than the raw comment, which
        // the comment-stripped view never contains. The edit entry point
        // detects this and retries with keep_comments enabled.
        Some(_) => None,
        // Unparseable fragment: fall back to safe outer whitespace cleanup.
        None => Some(normalize_fragment_fallback(fragment)),
    }
}

/// Reports whether `fragment` consists solely of comments (and whitespace):
/// it has real content, yet minifying it with comments removed yields nothing.
/// Used by the edit entry point to auto-enable `keep_comments` for an edit
/// that targets only comment text.
pub fn fragment_has_only_comments(path: &Path, fragment: &str) -> bool {
    if fragment.trim().is_empty() {
        return false;
    }
    match language_for_path(path) {
        Some(language) => minify_source(fragment, &language, false)
            .map(|result| result.text.trim().is_empty())
            .unwrap_or(false),
        None => false,
    }
}

fn normalize_fragment_fallback(fragment: &str) -> String {
    fragment
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_matches('\n')
        .to_string()
}

/// Like [`minify_for_path`], but also returns the byte-level mapping back to
/// the original source, used by the minified write/patch tools.
pub fn minify_with_map(path: &Path, source: &str, keep_comments: bool) -> Option<MinifyResult> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let language = language_for_extension(&ext)?;
    minify_source(source, &language, keep_comments)
}

pub fn minify_source(
    source: &str,
    language: &Language,
    keep_comments: bool,
) -> Option<MinifyResult> {
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    let tree = parser.parse(source, None)?;

    let mut comments: Vec<Range<usize>> = Vec::new();
    let mut leaves: Vec<Range<usize>> = Vec::new();
    collect_spans(tree.root_node(), &mut comments, &mut leaves, keep_comments);

    // Blank out comment bytes in place (keeping newlines) so the later line
    // pass can treat them as ordinary whitespace without any byte offsets
    // shifting. Replacing whole node spans with ASCII spaces keeps the
    // buffer valid UTF-8.
    let mut bytes = source.as_bytes().to_vec();
    for span in &comments {
        for b in &mut bytes[span.clone()] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
    }

    // Tokens that span multiple lines (raw strings, heredocs, template
    // literals): every line they cover must be emitted untouched.
    let multiline: Vec<Range<usize>> = leaves
        .iter()
        .filter(|r| source.as_bytes()[r.start..r.end].contains(&b'\n'))
        .cloned()
        .collect();

    let (text, src_map, indent_widths) =
        render_lines(&bytes, &leaves, &multiline, source.ends_with('\n'));
    Some(MinifyResult {
        text,
        src_map,
        indent_widths,
        comments,
    })
}

/// Walks the tree in document order. Comment nodes are recorded whole
/// (without descending into them): either for removal, or — when kept — as
/// opaque tokens so whitespace inside them is never altered. All other leaf
/// tokens are recorded for the same reason.
fn collect_spans(
    root: Node,
    comments: &mut Vec<Range<usize>>,
    leaves: &mut Vec<Range<usize>>,
    keep_comments: bool,
) {
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        let is_comment = node.kind().to_ascii_lowercase().ends_with("comment");
        if is_comment {
            if !node.byte_range().is_empty() {
                if keep_comments {
                    leaves.push(node.byte_range());
                } else {
                    comments.push(node.byte_range());
                }
            }
        } else if node.child_count() == 0 && !node.byte_range().is_empty() {
            leaves.push(node.byte_range());
        }
        if !is_comment && cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

/// How a physical line participates in minification.
#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    /// Begins inside a multiline literal: emit byte-for-byte.
    Verbatim,
    /// A multiline literal starts on this line: the leading indentation is
    /// still code and gets re-indented, but nothing after it may be trimmed.
    OpensLiteral,
    /// Plain code line: trim, drop when blank, re-indent, collapse gaps.
    Code,
}

struct LineInfo {
    /// Byte range of the line content, excluding the trailing newline.
    range: Range<usize>,
    kind: LineKind,
    /// Indentation width in columns (tabs expand to the next multiple of 8,
    /// matching Python's tokenizer) for lines that get re-indented.
    indent_width: usize,
    /// Offset of the first non-indentation byte.
    content_start: usize,
    /// End of content after trailing-whitespace trimming (Code lines only).
    content_end: usize,
}

/// Output buffer that records, for every emitted byte, the source offset it
/// originates from (or stands in for).
struct MappedOutput {
    text: String,
    map: Vec<usize>,
}

impl MappedOutput {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            text: String::with_capacity(capacity),
            map: Vec::with_capacity(capacity),
        }
    }

    /// Appends a byte range copied verbatim from the source.
    fn push_copied(&mut self, bytes: &[u8], range: Range<usize>) {
        self.text
            .push_str(std::str::from_utf8(&bytes[range.clone()]).unwrap_or_default());
        self.map.extend(range);
    }

    /// Appends a synthetic ASCII character standing in for the source region
    /// starting at `src`.
    fn push_synthetic(&mut self, ch: char, src: usize) {
        self.text.push(ch);
        self.map.push(src);
    }
}

fn render_lines(
    bytes: &[u8],
    leaves: &[Range<usize>],
    multiline: &[Range<usize>],
    trailing_newline: bool,
) -> (String, Vec<usize>, Vec<usize>) {
    let mut infos: Vec<LineInfo> = Vec::new();
    let mut widths: BTreeSet<usize> = BTreeSet::new();

    let mut line_start = 0usize;
    // Pointer into `multiline`, advanced as line offsets grow.
    let mut ml_idx = 0usize;

    while line_start <= bytes.len() {
        let line_end = bytes[line_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| line_start + i);
        let end = line_end.unwrap_or(bytes.len());

        // Skip literal spans that end before this line.
        while ml_idx < multiline.len() && multiline[ml_idx].end <= line_start {
            ml_idx += 1;
        }
        let covers = |pos: usize| {
            multiline[ml_idx..]
                .iter()
                .take_while(|r| r.start <= pos)
                .any(|r| pos < r.end)
        };
        // The line begins inside a literal when its first byte is covered;
        // the literal continues onto the next line when the newline byte
        // itself is covered.
        let lead_inside = covers(line_start);
        let tail_inside = line_end.is_some_and(&covers);

        let mut width = 0usize;
        let mut cs = line_start;
        while cs < end {
            match bytes[cs] {
                b' ' => width += 1,
                b'\t' => width = (width / 8 + 1) * 8,
                _ => break,
            }
            cs += 1;
        }

        // A multiline literal opening exactly at the first non-whitespace
        // byte means the leading whitespace sits between tokens but still
        // belongs to the literal's layout (e.g. bash heredoc bodies, whose
        // grammar excludes leading spaces from the body node). Such lines
        // carry no code indentation, so keep them verbatim.
        let opens_at_content = multiline[ml_idx..]
            .iter()
            .take_while(|r| r.start <= cs)
            .any(|r| r.start == cs && cs < end);

        let kind = match (lead_inside || opens_at_content, tail_inside) {
            (true, _) => LineKind::Verbatim,
            (false, true) => LineKind::OpensLiteral,
            (false, false) => LineKind::Code,
        };

        let mut info = LineInfo {
            range: line_start..end,
            kind,
            indent_width: 0,
            content_start: line_start,
            content_end: end,
        };

        if kind != LineKind::Verbatim {
            let mut ce = end;
            if kind == LineKind::Code {
                while ce > cs && matches!(bytes[ce - 1], b' ' | b'\t' | b'\r') {
                    ce -= 1;
                }
            }
            info.indent_width = width;
            info.content_start = cs;
            info.content_end = ce;
            // Blank Code lines are dropped and must not influence the
            // indentation mapping.
            if cs < ce {
                widths.insert(width);
            }
        }

        infos.push(info);
        match line_end {
            Some(nl) => line_start = nl + 1,
            None => break,
        }
    }

    // Monotone re-indentation: the i-th smallest indentation width becomes i
    // spaces. Order and equality of widths are preserved, which is all that
    // indentation-sensitive grammars require.
    let depth_of = |width: usize| widths.iter().take_while(|&&w| w < width).count();

    let mut out = MappedOutput::with_capacity(bytes.len());
    let mut first = true;
    // Source offset of the end of the previously emitted line; anchors the
    // synthetic '\n' separators.
    let mut last_src_end = 0usize;
    for info in &infos {
        match info.kind {
            LineKind::Verbatim => {
                if !first {
                    out.push_synthetic('\n', last_src_end);
                }
                out.push_copied(bytes, info.range.clone());
                last_src_end = info.range.end;
                first = false;
            }
            LineKind::Code | LineKind::OpensLiteral => {
                if info.content_start >= info.content_end {
                    continue; // drop blank line
                }
                if !first {
                    out.push_synthetic('\n', last_src_end);
                }
                for _ in 0..depth_of(info.indent_width) {
                    out.push_synthetic(' ', info.range.start);
                }
                push_collapsed(
                    &mut out,
                    bytes,
                    info.content_start..info.content_end,
                    leaves,
                );
                last_src_end = info.content_end;
                first = false;
            }
        }
    }
    if trailing_newline && !out.text.is_empty() {
        out.push_synthetic('\n', last_src_end);
    }
    let widths: Vec<usize> = widths.into_iter().collect();
    (out.text, out.map, widths)
}

/// Copies `range` into `out`, shrinking each whitespace run that lies fully
/// outside every leaf token to a single space. Whitespace inside tokens
/// (string literals and the like) is preserved exactly.
fn push_collapsed(
    out: &mut MappedOutput,
    bytes: &[u8],
    range: Range<usize>,
    leaves: &[Range<usize>],
) {
    let mut i = range.start;
    while i < range.end {
        if matches!(bytes[i], b' ' | b'\t') {
            let run_start = i;
            while i < range.end && matches!(bytes[i], b' ' | b'\t') {
                i += 1;
            }
            if intersects_leaf(leaves, run_start, i) {
                out.push_copied(bytes, run_start..i);
            } else {
                out.push_synthetic(' ', run_start);
            }
        } else {
            let chunk_start = i;
            while i < range.end && !matches!(bytes[i], b' ' | b'\t') {
                i += 1;
            }
            out.push_copied(bytes, chunk_start..i);
        }
    }
}

/// Reports whether [start, end) overlaps any leaf span. `leaves` is sorted
/// and non-overlapping, so both starts and ends increase monotonically.
fn intersects_leaf(leaves: &[Range<usize>], start: usize, end: usize) -> bool {
    let idx = leaves.partition_point(|r| r.end <= start);
    leaves.get(idx).is_some_and(|r| r.start < end)
}

/// Resolves the tree-sitter grammar for a path, for callers outside this
/// module (minified editing).
pub fn language_for_path(path: &Path) -> Option<Language> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    language_for_extension(&ext)
}

/// Indentation character convention of a file.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum IndentStyle {
    Spaces,
    Tabs,
}

/// Detects whether a file is predominantly tab-indented. Columns are counted
/// with tabs expanding to multiples of 8, matching `render_lines`.
pub fn detect_indent_style(source: &str) -> IndentStyle {
    let mut tabs = 0usize;
    let mut spaces = 0usize;
    for line in source.lines() {
        match line.as_bytes().first() {
            Some(b'\t') => tabs += 1,
            Some(b' ') => spaces += 1,
            _ => {}
        }
    }
    if tabs > spaces {
        IndentStyle::Tabs
    } else {
        IndentStyle::Spaces
    }
}

/// The typical indentation increment of a file, derived from the distinct
/// indentation widths observed during minification. Falls back to 4 columns.
fn indent_step(widths: &[usize]) -> usize {
    let mut diffs: Vec<usize> = widths.windows(2).map(|w| w[1] - w[0]).collect();
    diffs.retain(|&d| d > 0);
    if diffs.is_empty() {
        return 4;
    }
    diffs.sort_unstable();
    diffs[diffs.len() / 2]
}

/// Maps a minified indentation depth back to a column width of the original
/// file: known depths use the observed width, deeper levels extrapolate by
/// the file's typical indentation step.
fn target_width(widths: &[usize], depth: usize) -> usize {
    if let Some(&w) = widths.get(depth) {
        return w;
    }
    let step = indent_step(widths);
    match widths.last() {
        Some(&last) => last + step * (depth - (widths.len() - 1)),
        None => step * depth,
    }
}

/// Per-line flags for lines whose first byte sits inside a multiline literal
/// token; such lines must never be re-indented. The fragment is parsed
/// tolerantly — snippets that do not form a complete compilation unit still
/// yield usable string/heredoc tokens.
pub fn literal_interior_lines(text: &str, language: &Language) -> Vec<bool> {
    let line_count = text.split('\n').count();
    let mut flags = vec![false; line_count];
    let mut parser = Parser::new();
    if parser.set_language(language).is_err() {
        return flags;
    }
    let Some(tree) = parser.parse(text, None) else {
        return flags;
    };

    let mut comments: Vec<Range<usize>> = Vec::new();
    let mut leaves: Vec<Range<usize>> = Vec::new();
    collect_spans(tree.root_node(), &mut comments, &mut leaves, true);

    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(
            text.bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| i + 1),
        )
        .collect();
    for leaf in leaves
        .iter()
        .filter(|r| text.as_bytes()[r.start..r.end].contains(&b'\n'))
    {
        for (line, &start) in line_starts.iter().enumerate() {
            if start > leaf.start && start < leaf.end {
                flags[line] = true;
            }
        }
    }
    flags
}

/// Expands minified indentation (one space per depth) back to source-style
/// indentation. `widths` is the depth→column map of the target file (empty
/// for new files, where the step fallback applies). Lines beginning inside a
/// multiline literal are left untouched when `language` is given.
pub fn expand_indentation(
    text: &str,
    widths: &[usize],
    style: IndentStyle,
    language: Option<&Language>,
) -> String {
    let interior = language
        .map(|lang| literal_interior_lines(text, lang))
        .unwrap_or_default();

    let mut out = String::with_capacity(text.len() * 2);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if interior.get(i).copied().unwrap_or(false) || line.trim().is_empty() {
            out.push_str(line);
            continue;
        }
        let depth = line.bytes().take_while(|&b| b == b' ').count();
        let width = target_width(widths, depth);
        match style {
            IndentStyle::Spaces => {
                for _ in 0..width {
                    out.push(' ');
                }
            }
            IndentStyle::Tabs => {
                for _ in 0..width / 8 {
                    out.push('\t');
                }
                for _ in 0..width % 8 {
                    out.push(' ');
                }
            }
        }
        out.push_str(&line[depth..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::minify_for_path;

    fn minify(name: &str, source: &str) -> String {
        minify_for_path(Path::new(name), source, false).expect("language should be supported")
    }

    fn minify_keep_comments(name: &str, source: &str) -> String {
        minify_for_path(Path::new(name), source, true).expect("language should be supported")
    }

    fn normalize_fragment(name: &str, source: &str, keep_comments: bool) -> Option<String> {
        super::normalize_fragment_for_path(Path::new(name), source, keep_comments)
    }

    #[test]
    fn test_normalize_fragment_compacts_raw_indentation() {
        let fixture = "if a {\n        b();\n\n        c();\n    }";
        let actual = normalize_fragment("main.rs", fixture, false).unwrap();
        let expected = "if a {\n  b();\n  c();\n }";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_normalize_fragment_preserves_string_literal_whitespace() {
        let fixture = "let value = \"a    b\";";
        let actual = normalize_fragment("main.rs", fixture, false).unwrap();
        let expected = "let value = \"a    b\";";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_normalize_fragment_keep_comments_controls_comment_visibility() {
        let fixture = "// keep\nlet value = 1;";
        let actual = normalize_fragment("main.rs", fixture, true).unwrap();
        let expected = "// keep\nlet value = 1;";
        assert_eq!(actual, expected);

        let actual = normalize_fragment("main.rs", fixture, false).unwrap();
        let expected = "let value = 1;";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_rust_strips_comments_and_indentation() {
        let fixture = r#"/// Doc comment.
fn main() {
    // Line comment.
    let x = 1; /* inline */ let y = 2;

    println!("{} {}", x, y);
}
"#;
        let actual = minify("main.rs", fixture);
        let expected = "fn main() {\n let x = 1; let y = 2;\n println!(\"{} {}\", x, y);\n}\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_rust_preserves_string_contents() {
        let fixture = "fn f() -> &'static str {\n    \"a   b\\t// not a comment\"\n}\n";
        let actual = minify("lib.rs", fixture);
        let expected = "fn f() -> &'static str {\n \"a   b\\t// not a comment\"\n}\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_rust_preserves_multiline_string_verbatim() {
        let fixture = "const S: &str = \"line one\n    indented   line\";\nfn g() {}\n";
        let actual = minify("lib.rs", fixture);
        let expected = "const S: &str = \"line one\n    indented   line\";\nfn g() {}\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_python_keeps_relative_indentation() {
        let fixture = r#"def f(a):
    # comment
    if a:
        return 1
    return 2


def g():
    return 3
"#;
        let actual = minify("x.py", fixture);
        let expected = "def f(a):\n if a:\n  return 1\n return 2\ndef g():\n return 3\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_python_docstring_lines_untouched() {
        let fixture = "def f():\n    \"\"\"Doc\n       body   text\n    \"\"\"\n    return 1\n";
        let actual = minify("x.py", fixture);
        let expected = "def f():\n \"\"\"Doc\n       body   text\n    \"\"\"\n return 1\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_typescript_block_comment_spanning_lines() {
        let fixture = "const a = 1;\n/*\n * banner\n */\nfunction f(x: number): number {\n    return x + 1; // add\n}\n";
        let actual = minify("a.ts", fixture);
        let expected = "const a = 1;\nfunction f(x: number): number {\n return x + 1;\n}\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_go_alignment_spaces_collapse() {
        let fixture = "package main\n\n// entry\nfunc main() {\n\tx := 1  // tab indented\n\ty := 2\n\t_ = x\n\t_ = y\n}\n";
        let actual = minify("main.go", fixture);
        let expected = "package main\nfunc main() {\n x := 1\n y := 2\n _ = x\n _ = y\n}\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_json_indentation_collapses() {
        let fixture =
            "{\n    \"a\": {\n        \"b\":   [1, 2],\n        \"c\": \"x  y\"\n    }\n}\n";
        let actual = minify("cfg.json", fixture);
        let expected = "{\n \"a\": {\n  \"b\": [1, 2],\n  \"c\": \"x  y\"\n }\n}\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_bash_heredoc_preserved() {
        let fixture = "#!/bin/sh\n# setup\ncat <<EOF\n  keep   this\nEOF\necho done\n";
        let actual = minify("run.sh", fixture);
        // The shebang is lexed as a comment by tree-sitter-bash and removed;
        // heredoc body lines stay byte-identical.
        let expected = "cat <<EOF\n  keep   this\nEOF\necho done\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_unsupported_extension_returns_none() {
        let actual = minify_for_path(Path::new("notes.txt"), "hello\n", false);
        assert_eq!(actual, None);
    }

    #[test]
    fn test_no_extension_returns_none() {
        let actual = minify_for_path(Path::new("Makefile"), "all:\n\techo hi\n", false);
        assert_eq!(actual, None);
    }

    #[test]
    fn test_keep_comments_retains_comments_and_compacts_whitespace() {
        let fixture =
            "/// Doc comment.\nfn main() {\n    // keep me\n\n    let x = 1;    // trailing\n}\n";
        let actual = minify_keep_comments("main.rs", fixture);
        let expected = "/// Doc comment.\nfn main() {\n // keep me\n let x = 1; // trailing\n}\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_keep_comments_multiline_block_comment_verbatim() {
        let fixture = "/*\n * banner   text\n */\nfn f() {}\n";
        let actual = minify_keep_comments("lib.rs", fixture);
        let expected = "/*\n * banner   text\n */\nfn f() {}\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_src_map_copied_bytes_point_to_identical_source_bytes() {
        let fixture = "fn main() {\n    // gone\n    let x = 1;  /* mid */  let y = 2;\n}\n";
        let result = super::minify_with_map(Path::new("main.rs"), fixture, false).unwrap();
        assert_eq!(result.text.len(), result.src_map.len());
        let src = fixture.as_bytes();
        for (out_byte, &src_off) in result.text.bytes().zip(&result.src_map) {
            // Synthetic bytes are always ' ' or '\n'; everything else must
            // map onto the exact same byte in the source.
            if out_byte != b' ' && out_byte != b'\n' {
                assert_eq!(
                    out_byte, src[src_off],
                    "mismatch at source offset {src_off}"
                );
            }
        }
    }

    #[test]
    fn test_src_map_match_range_maps_back_to_source_range() {
        let fixture = "fn main() {\n    let x = 1;\n    // note\n    let y = 2;\n}\n";
        let result = super::minify_with_map(Path::new("main.rs"), fixture, false).unwrap();
        let needle = "let x = 1;\n let y = 2;";
        let a = result.text.find(needle).unwrap();
        let b = a + needle.len();
        let src_start = result.src_map[a];
        let src_end = result.src_map[b - 1] + 1;
        let expected = "let x = 1;\n    // note\n    let y = 2;";
        assert_eq!(&fixture[src_start..src_end], expected);
    }

    #[test]
    fn test_minify_with_map_reports_removed_comments() {
        let fixture = "fn main() {\n    // gone\n    let x = 1;\n}\n";
        let result = super::minify_with_map(Path::new("main.rs"), fixture, false).unwrap();
        assert_eq!(result.comments.len(), 1);
        assert_eq!(&fixture[result.comments[0].clone()], "// gone");

        let kept = super::minify_with_map(Path::new("main.rs"), fixture, true).unwrap();
        assert!(kept.comments.is_empty());
    }

    #[test]
    fn test_expand_indentation_roundtrip_is_idempotent() {
        let fixture = "def f(a):\n    if a:\n        return 1\n    return 2\n";
        let result = super::minify_with_map(Path::new("x.py"), fixture, false).unwrap();
        let expanded = super::expand_indentation(
            &result.text,
            &result.indent_widths,
            super::IndentStyle::Spaces,
            None,
        );
        assert_eq!(expanded, fixture);
    }

    #[test]
    fn test_expand_indentation_extrapolates_unknown_depths() {
        // widths only cover depths 0 and 1; depth 2 extrapolates by step 4.
        let actual =
            super::expand_indentation("a\n b\n  c", &[0, 4], super::IndentStyle::Spaces, None);
        assert_eq!(actual, "a\n    b\n        c");
    }

    #[test]
    fn test_expand_indentation_emits_tabs_for_tab_files() {
        let actual = super::expand_indentation("a\n b", &[0, 8], super::IndentStyle::Tabs, None);
        assert_eq!(actual, "a\n\tb");
    }

    #[test]
    fn test_expand_indentation_keeps_multiline_literal_interior_verbatim() {
        let lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
        let text = "def f():\n \"\"\"Doc\n   body\n \"\"\"\n return 1";
        let actual =
            super::expand_indentation(text, &[0, 4], super::IndentStyle::Spaces, Some(&lang));
        assert_eq!(
            actual,
            "def f():\n    \"\"\"Doc\n   body\n \"\"\"\n    return 1"
        );
    }

    #[test]
    fn test_detect_indent_style() {
        assert_eq!(
            super::detect_indent_style("fn f() {\n\tx();\n\ty();\n}\n"),
            super::IndentStyle::Tabs
        );
        assert_eq!(
            super::detect_indent_style("fn f() {\n    x();\n}\n"),
            super::IndentStyle::Spaces
        );
    }

    #[test]
    fn test_minified_rust_reparses_cleanly() {
        let fixture = r#"
//! Module docs.
pub struct Point {
    /// X coordinate.
    pub x: i32,
    pub y: i32, // trailing
}

impl Point {
    pub fn norm(&self) -> i32 {
        /* manhattan */
        self.x.abs() + self.y.abs()
    }
}
"#;
        let minified = minify("p.rs", fixture);
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(&minified, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "minified output must stay parseable: {minified}"
        );
        assert!(!minified.contains("manhattan"));
        assert!(!minified.contains("Module docs"));
    }

    #[test]
    fn test_minified_python_reparses_cleanly() {
        let fixture = r#"class A:
    def f(self, items):
        total = 0
        for i in items:
            if i > 0:
                total += i  # accumulate
            else:
                total -= i
        return total
"#;
        let minified = minify("a.py", fixture);
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(&minified, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "minified output must stay parseable: {minified}"
        );
    }
}
