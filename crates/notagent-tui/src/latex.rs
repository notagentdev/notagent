use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::latex_tables;
use crate::utils::visible_width;

const NEGATIVE_SPACE: &str = "\u{0}";
const NAMED_OPERATOR_START: &str = "\u{f0004}";
const NAMED_OPERATOR_END: &str = "\u{f0005}";
const LAYOUT_MARKER_START: &str = "\u{f0000}";
const LAYOUT_MARKER_END: &str = "\u{f0001}";
const PROTECTED_SPACE: &str = "\u{f0002}";

fn map(table: &'static [(&'static str, &'static str)]) -> HashMap<&'static str, &'static str> {
    table.iter().copied().collect()
}

fn set(table: &'static [&'static str]) -> HashSet<&'static str> {
    table.iter().copied().collect()
}

macro_rules! lazy_map {
    ($name:ident, $table:expr) => {
        fn $name() -> &'static HashMap<&'static str, &'static str> {
            static CELL: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
            CELL.get_or_init(|| map($table))
        }
    };
}

macro_rules! lazy_set {
    ($name:ident, $table:expr) => {
        fn $name() -> &'static HashSet<&'static str> {
            static CELL: OnceLock<HashSet<&'static str>> = OnceLock::new();
            CELL.get_or_init(|| set($table))
        }
    };
}

lazy_map!(symbols, latex_tables::SYMBOLS);
lazy_map!(negated_symbols, latex_tables::NEGATED_SYMBOLS);
lazy_map!(blackboard, latex_tables::BLACKBOARD);
lazy_map!(superscripts, latex_tables::SUPERSCRIPTS);
lazy_map!(subscripts, latex_tables::SUBSCRIPTS);
lazy_map!(accents, latex_tables::ACCENTS);
lazy_set!(named_operators, latex_tables::NAMED_OPERATORS);
lazy_set!(limit_operators, latex_tables::LIMIT_OPERATORS);
lazy_set!(display_limit_symbols, latex_tables::DISPLAY_LIMIT_SYMBOLS);
lazy_set!(relation_commands, latex_tables::RELATION_COMMANDS);
lazy_set!(spacing_commands, latex_tables::SPACING_COMMANDS);
lazy_set!(
    negative_spacing_commands,
    latex_tables::NEGATIVE_SPACING_COMMANDS
);
lazy_set!(ignored_commands, latex_tables::IGNORED_COMMANDS);
lazy_set!(size_commands, latex_tables::SIZE_COMMANDS);
lazy_set!(plain_wrappers, latex_tables::PLAIN_WRAPPERS);

/// Replace every character through `replacements`, or fail if one is missing.
fn replace_characters(
    value: &str,
    replacements: &HashMap<&'static str, &'static str>,
) -> Option<String> {
    let mut result = String::new();
    for character in value.chars() {
        let mut buffer = [0u8; 4];
        let key: &str = character.encode_utf8(&mut buffer);
        result.push_str(replacements.get(key)?);
    }
    Some(result)
}

/// `value.replace(/\s*([=+-])\s*/g, "$1")`.
fn strip_spaces_around_operators(value: &str) -> String {
    let characters: Vec<char> = value.chars().collect();
    let mut result = String::new();
    let mut index = 0;
    while index < characters.len() {
        let mut lookahead = index;
        while lookahead < characters.len() && characters[lookahead].is_whitespace() {
            lookahead += 1;
        }
        if lookahead < characters.len() && matches!(characters[lookahead], '=' | '+' | '-') {
            result.push(characters[lookahead]);
            index = lookahead + 1;
            while index < characters.len() && characters[index].is_whitespace() {
                index += 1;
            }
            continue;
        }
        result.push(characters[index]);
        index += 1;
    }
    result
}

/// Format a sub- or superscript.
fn format_script(value: &str, subscript: bool) -> String {
    let value = value.trim();
    let replacements = if subscript {
        subscripts()
    } else {
        superscripts()
    };
    if let Some(unicode) = replace_characters(&strip_spaces_around_operators(value), replacements) {
        return unicode;
    }

    let prefix = if subscript { "_" } else { "^" };
    if value.chars().count() == 1
        || (subscript && !value.is_empty() && value.chars().all(|c| c.is_ascii_alphabetic()))
    {
        return format!("{prefix}{value}");
    }
    format!("{prefix}({value})")
}

/// `/^[\p{L}\p{N}.]+$/u`.
fn is_simple_value(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_alphabetic() || character.is_numeric() || character == '.'
        })
}

/// `/^[\p{N}.]+$/u`.
fn is_numeric_value(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_numeric() || character == '.')
}

fn format_fraction(numerator: &str, denominator: &str) -> String {
    let numerator = numerator.trim();
    let denominator = denominator.trim();
    let simple_numerator = is_simple_value(numerator);
    let simple_denominator = is_numeric_value(denominator) || denominator.chars().count() == 1;
    format!(
        "{}/{}",
        if simple_numerator {
            numerator.to_string()
        } else {
            format!("({numerator})")
        },
        if simple_denominator {
            denominator.to_string()
        } else {
            format!("({denominator})")
        }
    )
}

fn format_root(value: &str, symbol: &str) -> String {
    let value = value.trim();
    if is_simple_value(value) {
        format!("{symbol}{value}")
    } else {
        format!("{symbol}({value})")
    }
}

/// `NAMED_OPERATOR_LEFT_SPACING_PATTERN`: letter, digit, closing bracket or a
/// layout marker end directly before a named operator.
fn is_left_spacing_context(character: char) -> bool {
    character.is_alphabetic()
        || character.is_numeric()
        || matches!(character, ')' | ']' | '}' | '\u{f0001}')
}

/// `NAMED_OPERATOR_RIGHT_SPACING_PATTERN`.
fn is_right_spacing_context(character: char) -> bool {
    character.is_alphabetic() || character.is_numeric() || matches!(character, '√' | '\u{f0000}')
}

fn normalize_output(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let characters: Vec<char> = value.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character == '\u{f0004}' {
            // The left spacing rule looks at the character before the marker.
            if result
                .chars()
                .next_back()
                .is_some_and(is_left_spacing_context)
            {
                result.push(' ');
            }
            index += 1;
            continue;
        }
        if character == '\u{f0005}' {
            if characters
                .get(index + 1)
                .copied()
                .is_some_and(is_right_spacing_context)
            {
                result.push(' ');
            }
            index += 1;
            continue;
        }
        result.push(character);
        index += 1;
    }

    let lines: Vec<String> = result
        .split('\n')
        .map(|line| collapse_spaces(line).trim().to_string())
        .collect();
    let line_count = lines.len();
    lines
        .into_iter()
        .enumerate()
        .filter(|(index, line)| !line.is_empty() || (*index > 0 && *index < line_count - 1))
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// `line.replace(/[ \t]+/g, " ")`.
fn collapse_spaces(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut in_run = false;
    for character in line.chars() {
        if character == ' ' || character == '\t' {
            if !in_run {
                result.push(' ');
                in_run = true;
            }
            continue;
        }
        in_run = false;
        result.push(character);
    }
    result
}

/// A stacked fraction.
#[derive(Clone, Debug)]
struct FractionNode {
    numerator: String,
    denominator: String,
}

/// A large operator with optional limits.
#[derive(Clone, Debug)]
struct OperatorNode {
    operator: String,
    lower: Option<String>,
    upper: Option<String>,
}

/// A rendered matrix block.
#[derive(Clone, Debug)]
struct MatrixNode {
    lines: Vec<String>,
    baseline: usize,
}

#[derive(Clone, Debug)]
enum LayoutNode {
    Fraction(FractionNode),
    Operator(OperatorNode),
    Matrix(MatrixNode),
}

/// A laid out block with its baseline row.
#[derive(Clone, Debug)]
struct Layout {
    lines: Vec<String>,
    width: usize,
    baseline: usize,
}

fn pad_layout_line(line: &str, width: usize, centered: bool) -> String {
    let padding = width.saturating_sub(visible_width(line));
    let left = if centered { padding / 2 } else { 0 };
    format!("{}{line}{}", " ".repeat(left), " ".repeat(padding - left))
}

fn join_layouts(layouts: &[Layout]) -> Layout {
    if layouts.is_empty() {
        return Layout {
            lines: vec![String::new()],
            width: 0,
            baseline: 0,
        };
    }
    let baseline = layouts
        .iter()
        .map(|layout| layout.baseline)
        .max()
        .expect("layouts not empty");
    let below = layouts
        .iter()
        .map(|layout| layout.lines.len() - layout.baseline - 1)
        .max()
        .expect("layouts not empty");

    let mut lines: Vec<String> = Vec::new();
    for row in 0..=baseline + below {
        let mut line = String::new();
        for layout in layouts {
            let source_row = row as i64 - baseline as i64 + layout.baseline as i64;
            if source_row >= 0 && (source_row as usize) < layout.lines.len() {
                line.push_str(&pad_layout_line(
                    &layout.lines[source_row as usize],
                    layout.width,
                    false,
                ));
            } else {
                line.push_str(&" ".repeat(layout.width));
            }
        }
        lines.push(line.trim_end().to_string());
    }

    Layout {
        width: layouts.iter().map(|layout| layout.width).sum(),
        lines,
        baseline,
    }
}

/// Find the layout markers of `line` as `(start, end, index)` byte spans.
fn layout_markers(line: &str) -> Vec<(usize, usize, usize)> {
    let mut markers = Vec::new();
    let mut search_from = 0;
    while let Some(relative) = line[search_from..].find(LAYOUT_MARKER_START) {
        let start = search_from + relative;
        let digits_start = start + LAYOUT_MARKER_START.len();
        let digits: String = line[digits_start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let end_marker = digits_start + digits.len();
        if digits.is_empty() || !line[end_marker..].starts_with(LAYOUT_MARKER_END) {
            search_from = digits_start;
            continue;
        }
        let end = end_marker + LAYOUT_MARKER_END.len();
        markers.push((start, end, digits.parse().expect("digits parse")));
        search_from = end;
    }
    markers
}

fn render_layout(source: &str, nodes: &[LayoutNode]) -> Layout {
    let mut rendered_lines: Vec<String> = Vec::new();
    let mut first_baseline = 0;

    for source_line in source.split('\n') {
        let mut layouts: Vec<Layout> = Vec::new();
        let mut position = 0;
        let mut previous_node: Option<&LayoutNode> = None;

        for (start, end, node_index) in layout_markers(source_line) {
            let Some(node) = nodes.get(node_index) else {
                continue;
            };
            if start > position {
                let sliced = &source_line[position..start];
                let trimmed = if previous_node.is_some() {
                    sliced.trim_start()
                } else {
                    sliced
                }
                .trim_end();
                let preserve_leading_space = matches!(previous_node, Some(LayoutNode::Matrix(_)))
                    && sliced.starts_with(char::is_whitespace);
                let preserve_trailing_space =
                    matches!(node, LayoutNode::Matrix(_)) && sliced.ends_with(char::is_whitespace);
                let text = if trimmed.is_empty() {
                    if preserve_leading_space || preserve_trailing_space {
                        " ".to_string()
                    } else {
                        String::new()
                    }
                } else {
                    format!(
                        "{}{trimmed}{}",
                        if preserve_leading_space { " " } else { "" },
                        if preserve_trailing_space { " " } else { "" }
                    )
                };
                layouts.push(Layout {
                    width: visible_width(&text),
                    lines: vec![text],
                    baseline: 0,
                });
            }

            match node {
                LayoutNode::Fraction(fraction) => {
                    let numerator = render_layout(&fraction.numerator, nodes);
                    let denominator = render_layout(&fraction.denominator, nodes);
                    let content_width = numerator.width.max(denominator.width).max(1);
                    let width = content_width + 2;
                    let mut lines: Vec<String> = numerator
                        .lines
                        .iter()
                        .map(|line| pad_layout_line(line, width, true))
                        .collect();
                    lines.push(format!(" {} ", "─".repeat(content_width)));
                    lines.extend(
                        denominator
                            .lines
                            .iter()
                            .map(|line| pad_layout_line(line, width, true)),
                    );
                    layouts.push(Layout {
                        baseline: numerator.lines.len(),
                        lines,
                        width,
                    });
                }
                LayoutNode::Operator(operator) => {
                    let content_width = visible_width(&operator.operator)
                        .max(operator.lower.as_deref().map_or(0, visible_width))
                        .max(operator.upper.as_deref().map_or(0, visible_width));
                    let mut lines: Vec<String> = Vec::new();
                    if let Some(upper) = &operator.upper {
                        lines.push(format!("{} ", pad_layout_line(upper, content_width, true)));
                    }
                    lines.push(format!(
                        "{} ",
                        pad_layout_line(&operator.operator, content_width, true)
                    ));
                    if let Some(lower) = &operator.lower {
                        lines.push(format!("{} ", pad_layout_line(lower, content_width, true)));
                    }
                    layouts.push(Layout {
                        lines,
                        width: content_width + 1,
                        baseline: usize::from(operator.upper.is_some()),
                    });
                }
                LayoutNode::Matrix(matrix) => {
                    let width = matrix
                        .lines
                        .iter()
                        .map(|line| visible_width(line))
                        .max()
                        .unwrap_or(0);
                    layouts.push(Layout {
                        lines: matrix
                            .lines
                            .iter()
                            .map(|line| pad_layout_line(line, width, false))
                            .collect(),
                        width,
                        baseline: matrix.baseline,
                    });
                }
            }

            position = end;
            previous_node = Some(node);
        }

        if position < source_line.len() {
            let sliced = &source_line[position..];
            let trimmed = if previous_node.is_some() {
                sliced.trim_start()
            } else {
                sliced
            };
            let text = if matches!(previous_node, Some(LayoutNode::Matrix(_)))
                && sliced.starts_with(char::is_whitespace)
            {
                format!(" {trimmed}")
            } else {
                trimmed.to_string()
            };
            layouts.push(Layout {
                width: visible_width(&text),
                lines: vec![text],
                baseline: 0,
            });
        }

        let line_layout = join_layouts(&layouts);
        if rendered_lines.is_empty() {
            first_baseline = line_layout.baseline;
        }
        rendered_lines.extend(line_layout.lines);
    }

    Layout {
        width: rendered_lines
            .iter()
            .map(|line| visible_width(line))
            .max()
            .unwrap_or(0),
        lines: rendered_lines,
        baseline: first_baseline,
    }
}

/// Options of [`render_latex`].
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderLatexOptions {
    /// Stack fractions and operator limits vertically for display math.
    pub display: bool,
}

/// Render a basic LaTeX math expression as Unicode text.
/// Returns `None` when the expression uses unsupported or malformed syntax.
pub fn render_latex(source: &str, options: RenderLatexOptions) -> Option<String> {
    let mut layout_nodes: Vec<LayoutNode> = Vec::new();
    let rendered = LatexParser::new(source, options.display).render(&mut layout_nodes)?;
    if layout_nodes.is_empty() {
        return Some(rendered.replace(PROTECTED_SPACE, " "));
    }

    let lines = render_layout(&rendered, &layout_nodes).lines;
    let indentation = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    Some(
        lines
            .iter()
            .map(|line| line[indentation.min(line.len())..].trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .replace(PROTECTED_SPACE, " "),
    )
}

/// Recursive-descent parser over the LaTeX source.
struct LatexParser<'a> {
    source: &'a str,
    characters: Vec<char>,
    /// Byte offset of each character, plus the total length as a sentinel.
    offsets: Vec<usize>,
    display: bool,
    position: usize,
    supported: bool,
    stack_fractions: bool,
}

impl<'a> LatexParser<'a> {
    fn new(source: &'a str, display: bool) -> Self {
        let characters: Vec<char> = source.chars().collect();
        let mut offsets: Vec<usize> = Vec::with_capacity(characters.len() + 1);
        let mut offset = 0;
        for character in &characters {
            offsets.push(offset);
            offset += character.len_utf8();
        }
        offsets.push(offset);
        Self {
            source,
            characters,
            offsets,
            display,
            position: 0,
            supported: true,
            stack_fractions: true,
        }
    }

    /// Remaining source from the current position.
    fn rest(&self) -> &'a str {
        &self.source[self.offsets[self.position]..]
    }

    fn at(&self, position: usize) -> Option<char> {
        self.characters.get(position).copied()
    }

    fn current(&self) -> Option<char> {
        self.at(self.position)
    }

    fn render(mut self, layout_nodes: &mut Vec<LayoutNode>) -> Option<String> {
        let rendered = self.parse_sequence(None, layout_nodes);
        if !self.supported || self.position != self.characters.len() {
            return None;
        }
        Some(normalize_output(&rendered))
    }

    fn parse_sequence(
        &mut self,
        end_character: Option<char>,
        nodes: &mut Vec<LayoutNode>,
    ) -> String {
        let mut result = String::new();
        while self.position < self.characters.len() {
            let character = self.characters[self.position];
            if end_character == Some(character) {
                self.position += 1;
                return result;
            }

            if character == '}' {
                self.supported = false;
                return result;
            }

            if character == '{' {
                self.position += 1;
                result.push_str(&self.parse_sequence(Some('}'), nodes));
                continue;
            }

            if character == '\\' {
                let command = self.parse_command(nodes);
                if command == NEGATIVE_SPACE {
                    result = result.trim_end().to_string();
                    if let Some(stripped) = result.strip_suffix(NAMED_OPERATOR_END) {
                        result = stripped.to_string();
                    }
                } else {
                    result.push_str(&command);
                }
                continue;
            }

            if character == '^' || character == '_' {
                self.position += 1;
                result = result.trim_end().to_string();
                let argument = self.parse_required_argument(false, nodes);
                let script = format_script(&argument, character == '_');
                if let Some(stripped) = result.strip_suffix(NAMED_OPERATOR_END) {
                    result = format!("{stripped}{script}{NAMED_OPERATOR_END}");
                } else {
                    result.push_str(&script);
                }
                continue;
            }

            if character.is_whitespace() {
                result.push_str(&self.parse_whitespace());
                continue;
            }

            if character == '=' || character == '<' || character == '>' {
                result = format!("{} {character} ", result.trim_end());
                self.position += 1;
                continue;
            }

            if character == '&' {
                self.position += 1;
                continue;
            }

            if character == '~' {
                self.position += 1;
                result.push(' ');
                continue;
            }

            if character == '.'
                && let Some(node_index) = trailing_layout_marker(&result)
                && let Some(LayoutNode::Matrix(matrix)) = nodes.get_mut(node_index)
            {
                let last_line = matrix.lines.len() - 1;
                matrix.lines[last_line].push(character);
                self.position += 1;
                continue;
            }

            result.push(character);
            self.position += 1;
        }

        if end_character.is_some() {
            self.supported = false;
        }
        result
    }

    fn parse_whitespace(&mut self) -> String {
        while self
            .current()
            .is_some_and(|character| character.is_whitespace())
        {
            self.position += 1;
        }
        " ".to_string()
    }

    fn parse_command(&mut self, nodes: &mut Vec<LayoutNode>) -> String {
        self.position += 1;
        let Some(first) = self.current() else {
            self.supported = false;
            return String::new();
        };

        if first == '\n' || first == '\r' {
            self.position += 1;
            if first == '\r' && self.current() == Some('\n') {
                self.position += 1;
            }
            return " ".to_string();
        }

        let command: String = if first.is_ascii_alphabetic() {
            let start = self.position;
            while self
                .current()
                .is_some_and(|character| character.is_ascii_alphabetic())
            {
                self.position += 1;
            }
            self.characters[start..self.position].iter().collect()
        } else {
            self.position += 1;
            first.to_string()
        };

        if command == "\\" {
            return "\n".to_string();
        }
        if spacing_commands().contains(command.as_str()) {
            return " ".to_string();
        }
        if negative_spacing_commands().contains(command.as_str()) {
            return NEGATIVE_SPACE.to_string();
        }
        if ignored_commands().contains(command.as_str()) {
            return String::new();
        }
        if matches!(command.as_str(), "{" | "}" | "$" | "%" | "#" | "_" | "&") {
            return command;
        }
        if command == "|" {
            return "‖".to_string();
        }
        if command == "not" {
            let value = self
                .parse_required_argument(false, nodes)
                .trim()
                .to_string();
            if let Some(negated) = negated_symbols().get(value.as_str()) {
                return format!(" {negated} ");
            }
            let mut characters = value.chars();
            let Some(first) = characters.next() else {
                self.supported = false;
                return String::new();
            };
            return format!(" {first}\u{338}{} ", characters.collect::<String>());
        }
        if limit_operators().contains(command.as_str()) {
            return self.parse_operator(&command, true, true, true, nodes);
        }

        if let Some(symbol) = symbols().get(command.as_str()).copied() {
            if display_limit_symbols().contains(command.as_str()) {
                return self.parse_operator(symbol, false, true, false, nodes);
            }
            return if command == "cdot"
                || command == "times"
                || relation_commands().contains(command.as_str())
            {
                format!(" {symbol} ")
            } else {
                symbol.to_string()
            };
        }
        if named_operators().contains(command.as_str()) {
            return format!("{NAMED_OPERATOR_START}{command}{NAMED_OPERATOR_END}");
        }
        if size_commands().contains(command.as_str()) {
            return String::new();
        }
        if command == "left" || command == "middle" || command == "right" {
            if self.current() == Some('.') {
                self.position += 1;
            }
            return String::new();
        }
        if command == "frac" || command == "dfrac" || command == "tfrac" {
            let should_stack = self.display && self.stack_fractions && command != "tfrac";
            let numerator = self.parse_required_argument(!should_stack, nodes);
            let denominator = self.parse_required_argument(!should_stack, nodes);
            if should_stack {
                nodes.push(LayoutNode::Fraction(FractionNode {
                    numerator: normalize_output(&numerator),
                    denominator: normalize_output(&denominator),
                }));
                return format!(
                    "{LAYOUT_MARKER_START}{}{LAYOUT_MARKER_END}",
                    nodes.len() - 1
                );
            }
            return format_fraction(&numerator, &denominator);
        }
        if command == "sqrt" {
            let degree = self
                .parse_optional_argument(nodes)
                .map(|value| value.trim().to_string());
            let value = self.parse_required_argument(true, nodes);
            return match degree.as_deref() {
                None | Some("2") => format_root(&value, "√"),
                Some("3") => format_root(&value, "∛"),
                Some("4") => format_root(&value, "∜"),
                Some(degree) => {
                    format!(
                        "{}{}",
                        format_script(degree, false),
                        format_root(&value, "√")
                    )
                }
            };
        }
        if command == "boxed" || command == "fbox" {
            return format!("[{}]", self.parse_required_argument(true, nodes).trim());
        }
        if command == "binom" || command == "dbinom" || command == "tbinom" {
            let first = self.parse_required_argument(true, nodes);
            let second = self.parse_required_argument(true, nodes);
            return format!("({first} choose {second})");
        }
        if let Some(accent) = accents().get(command.as_str()).copied() {
            let value = self.parse_required_argument(true, nodes);
            return if value.chars().count() == 1 {
                format!("{value}{accent}")
            } else {
                format!("{command}({value})")
            };
        }
        if command == "mathbb" {
            let value = self.parse_required_argument(true, nodes);
            return value
                .chars()
                .map(|character| {
                    let mut buffer = [0u8; 4];
                    let key: &str = character.encode_utf8(&mut buffer);
                    blackboard()
                        .get(key)
                        .map_or_else(|| character.to_string(), ToString::to_string)
                })
                .collect();
        }
        if command == "operatorname" {
            let starred = self.current() == Some('*');
            if starred {
                self.position += 1;
            }
            let operator = normalize_output(&self.parse_required_argument(true, nodes))
                .trim()
                .to_string();
            return self.parse_operator(&operator, true, starred, true, nodes);
        }
        if command == "mod" || command == "bmod" {
            return " mod ".to_string();
        }
        if command == "pmod" || command == "pod" {
            let value = self.parse_required_argument(true, nodes).trim().to_string();
            return if command == "pmod" {
                format!(" (mod {value})")
            } else {
                format!(" ({value})")
            };
        }
        if command == "overset" || command == "stackrel" {
            let upper = self.parse_required_argument(true, nodes);
            let value = self.parse_required_argument(true, nodes).trim().to_string();
            return format!("{value}{}", format_script(&upper, false));
        }
        if command == "underset" {
            let lower = self.parse_required_argument(true, nodes);
            let value = self.parse_required_argument(true, nodes).trim().to_string();
            return format!("{value}{}", format_script(&lower, true));
        }
        if plain_wrappers().contains(command.as_str()) {
            let value = self.parse_required_argument(true, nodes);
            return if command.starts_with("text") || command == "mbox" {
                value
            } else {
                value.trim().to_string()
            };
        }
        if command == "begin" {
            return self.parse_environment(nodes);
        }
        if command == "end" {
            self.supported = false;
            return String::new();
        }

        self.supported = false;
        format!("\\{command}")
    }

    fn parse_operator(
        &mut self,
        operator: &str,
        inline_lower_bracket: bool,
        display_limits: bool,
        spaced: bool,
        nodes: &mut Vec<LayoutNode>,
    ) -> String {
        let mut use_display_limits = display_limits;
        let mut modifier_position = self.position;
        while self
            .at(modifier_position)
            .is_some_and(|character| character == ' ' || character == '\t')
        {
            modifier_position += 1;
        }
        let rest = &self.source[self.offsets[modifier_position]..];
        for (name, limits) in [("\\limits", true), ("\\nolimits", false)] {
            if let Some(after) = rest.strip_prefix(name)
                && !after.starts_with(|character: char| character.is_ascii_alphabetic())
            {
                use_display_limits = limits;
                self.position = modifier_position + name.chars().count();
                break;
            }
        }

        let mut lower: Option<String> = None;
        let mut upper: Option<String> = None;
        loop {
            let mut script_position = self.position;
            while self
                .at(script_position)
                .is_some_and(|character| character == ' ' || character == '\t')
            {
                script_position += 1;
            }
            let kind = self.at(script_position);
            if kind != Some('_') && kind != Some('^') {
                break;
            }
            self.position = script_position + 1;
            let value =
                normalize_output(&self.parse_required_argument(false, nodes)).replace(' ', "");
            if kind == Some('_') {
                if lower.is_some() {
                    self.supported = false;
                }
                lower = Some(value);
            } else {
                if upper.is_some() {
                    self.supported = false;
                }
                upper = Some(value);
            }
        }

        if self.display && use_display_limits && (lower.is_some() || upper.is_some()) {
            nodes.push(LayoutNode::Operator(OperatorNode {
                operator: operator.to_string(),
                lower,
                upper,
            }));
            return format!(
                "{LAYOUT_MARKER_START}{}{LAYOUT_MARKER_END}",
                nodes.len() - 1
            );
        }

        let mut rendered = operator.to_string();
        if let Some(lower) = &lower {
            if inline_lower_bracket {
                rendered.push_str(&format!("[{lower}]"));
            } else {
                rendered.push_str(&format_script(lower, true));
            }
        }
        if let Some(upper) = &upper {
            rendered.push_str(&format_script(upper, false));
        }
        if spaced {
            format!(" {rendered} ")
        } else {
            rendered
        }
    }

    fn parse_required_argument(
        &mut self,
        stack_fractions: bool,
        nodes: &mut Vec<LayoutNode>,
    ) -> String {
        let previous = self.stack_fractions;
        self.stack_fractions = previous && stack_fractions;
        let value = self.parse_required_argument_value(nodes);
        self.stack_fractions = previous;
        value
    }

    fn parse_required_argument_value(&mut self, nodes: &mut Vec<LayoutNode>) -> String {
        while self
            .current()
            .is_some_and(|character| character.is_whitespace())
        {
            self.position += 1;
        }
        let Some(character) = self.current() else {
            self.supported = false;
            return String::new();
        };
        if character == '{' {
            self.position += 1;
            return self.parse_sequence(Some('}'), nodes);
        }
        if character == '\\' {
            return self.parse_command(nodes);
        }
        self.position += 1;
        character.to_string()
    }

    fn parse_optional_argument(&mut self, nodes: &mut Vec<LayoutNode>) -> Option<String> {
        while self
            .current()
            .is_some_and(|character| character == ' ' || character == '\t')
        {
            self.position += 1;
        }
        if self.current() != Some('[') {
            return None;
        }
        let end = self.characters[self.position + 1..]
            .iter()
            .position(|character| *character == ']')
            .map(|offset| self.position + 1 + offset);
        let Some(end) = end else {
            self.supported = false;
            return None;
        };
        let value: String = self.characters[self.position + 1..end].iter().collect();
        self.position = end + 1;
        Some(self.render_nested(&value, true, nodes))
    }

    fn read_raw_group(&mut self) -> Option<String> {
        while self
            .current()
            .is_some_and(|character| character == ' ' || character == '\t')
        {
            self.position += 1;
        }
        if self.current() != Some('{') {
            self.supported = false;
            return None;
        }

        self.position += 1;
        let start = self.position;
        let mut depth = 1;
        while self.position < self.characters.len() {
            let character = self.characters[self.position];
            if character == '\\' {
                self.position += 2;
                continue;
            }
            if character == '{' {
                depth += 1;
            }
            if character == '}' {
                depth -= 1;
            }
            if depth == 0 {
                let value: String = self.characters[start..self.position].iter().collect();
                self.position += 1;
                return Some(value);
            }
            self.position += 1;
        }
        self.supported = false;
        None
    }

    /// `body.split(/\\\\(?:\[[^\]\n]*\])?/)`.
    fn split_environment_rows(body: &str) -> Vec<String> {
        let mut rows: Vec<String> = Vec::new();
        let mut current = String::new();
        let characters: Vec<char> = body.chars().collect();
        let mut index = 0;
        while index < characters.len() {
            if characters[index] == '\\' && characters.get(index + 1) == Some(&'\\') {
                let mut next = index + 2;
                if characters.get(next) == Some(&'[') {
                    let mut scan = next + 1;
                    while scan < characters.len()
                        && characters[scan] != ']'
                        && characters[scan] != '\n'
                    {
                        scan += 1;
                    }
                    if characters.get(scan) == Some(&']') {
                        next = scan + 1;
                    }
                }
                rows.push(std::mem::take(&mut current));
                index = next;
                continue;
            }
            current.push(characters[index]);
            index += 1;
        }
        rows.push(current);
        rows
    }

    /// `body.replace(/^\s*\{[^}]*\}/, "")`.
    fn strip_leading_group(body: &str) -> String {
        let trimmed = body.trim_start();
        if !trimmed.starts_with('{') {
            return body.to_string();
        }
        match trimmed.find('}') {
            Some(end) => trimmed[end + 1..].to_string(),
            None => body.to_string(),
        }
    }

    fn parse_environment(&mut self, nodes: &mut Vec<LayoutNode>) -> String {
        let Some(environment) = self.read_raw_group().filter(|value| !value.is_empty()) else {
            return String::new();
        };
        let end_marker = format!("\\end{{{environment}}}");
        let Some(relative) = self.rest().find(&end_marker) else {
            self.supported = false;
            return String::new();
        };
        let body = self.rest()[..relative].to_string();
        self.position += body.chars().count() + end_marker.chars().count();

        if environment == "equation" || environment == "equation*" || environment == "displaymath" {
            return self.render_nested(&body, true, nodes).trim().to_string();
        }

        if matches!(
            environment.as_str(),
            "aligned"
                | "align"
                | "align*"
                | "alignedat"
                | "alignat"
                | "alignat*"
                | "gather"
                | "gathered"
                | "multline"
                | "multline*"
                | "split"
        ) {
            let aligned_at = matches!(environment.as_str(), "alignedat" | "alignat" | "alignat*");
            let aligned_body = if aligned_at {
                Self::strip_leading_group(&body)
            } else {
                body.clone()
            };
            return Self::split_environment_rows(&aligned_body)
                .iter()
                .map(|row| {
                    let cells: Vec<&str> = row.split('&').collect();
                    let source = if aligned_at {
                        cells
                            .chunks(2)
                            .map(|chunk| chunk.concat())
                            .collect::<Vec<_>>()
                            .join(" ")
                    } else {
                        cells.concat()
                    };
                    self.render_nested(&source, true, nodes).trim().to_string()
                })
                .filter(|row| !row.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
        }

        if environment == "cases" || environment == "cases*" {
            let rows: Vec<Vec<String>> = Self::split_environment_rows(&body)
                .iter()
                .map(|row| {
                    row.split('&')
                        .map(|cell| self.render_nested(cell, false, nodes).trim().to_string())
                        .collect::<Vec<_>>()
                })
                .filter(|row: &Vec<String>| row.iter().any(|cell| !cell.is_empty()))
                .collect();
            let row_count = rows.len();
            return rows
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    let value = row
                        .first()
                        .cloned()
                        .unwrap_or_default()
                        .trim_end()
                        .trim_end_matches(',')
                        .to_string();
                    let condition = row.get(1).cloned().unwrap_or_default();
                    let delimiter = if index == 0 {
                        "⎧"
                    } else if index == row_count - 1 {
                        "⎩"
                    } else {
                        "⎨"
                    };
                    let condition_prefix = if starts_with_condition_word(&condition) {
                        " "
                    } else {
                        " if "
                    };
                    format!(
                        "{delimiter} {value}{}",
                        if condition.is_empty() {
                            String::new()
                        } else {
                            format!("{condition_prefix}{condition}")
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
        }

        if matches!(
            environment.as_str(),
            "array"
                | "matrix"
                | "smallmatrix"
                | "pmatrix"
                | "bmatrix"
                | "Bmatrix"
                | "vmatrix"
                | "Vmatrix"
        ) {
            let matrix_body = if environment == "array" {
                Self::strip_leading_group(&body)
            } else {
                body.clone()
            };
            return self.render_matrix(&environment, &matrix_body, nodes);
        }

        self.supported = false;
        body
    }

    fn render_matrix(
        &mut self,
        environment: &str,
        body: &str,
        nodes: &mut Vec<LayoutNode>,
    ) -> String {
        let matrix: Vec<Vec<String>> = Self::split_environment_rows(body)
            .iter()
            .map(|row| {
                row.split('&')
                    .map(|cell| self.render_nested(cell, false, nodes).trim().to_string())
                    .collect::<Vec<_>>()
            })
            .filter(|row: &Vec<String>| row.iter().any(|cell| !cell.is_empty()))
            .collect();
        let column_count = matrix.iter().map(Vec::len).max().unwrap_or(0);
        let column_widths: Vec<usize> = (0..column_count)
            .map(|column| {
                matrix
                    .iter()
                    .map(|row| row.get(column).map_or(0, |cell| visible_width(cell)))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let rows: Vec<String> = matrix
            .iter()
            .map(|row| {
                (0..column_count)
                    .map(|column| {
                        let cell = row.get(column).cloned().unwrap_or_default();
                        let padding = column_widths[column].saturating_sub(visible_width(&cell));
                        format!("{cell}{}", PROTECTED_SPACE.repeat(padding))
                    })
                    .collect::<Vec<_>>()
                    .join(" │ ")
            })
            .collect();

        let lines: Vec<String> = if matches!(environment, "array" | "matrix" | "smallmatrix") {
            rows
        } else {
            let delimiter: Option<[&str; 6]> = match environment {
                "pmatrix" => Some(["⎛", "⎞", "⎜", "⎟", "⎝", "⎠"]),
                "bmatrix" => Some(["⎡", "⎤", "⎢", "⎥", "⎣", "⎦"]),
                "Bmatrix" => Some(["⎧", "⎫", "⎨", "⎬", "⎩", "⎭"]),
                "vmatrix" => Some(["│", "│", "│", "│", "│", "│"]),
                "Vmatrix" => Some(["║", "║", "║", "║", "║", "║"]),
                _ => None,
            };
            let Some(delimiter) = delimiter else {
                self.supported = false;
                return rows.join("\n");
            };
            let row_count = rows.len();
            rows.iter()
                .enumerate()
                .map(|(index, row)| {
                    let (left, right) = if index == 0 {
                        (delimiter[0], delimiter[1])
                    } else if index == row_count - 1 {
                        (delimiter[4], delimiter[5])
                    } else {
                        (delimiter[2], delimiter[3])
                    };
                    format!("{left} {row} {right}")
                })
                .collect()
        };

        if lines.len() <= 1 {
            return lines.first().cloned().unwrap_or_default();
        }
        nodes.push(LayoutNode::Matrix(MatrixNode { lines, baseline: 0 }));
        format!(
            "{LAYOUT_MARKER_START}{}{LAYOUT_MARKER_END}",
            nodes.len() - 1
        )
    }

    fn render_nested(
        &mut self,
        source: &str,
        stack_fractions: bool,
        nodes: &mut Vec<LayoutNode>,
    ) -> String {
        let parser = LatexParser::new(source, self.display && stack_fractions);
        match parser.render(nodes) {
            Some(rendered) => rendered,
            None => {
                self.supported = false;
                source.to_string()
            }
        }
    }
}

/// `/^(?:if|when|for|otherwise)\b/i`.
fn starts_with_condition_word(condition: &str) -> bool {
    let lowered = condition.to_lowercase();
    for word in ["if", "when", "for", "otherwise"] {
        if let Some(rest) = lowered.strip_prefix(word)
            && !rest.starts_with(|character: char| character.is_alphanumeric() || character == '_')
        {
            return true;
        }
    }
    false
}

/// `TRAILING_LAYOUT_MARKER_PATTERN`: index of a marker at the end of `value`.
fn trailing_layout_marker(value: &str) -> Option<usize> {
    let without_end = value.strip_suffix(LAYOUT_MARKER_END)?;
    let start = without_end.rfind(LAYOUT_MARKER_START)?;
    let digits = &without_end[start + LAYOUT_MARKER_START.len()..];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}
