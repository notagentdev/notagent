//! Port of `packages/ai/test/unicode-surrogate.test.ts` (839 LOC).
//!
//! Three scenarios (`testEmojiInToolResults`, `testRealWorldLinkedInData`,
//! `testUnpairedHighSurrogate`) across the provider matrix; one test per TS `describe`
//! block, gated as in TS.
//!
//! The third scenario builds its input from `String.fromCharCode(0xd83d)`. A Rust `String`
//! cannot hold a lone surrogate — that is exactly why `sanitize_surrogates` is the identity
//! (class 1, see the `sanitize_unicode.rs` ledger row) — so the port sends the replacement
//! character a lossy UTF-16 decode produces, which is what a provider would receive from
//! the sanitized TS payload.

mod e2e_support;

use e2e_support::*;
use notagent_ai::types::*;
use serde_json::json;

fn empty_schema_tool(name: &str, description: &str) -> Tool {
    Tool {
        name: name.to_string(),
        description: description.to_string(),
        // TS uses `Type.Object({})`; Cloud Code Assist requires a real object schema.
        parameters: json!({ "type": "object", "properties": {} }),
        constrained_sampling: None,
    }
}

fn tool_call_id(model: &Model, id: &str, mistral_id: &str) -> String {
    if model.provider == "mistral" {
        mistral_id.to_string()
    } else {
        id.to_string()
    }
}

/// user → assistant tool call → tool result → user, the shape all three scenarios share.
struct RoundTrip<'a> {
    tool_call_id: &'a str,
    tool_name: &'a str,
    tool_description: &'a str,
    first_user: &'a str,
    result_text: &'a str,
    follow_up: &'a str,
}

async fn run_tool_result_round_trip(
    model: &Model,
    options: &SimpleStreamOptions,
    case: RoundTrip<'_>,
) -> AssistantMessage {
    let RoundTrip {
        tool_call_id,
        tool_name,
        tool_description,
        first_user,
        result_text,
        follow_up,
    } = case;
    let mut context = context(
        Some("You are a helpful assistant."),
        vec![
            user(first_user),
            Message::Assistant(AssistantMessage {
                content: vec![AssistantContent::ToolCall(ToolCall {
                    id: tool_call_id.to_string(),
                    name: tool_name.to_string(),
                    arguments: serde_json::Map::new(),
                    ..ToolCall::default()
                })],
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage {
                    total_tokens: Some(0),
                    ..Usage::default()
                },
                stop_reason: StopReason::ToolUse,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: now_ms(),
            }),
        ],
    );
    context.tools = Some(vec![empty_schema_tool(tool_name, tool_description)]);
    context
        .messages
        .push(Message::ToolResult(ToolResultMessage {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            content: vec![text_block(result_text)],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: now_ms(),
        }));
    context.messages.push(user(follow_up));

    complete_simple(model, &context, options.clone()).await
}

/// `testEmojiInToolResults(llm, options)`
async fn test_emoji_in_tool_results(model: &Model, options: &SimpleStreamOptions) {
    let response = run_tool_result_round_trip(
        model,
        options,
        RoundTrip {
        tool_call_id: &tool_call_id(model, "test_1", "testtool1"),
        tool_name: "test_tool",
        tool_description: "A test tool",
        first_user: "Use the test tool",
        result_text: "Test with emoji 🙈 and other characters:\n- Monkey emoji: 🙈\n- Thumbs up: 👍\n- Heart: ❤️\n- Thinking face: 🤔\n- Rocket: 🚀\n- Mixed text: notagentdev wann? Wo? Bin grad äußersr eventuninformiert 🙈\n- Japanese: こんにちは\n- Chinese: 你好\n- Mathematical symbols: ∑∫∂√\n- Special quotes: \u{201c}curly\u{201d} \u{2018}quotes\u{2019}",
        follow_up: "Summarize the tool result briefly.",
        },
    )
    .await;

    assert_ne!(response.stop_reason, StopReason::Error);
    assert_eq!(response.error_message, None);
    assert!(!response.content.is_empty());
}

/// `testRealWorldLinkedInData(llm, options)`
async fn test_real_world_linkedin_data(model: &Model, options: &SimpleStreamOptions) {
    let response = run_tool_result_round_trip(
        model,
        options,
        RoundTrip {
        tool_call_id: &tool_call_id(model, "linkedin_1", "linkedin1"),
        tool_name: "linkedin_skill",
        tool_description: "Get LinkedIn comments",
        first_user: "Use the linkedin tool to get comments",
        result_text: "Post: Hab einen \"Generative KI für Nicht-Techniker\" Workshop gebaut.\nUnanswered Comments: 2\n\n=> {\n  \"comments\": [\n    {\n      \"author\": \"Matthias Neumayer's  graphic link\",\n      \"text\": \"Leider nehmen das viel zu wenige Leute ernst\"\n    },\n    {\n      \"author\": \"Matthias Neumayer's  graphic link\",\n      \"text\": \"notagentdev wann? Wo? Bin grad äußersr eventuninformiert 🙈\"\n    }\n  ]\n}",
        follow_up: "How many comments are there?",
        },
    )
    .await;

    assert_ne!(response.stop_reason, StopReason::Error);
    assert_eq!(response.error_message, None);
    assert!(has_text(&response));
}

/// `testUnpairedHighSurrogate(llm, options)`
async fn test_unpaired_high_surrogate(model: &Model, options: &SimpleStreamOptions) {
    let sanitized = String::from_utf16_lossy(&[0xd83d]);
    let response = run_tool_result_round_trip(
        model,
        options,
        RoundTrip {
            tool_call_id: &tool_call_id(model, "test_2", "testtool2"),
            tool_name: "test_tool",
            tool_description: "A test tool",
            first_user: "Use the test tool",
            result_text: &format!(
                "Text with unpaired surrogate: {sanitized} <- should be sanitized"
            ),
            follow_up: "What did the tool return?",
        },
    )
    .await;

    assert_ne!(response.stop_reason, StopReason::Error);
    assert_eq!(response.error_message, None);
    assert!(!response.content.is_empty());
}

async fn unicode_suite(model: &Model, options: &SimpleStreamOptions) {
    test_emoji_in_tool_results(model, options).await;
    test_real_world_linkedin_data(model, options).await;
    test_unpaired_high_surrogate(model, options).await;
}

macro_rules! unicode_block {
    ($name:ident, $variable:literal, $provider:literal, $model:literal) => {
        #[tokio::test(flavor = "multi_thread")]
        async fn $name() {
            skip_unless!(env($variable).is_some(), $variable);
            unicode_suite(&model($provider, $model), &simple_options(None)).await;
        }
    };
}

unicode_block!(
    unicode_google_provider,
    "GEMINI_API_KEY",
    "google",
    "gemini-2.5-flash"
);
unicode_block!(
    unicode_openai_completions_provider,
    "OPENAI_API_KEY",
    "openai",
    "gpt-4o-mini"
);
unicode_block!(
    unicode_openai_responses_provider,
    "OPENAI_API_KEY",
    "openai",
    "gpt-5-mini"
);
unicode_block!(
    unicode_anthropic_provider,
    "ANTHROPIC_API_KEY",
    "anthropic",
    "claude-haiku-4-5"
);
unicode_block!(unicode_xai_provider, "XAI_API_KEY", "xai", "grok-4.3");
unicode_block!(
    unicode_groq_provider,
    "GROQ_API_KEY",
    "groq",
    "openai/gpt-oss-20b"
);
unicode_block!(
    unicode_cerebras_provider,
    "CEREBRAS_API_KEY",
    "cerebras",
    "gpt-oss-120b"
);
unicode_block!(
    unicode_huggingface_provider,
    "HF_TOKEN",
    "huggingface",
    "moonshotai/Kimi-K2.5"
);
unicode_block!(
    unicode_together_provider,
    "TOGETHER_API_KEY",
    "together",
    "moonshotai/Kimi-K2.6"
);
unicode_block!(
    unicode_baseten_provider,
    "BASETEN_API_KEY",
    "baseten",
    "zai-org/GLM-5.2"
);
unicode_block!(unicode_zai_provider, "ZAI_API_KEY", "zai", "glm-5.2");
unicode_block!(
    unicode_mistral_provider,
    "MISTRAL_API_KEY",
    "mistral",
    "devstral-medium-latest"
);
unicode_block!(
    unicode_minimax_provider,
    "MINIMAX_API_KEY",
    "minimax",
    "MiniMax-M2.7"
);
unicode_block!(
    unicode_xiaomi_provider,
    "XIAOMI_API_KEY",
    "xiaomi",
    "mimo-v2.5-pro"
);
unicode_block!(
    unicode_xiaomi_cn_provider,
    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    "xiaomi-token-plan-cn",
    "mimo-v2.5-pro"
);
unicode_block!(
    unicode_xiaomi_ams_provider,
    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    "xiaomi-token-plan-ams",
    "mimo-v2.5-pro"
);
unicode_block!(
    unicode_xiaomi_sgp_provider,
    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
    "xiaomi-token-plan-sgp",
    "mimo-v2.5-pro"
);
unicode_block!(
    unicode_qwen_provider,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan",
    "qwen3.7-max"
);
unicode_block!(
    unicode_qwen_individual_provider,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan-individual",
    "qwen3.8-max"
);
unicode_block!(
    unicode_qwen_cn_provider,
    "QWEN_TOKEN_PLAN_CN_API_KEY",
    "qwen-token-plan-cn",
    "qwen3.7-max"
);
unicode_block!(
    unicode_kimi_provider,
    "KIMI_API_KEY",
    "kimi-coding",
    "kimi-for-coding"
);
unicode_block!(
    unicode_vercel_gateway_provider,
    "AI_GATEWAY_API_KEY",
    "vercel-ai-gateway",
    "google/gemini-2.5-flash"
);

#[tokio::test(flavor = "multi_thread")]
async fn unicode_azure_openai_responses_provider() {
    skip_unless!(has_azure_openai_credentials(), "Azure OpenAI credentials");
    let llm = model("azure-openai-responses", "gpt-4o-mini");
    let _ = resolve_azure_deployment_name(&llm.id);
    unicode_suite(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unicode_cloudflare_workers_ai_provider() {
    skip_unless!(
        has_cloudflare_workers_ai_credentials(),
        "Cloudflare Workers AI credentials"
    );
    unicode_suite(
        &model("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6"),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unicode_cloudflare_ai_gateway_provider() {
    skip_unless!(
        has_cloudflare_ai_gateway_credentials(),
        "Cloudflare AI Gateway credentials"
    );
    unicode_suite(
        &model(
            "cloudflare-ai-gateway",
            "workers-ai/@cf/moonshotai/kimi-k2.6",
        ),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unicode_amazon_bedrock_provider() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    unicode_suite(
        &model(
            "amazon-bedrock",
            "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
        ),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unicode_anthropic_oauth_provider() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    unicode_suite(
        &model("anthropic", "claude-haiku-4-5"),
        &simple_options(Some(&token)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unicode_github_copilot_provider() {
    let token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    for model_id in ["claude-haiku-4.5", "claude-sonnet-4.6"] {
        unicode_suite(
            &model("github-copilot", model_id),
            &simple_options(Some(&token)),
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unicode_openai_codex_provider() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    unicode_suite(
        &model("openai-codex", "gpt-5.5"),
        &simple_options(Some(&token)),
    )
    .await;
}
