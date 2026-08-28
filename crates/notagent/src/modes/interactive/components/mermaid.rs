use std::rc::Rc;
use std::sync::Arc;

use notagent_tui::markdown_lexer::{Token, lex};

use crate::core::settings_manager::MermaidRenderingMode;
use crate::modes::interactive::components::markdown_transform::{
    MarkdownMessageType, MarkdownTransformer,
};
use crate::modes::interactive::theme::theme::{Theme, ThemeColor};
use crate::utils::mermaid::render;
use crate::utils::mermaid::types::{Cls, MermaidArt, Span};

/// Options of [`create_mermaid_markdown_transformer`].
pub struct MermaidTransformerOptions {
    /// Reads the current `markdown.mermaid` setting.
    pub get_mode: Rc<dyn Fn() -> MermaidRenderingMode>,
    /// Theme the semantic spans are mapped through; unstyled when absent.
    pub theme: Option<Arc<Theme>>,
}

fn is_mermaid(token: &Token) -> bool {
    token.kind == "code"
        && token
            .lang
            .as_deref()
            .and_then(|lang| lang.split_whitespace().next())
            .is_some_and(|first| first.to_lowercase() == "mermaid")
}

fn code_span(line: &str) -> String {
    // Encode each diagram row as inline code (` ... `) so Markdown preserves its
    // spacing and box-drawing characters. Use a non-breaking space for blank
    // rows because an empty code span has no visible height.
    let content = if line.is_empty() { "\u{a0}" } else { line };
    // CommonMark code spans use matching backtick delimiters, so choose one
    // longer than any backtick run in the content (``hel`lo`` -> <code>hel`lo</code>).
    // If the content starts or ends with a backtick, separating it from the
    // delimiter with a space keeps that backtick as content; CommonMark removes
    // the padding when rendering (`` `edge` `` -> <code>`edge`</code>).
    let longest_backtick_run = content.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest_backtick_run + 1);
    let padding = if content.starts_with('`') || content.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{padding}{content}{padding}{fence}")
}

fn style_span(span: &Span, theme: &Theme) -> String {
    match span.cls {
        Cls::Border => theme.fg(ThemeColor::BorderMuted, &span.text),
        Cls::Text => theme.fg(ThemeColor::Text, &span.text),
        Cls::Edge => theme.fg(ThemeColor::Accent, &span.text),
        Cls::EdgeLabel => theme.fg(ThemeColor::Muted, &span.text),
        Cls::Title => theme.fg(ThemeColor::Accent, &theme.bold(&span.text)),
        Cls::None => span.text.clone(),
    }
}

fn themed_lines(art: &MermaidArt, theme: &Theme) -> Vec<String> {
    art.styled
        .iter()
        .map(|row| row.iter().map(|span| style_span(span, theme)).collect())
        .collect()
}

/// Create a transformer that replaces top-level Mermaid code blocks with
/// Unicode terminal diagrams.
pub fn create_mermaid_markdown_transformer(
    options: MermaidTransformerOptions,
) -> MarkdownTransformer {
    Rc::new(move |markdown, context| {
        let mode = (options.get_mode)();
        if mode == MermaidRenderingMode::Off
            || context.message_type == MarkdownMessageType::AssistantThinking
            || (context.is_streaming && mode != MermaidRenderingMode::Streaming)
        {
            return markdown.to_string();
        }

        lex(markdown)
            .iter()
            .map(|token| {
                if !is_mermaid(token) {
                    return token.raw.clone();
                }
                let Some(art) = render(token.text.as_deref().unwrap_or("")) else {
                    return token.raw.clone();
                };
                if art.width > context.available_width {
                    return token.raw.clone();
                }
                if !context.is_streaming && !art.warnings.is_empty() {
                    let suffix = if art.warnings.len() > 1 {
                        format!(" (+{} more)", art.warnings.len() - 1)
                    } else {
                        String::new()
                    };
                    let warning =
                        format!("Mermaid diagram not rendered: {}{suffix}", art.warnings[0]);
                    let styled_warning = match &options.theme {
                        Some(theme) => theme.fg(ThemeColor::Warning, &warning),
                        None => warning,
                    };
                    return format!("{}\n{}  \n", token.raw, code_span(&styled_warning));
                }
                let lines = match &options.theme {
                    Some(theme) => themed_lines(&art, theme),
                    None => art.plain.clone(),
                };
                // Markdown hard breaks keep every diagram row on its own line.
                let body: Vec<String> = lines.iter().map(|line| code_span(line)).collect();
                format!("{}\n", body.join("  \n"))
            })
            .collect()
    })
}
