//! Payload snapshot tests for the Anthropic request builder.
//!
//! The fixtures were produced by the real TypeScript implementation: a script runs
//! `stream()` from `packages/ai/src/api/anthropic-messages.ts` against a fake client and
//! captures the request body through the `onPayload` hook
//! (`/tmp/gen-anthropic-payloads.mts`, run with `node --experimental-strip-types`).
//! Each case below rebuilds the same request in Rust and must produce the identical
//! body — the "payload snapshot" verification the workstream plan asks for.

use notagent_ai::api::anthropic_params::*;
use notagent_ai::types::*;
use serde_json::{Map, Value, json};

const FIXTURE: &str = include_str!("fixtures/anthropic-payloads.jsonl");

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    payload: Value,
}

fn fixtures() -> Vec<Case> {
    FIXTURE
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn expected(name: &str) -> Value {
    fixtures()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("fixture {name}"))
        .payload
}

fn base_model() -> Model {
    Model {
        id: "claude-opus-4-5".to_string(),
        name: "Claude Opus 4.5".to_string(),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![Modality::Text, Modality::Image],
        cost: ModelCost {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
            tiers: None,
        },
        context_window: 200_000,
        max_tokens: 64_000,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn with_compat(compat: Value) -> Model {
    Model {
        compat: Some(ModelCompat::from_api_value("anthropic-messages", compat).unwrap()),
        ..base_model()
    }
}

fn user(text: &str, timestamp: i64) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp,
    })
}

fn tool() -> Tool {
    Tool {
        name: "read".to_string(),
        description: "Reads a file".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {"path": {"type": "string"}, "offset": {"type": "number"}},
            "required": ["path"]
        }),
        constrained_sampling: None,
    }
}

fn assistant(content: Vec<AssistantContent>, stop_reason: StopReason, timestamp: i64) -> Message {
    Message::Assistant(AssistantMessage {
        content,
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-opus-4-5".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp,
    })
}

fn check(name: &str, model: &Model, context: &Context, options: AnthropicOptions) {
    let actual = build_params(model, context, false, &options, 0);
    assert_eq!(
        actual,
        expected(name),
        "payload for {name} differs from the TypeScript body"
    );
}

#[test]
fn the_fixture_covers_every_case() {
    assert_eq!(fixtures().len(), 18);
}

#[test]
fn plain_text_request_matches_typescript() {
    let context = Context {
        system_prompt: Some("sys".to_string()),
        messages: vec![user("hello", 1)],
        tools: None,
    };
    check(
        "plain-text",
        &base_model(),
        &context,
        AnthropicOptions::default(),
    );
}

#[test]
fn cache_retention_variants_match_typescript() {
    let context = Context {
        system_prompt: None,
        messages: vec![user("hi", 1)],
        tools: None,
    };
    check(
        "no-cache",
        &base_model(),
        &context,
        AnthropicOptions {
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        },
    );
    check(
        "long-cache",
        &base_model(),
        &context,
        AnthropicOptions {
            cache_retention: Some(CacheRetention::Long),
            ..Default::default()
        },
    );
}

#[test]
fn tool_definitions_match_typescript() {
    let context = Context {
        system_prompt: None,
        messages: vec![user("hi", 1)],
        tools: Some(vec![tool()]),
    };
    check(
        "tools",
        &base_model(),
        &context,
        AnthropicOptions::default(),
    );
}

#[test]
fn thinking_modes_match_typescript() {
    let context = Context {
        system_prompt: None,
        messages: vec![user("hi", 1)],
        tools: None,
    };
    check(
        "thinking-budget",
        &base_model(),
        &context,
        AnthropicOptions {
            thinking_enabled: Some(true),
            thinking_budget_tokens: Some(4096),
            ..Default::default()
        },
    );
    check(
        "thinking-disabled",
        &base_model(),
        &context,
        AnthropicOptions {
            thinking_enabled: Some(false),
            ..Default::default()
        },
    );
    check(
        "adaptive-thinking",
        &with_compat(json!({"forceAdaptiveThinking": true})),
        &context,
        AnthropicOptions {
            thinking_enabled: Some(true),
            effort: Some(AnthropicEffort::Xhigh),
            ..Default::default()
        },
    );
}

#[test]
fn temperature_is_dropped_when_thinking_is_enabled() {
    let context = Context {
        system_prompt: None,
        messages: vec![user("hi", 1)],
        tools: None,
    };
    check(
        "temperature",
        &base_model(),
        &context,
        AnthropicOptions {
            temperature: Some(0.7),
            ..Default::default()
        },
    );
    check(
        "temperature-with-thinking",
        &base_model(),
        &context,
        AnthropicOptions {
            temperature: Some(0.7),
            thinking_enabled: Some(true),
            ..Default::default()
        },
    );
}

#[test]
fn tool_choice_and_metadata_match_typescript() {
    let context = Context {
        system_prompt: None,
        messages: vec![user("hi", 1)],
        tools: Some(vec![tool()]),
    };
    check(
        "tool-choice",
        &base_model(),
        &context,
        AnthropicOptions {
            tool_choice: Some(AnthropicToolChoice::Tool {
                name: "read".to_string(),
            }),
            ..Default::default()
        },
    );

    let context = Context {
        system_prompt: None,
        messages: vec![user("hi", 1)],
        tools: None,
    };
    let mut metadata = Map::new();
    metadata.insert("user_id".to_string(), json!("u1"));
    metadata.insert("other".to_string(), json!("ignored"));
    check(
        "metadata",
        &base_model(),
        &context,
        AnthropicOptions {
            metadata: Some(metadata),
            ..Default::default()
        },
    );
}

#[test]
fn image_content_matches_typescript() {
    let context = Context {
        system_prompt: None,
        messages: vec![Message::User(UserMessage {
            content: UserContent::Blocks(vec![
                TextOrImageContent::Text(TextContent::new("look")),
                TextOrImageContent::Image(ImageContent {
                    data: "AAA".to_string(),
                    mime_type: "image/png".to_string(),
                }),
            ]),
            timestamp: 1,
        })],
        tools: None,
    };
    check(
        "images",
        &base_model(),
        &context,
        AnthropicOptions::default(),
    );

    let context = Context {
        system_prompt: None,
        messages: vec![Message::User(UserMessage {
            content: UserContent::Blocks(vec![TextOrImageContent::Image(ImageContent {
                data: "AAA".to_string(),
                mime_type: "image/png".to_string(),
            })]),
            timestamp: 1,
        })],
        tools: None,
    };
    check(
        "image-only",
        &base_model(),
        &context,
        AnthropicOptions::default(),
    );
}

#[test]
fn assistant_turns_and_tool_results_match_typescript() {
    let context = Context {
        system_prompt: None,
        messages: vec![
            user("run", 1),
            assistant(
                vec![
                    AssistantContent::Text(TextContent::new("sure")),
                    AssistantContent::ToolCall(ToolCall {
                        id: "toolu_1".to_string(),
                        name: "read".to_string(),
                        arguments: json!({"path": "a.txt"}).as_object().unwrap().clone(),
                        ..Default::default()
                    }),
                ],
                StopReason::ToolUse,
                2,
            ),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "toolu_1".to_string(),
                tool_name: "read".to_string(),
                content: vec![TextOrImageContent::Text(TextContent::new("file body"))],
                details: None,
                usage: None,
                added_tool_names: None,
                is_error: false,
                timestamp: 3,
            }),
        ],
        tools: Some(vec![tool()]),
    };
    check(
        "assistant-and-tool-results",
        &base_model(),
        &context,
        AnthropicOptions::default(),
    );
}

#[test]
fn thinking_replay_matches_typescript() {
    let signed = Context {
        system_prompt: None,
        messages: vec![
            user("q", 1),
            assistant(
                vec![
                    AssistantContent::Thinking(ThinkingContent {
                        thinking: "reasoned".to_string(),
                        thinking_signature: Some("sig".to_string()),
                        redacted: None,
                        extra: Default::default(),
                    }),
                    AssistantContent::Text(TextContent::new("answer")),
                ],
                StopReason::Stop,
                2,
            ),
            user("next", 3),
        ],
        tools: None,
    };
    check(
        "thinking-replay",
        &base_model(),
        &signed,
        AnthropicOptions::default(),
    );

    // Without a signature the block becomes plain text.
    let unsigned = Context {
        system_prompt: None,
        messages: vec![
            assistant(
                vec![AssistantContent::Thinking(ThinkingContent {
                    thinking: "unsigned".to_string(),
                    thinking_signature: None,
                    redacted: None,
                    extra: Default::default(),
                })],
                StopReason::Stop,
                2,
            ),
            user("next", 3),
        ],
        tools: None,
    };
    check(
        "unsigned-thinking-replay",
        &base_model(),
        &unsigned,
        AnthropicOptions::default(),
    );
}

#[test]
fn strict_tools_and_eager_streaming_compat_match_typescript() {
    let strict_tool = Tool {
        constrained_sampling: Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::JsonSchema {
                strict: StrictMode::Prefer,
            },
        )),
        ..tool()
    };
    let context = Context {
        system_prompt: None,
        messages: vec![user("hi", 1)],
        tools: Some(vec![strict_tool]),
    };
    check(
        "strict-tools",
        &with_compat(json!({"supportsStrictTools": true})),
        &context,
        AnthropicOptions::default(),
    );

    let context = Context {
        system_prompt: None,
        messages: vec![user("hi", 1)],
        tools: Some(vec![tool()]),
    };
    check(
        "no-eager-streaming",
        &with_compat(json!({"supportsEagerToolInputStreaming": false})),
        &context,
        AnthropicOptions::default(),
    );
}

// ---------------------------------------------------------------------------
// Helpers that the payload fixtures do not reach
// ---------------------------------------------------------------------------

#[test]
fn oauth_requests_carry_the_claude_code_identity() {
    let context = Context {
        system_prompt: Some("app prompt".to_string()),
        messages: vec![user("hi", 1)],
        tools: Some(vec![tool()]),
    };
    let params = build_params(
        &base_model(),
        &context,
        true,
        &AnthropicOptions::default(),
        0,
    );
    let system = params["system"].as_array().expect("system blocks");
    assert_eq!(
        system[0]["text"],
        json!("You are Claude Code, Anthropic's official CLI for Claude.")
    );
    assert_eq!(system[1]["text"], json!("app prompt"));
    // Tool names take Claude Code's canonical casing.
    assert_eq!(params["tools"][0]["name"], json!("Read"));
}

#[test]
fn tool_call_ids_are_normalized_for_anthropic() {
    assert_eq!(
        normalize_tool_call_id("call|with:specials"),
        "call_with_specials"
    );
    assert_eq!(normalize_tool_call_id(&"x".repeat(100)).len(), 64);
    assert_eq!(normalize_tool_call_id("ok_id-1"), "ok_id-1");
}

#[test]
fn tool_reference_support_defaults_follow_the_model_family() {
    let supported = [
        "claude-opus-4-5",
        "claude-sonnet-4-5-20250929",
        "claude-fable-5",
        "claude-opus-5",
    ];
    for id in supported {
        let model = Model {
            id: id.to_string(),
            ..base_model()
        };
        assert!(
            default_supports_tool_references(&model),
            "{id} should support tool references"
        );
    }
    let unsupported = [
        "claude-haiku-4-5",
        "claude-opus-4-1",
        "claude-3-5-sonnet",
        "claude-sonnet-4-0",
    ];
    for id in unsupported {
        let model = Model {
            id: id.to_string(),
            ..base_model()
        };
        assert!(
            !default_supports_tool_references(&model),
            "{id} should not support tool references"
        );
    }
    // Third-party providers never default to true.
    let model = Model {
        provider: "fireworks".to_string(),
        ..base_model()
    };
    assert!(!default_supports_tool_references(&model));
}

#[test]
fn beta_headers_follow_auth_mode_and_compat() {
    let model = base_model();
    let (headers, is_oauth) = build_default_headers(
        &model,
        Some("sk-ant-api-key"),
        true,
        false,
        None,
        None,
        None,
    );
    assert!(!is_oauth);
    assert_eq!(
        headers.get("anthropic-beta").map(String::as_str),
        Some("interleaved-thinking-2025-05-14")
    );
    assert_eq!(
        headers.get("accept").map(String::as_str),
        Some("application/json")
    );

    let (headers, is_oauth) = build_default_headers(
        &model,
        Some("sk-ant-oat-token"),
        true,
        false,
        None,
        None,
        None,
    );
    assert!(is_oauth);
    assert_eq!(
        headers.get("anthropic-beta").map(String::as_str),
        Some("claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14")
    );
    assert_eq!(
        headers.get("user-agent").map(String::as_str),
        Some("claude-cli/2.1.75")
    );
    assert_eq!(headers.get("x-app").map(String::as_str), Some("cli"));

    // Adaptive-thinking models skip the interleaved beta.
    let adaptive = with_compat(json!({"forceAdaptiveThinking": true}));
    let (headers, _) = build_default_headers(&adaptive, Some("key"), true, false, None, None, None);
    assert_eq!(headers.get("anthropic-beta"), None);

    // Fine-grained tool streaming is requested when eager streaming is unsupported.
    let legacy = with_compat(json!({"supportsEagerToolInputStreaming": false}));
    let (headers, _) = build_default_headers(&legacy, Some("key"), false, true, None, None, None);
    assert_eq!(
        headers.get("anthropic-beta").map(String::as_str),
        Some("fine-grained-tool-streaming-2025-05-14")
    );

    // A null request header suppresses a default.
    let suppressed: ProviderHeaders = [("accept".to_string(), None)].into_iter().collect();
    let (headers, _) = build_default_headers(
        &model,
        Some("key"),
        false,
        false,
        Some(&suppressed),
        None,
        None,
    );
    assert!(!headers.contains_key("accept"));
}

#[test]
fn session_affinity_headers_require_the_compat_flag() {
    let (headers, _) = build_default_headers(
        &base_model(),
        Some("key"),
        false,
        false,
        None,
        None,
        Some("s1"),
    );
    assert!(!headers.contains_key("x-session-affinity"));

    let model = with_compat(json!({"sendSessionAffinityHeaders": true}));
    let (headers, _) =
        build_default_headers(&model, Some("key"), false, false, None, None, Some("s1"));
    assert_eq!(
        headers.get("x-session-affinity").map(String::as_str),
        Some("s1")
    );
}
