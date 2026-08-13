//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/markdown-transform.ts` (29 LOC).
//!
//! `MarkdownTransformer` and `MarkdownTransformContext` are declared in
//! `core/extensions/types.ts:1146-1153`, which the port drops with the extension
//! system. They live here because the mermaid renderer — a core feature, not an
//! extension — is a markdown transformer
//! (`interactive-mode.ts:471,1991`).

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
        // TypeScript guards against a transformer throwing or returning a
        // non-string; both were only reachable from untyped extension code,
        // which the port drops.
        transformed_markdown = transformer(&transformed_markdown, context);
    }
    transformed_markdown
}
