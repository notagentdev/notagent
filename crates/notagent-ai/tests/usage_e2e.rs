mod e2e_support;

use e2e_support::*;
use notagent_ai::types::*;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// `testTokensOnAbort(llm, options)`
async fn test_tokens_on_abort(model: &Model, options: &SimpleStreamOptions) {
    let context = context(
        Some("You are a helpful assistant."),
        vec![user(
            "Write a long poem with 20 stanzas about the beauty of nature.",
        )],
    );

    let signal = CancellationToken::new();
    let mut options = options.clone();
    options.base.base.signal = Some(signal.clone());
    let stream = provider_for(model).stream_simple(
        model,
        &context,
        Some(resolved_simple_options(model, options)),
    );

    let mut abort_fired = false;
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        if abort_fired {
            continue;
        }
        if let AssistantMessageEvent::TextDelta { delta, .. }
        | AssistantMessageEvent::ThinkingDelta { delta, .. } = &event
        {
            text.push_str(delta);
            if text.chars().count() >= 1000 {
                abort_fired = true;
                signal.cancel();
            }
        }
    }

    let message = stream.result().await;
    assert_eq!(message.stop_reason, StopReason::Aborted);

    // OpenAI-shaped providers, Codex, zai, Bedrock and the Vercel gateway only send usage
    // in the final chunk, so an aborted turn carries none. Anthropic and Google send usage
    // early. MiniMax reports nothing, Kimi only the input tokens.
    let usage_only_at_the_end = matches!(
        model.api.as_str(),
        "openai-completions"
            | "mistral-conversations"
            | "openai-responses"
            | "azure-openai-responses"
            | "openai-codex-responses"
    ) || matches!(
        model.provider.as_str(),
        "zai" | "amazon-bedrock" | "vercel-ai-gateway" | "minimax"
    );

    if usage_only_at_the_end {
        assert_eq!(message.usage.input, 0);
        assert_eq!(message.usage.output, 0);
    } else if model.provider == "kimi-coding" {
        assert!(message.usage.input > 0);
        assert_eq!(message.usage.output, 0);
    } else {
        assert!(message.usage.input > 0);
        assert!(message.usage.output > 0);
        // Some providers (Copilot) have zero cost rates.
        if model.cost.input > 0.0 {
            assert!(message.usage.cost.input > 0.0);
            assert!(message.usage.cost.total > 0.0);
        }
    }
}

macro_rules! tokens_block {
    ($name:ident, $variable:literal, $provider:literal, $model:literal) => {
        #[tokio::test(flavor = "multi_thread")]
        async fn $name() {
            skip_unless!(env($variable).is_some(), $variable);
            test_tokens_on_abort(&model($provider, $model), &simple_options(None)).await;
        }
    };
}

tokens_block!(
    tokens_google_provider,
    "GEMINI_API_KEY",
    "google",
    "gemini-2.5-flash"
);
tokens_block!(
    tokens_openai_responses_provider,
    "OPENAI_API_KEY",
    "openai",
    "gpt-5.4-mini"
);
tokens_block!(
    tokens_anthropic_provider,
    "ANTHROPIC_API_KEY",
    "anthropic",
    "claude-sonnet-4-6"
);
tokens_block!(tokens_xai_provider, "XAI_API_KEY", "xai", "grok-4.3");
tokens_block!(
    tokens_groq_provider,
    "GROQ_API_KEY",
    "groq",
    "openai/gpt-oss-20b"
);
tokens_block!(
    tokens_huggingface_provider,
    "HF_TOKEN",
    "huggingface",
    "moonshotai/Kimi-K2.5"
);
tokens_block!(
    tokens_together_provider,
    "TOGETHER_API_KEY",
    "together",
    "moonshotai/Kimi-K2.6"
);
tokens_block!(
    tokens_baseten_provider,
    "BASETEN_API_KEY",
    "baseten",
    "zai-org/GLM-5.2"
);
tokens_block!(tokens_zai_provider, "ZAI_API_KEY", "zai", "glm-5.2");
tokens_block!(
    tokens_mistral_provider,
    "MISTRAL_API_KEY",
    "mistral",
    "devstral-medium-latest"
);
tokens_block!(
    tokens_minimax_provider,
    "MINIMAX_API_KEY",
    "minimax",
    "MiniMax-M2.7"
);
tokens_block!(
    tokens_kimi_provider,
    "KIMI_API_KEY",
    "kimi-coding",
    "kimi-for-coding"
);
tokens_block!(
    tokens_vercel_gateway_provider,
    "AI_GATEWAY_API_KEY",
    "vercel-ai-gateway",
    "google/gemini-2.5-flash"
);
tokens_block!(
    tokens_xiaomi_provider,
    "XIAOMI_API_KEY",
    "xiaomi",
    "mimo-v2.5-pro"
);
tokens_block!(
    tokens_xiaomi_cn_provider,
    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    "xiaomi-token-plan-cn",
    "mimo-v2.5-pro"
);
tokens_block!(
    tokens_xiaomi_ams_provider,
    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    "xiaomi-token-plan-ams",
    "mimo-v2.5-pro"
);
tokens_block!(
    tokens_xiaomi_sgp_provider,
    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
    "xiaomi-token-plan-sgp",
    "mimo-v2.5-pro"
);
tokens_block!(
    tokens_qwen_provider,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan",
    "qwen3.7-max"
);
tokens_block!(
    tokens_qwen_individual_provider,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan-individual",
    "qwen3.8-max"
);
tokens_block!(
    tokens_qwen_cn_provider,
    "QWEN_TOKEN_PLAN_CN_API_KEY",
    "qwen-token-plan-cn",
    "qwen3.7-max"
);

#[tokio::test(flavor = "multi_thread")]
async fn tokens_openai_completions_provider() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    let llm = Model {
        api: "openai-completions".to_string(),
        compat: None,
        ..model("openai", "gpt-4o-mini")
    };
    test_tokens_on_abort(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tokens_azure_openai_responses_provider() {
    skip_unless!(has_azure_openai_credentials(), "Azure OpenAI credentials");
    let llm = model("azure-openai-responses", "gpt-4o-mini");
    let _ = resolve_azure_deployment_name(&llm.id);
    test_tokens_on_abort(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tokens_cerebras_provider() {
    skip_unless!(env("CEREBRAS_API_KEY").is_some(), "CEREBRAS_API_KEY");
    for llm in notagent_ai::model_catalog::get_builtin_models("cerebras") {
        test_tokens_on_abort(&llm, &simple_options(None)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tokens_cloudflare_workers_ai_provider() {
    skip_unless!(
        has_cloudflare_workers_ai_credentials(),
        "Cloudflare Workers AI credentials"
    );
    test_tokens_on_abort(
        &model("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6"),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tokens_cloudflare_ai_gateway_provider() {
    skip_unless!(
        has_cloudflare_ai_gateway_credentials(),
        "Cloudflare AI Gateway credentials"
    );
    test_tokens_on_abort(
        &model(
            "cloudflare-ai-gateway",
            "workers-ai/@cf/moonshotai/kimi-k2.6",
        ),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tokens_anthropic_oauth_provider() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    test_tokens_on_abort(
        &model("anthropic", "claude-sonnet-4-6"),
        &simple_options(Some(&token)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tokens_github_copilot_provider() {
    let token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    for model_id in ["claude-haiku-4.5", "claude-sonnet-4.6"] {
        test_tokens_on_abort(
            &model("github-copilot", model_id),
            &simple_options(Some(&token)),
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tokens_openai_codex_provider() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    test_tokens_on_abort(
        &model("openai-codex", "gpt-5.5"),
        &simple_options(Some(&token)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tokens_amazon_bedrock_provider() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    test_tokens_on_abort(
        &model(
            "amazon-bedrock",
            "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
        ),
        &simple_options(None),
    )
    .await;
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

fn long_system_prompt() -> String {
    let filler = std::iter::repeat_n(
        "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris.",
        50,
    )
    .collect::<Vec<_>>()
    .join("\n\n");
    format!(
        "You are a helpful assistant. Be concise in your responses.\n\nHere is some additional context that makes this system prompt long enough to trigger caching:\n\n{filler}\n\nRemember: Always be helpful and concise."
    )
}

fn assert_total_tokens_equals_components(usage: &Usage) {
    let computed = usage.input + usage.output + usage.cache_read + usage.cache_write;
    assert_eq!(usage.total_tokens, Some(computed), "{usage:?}");
}

/// `testTotalTokensWithCache(llm, options)`
async fn test_total_tokens_with_cache(
    model: &Model,
    options: &SimpleStreamOptions,
) -> (Usage, Usage) {
    let system_prompt = long_system_prompt();
    let mut context = context(
        Some(&system_prompt),
        vec![user("What is 2 + 2? Reply with just the number.")],
    );

    let first = complete_simple(model, &context, options.clone()).await;
    assert_eq!(
        first.stop_reason,
        StopReason::Stop,
        "{:?}",
        first.error_message
    );

    let first_usage = first.usage;
    context.messages.push(Message::Assistant(first));
    context
        .messages
        .push(user("What is 3 + 3? Reply with just the number."));

    let second = complete_simple(model, &context, options.clone()).await;
    assert_eq!(
        second.stop_reason,
        StopReason::Stop,
        "{:?}",
        second.error_message
    );

    (first_usage, second.usage)
}

async fn assert_total_tokens(model: &Model, options: &SimpleStreamOptions) {
    let (first, second) = test_total_tokens_with_cache(model, options).await;
    assert_total_tokens_equals_components(&first);
    assert_total_tokens_equals_components(&second);
}

macro_rules! total_tokens_block {
    ($name:ident, $variable:literal, $provider:literal, $model:literal) => {
        #[tokio::test(flavor = "multi_thread")]
        async fn $name() {
            skip_unless!(env($variable).is_some(), $variable);
            assert_total_tokens(&model($provider, $model), &simple_options(None)).await;
        }
    };
}

total_tokens_block!(
    total_tokens_anthropic,
    "ANTHROPIC_API_KEY",
    "anthropic",
    "claude-sonnet-4-5"
);
total_tokens_block!(
    total_tokens_openai_responses,
    "OPENAI_API_KEY",
    "openai",
    "gpt-4o"
);
total_tokens_block!(
    total_tokens_google,
    "GEMINI_API_KEY",
    "google",
    "gemini-2.5-flash"
);
total_tokens_block!(total_tokens_xai, "XAI_API_KEY", "xai", "grok-4.3");
total_tokens_block!(
    total_tokens_groq,
    "GROQ_API_KEY",
    "groq",
    "openai/gpt-oss-120b"
);
total_tokens_block!(
    total_tokens_cerebras,
    "CEREBRAS_API_KEY",
    "cerebras",
    "gpt-oss-120b"
);
total_tokens_block!(
    total_tokens_huggingface,
    "HF_TOKEN",
    "huggingface",
    "moonshotai/Kimi-K2.5"
);
total_tokens_block!(
    total_tokens_together,
    "TOGETHER_API_KEY",
    "together",
    "moonshotai/Kimi-K2.6"
);
total_tokens_block!(
    total_tokens_baseten,
    "BASETEN_API_KEY",
    "baseten",
    "zai-org/GLM-5.2"
);
total_tokens_block!(total_tokens_zai, "ZAI_API_KEY", "zai", "glm-5.2");
total_tokens_block!(
    total_tokens_mistral,
    "MISTRAL_API_KEY",
    "mistral",
    "devstral-medium-latest"
);
total_tokens_block!(
    total_tokens_minimax,
    "MINIMAX_API_KEY",
    "minimax",
    "MiniMax-M2.7"
);
total_tokens_block!(
    total_tokens_xiaomi,
    "XIAOMI_API_KEY",
    "xiaomi",
    "mimo-v2.5-pro"
);
total_tokens_block!(
    total_tokens_xiaomi_cn,
    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    "xiaomi-token-plan-cn",
    "mimo-v2.5-pro"
);
total_tokens_block!(
    total_tokens_xiaomi_ams,
    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    "xiaomi-token-plan-ams",
    "mimo-v2.5-pro"
);
total_tokens_block!(
    total_tokens_xiaomi_sgp,
    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
    "xiaomi-token-plan-sgp",
    "mimo-v2.5-pro"
);
total_tokens_block!(
    total_tokens_qwen,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan",
    "qwen3.7-max"
);
total_tokens_block!(
    total_tokens_qwen_individual,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan-individual",
    "qwen3.8-max"
);
total_tokens_block!(
    total_tokens_qwen_cn,
    "QWEN_TOKEN_PLAN_CN_API_KEY",
    "qwen-token-plan-cn",
    "qwen3.7-max"
);
total_tokens_block!(
    total_tokens_kimi,
    "KIMI_API_KEY",
    "kimi-coding",
    "kimi-for-coding"
);
total_tokens_block!(
    total_tokens_vercel_gateway,
    "AI_GATEWAY_API_KEY",
    "vercel-ai-gateway",
    "google/gemini-2.5-flash"
);

#[tokio::test(flavor = "multi_thread")]
async fn total_tokens_anthropic_oauth() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    let (first, second) = test_total_tokens_with_cache(
        &model("anthropic", "claude-sonnet-4-6"),
        &simple_options(Some(&token)),
    )
    .await;
    assert_total_tokens_equals_components(&first);
    assert_total_tokens_equals_components(&second);
    // Anthropic has to show cache activity.
    assert!(second.cache_read > 0 || second.cache_write > 0 || first.cache_write > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn total_tokens_openai_completions() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    let llm = Model {
        api: "openai-completions".to_string(),
        compat: None,
        ..model("openai", "gpt-4o-mini")
    };
    assert_total_tokens(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn total_tokens_azure_openai_responses() {
    skip_unless!(has_azure_openai_credentials(), "Azure OpenAI credentials");
    let llm = model("azure-openai-responses", "gpt-4o-mini");
    let _ = resolve_azure_deployment_name(&llm.id);
    assert_total_tokens(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn total_tokens_cloudflare_workers_ai() {
    skip_unless!(
        has_cloudflare_workers_ai_credentials(),
        "Cloudflare Workers AI credentials"
    );
    assert_total_tokens(
        &model("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6"),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn total_tokens_cloudflare_ai_gateway() {
    skip_unless!(
        has_cloudflare_ai_gateway_credentials(),
        "Cloudflare AI Gateway credentials"
    );
    assert_total_tokens(
        &model(
            "cloudflare-ai-gateway",
            "workers-ai/@cf/moonshotai/kimi-k2.6",
        ),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn total_tokens_openrouter() {
    skip_unless!(env("OPENROUTER_API_KEY").is_some(), "OPENROUTER_API_KEY");
    for model_id in [
        "anthropic/claude-sonnet-4",
        "deepseek/deepseek-chat",
        "mistralai/mistral-small-3.2-24b-instruct",
        "google/gemini-2.5-flash",
    ] {
        assert_total_tokens(&model("openrouter", model_id), &simple_options(None)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn total_tokens_github_copilot_oauth() {
    let token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    for model_id in ["claude-haiku-4.5", "claude-sonnet-4.6"] {
        assert_total_tokens(
            &model("github-copilot", model_id),
            &simple_options(Some(&token)),
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn total_tokens_amazon_bedrock() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    assert_total_tokens(
        &model(
            "amazon-bedrock",
            "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
        ),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn total_tokens_openai_codex_oauth() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    assert_total_tokens(
        &model("openai-codex", "gpt-5.5"),
        &simple_options(Some(&token)),
    )
    .await;
}
