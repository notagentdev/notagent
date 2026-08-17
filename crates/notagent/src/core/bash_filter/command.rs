use std::path::Path;

/// A word of a shell command: the executable, an argument, or an environment
/// assignment. Offsets index into [`ClassifiedCommand::original`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub value: String,
    pub start: usize,
    pub end: usize,
}

/// What a lexed token is, beyond the words a command is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    /// The executable, an argument, an environment assignment, or the target
    /// of a redirect.
    Word,
    /// `|` — the boundary between two stages of a pipeline.
    Pipe,
    /// `&&`, `||`, `;` — the boundary between two commands.
    Operator,
    /// A redirection: `>`, `>>`, `<`, `2>&1`, `&>`, `>|`, `<<`.
    Redirect,
    /// Background `&` and the grouping parentheses.
    Shellism,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedToken {
    kind: TokenKind,
    value: String,
    start: usize,
    end: usize,
}

/// The leading command of a shell line, with everything the shell does around
/// it set aside rather than thrown away.
///
/// A pipeline is not rejected: its first stage is what a filter is chosen from,
/// and the rest travels in [`Self::suffix`] so an execution rewrite can put the
/// line back together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassifiedCommand {
    /// The words of the leading command alone — no redirects, nothing past a
    /// pipe. Offsets index into [`Self::original`].
    pub tokens: Vec<Token>,
    pub executable_index: usize,
    pub effective_index: usize,
    /// The leading command as written, up to its first redirect or boundary.
    pub original: String,
    /// [`Self::original`] with the executable reduced to its basename.
    pub normalized: String,
    /// The rest of the shell line: redirects, further pipeline stages, further
    /// commands. Appended verbatim to an execution rewrite.
    pub suffix: String,
    /// Whether a pipe, an operator, a background `&`, or a subshell follows.
    ///
    /// Such a line still yields a filter for its captured output, but nothing
    /// may be rewritten into it: the stages behind the pipe read what the
    /// first one writes, and changing that changes their meaning.
    pub compound: bool,
}

pub(crate) fn classify(command: &str) -> Option<ClassifiedCommand> {
    if command.contains(['\n', '\r']) {
        return None;
    }
    // Command substitution runs a second command whose output this one is
    // built from. Nothing here can attest to what that command is.
    if contains_substitution(command) {
        return None;
    }
    let parsed = lex(command)?;

    // A heredoc's body is not on this line, and a redirect that names a file
    // sends the very output a filter would work on somewhere else.
    if parsed.iter().enumerate().any(|(index, token)| {
        token.kind == TokenKind::Redirect
            && (token.value.starts_with("<<") || redirect_has_file_target(&parsed, index))
    }) {
        return None;
    }

    let compound = parsed.iter().any(|token| {
        matches!(
            token.kind,
            TokenKind::Pipe | TokenKind::Operator | TokenKind::Shellism
        )
    });

    // The leading command ends where it stops being one: at its first redirect,
    // pipe, operator, or subshell.
    let end = parsed
        .iter()
        .position(|token| token.kind != TokenKind::Word)
        .map_or(command.len(), |index| parsed[index].start);
    let original = command[..end].trim_end().to_string();
    let suffix = command[original.len()..].to_string();

    let tokens = parsed
        .into_iter()
        .take_while(|token| token.kind == TokenKind::Word)
        .map(|token| Token {
            value: token.value,
            start: token.start,
            end: token.end,
        })
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }

    let executable_index = tokens
        .iter()
        .position(|token| !is_environment_assignment(&token.value))?;
    let executable = basename(&tokens[executable_index].value);
    if executable == "sudo" {
        return None;
    }
    let effective_index = wrapper_target(&tokens, executable_index)?;
    let normalized = normalize(&original, &tokens, executable_index);
    Some(ClassifiedCommand {
        tokens,
        executable_index,
        effective_index,
        original,
        normalized,
        suffix,
        compound,
    })
}

/// Quote-aware substitution scan: bash runs backticks and `$(…)` unquoted and
/// inside double quotes but treats single-quoted text literally, while process
/// substitution `<(`/`>(` is unquoted-only.
fn contains_substitution(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if !in_single => {
                index += 2;
                continue;
            }
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'`' if !in_single => return true,
            b'$' if !in_single && matches!(bytes.get(index + 1), Some(b'(') | Some(b'{')) => {
                return true;
            }
            b'<' | b'>' if !in_single && !in_double && bytes.get(index + 1) == Some(&b'(') => {
                return true;
            }
            _ => {}
        }
        index += 1;
    }
    false
}

/// Whether a redirect sends output to a file rather than duplicating or closing
/// a descriptor.
///
/// `2>&1` and `2>&-` rearrange descriptors and leave the output where a filter
/// can still reach it; `> out.txt` does not. `/dev/null` is a discard, not a
/// file the caller means to read back.
fn redirect_has_file_target(tokens: &[ParsedToken], index: usize) -> bool {
    let value = &tokens[index].value;
    if let Some(position) = value.find(">&") {
        let tail = &value[position + 2..];
        if !tail.is_empty()
            && tail
                .chars()
                .all(|character| character.is_ascii_digit() || character == '-')
        {
            return false;
        }
    }
    match tokens.get(index + 1) {
        Some(next) if next.kind == TokenKind::Word => next.value != "/dev/null",
        _ => true,
    }
}

fn lex(command: &str) -> Option<Vec<ParsedToken>> {
    let mut tokens = Vec::new();
    let bytes = command.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index == bytes.len() {
            break;
        }
        let start = index;

        if let Some(operator) = ["&&", "||", ";"]
            .into_iter()
            .find(|operator| command[index..].starts_with(operator))
        {
            index += operator.len();
            tokens.push(ParsedToken {
                kind: TokenKind::Operator,
                value: operator.to_string(),
                start,
                end: index,
            });
            continue;
        }
        // `&>` redirects both streams; a bare `&` backgrounds.
        if bytes[index] == b'&' && matches!(bytes.get(index + 1), Some(b'>')) {
            index = scan_redirect(bytes, index);
            tokens.push(ParsedToken {
                kind: TokenKind::Redirect,
                value: command[start..index].to_string(),
                start,
                end: index,
            });
            continue;
        }
        if matches!(bytes[index], b'&' | b'(' | b')' | b'|') {
            let kind = if bytes[index] == b'|' {
                TokenKind::Pipe
            } else {
                TokenKind::Shellism
            };
            index += 1;
            tokens.push(ParsedToken {
                kind,
                value: command[start..index].to_string(),
                start,
                end: index,
            });
            continue;
        }
        if matches!(bytes[index], b'<' | b'>') {
            index = scan_redirect(bytes, index);
            tokens.push(ParsedToken {
                kind: TokenKind::Redirect,
                value: command[start..index].to_string(),
                start,
                end: index,
            });
            continue;
        }

        let mut value = String::new();
        let mut quote = None;
        let mut redirect = false;
        while index < bytes.len() {
            let character = command[index..].chars().next()?;
            let width = character.len_utf8();
            if let Some(active) = quote {
                if character == active {
                    quote = None;
                    index += width;
                } else if character == '\\' && active == '"' {
                    index += width;
                    let escaped = command[index..].chars().next()?;
                    value.push(escaped);
                    index += escaped.len_utf8();
                } else {
                    value.push(character);
                    index += width;
                }
                continue;
            }
            match character {
                '\'' | '"' => {
                    quote = Some(character);
                    index += width;
                }
                '\\' => {
                    index += width;
                    let escaped = command[index..].chars().next()?;
                    value.push(escaped);
                    index += escaped.len_utf8();
                }
                character if character.is_whitespace() => break,
                // A descriptor number belongs to the redirect that follows it,
                // not to the command: `cargo test 2>&1` has two arguments, not
                // three.
                '<' | '>' if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) => {
                    redirect = true;
                    break;
                }
                '|' | ';' | '&' | '<' | '>' | '(' | ')' => break,
                _ => {
                    value.push(character);
                    index += width;
                }
            }
        }
        if quote.is_some() {
            return None;
        }
        if redirect {
            index = scan_redirect(bytes, index);
            tokens.push(ParsedToken {
                kind: TokenKind::Redirect,
                value: command[start..index].to_string(),
                start,
                end: index,
            });
            continue;
        }
        if value.is_empty() {
            return None;
        }
        tokens.push(ParsedToken {
            kind: TokenKind::Word,
            value,
            start,
            end: index,
        });
    }
    Some(tokens)
}

/// Consumes one redirection operator, descriptor prefix already behind `index`.
fn scan_redirect(bytes: &[u8], mut index: usize) -> usize {
    if bytes.get(index) == Some(&b'&') {
        index += 1;
    }
    while matches!(bytes.get(index), Some(b'<' | b'>')) {
        index += 1;
    }
    match bytes.get(index) {
        Some(b'&') => {
            index += 1;
            while matches!(bytes.get(index), Some(character) if character.is_ascii_digit() || *character == b'-')
            {
                index += 1;
            }
        }
        Some(b'|') => index += 1,
        _ => {}
    }
    index
}

fn wrapper_target(tokens: &[Token], executable_index: usize) -> Option<usize> {
    let executable = basename(&tokens[executable_index].value);
    match executable {
        "npx" | "bunx" => tokens
            .get(executable_index + 1)
            .map(|_| executable_index + 1),
        "pnpm" | "npm" | "yarn"
            if tokens
                .get(executable_index + 1)
                .map(|token| token.value.as_str())
                == Some("exec") =>
        {
            tokens
                .get(executable_index + 2)
                .map(|_| executable_index + 2)
        }
        "pnpm" | "npm" | "yarn" => Some(executable_index),
        _ => Some(executable_index),
    }
}

pub(crate) fn basename(value: &str) -> &str {
    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(value)
}

fn normalize(command: &str, tokens: &[Token], executable_index: usize) -> String {
    let executable = &tokens[executable_index];
    let suffix = &command[executable.end..];
    format!("{}{}", basename(&executable.value), suffix)
}

fn is_environment_assignment(value: &str) -> bool {
    let Some((name, _)) = value.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.chars().enumerate().all(|(index, character)| {
            character == '_'
                || character.is_ascii_alphabetic()
                || (index > 0 && character.is_ascii_digit())
        })
}

pub(crate) fn append_after(command: &str, token: &Token, text: &str) -> String {
    let mut rewritten = String::with_capacity(command.len() + text.len() + 1);
    rewritten.push_str(&command[..token.end]);
    rewritten.push(' ');
    rewritten.push_str(text);
    rewritten.push_str(&command[token.end..]);
    rewritten
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_direct_absolute_environment_and_wrapped_commands() {
        let fixture = [
            "git diff -- src/lib.rs",
            "/usr/local/bin/cargo test --all",
            "RUST_LOG=debug cargo check",
            "npx eslint src",
            "pnpm exec eslint src",
            "npm exec tsc --noEmit",
            "yarn exec playwright test",
            "bunx vitest run",
        ];
        let actual = fixture
            .into_iter()
            .map(|command| {
                classify(command)
                    .map(|value| basename(&value.tokens[value.effective_index].value).to_string())
            })
            .collect::<Vec<_>>();
        let expected = vec![
            Some("git".into()),
            Some("cargo".into()),
            Some("cargo".into()),
            Some("eslint".into()),
            Some("eslint".into()),
            Some("tsc".into()),
            Some("playwright".into()),
            Some("vitest".into()),
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn rejects_unsafe_or_unsupported_shell_syntax() {
        let fixture = [
            "sudo cargo test",
            "cargo test > result",
            "cargo test >> result",
            "cargo test 2> errors.log",
            "(cargo test)",
            "echo $(cargo test)",
            "echo `cargo test`",
            "diff <(cargo test) old",
            "cat <<EOF",
        ];
        let actual = fixture.into_iter().map(classify).collect::<Vec<_>>();
        let expected = vec![None; fixture.len()];
        assert_eq!(actual, expected);
    }

    #[test]
    fn keeps_the_leading_command_of_a_pipeline_and_sets_the_rest_aside() {
        // Step 1: setup: the shapes an agent writes constantly — a pipe, a
        // descriptor duplication, both at once, a discard, a chain
        let fixture = [
            "ls -la src 2>&1 | head -50",
            "cargo test | tee out",
            "cargo test 2>&1",
            "cargo test > /dev/null",
            "cargo test && echo done",
            "cargo test &",
        ];

        // Step 2: actual
        let actual = fixture
            .into_iter()
            .map(|command| {
                let classified = classify(command).expect("classified");
                (classified.original, classified.suffix, classified.compound)
            })
            .collect::<Vec<_>>();

        // Step 3: expected: the leading command alone, everything the shell
        // adds kept verbatim, and `compound` set wherever another stage reads
        // what the first one writes
        let expected = vec![
            ("ls -la src".into(), " 2>&1 | head -50".into(), true),
            ("cargo test".into(), " | tee out".into(), true),
            ("cargo test".into(), " 2>&1".into(), false),
            ("cargo test".into(), " > /dev/null".into(), false),
            ("cargo test".into(), " && echo done".into(), true),
            ("cargo test".into(), " &".into(), true),
        ];

        // Assertions
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_descriptor_number_is_not_an_argument() {
        // Step 1: setup / Step 2: actual
        let classified = classify("ls -la src 2>&1 | head -50").expect("classified");
        let actual = classified
            .tokens
            .iter()
            .map(|token| token.value.clone())
            .collect::<Vec<_>>();

        // Step 3: expected: `2>&1` is a redirect, `head` is another stage
        let expected = vec!["ls".to_string(), "-la".into(), "src".into()];

        // Assertions
        assert_eq!(actual, expected);
    }

    #[test]
    fn preserves_original_arguments_in_normalized_view() {
        let fixture = "A=1 /opt/bin/terraform plan 'a b' --var=x\\ y";
        let actual = classify(fixture).unwrap().normalized;
        let expected = "terraform plan 'a b' --var=x\\ y";
        assert_eq!(actual, expected);
    }
}
