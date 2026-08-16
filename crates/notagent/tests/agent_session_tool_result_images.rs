//! The `afterToolCall` seam of `_installAgentHooks` (`agent-session.ts:567-597`).
//!
//! Without the extension runner two things are left at that point: the
//! `PostToolUse`/`PostToolUseFailure` hook — which the extension mapped from
//! its `tool_result` event (`plans/facts/extension-boundary.md` §2.2) — and
//! `normalizeToolResultImages`, which runs after it. `tests/hook_dispatch.rs`
//! pins the payloads of the hook itself and `tests/tool_result_images.rs` the
//! normalisation; what is asserted here is that a tool result of a real session
//! goes through both.

#![cfg(unix)]

mod suite;

use std::path::PathBuf;
use std::sync::Arc;

use image::{DynamicImage, ImageFormat};
use notagent::core::hooks::dispatch::HookDispatcher;
use notagent::core::hooks::payload::HookSessionContext;
use notagent::core::hooks::runtime::{HookRuntime, HookRuntimeOptions};
use notagent::core::hooks::{HOOKS_FILE_NAME, load_hooks};
use notagent_agent::types::{
    AgentMessage, AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture,
    ToolExecutionError,
};
use notagent_ai::providers::faux::{
    FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use notagent_ai::types::{ImageContent, StopReason, TextOrImageContent};
use serde_json::{Map, Value, json};
use suite::{HarnessOptions, create_harness};

fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

fn tool_call_reply(name: &str, id: &str) -> FauxResponseStep {
    faux_assistant_message(
        vec![faux_tool_call(name, json!({}), Some(id.to_string()))],
        StopReason::ToolUse,
    )
    .into()
}

/// A tool that hands back an image far above the inline limits, the way a
/// screenshot tool does.
struct ScreenshotTool {
    parameters: Value,
    data: String,
}

impl ScreenshotTool {
    fn build(width: u32, height: u32) -> Arc<dyn AgentTool> {
        use base64::Engine;
        let mut bytes = std::io::Cursor::new(Vec::new());
        DynamicImage::new_rgb8(width, height)
            .write_to(&mut bytes, ImageFormat::Png)
            .expect("encode");
        Arc::new(ScreenshotTool {
            parameters: json!({ "type": "object", "properties": {} }),
            data: base64::engine::general_purpose::STANDARD.encode(bytes.into_inner()),
        })
    }
}

impl AgentTool for ScreenshotTool {
    fn name(&self) -> &str {
        "screenshot"
    }
    fn label(&self) -> &str {
        "screenshot"
    }
    fn description(&self) -> &str {
        "a tool that returns an image"
    }
    fn parameters(&self) -> &Value {
        &self.parameters
    }
    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _params: Value,
        _signal: Option<tokio_util::sync::CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Image(ImageContent {
                    data: self.data.clone(),
                    mime_type: "image/png".to_owned(),
                })],
                ..AgentToolResult::default()
            })
        })
    }
}

/// A hooks file that logs every `PostToolUse` payload.
struct Wire {
    directory: PathBuf,
    dispatcher: Arc<HookDispatcher>,
}

impl Wire {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("tool-result-hooks-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        let log = path.join("log.jsonl");
        std::fs::write(
            path.join(HOOKS_FILE_NAME),
            json!([{ "event": "PostToolUse", "command": format!("cat >> {}", log.display()) }])
                .to_string(),
        )
        .expect("writes");
        let loaded = load_hooks(std::slice::from_ref(&path));
        assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
        let session_dir = path.to_string_lossy().into_owned();
        let runtime = Arc::new(HookRuntime::new(HookRuntimeOptions {
            hooks: loaded.hooks,
            diagnostics: loaded.diagnostics,
            context: Arc::new(move || HookSessionContext {
                session_id: "wired".to_string(),
                transcript_path: None,
                cwd: session_dir.clone(),
            }),
            report: None,
            signal: None,
        }));
        Self {
            directory: path,
            dispatcher: Arc::new(HookDispatcher::new(runtime)),
        }
    }

    fn payloads(&self) -> Vec<Map<String, Value>> {
        let raw = std::fs::read_to_string(self.directory.join("log.jsonl")).unwrap_or_default();
        raw.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<Value>(line)
                    .expect("payload")
                    .as_object()
                    .cloned()
                    .expect("object")
            })
            .collect()
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// The image block of the tool result the session stored.
fn stored_image(session: &notagent::core::agent_session::AgentSession) -> (ImageContent, usize) {
    let messages = session.agent().state().messages;
    let result = messages
        .iter()
        .rev()
        .find_map(|message| match message {
            AgentMessage::ToolResult(result) => Some(result.clone()),
            _ => None,
        })
        .expect("a tool result reached the session");
    let image = result
        .content
        .iter()
        .find_map(|block| match block {
            TextOrImageContent::Image(image) => Some(image.clone()),
            _ => None,
        })
        .expect("the result still carries its image");
    (image, result.content.len())
}

#[tokio::test]
async fn an_oversized_tool_image_is_resized_before_it_reaches_the_history() {
    let harness = create_harness(HarnessOptions {
        tools: Some(vec![ScreenshotTool::build(2400, 4800)]),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![
        tool_call_reply("screenshot", "call-1"),
        reply("saw it"),
    ]);

    harness
        .session
        .prompt("take a screenshot", Default::default())
        .await
        .expect("prompt");

    let (image, blocks) = stored_image(&harness.session);
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.data)
        .expect("base64");
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("width"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("height"));
    assert!(
        width <= 2000 && height <= 2000,
        "the stored image was resized: {width}x{height}"
    );
    assert_eq!(blocks, 2, "the note about the original size came with it");
}

#[tokio::test]
async fn the_post_tool_use_hook_sees_the_tool_result() {
    let wire = Wire::new();
    let harness = create_harness(HarnessOptions {
        tools: Some(vec![ScreenshotTool::build(8, 8)]),
        hooks: Some(Arc::clone(&wire.dispatcher)),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![
        tool_call_reply("screenshot", "call-1"),
        reply("saw it"),
    ]);

    harness
        .session
        .prompt("take a screenshot", Default::default())
        .await
        .expect("prompt");

    let payloads = wire.payloads();
    assert_eq!(
        payloads.len(),
        1,
        "one tool call, one PostToolUse: {payloads:?}"
    );
    assert_eq!(
        payloads[0].get("hook_event_name").and_then(Value::as_str),
        Some("PostToolUse")
    );
    assert_eq!(
        payloads[0].get("tool_name").and_then(Value::as_str),
        Some("screenshot")
    );
}
