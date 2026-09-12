use std::rc::Rc;

/// Context handed to every transformer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownTransformContext {
    /// Which message the markdown belongs to.
    pub message_type: MarkdownMessageType,
    /// Whether the message is still streaming.
    pub is_streaming: bool,
    /// Width the markdown will be rendered at.
    pub available_width: usize,
}

/// `MarkdownTransformContext["messageType"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownMessageType {
    /// A user message.
    User,
    /// An assistant message.
    Assistant,
    /// The thinking block of an assistant message.
    AssistantThinking,
}

/// Transforms markdown before it is parsed.
pub type MarkdownTransformer = Rc<dyn Fn(&str, &MarkdownTransformContext) -> String>;

/// The transform closure a message component applies before rendering.
pub type MarkdownTransform = Rc<dyn Fn(&str, usize) -> String>;

/// Build the transform closure for one message.
pub fn create_markdown_transform(
    message_type: MarkdownMessageType,
    is_streaming: bool,
    transformers: Vec<MarkdownTransformer>,
) -> MarkdownTransform {
    Rc::new(move |markdown: &str, available_width: usize| {
        apply_markdown_transformers(
            markdown,
            &MarkdownTransformContext {
                message_type,
                is_streaming,
                available_width,
            },
            &transformers,
        )
    })
}

fn apply_markdown_transformers(
    markdown: &str,
    context: &MarkdownTransformContext,
    transformers: &[MarkdownTransformer],
) -> String {
    let mut transformed_markdown = markdown.to_string();
    for transformer in transformers {
        // non-string; both were only reachable from untyped extension code,
        transformed_markdown = transformer(&transformed_markdown, context);
    }
    if context.message_type == MarkdownMessageType::Assistant {
        return format_json_output(&transformed_markdown, context.is_streaming)
            .unwrap_or(transformed_markdown);
    }
    transformed_markdown
}

fn format_json_output(source: &str, is_streaming: bool) -> Option<String> {
    let source = source.trim();
    if !source.starts_with(['{', '[']) {
        return None;
    }
    // Validate without decoding values: reserializing a Value can round numbers,
    // discard duplicate keys and change the spelling of string escapes.
    if let Err(error) = serde_json::from_str::<&serde_json::value::RawValue>(source)
        && !(is_streaming && error.is_eof())
    {
        return None;
    }

    let mut formatted = String::from("```json\n");
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut previous = None;
    let mut chars = source.chars().peekable();
    while let Some(character) = chars.next() {
        if in_string {
            formatted.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        if character.is_ascii_whitespace() {
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                formatted.push(character);
            }
            '{' | '[' => {
                formatted.push(character);
                depth += 1;
                while chars.peek().is_some_and(char::is_ascii_whitespace) {
                    chars.next();
                }
                let closing = if character == '{' { '}' } else { ']' };
                if chars.peek().is_some_and(|next| *next != closing) {
                    json_line_break(&mut formatted, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                let opening = if character == '}' { '{' } else { '[' };
                if previous != Some(opening) {
                    json_line_break(&mut formatted, depth);
                }
                formatted.push(character);
            }
            ',' => {
                formatted.push(character);
                json_line_break(&mut formatted, depth);
            }
            ':' => formatted.push_str(": "),
            _ => formatted.push(character),
        }
        previous = Some(character);
    }
    formatted.push_str("\n```");
    Some(formatted)
}

fn json_line_break(output: &mut String, depth: usize) {
    output.push('\n');
    for _ in 0..depth {
        output.push_str("  ");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_json_keeps_values_key_order_and_duplicate_keys() {
        let source =
            r#"{"z":9007199254740993,"z":1.2300e+40,"a":[{},[],{"text":"a,}:\\\"\u0061"}]}"#;
        let formatted = format_json_output(source, false).expect("JSON is recognized");
        assert_eq!(
            formatted,
            "```json\n{\n  \"z\": 9007199254740993,\n  \"z\": 1.2300e+40,\n  \"a\": [\n    {},\n    [],\n    {\n      \"text\": \"a,}:\\\\\\\"\\u0061\"\n    }\n  ]\n}\n```"
        );
    }

    #[test]
    fn streaming_json_is_indented_without_inventing_missing_values_or_delimiters() {
        assert_eq!(
            format_json_output(r#"{"items":[{"text":"hel"#, true).as_deref(),
            Some("```json\n{\n  \"items\": [\n    {\n      \"text\": \"hel\n```")
        );
        assert!(format_json_output(r#"{"items":[{"text":"hel"#, false).is_none());
    }

    #[test]
    fn prose_fenced_code_and_invalid_json_are_unchanged() {
        for source in [
            "[link](https://example.com)",
            "Text: {\"a\":1}",
            "```json\n{}\n```",
            "{not json}",
            "{\"a\":1} explanation",
            "[1,]",
        ] {
            for streaming in [false, true] {
                assert!(
                    format_json_output(source, streaming).is_none(),
                    "must preserve {source:?}"
                );
            }
        }
    }

    #[test]
    fn only_assistant_output_is_automatically_formatted() {
        for message_type in [
            MarkdownMessageType::User,
            MarkdownMessageType::AssistantThinking,
        ] {
            let transform = create_markdown_transform(message_type, false, Vec::new());
            assert_eq!(transform("{\"a\":1}", 80), "{\"a\":1}");
        }
    }
}
