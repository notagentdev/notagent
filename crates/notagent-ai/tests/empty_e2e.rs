//! Port of `packages/ai/test/empty.test.ts` (822 LOC).
//!
//! Four scenarios (`testEmptyMessage`, `testEmptyStringMessage`,
//! `testWhitespaceOnlyMessage`, `testEmptyAssistantMessage`) across the provider matrix;
//! one test per TS `describe` block, gated as in TS.

mod e2e_support;

use e2e_support::*;
use notagent_ai::types::*;

/// Every scenario accepts an error as long as it carries a message, exactly like TS.
fn assert_handled(response: &AssistantMessage) {
    if response.stop_reason == StopReason::Error {
        assert!(response.error_message.is_some());
    }
}

/// `testEmptyMessage(llm, options)` — a user message with an empty content array.
async fn test_empty_message(model: &Model, options: &SimpleStreamOptions) {
    let context = context(None, vec![user_blocks(Vec::new())]);
    assert_handled(&complete_simple(model, &context, options.clone()).await);
}

/// `testEmptyStringMessage(llm, options)`
async fn test_empty_string_message(model: &Model, options: &SimpleStreamOptions) {
    let context = context(None, vec![user("")]);
    assert_handled(&complete_simple(model, &context, options.clone()).await);
}

/// `testWhitespaceOnlyMessage(llm, options)`
async fn test_whitespace_only_message(model: &Model, options: &SimpleStreamOptions) {
    let context = context(None, vec![user("   \n\t  ")]);
    assert_handled(&complete_simple(model, &context, options.clone()).await);
}

/// `testEmptyAssistantMessage(llm, options)` — user → empty assistant → user.
async fn test_empty_assistant_message(model: &Model, options: &SimpleStreamOptions) {
    let empty_assistant = Message::Assistant(AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage {
            input: 10,
            total_tokens: Some(10),
            ..Usage::default()
        },
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    });
    let context = context(
        None,
        vec![
            user("Hello, how are you?"),
            empty_assistant,
            user("Please respond this time."),
        ],
    );

    let response = complete_simple(model, &context, options.clone()).await;
    if response.stop_reason == StopReason::Error {
        assert!(response.error_message.is_some());
    } else {
        assert!(!response.content.is_empty());
    }
}

async fn empty_suite(model: &Model, options: &SimpleStreamOptions) {
    test_empty_message(model, options).await;
    test_empty_string_message(model, options).await;
    test_whitespace_only_message(model, options).await;
    test_empty_assistant_message(model, options).await;
}

/// The three scenarios the OAuth blocks run (they leave out the empty-assistant case).
async fn empty_suite_without_assistant(model: &Model, options: &SimpleStreamOptions) {
    test_empty_message(model, options).await;
    test_empty_string_message(model, options).await;
    test_whitespace_only_message(model, options).await;
}

macro_rules! env_gated_block {
    ($name:ident, $variable:literal, $provider:literal, $model:literal) => {
        #[tokio::test(flavor = "multi_thread")]
        async fn $name() {
            skip_unless!(env($variable).is_some(), $variable);
            empty_suite(&model($provider, $model), &simple_options(None)).await;
        }
    };
}

env_gated_block!(
    google_provider_empty_messages,
    "GEMINI_API_KEY",
    "google",
    "gemini-2.5-flash"
);
env_gated_block!(
    openai_completions_provider_empty_messages,
    "OPENAI_API_KEY",
    "openai",
    "gpt-4o-mini"
);
env_gated_block!(
    openai_responses_provider_empty_messages,
    "OPENAI_API_KEY",
    "openai",
    "gpt-5-mini"
);
env_gated_block!(
    anthropic_provider_empty_messages,
    "ANTHROPIC_API_KEY",
    "anthropic",
    "claude-haiku-4-5"
);
env_gated_block!(
    xai_provider_empty_messages,
    "XAI_API_KEY",
    "xai",
    "grok-4.3"
);
env_gated_block!(
    groq_provider_empty_messages,
    "GROQ_API_KEY",
    "groq",
    "openai/gpt-oss-20b"
);
env_gated_block!(
    cerebras_provider_empty_messages,
    "CEREBRAS_API_KEY",
    "cerebras",
    "gpt-oss-120b"
);
env_gated_block!(
    huggingface_provider_empty_messages,
    "HF_TOKEN",
    "huggingface",
    "moonshotai/Kimi-K2.5"
);
env_gated_block!(
    together_provider_empty_messages,
    "TOGETHER_API_KEY",
    "together",
    "moonshotai/Kimi-K2.6"
);
env_gated_block!(
    baseten_provider_empty_messages,
    "BASETEN_API_KEY",
    "baseten",
    "zai-org/GLM-5.2"
);
env_gated_block!(zai_provider_empty_messages, "ZAI_API_KEY", "zai", "glm-5.2");
env_gated_block!(
    mistral_provider_empty_messages,
    "MISTRAL_API_KEY",
    "mistral",
    "devstral-medium-latest"
);
env_gated_block!(
    minimax_provider_empty_messages,
    "MINIMAX_API_KEY",
    "minimax",
    "MiniMax-M2.7"
);
env_gated_block!(
    xiaomi_provider_empty_messages,
    "XIAOMI_API_KEY",
    "xiaomi",
    "mimo-v2.5-pro"
);
env_gated_block!(
    xiaomi_cn_provider_empty_messages,
    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    "xiaomi-token-plan-cn",
    "mimo-v2.5-pro"
);
env_gated_block!(
    xiaomi_ams_provider_empty_messages,
    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    "xiaomi-token-plan-ams",
    "mimo-v2.5-pro"
);
env_gated_block!(
    xiaomi_sgp_provider_empty_messages,
    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
    "xiaomi-token-plan-sgp",
    "mimo-v2.5-pro"
);
env_gated_block!(
    qwen_token_plan_provider_empty_messages,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan",
    "qwen3.7-max"
);
env_gated_block!(
    qwen_token_plan_individual_provider_empty_messages,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan-individual",
    "qwen3.8-max"
);
env_gated_block!(
    qwen_token_plan_cn_provider_empty_messages,
    "QWEN_TOKEN_PLAN_CN_API_KEY",
    "qwen-token-plan-cn",
    "qwen3.7-max"
);
env_gated_block!(
    kimi_for_coding_provider_empty_messages,
    "KIMI_API_KEY",
    "kimi-coding",
    "kimi-for-coding"
);
env_gated_block!(
    vercel_ai_gateway_provider_empty_messages,
    "AI_GATEWAY_API_KEY",
    "vercel-ai-gateway",
    "google/gemini-2.5-flash"
);

#[tokio::test(flavor = "multi_thread")]
async fn azure_openai_responses_provider_empty_messages() {
    skip_unless!(has_azure_openai_credentials(), "Azure OpenAI credentials");
    let llm = model("azure-openai-responses", "gpt-4o-mini");
    let _ = resolve_azure_deployment_name(&llm.id);
    empty_suite(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cloudflare_workers_ai_provider_empty_messages() {
    skip_unless!(
        has_cloudflare_workers_ai_credentials(),
        "Cloudflare Workers AI credentials"
    );
    empty_suite(
        &model("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6"),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cloudflare_ai_gateway_provider_empty_messages() {
    skip_unless!(
        has_cloudflare_ai_gateway_credentials(),
        "Cloudflare AI Gateway credentials"
    );
    empty_suite(
        &model(
            "cloudflare-ai-gateway",
            "workers-ai/@cf/moonshotai/kimi-k2.6",
        ),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn amazon_bedrock_provider_empty_messages() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    empty_suite(
        &model(
            "amazon-bedrock",
            "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
        ),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_oauth_provider_empty_messages() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    empty_suite_without_assistant(
        &model("anthropic", "claude-haiku-4-5"),
        &simple_options(Some(&token)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn github_copilot_provider_empty_messages() {
    let token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    empty_suite_without_assistant(
        &model("github-copilot", "claude-haiku-4.5"),
        &simple_options(Some(&token)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_codex_provider_empty_messages() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    empty_suite_without_assistant(
        &model("openai-codex", "gpt-5.5"),
        &simple_options(Some(&token)),
    )
    .await;
}
