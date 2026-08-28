use crate::types::{AssistantContent, TextOrImageContent, UserContent};

/// Content shapes `contentText` accepts.
pub enum ContentRef<'a> {
    Text(&'a str),
    User(&'a [TextOrImageContent]),
    Assistant(&'a [AssistantContent]),
}

impl<'a> From<&'a str> for ContentRef<'a> {
    fn from(value: &'a str) -> Self {
        ContentRef::Text(value)
    }
}

impl<'a> From<&'a [TextOrImageContent]> for ContentRef<'a> {
    fn from(value: &'a [TextOrImageContent]) -> Self {
        ContentRef::User(value)
    }
}

impl<'a> From<&'a Vec<TextOrImageContent>> for ContentRef<'a> {
    fn from(value: &'a Vec<TextOrImageContent>) -> Self {
        ContentRef::User(value.as_slice())
    }
}

impl<'a> From<&'a [AssistantContent]> for ContentRef<'a> {
    fn from(value: &'a [AssistantContent]) -> Self {
        ContentRef::Assistant(value)
    }
}

impl<'a> From<&'a Vec<AssistantContent>> for ContentRef<'a> {
    fn from(value: &'a Vec<AssistantContent>) -> Self {
        ContentRef::Assistant(value.as_slice())
    }
}

impl<'a> From<&'a UserContent> for ContentRef<'a> {
    fn from(value: &'a UserContent) -> Self {
        match value {
            UserContent::Text(text) => ContentRef::Text(text),
            UserContent::Blocks(blocks) => ContentRef::User(blocks),
        }
    }
}

/// `contentText(content, separator = "\n")` — extract and join text blocks.
pub fn content_text<'a>(content: impl Into<ContentRef<'a>>) -> String {
    content_text_with(content, "\n")
}

/// `contentText` with an explicit separator.
pub fn content_text_with<'a>(content: impl Into<ContentRef<'a>>, separator: &str) -> String {
    match content.into() {
        ContentRef::Text(text) => text.to_string(),
        ContentRef::User(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                TextOrImageContent::Text(text) => Some(text.text.as_str()),
                TextOrImageContent::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join(separator),
        ContentRef::Assistant(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(separator),
    }
}
