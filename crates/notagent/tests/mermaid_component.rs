//! Port of `packages/coding-agent/test/mermaid.test.ts` (99 LOC, 7 cases).
//!
//! Class-1 deviation in the harness: the TS suite passes a fake `Theme` object
//! whose `fg` writes `<color>…</color>` markers. `Theme` is a struct here, so
//! the themed cases install a real theme and assert against what it produces
//! for the same colours.

use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use notagent::core::settings_manager::MermaidRenderingMode;
use notagent::modes::interactive::components::markdown_transform::{
    MarkdownMessageType, MarkdownTransformContext,
};
use notagent::modes::interactive::components::mermaid::{
    MermaidTransformerOptions, create_mermaid_markdown_transformer,
};
use notagent::modes::interactive::theme::theme::{Theme, ThemeColor, init_theme, theme};

/// The theme is process-global; the themed cases share it under a lock.
fn guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct TransformOptions {
    max_width: Option<usize>,
    is_streaming: bool,
    message_type: Option<MarkdownMessageType>,
    mode: Option<MermaidRenderingMode>,
    theme: Option<Arc<Theme>>,
}

fn transform_mermaid(markdown: &str, options: TransformOptions) -> String {
    let mode = options.mode.unwrap_or(MermaidRenderingMode::Streaming);
    let transformer = create_mermaid_markdown_transformer(MermaidTransformerOptions {
        get_mode: Rc::new(move || mode),
        theme: options.theme,
    });
    transformer(
        markdown,
        &MarkdownTransformContext {
            available_width: options.max_width.unwrap_or(100),
            is_streaming: options.is_streaming,
            message_type: options
                .message_type
                .unwrap_or(MarkdownMessageType::Assistant),
        },
    )
}

/// The escape sequence `Theme::fg` opens a run of `color` with.
fn opening_sequence(theme: &Theme, color: ThemeColor) -> String {
    let styled = theme.fg(color, "x");
    let opening = styled
        .split_once('x')
        .expect("the styled text contains its content")
        .0
        .to_string();
    assert!(!opening.is_empty(), "the theme styles {color:?} at all");
    opening
}

fn themed() -> Arc<Theme> {
    init_theme(None, false);
    theme()
}

#[test]
fn replaces_mermaid_code_blocks_with_unicode_diagrams() {
    let markdown = "Before\n\n```mermaid\nflowchart LR\n  A[Start] --> B[Done]\n```\nAfter";
    let rendered = transform_mermaid(markdown, TransformOptions::default());

    assert!(rendered.contains("Before"));
    assert!(rendered.contains("┌───────┐"));
    assert!(rendered.contains("│ Start ├───▶│ Done │"));
    assert!(rendered.contains("└───────┘    └──────┘`\nAfter"));
    assert!(!rendered.contains("```mermaid"));
    assert!(rendered.contains("After"));
}

#[test]
fn leaves_unsupported_and_oversized_diagrams_unchanged() {
    let unsupported = "```mermaid\npie\n  title Pets\n  \"Dogs\" : 4\n```";
    let oversized = "```mermaid\nflowchart LR\n  A[Start] --> B[Done]\n```";

    assert_eq!(
        transform_mermaid(unsupported, TransformOptions::default()),
        unsupported
    );
    assert_eq!(
        transform_mermaid(
            oversized,
            TransformOptions {
                max_width: Some(10),
                ..TransformOptions::default()
            }
        ),
        oversized
    );
}

#[test]
fn maps_semantic_spans_through_the_notagent_theme() {
    let _guard = guard();
    let theme = themed();
    let rendered = transform_mermaid(
        "```mermaid\nflowchart LR\n  A --> B\n```",
        TransformOptions {
            theme: Some(Arc::clone(&theme)),
            ..TransformOptions::default()
        },
    );

    // The TS suite looks for the `<borderMuted>` / `<accent>` markers its fake
    // theme writes; here the equivalent is the escape sequence the real theme
    // opens a run of that colour with.
    assert!(rendered.contains(&opening_sequence(&theme, ThemeColor::BorderMuted)));
    assert!(rendered.contains(&opening_sequence(&theme, ThemeColor::Accent)));
}

#[test]
fn renders_incomplete_mermaid_blocks_during_streaming() {
    let partial_markdown = "```mermaid\nflowchart LR\n  A --> B";

    assert!(
        transform_mermaid(
            partial_markdown,
            TransformOptions {
                is_streaming: true,
                ..TransformOptions::default()
            }
        )
        .contains("───▶")
    );
}

#[test]
fn falls_back_to_the_code_block_with_a_warning_after_streaming() {
    let markdown = "```mermaid\nflowchart LR\n  A[Foo]:::highlight --> B[Bar]\n```";
    let final_render = transform_mermaid(markdown, TransformOptions::default());
    let followed_by_text = transform_mermaid(
        &format!("{markdown}\nFollowing text"),
        TransformOptions::default(),
    );
    let streaming = transform_mermaid(
        markdown,
        TransformOptions {
            is_streaming: true,
            ..TransformOptions::default()
        },
    );

    assert!(final_render.contains(markdown));
    assert!(final_render.contains("```\n`Mermaid diagram not rendered"));
    assert!(final_render.contains("dropped, expected a link: \":::highlight --> B[Bar]\""));
    assert!(!final_render.contains("more)"));
    assert!(followed_by_text.contains("  \nFollowing text"));
    assert!(!streaming.contains("Mermaid diagram not rendered"));
    assert!(!streaming.contains("```mermaid"));
    assert!(streaming.contains("│ Foo │"));
}

#[test]
fn summarizes_additional_partial_render_warnings() {
    let markdown = "```mermaid\nflowchart LR\n  A[Foo]:::highlight --> B[Bar]\n  C[Baz]:::other --> D[Qux]\n```";
    let rendered = transform_mermaid(markdown, TransformOptions::default());

    assert!(rendered.contains(markdown));
    assert!(rendered.contains("dropped, expected a link: \":::highlight --> B[Bar]\""));
    assert!(rendered.contains("(+1 more)"));
    assert!(!rendered.contains("dropped, expected a link: \":::other --> D[Qux]\""));
}

#[test]
fn respects_rendering_modes_and_skips_thinking_blocks() {
    let markdown = "```mermaid\nflowchart LR\n  A --> B\n```";

    assert_eq!(
        transform_mermaid(
            markdown,
            TransformOptions {
                mode: Some(MermaidRenderingMode::Off),
                ..TransformOptions::default()
            }
        ),
        markdown
    );
    assert_eq!(
        transform_mermaid(
            markdown,
            TransformOptions {
                mode: Some(MermaidRenderingMode::Final),
                is_streaming: true,
                ..TransformOptions::default()
            }
        ),
        markdown
    );
    assert!(
        !transform_mermaid(
            markdown,
            TransformOptions {
                mode: Some(MermaidRenderingMode::Final),
                ..TransformOptions::default()
            }
        )
        .contains("```mermaid")
    );
    assert_eq!(
        transform_mermaid(
            markdown,
            TransformOptions {
                message_type: Some(MarkdownMessageType::AssistantThinking),
                ..TransformOptions::default()
            }
        ),
        markdown
    );
}
