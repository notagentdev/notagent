use serde_yaml_ng::Value;

/// The frontmatter mapping and the body below it.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedFrontmatter {
    pub frontmatter: Value,
    pub body: String,
}

impl ParsedFrontmatter {
    /// A frontmatter field, or `None` when the block is absent or has no such key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.frontmatter.get(key)
    }

    /// A frontmatter field as a string, matching what `typeof x === "string"` accepts.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FrontmatterError(String);

fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

/// Splits the YAML block from the body, both already newline-normalized.
/// An unterminated block is not an error: the file is treated as having no
/// frontmatter at all, so a document that merely opens with a horizontal rule
/// is still readable.
fn extract_frontmatter(content: &str) -> (Option<String>, String) {
    let normalized = normalize_newlines(content);

    if !normalized.starts_with("---") {
        return (None, normalized);
    }

    let Some(offset) = normalized[3..].find("\n---") else {
        return (None, normalized);
    };
    let end_index = offset + 3;

    let yaml = normalized[4..end_index].to_string();
    let body = normalized[end_index + 4..].trim().to_string();
    (Some(yaml), body)
}

/// where the parse throws — a mode or skill whose attributes cannot be read
/// must not silently load with none of them.
pub fn parse_frontmatter(content: &str) -> Result<ParsedFrontmatter, FrontmatterError> {
    let (yaml, body) = extract_frontmatter(content);
    let Some(yaml) = yaml else {
        return Ok(ParsedFrontmatter {
            frontmatter: Value::Mapping(Default::default()),
            body,
        });
    };
    // The block is sliced without its closing line break, and libyaml — unlike
    // the `yaml` package — does not treat end of input as one: a `|` block
    // scalar would lose the trailing newline that clip chomping keeps. Putting
    // the line break back is a no-op for every other document shape.
    let parsed: Value = serde_yaml_ng::from_str(&format!("{yaml}\n"))
        .map_err(|error| FrontmatterError(error.to_string()))?;
    // turns that into `{}` so callers never have to check.
    let frontmatter = if parsed.is_null() {
        Value::Mapping(Default::default())
    } else {
        parsed
    };
    Ok(ParsedFrontmatter { frontmatter, body })
}

/// The body without its frontmatter block.
pub fn strip_frontmatter(content: &str) -> Result<String, FrontmatterError> {
    Ok(parse_frontmatter(content)?.body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys_strips_quotes_and_returns_body() {
        let input =
            "---\nname: \"skill-name\"\ndescription: 'A desc'\nfoo-bar: value\n---\n\nBody text";
        let parsed = parse_frontmatter(input).unwrap();
        assert_eq!(parsed.get_str("name"), Some("skill-name"));
        assert_eq!(parsed.get_str("description"), Some("A desc"));
        assert_eq!(parsed.get_str("foo-bar"), Some("value"));
        assert_eq!(parsed.body, "Body text");
    }

    #[test]
    fn normalizes_newlines_and_handles_crlf() {
        let parsed = parse_frontmatter("---\r\nname: test\r\n---\r\nLine one\r\nLine two").unwrap();
        assert_eq!(parsed.body, "Line one\nLine two");
    }

    #[test]
    fn fails_on_invalid_yaml_frontmatter() {
        // position ("at line 1, column 10"); the Rust parser's message differs
        // in wording, so only the failure itself is pinned.
        let error = parse_frontmatter("---\nfoo: [bar\n---\nBody").unwrap_err();
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn parses_block_scalars() {
        let parsed =
            parse_frontmatter("---\ndescription: |\n  Line one\n  Line two\n---\n\nBody").unwrap();
        assert_eq!(parsed.get_str("description"), Some("Line one\nLine two\n"));
        assert_eq!(parsed.body, "Body");
    }

    #[test]
    fn returns_original_content_when_frontmatter_is_missing_or_unterminated() {
        assert_eq!(
            parse_frontmatter("Just text\nsecond line").unwrap().body,
            "Just text\nsecond line"
        );
        assert_eq!(
            parse_frontmatter("---\nname: test\nBody without terminator")
                .unwrap()
                .body,
            "---\nname: test\nBody without terminator"
        );
    }

    #[test]
    fn returns_an_empty_mapping_for_empty_or_comment_only_frontmatter() {
        let parsed = parse_frontmatter("---\n# just a comment\n---\nBody").unwrap();
        assert_eq!(parsed.frontmatter, Value::Mapping(Default::default()));
        assert_eq!(parsed.get("anything"), None);
    }

    #[test]
    fn strip_frontmatter_removes_the_block_and_trims_the_body() {
        assert_eq!(
            strip_frontmatter("---\nkey: value\n---\n\nBody\n").unwrap(),
            "Body"
        );
    }

    #[test]
    fn strip_frontmatter_returns_the_body_when_no_frontmatter_is_present() {
        assert_eq!(
            strip_frontmatter("\n  No frontmatter body  \n").unwrap(),
            "\n  No frontmatter body  \n"
        );
    }
}
