use std::rc::Rc;

use crate::latex::{RenderLatexOptions, render_latex};
use crate::markdown_lexer::{TableCell, Token, lex};
use crate::terminal_image::{get_capabilities, hyperlink, is_image_line};
use crate::tui::{Component, Line, finish_line};
use crate::utils::{apply_background_to_line, visible_width, wrap_text_with_ansi};

/// A styling function.
pub type StyleFn = Rc<dyn Fn(&str) -> String>;

/// Syntax highlighting for fenced code blocks.
pub type HighlightCodeFn = Rc<dyn Fn(&str, Option<&str>) -> Vec<String>>;

/// Base styling applied to all markdown text.
#[derive(Clone, Default)]
pub struct DefaultTextStyle {
    /// Foreground colour.
    pub color: Option<StyleFn>,
    /// Background colour, applied to the padded line.
    pub bg_color: Option<StyleFn>,
    /// Bold text.
    pub bold: bool,
    /// Italic text.
    pub italic: bool,
    /// Struck-through text.
    pub strikethrough: bool,
    /// Underlined text.
    pub underline: bool,
}

/// Colouring of the markdown elements.
#[derive(Clone)]
pub struct MarkdownTheme {
    /// Headings.
    pub heading: StyleFn,
    /// Link text.
    pub link: StyleFn,
    /// Link target shown in parentheses.
    pub link_url: StyleFn,
    /// Inline code.
    pub code: StyleFn,
    /// Code block content.
    pub code_block: StyleFn,
    /// Code block fences.
    pub code_block_border: StyleFn,
    /// Block quote text.
    pub quote: StyleFn,
    /// Block quote border.
    pub quote_border: StyleFn,
    /// Horizontal rules.
    pub hr: StyleFn,
    /// List bullets.
    pub list_bullet: StyleFn,
    /// Bold text.
    pub bold: StyleFn,
    /// Italic text.
    pub italic: StyleFn,
    /// Struck-through text.
    pub strikethrough: StyleFn,
    /// Underlined text.
    pub underline: StyleFn,
    /// Optional syntax highlighting.
    pub highlight_code: Option<HighlightCodeFn>,
    /// Prefix of every code block line (default `"  "`).
    pub code_block_indent: Option<String>,
}

/// Source transformation applied before parsing.
pub type TransformFn = Rc<dyn Fn(&str, usize) -> String>;

/// Options of the [`Markdown`] component.
#[derive(Clone, Default)]
pub struct MarkdownOptions {
    /// Keep the source list markers instead of normalizing them.
    pub preserve_ordered_list_markers: bool,
    /// Keep source backslash escapes instead of resolving them.
    pub preserve_backslash_escapes: bool,
    /// Transform the source before parsing, with the available content width.
    pub transform: Option<TransformFn>,
    /// Render supported LaTeX as Unicode text (default `true`).
    pub render_latex: Option<bool>,
}

/// Inline styling context handed down while rendering.
struct InlineStyleContext {
    apply_text: Box<dyn Fn(&str) -> String>,
    style_prefix: String,
}

/// Markdown renderer.
pub struct Markdown {
    text: String,
    padding_x: usize,
    padding_y: usize,
    default_text_style: Option<DefaultTextStyle>,
    theme: MarkdownTheme,
    options: MarkdownOptions,
    default_style_prefix: Option<String>,
    cached_text: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<Line>>,
}

impl Markdown {
    /// New component for `text`.
    pub fn new(
        text: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        theme: MarkdownTheme,
        default_text_style: Option<DefaultTextStyle>,
        options: Option<MarkdownOptions>,
    ) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
            default_text_style,
            theme,
            options: options.unwrap_or_default(),
            default_style_prefix: None,
            cached_text: None,
            cached_width: None,
            cached_lines: None,
        }
    }

    /// Replace the source text.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.invalidate_cache();
    }

    fn invalidate_cache(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }

    /// Apply the base text style (without the background).
    fn apply_default_style(&self, text: &str) -> String {
        let Some(style) = &self.default_text_style else {
            return text.to_string();
        };
        let mut styled = text.to_string();
        if let Some(color) = &style.color {
            styled = color(&styled);
        }
        if style.bold {
            styled = (self.theme.bold)(&styled);
        }
        if style.italic {
            styled = (self.theme.italic)(&styled);
        }
        if style.strikethrough {
            styled = (self.theme.strikethrough)(&styled);
        }
        if style.underline {
            styled = (self.theme.underline)(&styled);
        }
        styled
    }

    /// ANSI prefix the default style emits before the text.
    fn get_default_style_prefix(&mut self) -> String {
        if self.default_text_style.is_none() {
            return String::new();
        }
        if let Some(prefix) = &self.default_style_prefix {
            return prefix.clone();
        }
        let sentinel = "\u{0}";
        let styled = self.apply_default_style(sentinel);
        let prefix = styled
            .find(sentinel)
            .map_or_else(String::new, |index| styled[..index].to_string());
        self.default_style_prefix = Some(prefix.clone());
        prefix
    }

    fn get_style_prefix(style_fn: &dyn Fn(&str) -> String) -> String {
        let sentinel = "\u{0}";
        let styled = style_fn(sentinel);
        styled
            .find(sentinel)
            .map_or_else(String::new, |index| styled[..index].to_string())
    }
}

impl Markdown {
    fn default_inline_style_context(&mut self) -> InlineStyleContext {
        let style_prefix = self.get_default_style_prefix();
        let style = self.default_text_style.clone();
        let theme = self.theme.clone();
        InlineStyleContext {
            apply_text: Box::new(move |text| apply_style(&style, &theme, text)),
            style_prefix,
        }
    }

    /// Render one block token.
    fn render_token(
        &mut self,
        token: &Token,
        width: usize,
        next_token_type: Option<&str>,
        style_context: Option<&InlineStyleContext>,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();

        match token.kind.as_str() {
            "heading" => {
                let heading_level = token.depth.unwrap_or(1);
                let theme = self.theme.clone();
                let heading_style: Box<dyn Fn(&str) -> String> = if heading_level == 1 {
                    Box::new(move |text: &str| {
                        (theme.heading)(&(theme.bold)(&(theme.underline)(text)))
                    })
                } else {
                    Box::new(move |text: &str| (theme.heading)(&(theme.bold)(text)))
                };
                let style_prefix = Self::get_style_prefix(heading_style.as_ref());
                let heading_context = InlineStyleContext {
                    apply_text: heading_style,
                    style_prefix,
                };

                let heading_text = self.render_inline_tokens(
                    token.tokens.as_deref().unwrap_or_default(),
                    Some(&heading_context),
                );
                lines.push(heading_text);
                if next_token_type.is_some_and(|kind| kind != "space") {
                    lines.push(String::new());
                }
            }

            "paragraph" => {
                let text = self.render_inline_tokens(
                    token.tokens.as_deref().unwrap_or_default(),
                    style_context,
                );
                lines.push(text);
                if next_token_type.is_some_and(|kind| kind != "list" && kind != "space") {
                    lines.push(String::new());
                }
            }

            "text" => {
                let rendered =
                    self.render_inline_tokens(std::slice::from_ref(token), style_context);
                lines.push(rendered);
            }

            "latexBlock" => {
                let text = token.text.clone().unwrap_or_default();
                let rendered =
                    if token.pending != Some(true) && self.options.render_latex != Some(false) {
                        render_latex(&text, RenderLatexOptions { display: true })
                            .unwrap_or_else(|| token.raw.trim().to_string())
                    } else {
                        token.raw.trim().to_string()
                    };
                for line in rendered.split('\n') {
                    lines.push(self.apply_default_style(line));
                }
                if next_token_type.is_some_and(|kind| kind != "space") {
                    lines.push(String::new());
                }
            }

            "code" => {
                let indent = self
                    .theme
                    .code_block_indent
                    .clone()
                    .unwrap_or_else(|| "  ".to_string());
                lines.push((self.theme.code_block_border)(&format!(
                    "```{}",
                    token.lang.clone().unwrap_or_default()
                )));
                let text = token.text.clone().unwrap_or_default();
                if let Some(highlight) = &self.theme.highlight_code {
                    for line in highlight(&text, token.lang.as_deref()) {
                        lines.push(format!("{indent}{line}"));
                    }
                } else {
                    for line in text.split('\n') {
                        lines.push(format!("{indent}{}", (self.theme.code_block)(line)));
                    }
                }
                lines.push((self.theme.code_block_border)("```"));
                if next_token_type.is_some_and(|kind| kind != "space") {
                    lines.push(String::new());
                }
            }

            "list" => {
                lines.extend(self.render_list(token, 0, width, style_context));
            }

            "table" => {
                lines.extend(self.render_table(token, width, next_token_type, style_context));
            }

            "blockquote" => {
                let theme = self.theme.clone();
                let quote_style = move |text: &str| (theme.quote)(&(theme.italic)(text));
                let quote_style_prefix = Self::get_style_prefix(&quote_style);
                let quote_content_width = width.saturating_sub(2).max(1);

                let quote_context = InlineStyleContext {
                    apply_text: Box::new(|text: &str| text.to_string()),
                    style_prefix: quote_style_prefix.clone(),
                };
                let quote_tokens = token.tokens.clone().unwrap_or_default();
                let mut rendered_quote_lines: Vec<String> = Vec::new();
                for (index, quote_token) in quote_tokens.iter().enumerate() {
                    let next = quote_tokens.get(index + 1).map(|token| token.kind.as_str());
                    rendered_quote_lines.extend(self.render_token(
                        quote_token,
                        quote_content_width,
                        next,
                        Some(&quote_context),
                    ));
                }
                while rendered_quote_lines
                    .last()
                    .is_some_and(|line| line.is_empty())
                {
                    rendered_quote_lines.pop();
                }

                for quote_line in rendered_quote_lines {
                    let styled_line = if quote_style_prefix.is_empty() {
                        quote_style(&quote_line)
                    } else {
                        quote_style(
                            &quote_line.replace("\x1b[0m", &format!("\x1b[0m{quote_style_prefix}")),
                        )
                    };
                    for wrapped in wrap_text_with_ansi(&styled_line, quote_content_width) {
                        lines.push(format!("{}{wrapped}", (self.theme.quote_border)("│ ")));
                    }
                }
                if next_token_type.is_some_and(|kind| kind != "space") {
                    lines.push(String::new());
                }
            }

            "hr" => {
                lines.push((self.theme.hr)(&"─".repeat(width.min(80))));
                if next_token_type.is_some_and(|kind| kind != "space") {
                    lines.push(String::new());
                }
            }

            "html" => {
                lines.push(self.apply_default_style(token.raw.trim()));
            }

            "space" => lines.push(String::new()),

            _ => {
                if let Some(text) = &token.text {
                    lines.push(text.clone());
                }
            }
        }

        lines
    }

    fn render_inline_tokens(
        &mut self,
        tokens: &[Token],
        style_context: Option<&InlineStyleContext>,
    ) -> String {
        let owned_context;
        let context = match style_context {
            Some(context) => context,
            None => {
                owned_context = self.default_inline_style_context();
                &owned_context
            }
        };
        let apply_with_newlines = |text: &str| -> String {
            text.split('\n')
                .map(|segment| (context.apply_text)(segment))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let mut result = String::new();
        for token in tokens {
            match token.kind.as_str() {
                "latex" => {
                    let text = token.text.clone().unwrap_or_default();
                    let rendered = if token.pending != Some(true)
                        && self.options.render_latex != Some(false)
                    {
                        render_latex(&text, RenderLatexOptions::default())
                            .unwrap_or_else(|| token.raw.clone())
                    } else {
                        token.raw.clone()
                    };
                    result.push_str(&apply_with_newlines(&rendered));
                }

                "escape" => {
                    let text = if self.options.preserve_backslash_escapes {
                        token.raw.clone()
                    } else {
                        token.text.clone().unwrap_or_default()
                    };
                    result.push_str(&apply_with_newlines(&text));
                }

                "text" => {
                    if let Some(children) = &token.tokens
                        && !children.is_empty()
                    {
                        result.push_str(&self.render_inline_tokens(children, Some(context)));
                    } else {
                        result.push_str(&apply_with_newlines(
                            &token.text.clone().unwrap_or_default(),
                        ));
                    }
                }

                "paragraph" => {
                    result.push_str(&self.render_inline_tokens(
                        token.tokens.as_deref().unwrap_or_default(),
                        Some(context),
                    ));
                }

                "strong" => {
                    let content = self.render_inline_tokens(
                        token.tokens.as_deref().unwrap_or_default(),
                        Some(context),
                    );
                    result.push_str(&(self.theme.bold)(&content));
                    result.push_str(&context.style_prefix);
                }

                "em" => {
                    let content = self.render_inline_tokens(
                        token.tokens.as_deref().unwrap_or_default(),
                        Some(context),
                    );
                    result.push_str(&(self.theme.italic)(&content));
                    result.push_str(&context.style_prefix);
                }

                "codespan" => {
                    result.push_str(&(self.theme.code)(&token.text.clone().unwrap_or_default()));
                    result.push_str(&context.style_prefix);
                }

                "link" => {
                    let link_text = self.render_inline_tokens(
                        token.tokens.as_deref().unwrap_or_default(),
                        Some(context),
                    );
                    let styled_link = (self.theme.link)(&(self.theme.underline)(&link_text));
                    let href = token.href.clone().unwrap_or_default();
                    if get_capabilities().hyperlinks {
                        result.push_str(&hyperlink(&styled_link, &href));
                        result.push_str(&context.style_prefix);
                    } else {
                        let href_for_comparison =
                            href.strip_prefix("mailto:").unwrap_or(&href).to_string();
                        let text = token.text.clone().unwrap_or_default();
                        if text == href || text == href_for_comparison {
                            result.push_str(&styled_link);
                        } else {
                            result.push_str(&styled_link);
                            result.push_str(&(self.theme.link_url)(&format!(" ({href})")));
                        }
                        result.push_str(&context.style_prefix);
                    }
                }

                "br" => result.push('\n'),

                "del" => {
                    let content = self.render_inline_tokens(
                        token.tokens.as_deref().unwrap_or_default(),
                        Some(context),
                    );
                    result.push_str(&(self.theme.strikethrough)(&content));
                    result.push_str(&context.style_prefix);
                }

                "html" => result.push_str(&apply_with_newlines(&token.raw)),

                _ => {
                    if let Some(text) = &token.text {
                        result.push_str(&apply_with_newlines(text));
                    }
                }
            }
        }

        while !context.style_prefix.is_empty() && result.ends_with(&context.style_prefix) {
            result.truncate(result.len() - context.style_prefix.len());
        }

        result
    }
}

/// Apply a [`DefaultTextStyle`] with `theme`.
fn apply_style(style: &Option<DefaultTextStyle>, theme: &MarkdownTheme, text: &str) -> String {
    let Some(style) = style else {
        return text.to_string();
    };
    let mut styled = text.to_string();
    if let Some(color) = &style.color {
        styled = color(&styled);
    }
    if style.bold {
        styled = (theme.bold)(&styled);
    }
    if style.italic {
        styled = (theme.italic)(&styled);
    }
    if style.strikethrough {
        styled = (theme.strikethrough)(&styled);
    }
    if style.underline {
        styled = (theme.underline)(&styled);
    }
    styled
}

impl Markdown {
    /// `/^(?: {0,3})(\d{1,9}[.)])[ \t]+/`
    fn ordered_list_marker(item: &Token) -> Option<String> {
        let trimmed = item.raw.trim_start_matches(' ');
        if item.raw.len() - trimmed.len() > 3 {
            return None;
        }
        let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() || digits.len() > 9 {
            return None;
        }
        let delimiter = trimmed[digits.len()..].chars().next()?;
        if delimiter != '.' && delimiter != ')' {
            return None;
        }
        if !trimmed[digits.len() + 1..].starts_with([' ', '\t']) {
            return None;
        }
        Some(format!("{digits}{delimiter} "))
    }

    /// `/^(?: {0,3})([-+*])(?:[ \t]+|(?=\r?\n|$))/`
    fn unordered_list_marker(item: &Token) -> Option<String> {
        let trimmed = item.raw.trim_start_matches(' ');
        if item.raw.len() - trimmed.len() > 3 {
            return None;
        }
        let marker = trimmed.chars().next()?;
        if !matches!(marker, '-' | '+' | '*') {
            return None;
        }
        let rest = &trimmed[1..];
        if rest.starts_with([' ', '\t']) || rest.is_empty() || rest.starts_with('\n') {
            return Some(format!("{marker} "));
        }
        None
    }

    fn render_list(
        &mut self,
        token: &Token,
        depth: usize,
        width: usize,
        style_context: Option<&InlineStyleContext>,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let indent = "    ".repeat(depth);
        let start_number = token.start.unwrap_or(1);
        let ordered = token.ordered.unwrap_or(false);
        let items = token.items.clone().unwrap_or_default();
        let loose = token.loose.unwrap_or(false);

        for (index, item) in items.iter().enumerate() {
            let is_last_item = index == items.len() - 1;
            let bullet = if ordered {
                if self.options.preserve_ordered_list_markers {
                    Self::ordered_list_marker(item)
                        .unwrap_or_else(|| format!("{}. ", start_number + index as i64))
                } else {
                    format!("{}. ", start_number + index as i64)
                }
            } else if self.options.preserve_ordered_list_markers {
                Self::unordered_list_marker(item).unwrap_or_else(|| "- ".to_string())
            } else {
                "- ".to_string()
            };
            let task_marker = if item.task == Some(true) {
                format!("[{}] ", if item.checked == Some(true) { "x" } else { " " })
            } else {
                String::new()
            };
            let marker = format!("{bullet}{task_marker}");
            let first_prefix = format!("{indent}{}", (self.theme.list_bullet)(&marker));
            let continuation_prefix = format!("{indent}{}", " ".repeat(visible_width(&marker)));
            let item_width = width.saturating_sub(visible_width(&first_prefix)).max(1);
            let mut rendered_any_line = false;

            for item_token in item.tokens.clone().unwrap_or_default() {
                if item_token.kind == "list" {
                    lines.extend(self.render_list(&item_token, depth + 1, width, style_context));
                    rendered_any_line = true;
                    continue;
                }

                for line in self.render_token(&item_token, item_width, None, style_context) {
                    for wrapped in wrap_text_with_ansi(&line, item_width) {
                        let prefix = if rendered_any_line {
                            &continuation_prefix
                        } else {
                            &first_prefix
                        };
                        lines.push(format!("{prefix}{wrapped}"));
                        rendered_any_line = true;
                    }
                }
            }

            if !rendered_any_line {
                lines.push(first_prefix);
            }

            if loose && !is_last_item {
                lines.push(String::new());
            }
        }

        lines
    }

    /// Visible width of the longest word, capped at `max_width`.
    fn longest_word_width(text: &str, max_width: Option<usize>) -> usize {
        let longest = text
            .split_whitespace()
            .map(visible_width)
            .max()
            .unwrap_or(0);
        match max_width {
            Some(max_width) => longest.min(max_width),
            None => longest,
        }
    }

    fn wrap_cell_text(text: &str, max_width: usize) -> Vec<String> {
        wrap_text_with_ansi(text, max_width.max(1))
    }

    fn render_table(
        &mut self,
        token: &Token,
        available_width: usize,
        next_token_type: Option<&str>,
        style_context: Option<&InlineStyleContext>,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let header: Vec<TableCell> = token.header.clone().unwrap_or_default();
        let rows: Vec<Vec<TableCell>> = token.rows.clone().unwrap_or_default();
        let column_count = header.len();
        if column_count == 0 {
            return lines;
        }

        let border_overhead = 3 * column_count + 1;
        if available_width < border_overhead + column_count {
            let mut fallback = if token.raw.is_empty() {
                Vec::new()
            } else {
                wrap_text_with_ansi(&token.raw, available_width)
            };
            if next_token_type.is_some_and(|kind| kind != "space") {
                fallback.push(String::new());
            }
            return fallback;
        }
        let available_for_cells = available_width - border_overhead;

        let max_unbroken_word_width = 30;
        let mut natural_widths: Vec<usize> = Vec::with_capacity(column_count);
        let mut min_word_widths: Vec<usize> = Vec::with_capacity(column_count);
        for cell in &header {
            let text = self.render_inline_tokens(&cell.tokens, style_context);
            natural_widths.push(visible_width(&text));
            min_word_widths
                .push(Self::longest_word_width(&text, Some(max_unbroken_word_width)).max(1));
        }
        for row in &rows {
            for (index, cell) in row.iter().enumerate().take(column_count) {
                let text = self.render_inline_tokens(&cell.tokens, style_context);
                natural_widths[index] = natural_widths[index].max(visible_width(&text));
                min_word_widths[index] = min_word_widths[index].max(Self::longest_word_width(
                    &text,
                    Some(max_unbroken_word_width),
                ));
            }
        }

        let mut min_column_widths = min_word_widths.clone();
        let mut min_cells_width: usize = min_column_widths.iter().sum();

        if min_cells_width > available_for_cells {
            min_column_widths = vec![1; column_count];
            let remaining = available_for_cells - column_count;
            if remaining > 0 {
                let total_weight: usize = min_word_widths
                    .iter()
                    .map(|width| width.saturating_sub(1))
                    .sum();
                let growth: Vec<usize> = min_word_widths
                    .iter()
                    .map(|width| {
                        let weight = width.saturating_sub(1);
                        (weight * remaining).checked_div(total_weight).unwrap_or(0)
                    })
                    .collect();
                for (index, grow) in growth.iter().enumerate() {
                    min_column_widths[index] += grow;
                }
                let allocated: usize = growth.iter().sum();
                let mut leftover = remaining - allocated;
                let mut index = 0;
                while leftover > 0 && index < column_count {
                    min_column_widths[index] += 1;
                    leftover -= 1;
                    index += 1;
                }
            }
            min_cells_width = min_column_widths.iter().sum();
        }

        let total_natural_width: usize = natural_widths.iter().sum::<usize>() + border_overhead;
        let mut column_widths: Vec<usize> = if total_natural_width <= available_width {
            natural_widths
                .iter()
                .enumerate()
                .map(|(index, width)| (*width).max(min_column_widths[index]))
                .collect()
        } else {
            let total_grow_potential: usize = natural_widths
                .iter()
                .enumerate()
                .map(|(index, width)| width.saturating_sub(min_column_widths[index]))
                .sum();
            let extra_width = available_for_cells.saturating_sub(min_cells_width);
            let mut widths: Vec<usize> = min_column_widths
                .iter()
                .enumerate()
                .map(|(index, min_width)| {
                    let delta = natural_widths[index].saturating_sub(*min_width);
                    let grow = (delta * extra_width)
                        .checked_div(total_grow_potential)
                        .unwrap_or(0);
                    min_width + grow
                })
                .collect();

            let allocated: usize = widths.iter().sum();
            let mut remaining = available_for_cells.saturating_sub(allocated);
            while remaining > 0 {
                let mut grew = false;
                for index in 0..column_count {
                    if remaining == 0 {
                        break;
                    }
                    if widths[index] < natural_widths[index] {
                        widths[index] += 1;
                        remaining -= 1;
                        grew = true;
                    }
                }
                if !grew {
                    break;
                }
            }
            widths
        };
        column_widths.truncate(column_count);

        let border = |left: &str, middle: &str, right: &str, widths: &[usize]| {
            format!(
                "{left}{}{right}",
                widths
                    .iter()
                    .map(|width| "─".repeat(*width))
                    .collect::<Vec<_>>()
                    .join(middle)
            )
        };
        lines.push(border("┌─", "─┬─", "─┐", &column_widths));

        let header_cell_lines: Vec<Vec<String>> = header
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let text = self.render_inline_tokens(&cell.tokens, style_context);
                Self::wrap_cell_text(&text, column_widths[index])
            })
            .collect();
        let header_line_count = header_cell_lines.iter().map(Vec::len).max().unwrap_or(0);
        for line_index in 0..header_line_count {
            let parts: Vec<String> = header_cell_lines
                .iter()
                .enumerate()
                .map(|(column, cell_lines)| {
                    let text = cell_lines.get(line_index).cloned().unwrap_or_default();
                    let padded = format!(
                        "{text}{}",
                        " ".repeat(column_widths[column].saturating_sub(visible_width(&text)))
                    );
                    (self.theme.bold)(&padded)
                })
                .collect();
            lines.push(format!("│ {} │", parts.join(" │ ")));
        }

        let separator_line = border("├─", "─┼─", "─┤", &column_widths);
        lines.push(separator_line.clone());

        for (row_index, row) in rows.iter().enumerate() {
            let row_cell_lines: Vec<Vec<String>> = row
                .iter()
                .enumerate()
                .take(column_count)
                .map(|(index, cell)| {
                    let text = self.render_inline_tokens(&cell.tokens, style_context);
                    Self::wrap_cell_text(&text, column_widths[index])
                })
                .collect();
            let row_line_count = row_cell_lines.iter().map(Vec::len).max().unwrap_or(0);

            for line_index in 0..row_line_count {
                let parts: Vec<String> = row_cell_lines
                    .iter()
                    .enumerate()
                    .map(|(column, cell_lines)| {
                        let text = cell_lines.get(line_index).cloned().unwrap_or_default();
                        format!(
                            "{text}{}",
                            " ".repeat(column_widths[column].saturating_sub(visible_width(&text)))
                        )
                    })
                    .collect();
                lines.push(format!("│ {} │", parts.join(" │ ")));
            }

            if row_index < rows.len() - 1 {
                lines.push(separator_line.clone());
            }
        }

        lines.push(border("└─", "─┴─", "─┘", &column_widths));

        if next_token_type.is_some_and(|kind| kind != "space") {
            lines.push(String::new());
        }
        lines
    }
}

impl Component for Markdown {
    fn render(&mut self, width: usize) -> Vec<Line> {
        if let Some(cached) = &self.cached_lines
            && self.cached_text.as_deref() == Some(self.text.as_str())
            && self.cached_width == Some(width)
        {
            return cached.clone();
        }

        let content_width = width.saturating_sub(self.padding_x * 2).max(1);
        let text = match &self.options.transform {
            Some(transform) => transform(&self.text, content_width),
            None => self.text.clone(),
        };

        if text.trim().is_empty() {
            let result: Vec<Line> = Vec::new();
            self.cached_text = Some(self.text.clone());
            self.cached_width = Some(width);
            self.cached_lines = Some(result.clone());
            return result;
        }

        let normalized_text = text.replace('\t', "   ");
        let tokens = trim_partial_closing_fences(lex(&normalized_text));

        let mut rendered_lines: Vec<String> = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            let next = tokens.get(index + 1).map(|token| token.kind.as_str());
            rendered_lines.extend(self.render_token(token, content_width, next, None));
        }

        let mut wrapped_lines: Vec<String> = Vec::new();
        for line in rendered_lines {
            if is_image_line(&line) {
                wrapped_lines.push(line);
            } else {
                wrapped_lines.extend(wrap_text_with_ansi(&line, content_width));
            }
        }

        let left_margin = " ".repeat(self.padding_x);
        let right_margin = left_margin.clone();
        let mut content_lines: Vec<String> = Vec::new();
        for line in wrapped_lines {
            if is_image_line(&line) {
                content_lines.push(line);
                continue;
            }
            let line_with_margins = format!("{left_margin}{line}{right_margin}");
            match self
                .default_text_style
                .as_ref()
                .and_then(|style| style.bg_color.clone())
            {
                Some(bg_color) => content_lines.push(apply_background_to_line(
                    &line_with_margins,
                    width,
                    |text| bg_color(text),
                )),
                None => {
                    let padding = width.saturating_sub(visible_width(&line_with_margins));
                    content_lines.push(format!("{line_with_margins}{}", " ".repeat(padding)));
                }
            }
        }

        let empty_line = " ".repeat(width);
        let mut empty_lines: Vec<String> = Vec::new();
        for _ in 0..self.padding_y {
            match self
                .default_text_style
                .as_ref()
                .and_then(|style| style.bg_color.clone())
            {
                Some(bg_color) => {
                    empty_lines.push(apply_background_to_line(&empty_line, width, |text| {
                        bg_color(text)
                    }))
                }
                None => empty_lines.push(empty_line.clone()),
            }
        }

        let mut result = empty_lines.clone();
        result.extend(content_lines);
        result.extend(empty_lines);

        // Shared and finished from here on: every non-image line is stored the
        // way `apply_line_resets` would leave it (normalized, reset at the
        // end), so the paint pass skips it and a cache hit keeps its pointer
        // which resets only in the paint path; the reference moved exactly
        // this one component (`../notagent-main-rust/.../markdown.rs:351`).
        let result: Vec<Line> = result
            .into_iter()
            .map(|line| {
                if is_image_line(&line) {
                    Line::from(line)
                } else {
                    Line::from(finish_line(&line))
                }
            })
            .collect();
        self.cached_text = Some(self.text.clone());
        self.cached_width = Some(width);
        self.cached_lines = Some(result.clone());

        if result.is_empty() {
            vec![Line::from("")]
        } else {
            result
        }
    }

    fn invalidate(&mut self) {
        self.invalidate_cache();
    }
}

/// Trim a streamed partial closing fence so code blocks do not flicker.
fn trim_partial_closing_fences(mut tokens: Vec<Token>) -> Vec<Token> {
    fn trim(tokens: &mut [Token]) {
        let Some(token) = tokens.last_mut() else {
            return;
        };
        if token.kind == "list" {
            if let Some(items) = token.items.as_mut()
                && let Some(last) = items.last_mut()
                && let Some(item_tokens) = last.tokens.as_mut()
            {
                trim(item_tokens);
            }
            return;
        }
        if token.kind == "blockquote" {
            if let Some(child) = token.tokens.as_mut() {
                trim(child);
            }
            return;
        }
        if token.kind != "code" {
            return;
        }

        let marker: String = token
            .raw
            .chars()
            .take_while(|character| *character == '`' || *character == '~')
            .collect();
        let Some(last_line) = token.raw.split('\n').next_back() else {
            return;
        };
        let marker_character = marker.chars().next();
        if marker.len() < 3
            || last_line.is_empty()
            || last_line.len() >= marker.len()
            || marker_character.is_none_or(|character| !last_line.chars().all(|c| c == character))
        {
            return;
        }
        if let Some(text) = token.text.as_mut() {
            let trimmed = text[..text.len().saturating_sub(last_line.len())]
                .trim_end_matches('\n')
                .to_string();
            *text = trimmed;
        }
    }
    trim(&mut tokens);
    tokens
}
