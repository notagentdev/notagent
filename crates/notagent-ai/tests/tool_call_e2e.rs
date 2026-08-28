mod e2e_support;

use e2e_support::*;
use notagent_ai::types::*;
use serde_json::json;

fn calculate_tool() -> Tool {
    Tool {
        name: "calculate".to_string(),
        description: "Evaluate mathematical expressions".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "The mathematical expression to evaluate",
                },
            },
            "required": ["expression"],
        }),
        constrained_sampling: None,
    }
}

fn echo_tool() -> Tool {
    Tool {
        name: "echo".to_string(),
        description: "Echoes the message back".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "message": { "type": "string", "description": "Message to echo back" },
            },
            "required": ["message"],
        }),
        constrained_sampling: None,
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// `testToolCallWithoutResult(model, options)` — an orphaned tool call must be filtered
/// out instead of failing the next request.
async fn test_tool_call_without_result(model: &Model, options: &SimpleStreamOptions) {
    let mut context = context(
        Some(
            "You are a helpful assistant. Use the calculate tool when asked to perform calculations.",
        ),
        vec![user("Please calculate 25 * 18 using the calculate tool.")],
    );
    context.tools = Some(vec![calculate_tool()]);

    let first = complete_simple(model, &context, options.clone()).await;
    assert!(
        !tool_calls(&first).is_empty(),
        "Expected assistant to make a tool call, but none was found"
    );
    context.messages.push(Message::Assistant(first));

    // A user message follows with no tool result, as when a call is cancelled.
    context
        .messages
        .push(user("Never mind, just tell me what is 2+2?"));

    let second = complete_simple(model, &context, options.clone()).await;
    assert_ne!(
        second.stop_reason,
        StopReason::Error,
        "{:?}",
        second.error_message
    );
    assert!(!second.content.is_empty());
}

macro_rules! orphan_block {
    ($name:ident, $variable:literal, $provider:literal, $model:literal) => {
        #[tokio::test(flavor = "multi_thread")]
        async fn $name() {
            skip_unless!(env($variable).is_some(), $variable);
            test_tool_call_without_result(&model($provider, $model), &simple_options(None)).await;
        }
    };
}

orphan_block!(
    orphan_google_provider,
    "GEMINI_API_KEY",
    "google",
    "gemini-2.5-flash"
);
orphan_block!(
    orphan_openai_responses_provider,
    "OPENAI_API_KEY",
    "openai",
    "gpt-5-mini"
);
orphan_block!(
    orphan_anthropic_provider,
    "ANTHROPIC_API_KEY",
    "anthropic",
    "claude-haiku-4-5"
);
orphan_block!(orphan_xai_provider, "XAI_API_KEY", "xai", "grok-4.3");
orphan_block!(
    orphan_groq_provider,
    "GROQ_API_KEY",
    "groq",
    "openai/gpt-oss-20b"
);
orphan_block!(
    orphan_cerebras_provider,
    "CEREBRAS_API_KEY",
    "cerebras",
    "gpt-oss-120b"
);
orphan_block!(
    orphan_huggingface_provider,
    "HF_TOKEN",
    "huggingface",
    "moonshotai/Kimi-K2.5"
);
orphan_block!(
    orphan_together_provider,
    "TOGETHER_API_KEY",
    "together",
    "moonshotai/Kimi-K2.6"
);
orphan_block!(
    orphan_baseten_provider,
    "BASETEN_API_KEY",
    "baseten",
    "zai-org/GLM-5.2"
);
orphan_block!(orphan_zai_provider, "ZAI_API_KEY", "zai", "glm-5.2");
orphan_block!(
    orphan_mistral_provider,
    "MISTRAL_API_KEY",
    "mistral",
    "devstral-medium-latest"
);
orphan_block!(
    orphan_minimax_provider,
    "MINIMAX_API_KEY",
    "minimax",
    "MiniMax-M2.7"
);
orphan_block!(
    orphan_xiaomi_provider,
    "XIAOMI_API_KEY",
    "xiaomi",
    "mimo-v2.5-pro"
);
orphan_block!(
    orphan_xiaomi_cn_provider,
    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    "xiaomi-token-plan-cn",
    "mimo-v2.5-pro"
);
orphan_block!(
    orphan_xiaomi_ams_provider,
    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    "xiaomi-token-plan-ams",
    "mimo-v2.5-pro"
);
orphan_block!(
    orphan_xiaomi_sgp_provider,
    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
    "xiaomi-token-plan-sgp",
    "mimo-v2.5-pro"
);
orphan_block!(
    orphan_qwen_provider,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan",
    "qwen3.7-max"
);
orphan_block!(
    orphan_qwen_individual_provider,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan-individual",
    "qwen3.8-max"
);
orphan_block!(
    orphan_qwen_cn_provider,
    "QWEN_TOKEN_PLAN_CN_API_KEY",
    "qwen-token-plan-cn",
    "qwen3.7-max"
);
orphan_block!(
    orphan_kimi_provider,
    "KIMI_API_KEY",
    "kimi-coding",
    "kimi-for-coding"
);
orphan_block!(
    orphan_vercel_gateway_provider,
    "AI_GATEWAY_API_KEY",
    "vercel-ai-gateway",
    "google/gemini-2.5-flash"
);

#[tokio::test(flavor = "multi_thread")]
async fn orphan_openai_completions_provider() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    let llm = Model {
        api: "openai-completions".to_string(),
        compat: None,
        ..model("openai", "gpt-4o-mini")
    };
    test_tool_call_without_result(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn orphan_azure_openai_responses_provider() {
    skip_unless!(has_azure_openai_credentials(), "Azure OpenAI credentials");
    let llm = model("azure-openai-responses", "gpt-4o-mini");
    let _ = resolve_azure_deployment_name(&llm.id);
    test_tool_call_without_result(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn orphan_cloudflare_workers_ai_provider() {
    skip_unless!(
        has_cloudflare_workers_ai_credentials(),
        "Cloudflare Workers AI credentials"
    );
    test_tool_call_without_result(
        &model("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6"),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn orphan_cloudflare_ai_gateway_provider() {
    skip_unless!(
        has_cloudflare_ai_gateway_credentials(),
        "Cloudflare AI Gateway credentials"
    );
    test_tool_call_without_result(
        &model(
            "cloudflare-ai-gateway",
            "workers-ai/@cf/moonshotai/kimi-k2.6",
        ),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn orphan_amazon_bedrock_provider() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    test_tool_call_without_result(
        &model(
            "amazon-bedrock",
            "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
        ),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn orphan_anthropic_oauth_provider() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    test_tool_call_without_result(
        &model("anthropic", "claude-haiku-4-5"),
        &simple_options(Some(&token)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn orphan_github_copilot_provider() {
    let token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    for model_id in ["claude-haiku-4.5", "claude-sonnet-4.6"] {
        test_tool_call_without_result(
            &model("github-copilot", model_id),
            &simple_options(Some(&token)),
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn orphan_openai_codex_provider() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    test_tool_call_without_result(
        &model("openai-codex", "gpt-5.5"),
        &simple_options(Some(&token)),
    )
    .await;
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// Generates a pipe-separated tool call with Copilot, then replays it on `target`.
async fn live_handoff(target: &Model, target_options: SimpleStreamOptions, copilot_token: &str) {
    let copilot_model = model("github-copilot", "gpt-5.2-codex");
    let user_message = user("Use the echo tool to echo 'hello world'");
    let mut context = context(
        Some("You are a helpful assistant. Use the echo tool when asked."),
        vec![user_message.clone()],
    );
    context.tools = Some(vec![echo_tool()]);

    let assistant = complete_simple(
        &copilot_model,
        &context,
        simple_options(Some(copilot_token)),
    )
    .await;
    assert_eq!(
        assistant.stop_reason,
        StopReason::ToolUse,
        "Copilot error: {:?}",
        assistant.error_message
    );
    let tool_call = tool_calls(&assistant)
        .first()
        .cloned()
        .cloned()
        .expect("a tool call");
    // The OpenAI Responses format carries a pipe-separated id.
    assert!(tool_call.id.contains('|'), "{}", tool_call.id);

    let mut replay = e2e_support::context(Some("You are a helpful assistant."), vec![user_message]);
    replay.tools = Some(vec![echo_tool()]);
    replay.messages.push(Message::Assistant(assistant));
    replay.messages.push(Message::ToolResult(ToolResultMessage {
        tool_call_id: tool_call.id.clone(),
        tool_name: "echo".to_string(),
        content: vec![text_block("hello world")],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: now_ms(),
    }));
    replay.messages.push(user("Say hi"));

    let response = complete_simple(target, &replay, target_options).await;
    assert_ne!(
        response.stop_reason,
        StopReason::Error,
        "{} error: {:?}",
        target.provider,
        response.error_message
    );
    assert_eq!(response.error_message, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn github_copilot_to_openrouter_normalizes_pipe_separated_ids() {
    let copilot_token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    let openrouter_key = skip_unless_some!(env("OPENROUTER_API_KEY"), "OPENROUTER_API_KEY");
    live_handoff(
        &model("openrouter", "openai/gpt-5.2-codex"),
        simple_options(Some(&openrouter_key)),
        &copilot_token,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn github_copilot_to_openai_codex_normalizes_pipe_separated_ids() {
    let copilot_token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    let codex_token =
        skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    live_handoff(
        &model("openai-codex", "gpt-5.5"),
        simple_options(Some(&codex_token)),
        &copilot_token,
    )
    .await;
}

/// The exact tool call id from issue #1022.
const FAILING_TOOL_CALL_ID: &str = "call_pAYbIr76hXIjncD9UE4eGfnS|t5nnb2qYMFWGSsr13fhCd1CaCu3t3qONEPuOudu4HSVEtA8YJSL6FAZUxvoOoD792VIJWl91g87EdqsCWp9krVsdBysQoDaf9lMCLb8BS4EYi4gQd5kBQBYLlgD71PYwvf+TbMD9J9/5OMD42oxSRj8H+vRf78/l2Xla33LWz4nOgsddBlbvabICRs8GHt5C9PK5keFtzyi3lsyVKNlfduK3iphsZqs4MLv4zyGJnvZo/+QzShyk5xnMSQX/f98+aEoNflEApCdEOXipipgeiNWnpFSHbcwmMkZoJhURNu+JEz3xCh1mrXeYoN5o+trLL3IXJacSsLYXDrYTipZZbJFRPAucgbnjYBC+/ZzJOfkwCs+Gkw7EoZR7ZQgJ8ma+9586n4tT4cI8DEhBSZsWMjrCt8dxKg==";

/// `buildPrefilledMessages()`
fn prefilled_context() -> Context {
    let now = now_ms();
    let mut context = context(
        Some("You are a helpful assistant."),
        vec![
            Message::User(UserMessage {
                content: UserContent::Text("Use the echo tool to echo 'hello'".to_string()),
                timestamp: now - 2000,
            }),
            Message::Assistant(AssistantMessage {
                content: vec![AssistantContent::ToolCall(ToolCall {
                    id: FAILING_TOOL_CALL_ID.to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "hello" })
                        .as_object()
                        .cloned()
                        .expect("object"),
                    ..ToolCall::default()
                })],
                api: "openai-responses".to_string(),
                provider: "github-copilot".to_string(),
                model: "gpt-5.2-codex".to_string(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage {
                    input: 100,
                    output: 50,
                    total_tokens: Some(150),
                    ..Usage::default()
                },
                stop_reason: StopReason::ToolUse,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: now - 1500,
            }),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: FAILING_TOOL_CALL_ID.to_string(),
                tool_name: "echo".to_string(),
                content: vec![text_block("hello")],
                details: None,
                usage: None,
                added_tool_names: None,
                is_error: false,
                timestamp: now - 1000,
            }),
            user("Say hi"),
        ],
    );
    context.tools = Some(vec![echo_tool()]);
    context
}

#[tokio::test(flavor = "multi_thread")]
async fn openrouter_handles_prefilled_context_with_long_pipe_separated_ids() {
    let openrouter_key = skip_unless_some!(env("OPENROUTER_API_KEY"), "OPENROUTER_API_KEY");
    let response = complete_simple(
        &model("openrouter", "openai/gpt-5.2-codex"),
        &prefilled_context(),
        simple_options(Some(&openrouter_key)),
    )
    .await;

    assert_ne!(
        response.stop_reason,
        StopReason::Error,
        "OpenRouter error: {:?}",
        response.error_message
    );
    if let Some(message) = &response.error_message {
        assert!(!message.contains("call_id"), "{message}");
        assert!(!message.contains("too long"), "{message}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_codex_handles_prefilled_context_with_long_pipe_separated_ids() {
    let codex_token =
        skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    let response = complete_simple(
        &model("openai-codex", "gpt-5.5"),
        &prefilled_context(),
        simple_options(Some(&codex_token)),
    )
    .await;

    assert_ne!(
        response.stop_reason,
        StopReason::Error,
        "Codex error: {:?}",
        response.error_message
    );
    if let Some(message) = &response.error_message {
        assert!(!message.contains("id"), "{message}");
        assert!(!message.contains("additional characters"), "{message}");
    }
}
