//! Port of `packages/coding-agent/src/utils/syntax-highlight.ts`.
//!
//! Syntax coloring for the terminal. The TypeScript runs highlight.js, takes
//! the HTML it produces and walks it, replacing every `<span class="hljs-…">`
//! with the terminal escape the theme has for that scope. The master plan
//! substitutes highlight.js with tree-sitter (`tree-sitter-highlight`), so this
//! module keeps the second half verbatim — the HTML walker, the scope stack and
//! the theme lookup are the port of `renderHighlightedHtml` — and puts a
//! tree-sitter front end where the highlight.js call was: the highlighter emits
//! the same `<span class="hljs-…">` markup, with capture names translated to
//! highlight.js scope names by `HIGHLIGHT_SCOPES` below.
//!
//! Two consequences of the substitution, both visible to users:
//!
//!   * only the languages with a bundled grammar are highlighted (see
//!     `LANGUAGES`); for everything else `supports_language` reports `false`,
//!     which is the path `highlightCode` already takes for unknown languages —
//!     the code block is drawn in the flat `mdCodeBlock` color.
//!   * `highlight` without a language does not auto-detect. highlight.js guesses
//!     from a language subset; tree-sitter has nothing equivalent, and no caller
//!     in the app asks for it (`theme.rs` always passes a validated language,
//!     deliberately, because the guessing was unreliable).

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use regex::Regex;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::utils::html::decode_html_entity_at;

/// Wraps a run of text in whatever the theme uses for its scope.
pub type HighlightFormatter = Rc<dyn Fn(&str) -> String>;

/// highlight.js scope name (`keyword`, `string`, `punctuation`, …) → formatter.
pub type HighlightTheme = HashMap<String, HighlightFormatter>;

#[derive(Default, Clone)]
pub struct HighlightOptions {
    pub language: Option<String>,
    /// Kept for signature parity: highlight.js aborts on an `illegal` match
    /// unless this is set, and every caller in the app sets it. tree-sitter has
    /// no such concept — an unparsable region becomes an ERROR node and the
    /// surrounding code is still highlighted.
    pub ignore_illegals: bool,
    /// Only consulted by highlight.js' auto-detection, which has no tree-sitter
    /// equivalent (see the module docs).
    pub language_subset: Option<Vec<String>>,
    pub theme: HighlightTheme,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HighlightError {
    #[error("Unknown language: \"{0}\"")]
    UnknownLanguage(String),
    #[error("{0}")]
    Engine(String),
}

const SPAN_CLOSE: &str = "</span>";
const HIGHLIGHT_CLASS_PREFIX: &str = "hljs-";

fn get_scope_from_span_tag(tag: &str) -> Option<String> {
    static CLASS_ATTRIBUTE: OnceLock<Regex> = OnceLock::new();
    let class_attribute = CLASS_ATTRIBUTE
        .get_or_init(|| Regex::new(r#"\sclass\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());

    let captures = class_attribute.captures(tag)?;
    let class_value = captures.get(1).or_else(|| captures.get(2))?.as_str();
    if class_value.is_empty() {
        return None;
    }

    for class_name in class_value.split(char::is_whitespace) {
        if let Some(scope) = class_name.strip_prefix(HIGHLIGHT_CLASS_PREFIX) {
            return Some(scope.to_string());
        }
    }

    None
}

/// The theme entry for `scope`, falling back to its `.`- and `-`-prefix.
fn get_scope_formatter<'a>(
    scope: &str,
    theme: &'a HighlightTheme,
) -> Option<&'a HighlightFormatter> {
    if let Some(exact) = theme.get(scope) {
        return Some(exact);
    }

    if let Some((prefix, _)) = scope.split_once('.')
        && let Some(formatter) = theme.get(prefix)
    {
        return Some(formatter);
    }

    if let Some((prefix, _)) = scope.split_once('-')
        && let Some(formatter) = theme.get(prefix)
    {
        return Some(formatter);
    }

    None
}

/// The innermost open scope the theme knows about — a `<span>` the theme has no
/// entry for keeps the formatting of its parent.
fn get_active_formatter<'a>(
    scopes: &[Option<String>],
    theme: &'a HighlightTheme,
) -> Option<&'a HighlightFormatter> {
    for scope in scopes.iter().rev() {
        let Some(scope) = scope else { continue };
        if let Some(formatter) = get_scope_formatter(scope, theme) {
            return Some(formatter);
        }
    }
    theme.get("default")
}

fn is_span_open_tag_start(html: &str, index: usize) -> bool {
    if !html[index..].starts_with("<span") {
        return false;
    }
    matches!(
        html.as_bytes().get(index + "<span".len()),
        Some(b'>' | b' ' | b'\t' | b'\n' | b'\r')
    )
}

fn flush_text(
    output: &mut String,
    text_buffer: &mut String,
    scopes: &[Option<String>],
    theme: &HighlightTheme,
) {
    if text_buffer.is_empty() {
        return;
    }
    match get_active_formatter(scopes, theme) {
        Some(formatter) => output.push_str(&formatter(text_buffer)),
        None => output.push_str(text_buffer),
    }
    text_buffer.clear();
}

/// Turns highlighted HTML into themed terminal output.
pub fn render_highlighted_html(html: &str, theme: &HighlightTheme) -> String {
    let mut output = String::new();
    let mut text_buffer = String::new();
    let mut scopes: Vec<Option<String>> = Vec::new();

    let mut index = 0usize;
    while index < html.len() {
        if is_span_open_tag_start(html, index)
            && let Some(relative_end) = html[index + "<span".len()..].find('>')
        {
            let tag_end_index = index + "<span".len() + relative_end;
            flush_text(&mut output, &mut text_buffer, &scopes, theme);
            let tag = &html[index..=tag_end_index];
            scopes.push(get_scope_from_span_tag(tag));
            index = tag_end_index + 1;
            continue;
        }

        if html[index..].starts_with(SPAN_CLOSE) {
            flush_text(&mut output, &mut text_buffer, &scopes, theme);
            scopes.pop();
            index += SPAN_CLOSE.len();
            continue;
        }

        if html.as_bytes()[index] == b'&'
            && let Some(decoded) = decode_html_entity_at(html, index)
        {
            text_buffer.push_str(&decoded.text);
            index += decoded.length;
            continue;
        }

        let character = html[index..]
            .chars()
            .next()
            .expect("index is a char boundary");
        text_buffer.push(character);
        index += character.len_utf8();
    }

    flush_text(&mut output, &mut text_buffer, &scopes, theme);
    output
}

pub fn highlight(code: &str, options: &HighlightOptions) -> Result<String, HighlightError> {
    let html = match options.language.as_deref() {
        Some(language) => highlight_to_html(code, language)?,
        None => escape_html(code),
    };
    Ok(render_highlighted_html(&html, &options.theme))
}

pub fn supports_language(name: &str) -> bool {
    canonical_language(name).is_some()
}

// ---------------------------------------------------------------------------
// tree-sitter front end
// ---------------------------------------------------------------------------

/// tree-sitter capture name → highlight.js scope name.
///
/// The theme (`modes/interactive/theme/theme.rs`, `buildCliHighlightTheme` in
/// TypeScript) is keyed by highlight.js scopes, so the capture names of the
/// bundled queries are translated here instead of duplicating the theme. Names
/// that map to a scope the theme has no entry for (`subst`, `symbol`) inherit
/// the enclosing scope, exactly as they do under highlight.js — a `${…}` inside
/// a template literal stays string-colored.
///
/// `HighlightConfiguration::configure` matches a capture against the entry with
/// the most dot-separated parts, so the specific names below win over their
/// prefixes. Every name that the bundled `highlights.scm`/`injections.scm`/
/// `locals.scm` files actually produce has an entry.
const HIGHLIGHT_SCOPES: &[(&str, &str)] = &[
    ("attribute", "attr"),
    ("comment", "comment"),
    ("comment.documentation", "doctag"),
    ("constant", "literal"),
    ("constant.builtin", "literal"),
    ("constructor", "class"),
    ("delimiter", "punctuation"),
    ("embedded", "subst"),
    ("escape", "subst"),
    ("function", "function"),
    ("function.builtin", "built_in"),
    ("function.macro", "built_in"),
    ("function.method", "function"),
    ("function.method.builtin", "built_in"),
    ("function.special", "function"),
    ("keyword", "keyword"),
    ("label", "symbol"),
    ("number", "number"),
    ("operator", "operator"),
    ("property", "attr"),
    ("punctuation", "punctuation"),
    ("punctuation.bracket", "punctuation"),
    ("punctuation.delimiter", "punctuation"),
    ("punctuation.special", "subst"),
    ("string", "string"),
    ("string.escape", "subst"),
    ("string.special", "string"),
    ("string.special.key", "attr"),
    ("string.special.regex", "regexp"),
    ("string.special.symbol", "symbol"),
    ("tag", "name"),
    ("tag.error", "name"),
    ("type", "type"),
    ("type.builtin", "built_in"),
    ("variable", "variable"),
    ("variable.builtin", "variable"),
    ("variable.parameter", "params"),
];

/// A language the bundled grammars can highlight, with the highlight.js aliases
/// that resolve to it (`hljs.getLanguage` lowercases and looks up name first,
/// then alias).
struct LanguageDefinition {
    canonical: &'static str,
    aliases: &'static [&'static str],
}

const LANGUAGES: &[LanguageDefinition] = &[
    LanguageDefinition {
        canonical: "bash",
        aliases: &["sh", "zsh"],
    },
    LanguageDefinition {
        canonical: "c",
        aliases: &["h"],
    },
    LanguageDefinition {
        canonical: "cpp",
        aliases: &["cc", "c++", "h++", "hpp", "hh", "hxx", "cxx"],
    },
    LanguageDefinition {
        canonical: "css",
        aliases: &[],
    },
    LanguageDefinition {
        canonical: "diff",
        aliases: &["patch"],
    },
    LanguageDefinition {
        canonical: "go",
        aliases: &["golang"],
    },
    LanguageDefinition {
        canonical: "xml",
        aliases: &[
            "html", "xhtml", "rss", "atom", "xjb", "xsd", "xsl", "plist", "wsf", "svg",
        ],
    },
    LanguageDefinition {
        canonical: "java",
        aliases: &["jsp"],
    },
    LanguageDefinition {
        canonical: "javascript",
        aliases: &["js", "jsx", "mjs", "cjs"],
    },
    LanguageDefinition {
        canonical: "json",
        aliases: &[],
    },
    LanguageDefinition {
        canonical: "python",
        aliases: &["py", "gyp", "ipython"],
    },
    LanguageDefinition {
        canonical: "ruby",
        aliases: &["rb", "gemspec", "podspec", "thor", "irb"],
    },
    LanguageDefinition {
        canonical: "rust",
        aliases: &["rs"],
    },
    LanguageDefinition {
        canonical: "typescript",
        aliases: &["ts"],
    },
    LanguageDefinition {
        canonical: "tsx",
        aliases: &[],
    },
];

fn canonical_language(name: &str) -> Option<&'static str> {
    let name = name.to_lowercase();
    LANGUAGES
        .iter()
        .find(|language| language.canonical == name || language.aliases.contains(&name.as_str()))
        .map(|language| language.canonical)
}

fn build_configuration(canonical: &'static str) -> Result<HighlightConfiguration, HighlightError> {
    // The typescript and tsx grammars extend javascript, so their queries only
    // carry the additions; the javascript patterns have to come first because a
    // later pattern wins for the same node. The JSX patterns are left out of
    // plain typescript — that grammar has no `jsx_opening_element` node and the
    // query would not compile.
    let javascript_highlights = tree_sitter_javascript::HIGHLIGHT_QUERY;
    let jsx_highlights = tree_sitter_javascript::JSX_HIGHLIGHT_QUERY;
    let (language, highlights, injections, locals) = match canonical {
        "bash" => (
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY.to_string(),
            "",
            "",
        ),
        "c" => (
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY.to_string(),
            "",
            "",
        ),
        "cpp" => (
            tree_sitter_cpp::LANGUAGE.into(),
            format!(
                "{}{}",
                tree_sitter_c::HIGHLIGHT_QUERY,
                tree_sitter_cpp::HIGHLIGHT_QUERY
            ),
            "",
            "",
        ),
        "css" => (
            tree_sitter_css::LANGUAGE.into(),
            tree_sitter_css::HIGHLIGHTS_QUERY.to_string(),
            "",
            "",
        ),
        "go" => (
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY.to_string(),
            "",
            "",
        ),
        "xml" => (
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY.to_string(),
            tree_sitter_html::INJECTIONS_QUERY,
            "",
        ),
        "java" => (
            tree_sitter_java::LANGUAGE.into(),
            tree_sitter_java::HIGHLIGHTS_QUERY.to_string(),
            "",
            "",
        ),
        "javascript" => (
            tree_sitter_javascript::LANGUAGE.into(),
            format!("{javascript_highlights}{jsx_highlights}"),
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_javascript::LOCALS_QUERY,
        ),
        "json" => (
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY.to_string(),
            "",
            "",
        ),
        "python" => (
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY.to_string(),
            "",
            "",
        ),
        "ruby" => (
            tree_sitter_ruby::LANGUAGE.into(),
            tree_sitter_ruby::HIGHLIGHTS_QUERY.to_string(),
            "",
            tree_sitter_ruby::LOCALS_QUERY,
        ),
        "rust" => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY.to_string(),
            tree_sitter_rust::INJECTIONS_QUERY,
            "",
        ),
        "typescript" => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            format!(
                "{javascript_highlights}{}",
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        "tsx" => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            format!(
                "{javascript_highlights}{jsx_highlights}{}",
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        _ => return Err(HighlightError::UnknownLanguage(canonical.to_string())),
    };

    let mut configuration =
        HighlightConfiguration::new(language, canonical, &highlights, injections, locals)
            .map_err(|error| HighlightError::Engine(error.to_string()))?;
    let recognized: Vec<&str> = HIGHLIGHT_SCOPES
        .iter()
        .map(|(capture, _)| *capture)
        .collect();
    configuration.configure(&recognized);
    Ok(configuration)
}

/// Grammars are compiled on first use and then shared; `HighlightConfiguration`
/// is immutable and `Send + Sync`, so one leaked instance serves every thread.
fn configuration_for(
    canonical: &'static str,
) -> Result<&'static HighlightConfiguration, HighlightError> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, &'static HighlightConfiguration>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let mut cache = cache.lock().expect("syntax highlight cache poisoned");
    if let Some(configuration) = cache.get(canonical) {
        return Ok(configuration);
    }
    let configuration: &'static HighlightConfiguration =
        Box::leak(Box::new(build_configuration(canonical)?));
    cache.insert(canonical, configuration);
    Ok(configuration)
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#x27;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// The one language highlight.js does not describe with a grammar: `diff` is a
/// list of line-prefix rules (`highlight.js/lib/languages/diff.js`), and there
/// is no tree-sitter diff grammar to substitute for it. The rules are ported
/// directly, including their order — highlight.js takes the leftmost match and,
/// where two rules start at the same place, the one declared first. That is
/// what keeps `+++ b/file` a header instead of an addition.
struct DiffRule {
    scope: &'static str,
    begin: &'static str,
    /// highlight.js writes `end: /$/` for these; a rule without it covers only
    /// what `begin` matched (highlight.js defaults `end` to a zero-width match).
    to_end_of_line: bool,
}

const DIFF_RULES: &[DiffRule] = &[
    DiffRule {
        scope: "meta",
        begin: r"(?m)^@@ +-\d+,\d+ +\+\d+,\d+ +@@",
        to_end_of_line: false,
    },
    DiffRule {
        scope: "meta",
        begin: r"(?m)^\*\*\* +\d+,\d+ +\*\*\*\*$",
        to_end_of_line: false,
    },
    DiffRule {
        scope: "meta",
        begin: r"(?m)^--- +\d+,\d+ +----$",
        to_end_of_line: false,
    },
    DiffRule {
        scope: "comment",
        begin: r"Index: ",
        to_end_of_line: true,
    },
    DiffRule {
        scope: "comment",
        begin: r"(?m)^index",
        to_end_of_line: true,
    },
    DiffRule {
        scope: "comment",
        begin: r"={3,}",
        to_end_of_line: true,
    },
    DiffRule {
        scope: "comment",
        begin: r"(?m)^-{3}",
        to_end_of_line: true,
    },
    DiffRule {
        scope: "comment",
        begin: r"(?m)^\*{3} ",
        to_end_of_line: true,
    },
    DiffRule {
        scope: "comment",
        begin: r"(?m)^\+{3}",
        to_end_of_line: true,
    },
    DiffRule {
        scope: "comment",
        begin: r"(?m)^\*{15}$",
        to_end_of_line: false,
    },
    DiffRule {
        scope: "comment",
        begin: r"(?m)^diff --git",
        to_end_of_line: true,
    },
    DiffRule {
        scope: "addition",
        begin: r"(?m)^\+",
        to_end_of_line: true,
    },
    DiffRule {
        scope: "deletion",
        begin: r"(?m)^-",
        to_end_of_line: true,
    },
    DiffRule {
        scope: "addition",
        begin: r"(?m)^!",
        to_end_of_line: true,
    },
];

fn diff_rule_regexes() -> &'static [Regex] {
    static REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
    REGEXES.get_or_init(|| {
        DIFF_RULES
            .iter()
            .map(|rule| Regex::new(rule.begin).expect("diff rule"))
            .collect()
    })
}

fn highlight_diff_to_html(code: &str) -> String {
    let regexes = diff_rule_regexes();
    let mut html = String::with_capacity(code.len());
    let mut index = 0usize;

    while index < code.len() {
        let next = DIFF_RULES
            .iter()
            .zip(regexes)
            .filter_map(|(rule, regex)| regex.find_at(code, index).map(|found| (rule, found)))
            .min_by_key(|(_, found)| found.start());

        let Some((rule, found)) = next else { break };
        html.push_str(&escape_html(&code[index..found.start()]));

        let span_end = if rule.to_end_of_line {
            code[found.end()..]
                .find('\n')
                .map_or(code.len(), |offset| found.end() + offset)
        } else {
            found.end()
        };
        html.push_str("<span class=\"");
        html.push_str(HIGHLIGHT_CLASS_PREFIX);
        html.push_str(rule.scope);
        html.push_str("\">");
        html.push_str(&escape_html(&code[found.start()..span_end]));
        html.push_str(SPAN_CLOSE);
        index = span_end;
    }

    html.push_str(&escape_html(&code[index..]));
    html
}

/// Highlights `code` into the `<span class="hljs-…">` markup the renderer above
/// expects — the shape highlight.js' `.value` has.
fn highlight_to_html(code: &str, language: &str) -> Result<String, HighlightError> {
    let canonical = canonical_language(language)
        .ok_or_else(|| HighlightError::UnknownLanguage(language.to_string()))?;
    if canonical == "diff" {
        return Ok(highlight_diff_to_html(code));
    }
    let configuration = configuration_for(canonical)?;

    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(configuration, code.as_bytes(), None, |name| {
            canonical_language(name).and_then(|canonical| configuration_for(canonical).ok())
        })
        .map_err(|error| HighlightError::Engine(error.to_string()))?;

    let mut html = String::with_capacity(code.len());
    for event in events {
        match event.map_err(|error| HighlightError::Engine(error.to_string()))? {
            HighlightEvent::Source { start, end } => {
                html.push_str(&escape_html(&code[start..end]));
            }
            HighlightEvent::HighlightStart(highlight) => {
                let scope = HIGHLIGHT_SCOPES[highlight.0].1;
                html.push_str("<span class=\"");
                html.push_str(HIGHLIGHT_CLASS_PREFIX);
                html.push_str(scope);
                html.push_str("\">");
            }
            HighlightEvent::HighlightEnd => html.push_str(SPAN_CLOSE),
        }
    }
    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(entries: &[(&str, &'static str)]) -> HighlightTheme {
        entries
            .iter()
            .map(|(scope, label)| {
                let label = *label;
                let formatter: HighlightFormatter =
                    Rc::new(move |text: &str| format!("[{label}:{text}]"));
                ((*scope).to_string(), formatter)
            })
            .collect()
    }

    // Ported from `packages/coding-agent/test/syntax-highlight.test.ts`.

    #[test]
    fn renders_highlighted_spans_with_the_provided_theme() {
        let rendered = render_highlighted_html(
            r#"<span class="hljs-keyword">const</span> value"#,
            &theme(&[("keyword", "keyword")]),
        );
        assert_eq!(rendered, "[keyword:const] value");
    }

    #[test]
    fn decodes_html_entities_emitted_by_the_highlighter() {
        let rendered = render_highlighted_html(
            "&lt;tag attr=&quot;value&quot;&gt;&amp;#x41;&#65;&lt;/tag&gt;",
            &HighlightTheme::new(),
        );
        assert_eq!(rendered, "<tag attr=\"value\">&#x41;A</tag>");
    }

    #[test]
    fn inherits_parent_formatting_for_unmapped_nested_scopes() {
        let interpolation = "${x}";
        let rendered = render_highlighted_html(
            &format!(
                r#"<span class="hljs-string">a<span class="hljs-subst">{interpolation}</span>b</span>"#
            ),
            &theme(&[("string", "string")]),
        );
        assert_eq!(
            rendered,
            format!("[string:a][string:{interpolation}][string:b]")
        );
    }

    #[test]
    fn keeps_parent_formatting_across_unscoped_nested_spans() {
        let rendered = render_highlighted_html(
            r#"<span class="hljs-string">a<span class="language-xml">b</span>c</span>"#,
            &theme(&[("string", "string")]),
        );
        assert_eq!(rendered, "[string:a][string:b][string:c]");
    }

    #[test]
    fn highlights_code_through_the_engine() {
        assert!(supports_language("typescript"));
        let rendered = highlight(
            "const value = 1",
            &HighlightOptions {
                language: Some("typescript".to_string()),
                ignore_illegals: true,
                theme: theme(&[("keyword", "keyword"), ("number", "number")]),
                ..HighlightOptions::default()
            },
        )
        .unwrap();
        assert!(rendered.contains("[keyword:const]"), "{rendered}");
        assert!(rendered.contains("[number:1]"), "{rendered}");
    }

    // Beyond the TypeScript suite: the substitution's own surface.

    #[test]
    fn resolves_language_names_case_insensitively_and_through_aliases() {
        assert_eq!(canonical_language("TypeScript"), Some("typescript"));
        assert_eq!(canonical_language("ts"), Some("typescript"));
        assert_eq!(canonical_language("rs"), Some("rust"));
        assert_eq!(canonical_language("c++"), Some("cpp"));
        assert_eq!(canonical_language("html"), Some("xml"));
        assert!(!supports_language("cobol"));
        assert!(!supports_language(""));
    }

    #[test]
    fn every_bundled_grammar_compiles_and_highlights() {
        for language in LANGUAGES
            .iter()
            .filter(|language| language.canonical != "diff")
        {
            let configuration = configuration_for(language.canonical)
                .unwrap_or_else(|error| panic!("{}: {error}", language.canonical));
            assert_eq!(configuration.language_name, language.canonical);
        }
    }

    #[test]
    fn colors_diff_additions_and_deletions() {
        // `test/syntax-highlight.test.ts` pins this through the theme: the
        // whole line, prefix included, carries the addition/deletion color.
        assert_eq!(
            highlight_to_html("-old\n+new\n", "diff").unwrap(),
            "<span class=\"hljs-deletion\">-old</span>\n<span class=\"hljs-addition\">+new</span>\n"
        );
    }

    #[test]
    fn reads_diff_headers_as_comments_and_hunks_as_meta() {
        let html = highlight_to_html(
            "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n old\n",
            "patch",
        )
        .unwrap();
        assert_eq!(
            html,
            concat!(
                "<span class=\"hljs-comment\">diff --git a/f b/f</span>\n",
                "<span class=\"hljs-comment\">index 1..2 100644</span>\n",
                "<span class=\"hljs-comment\">--- a/f</span>\n",
                "<span class=\"hljs-comment\">+++ b/f</span>\n",
                "<span class=\"hljs-meta\">@@ -1,2 +1,2 @@</span>\n",
                " old\n"
            )
        );
    }

    #[test]
    fn reads_context_diffs_the_way_highlight_js_does() {
        // Verified against highlight.js 11 itself, byte for byte.
        assert_eq!(
            highlight_to_html(
                "*** 1,4 ****\n! ctx\n***************\nIndex: foo\n=== bar ===\n",
                "diff"
            )
            .unwrap(),
            concat!(
                "<span class=\"hljs-meta\">*** 1,4 ****</span>\n",
                "<span class=\"hljs-addition\">! ctx</span>\n",
                "<span class=\"hljs-comment\">***************</span>\n",
                "<span class=\"hljs-comment\">Index: foo</span>\n",
                "<span class=\"hljs-comment\">=== bar ===</span>\n"
            )
        );
    }

    #[test]
    fn pins_the_scopes_the_typescript_theme_suite_checks() {
        // Two of the three cases in `test/syntax-highlight.test.ts` land on the
        // same theme slot as under highlight.js: a JavaScript regex literal is
        // string-colored, an HTML tag name keyword-colored (`name`).
        assert!(
            highlight_to_html("const re = /foo+/gi;", "javascript")
                .unwrap()
                .contains("<span class=\"hljs-string\">")
        );
        assert!(
            highlight_to_html("<div></div>", "html")
                .unwrap()
                .contains("<span class=\"hljs-name\">div</span>")
        );
        // The third diverges: tree-sitter reads a Python decorator as a
        // function, highlight.js called it `meta` (the muted color).
        assert!(
            highlight_to_html("@decorator", "python")
                .unwrap()
                .contains("<span class=\"hljs-function\">")
        );
    }

    #[test]
    fn reports_an_unknown_language_like_highlight_js() {
        let error = highlight(
            "puts 1",
            &HighlightOptions {
                language: Some("cobol".to_string()),
                ..HighlightOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(error, HighlightError::UnknownLanguage("cobol".to_string()));
        assert_eq!(error.to_string(), "Unknown language: \"cobol\"");
    }

    #[test]
    fn passes_text_through_unhighlighted_without_a_language() {
        let rendered = highlight(
            "a < b && c",
            &HighlightOptions {
                theme: theme(&[("keyword", "keyword")]),
                ..HighlightOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, "a < b && c");
    }

    #[test]
    fn maps_captures_onto_highlight_js_scopes() {
        let cases: &[(&str, &str, &str)] = &[
            ("rust", "fn main() {}", "hljs-keyword"),
            ("python", "x = \"s\"", "hljs-string"),
            // A JSON key is `@string.special.key` and then `@string` a pattern
            // later, and the later pattern wins — highlight.js calls it `attr`.
            ("json", "{\"a\": true}", "hljs-literal"),
            ("xml", "<div></div>", "hljs-name"),
            ("css", "a { color: red; }", "hljs-attr"),
            ("bash", "echo hi # note", "hljs-comment"),
            ("go", "package main", "hljs-keyword"),
        ];
        for (language, code, expected) in cases {
            let html = highlight_to_html(code, language).unwrap();
            assert!(html.contains(expected), "{language}: {html}");
        }
    }

    #[test]
    fn keeps_the_source_text_intact_across_the_html_round_trip() {
        let code = "const s = \"a & b < c\";\n// »ünïcödé«\n";
        let rendered = highlight(
            code,
            &HighlightOptions {
                language: Some("ts".to_string()),
                ..HighlightOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, code);
    }
}
