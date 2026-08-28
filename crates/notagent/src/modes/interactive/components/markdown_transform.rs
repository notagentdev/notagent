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
    transformed_markdown
}
