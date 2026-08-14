//! Port of `packages/ai/test/abort.test.ts` (351 LOC).
//!
//! Three scenarios (`testAbortSignal`, `testImmediateAbort`, `testAbortThenNewMessage`)
//! across the provider matrix; one test per TS `describe` block, gated as in TS.

mod e2e_support;

use e2e_support::*;
use notagent_ai::types::*;
use tokio_util::sync::CancellationToken;

/// `testAbortSignal(llm, options)` — cancel once 50 characters have arrived.
async fn test_abort_signal(model: &Model, options: &SimpleStreamOptions) {
    let mut context = context(
        Some("You are a helpful assistant."),
        vec![user(
            "What is 15 + 27? Think step by step. Then list 50 first names.",
        )],
    );

    let signal = CancellationToken::new();
    let mut options_with_signal = options.clone();
    options_with_signal.base.base.signal = Some(signal.clone());
    let stream = provider_for(model).stream_simple(
        model,
        &context,
        Some(resolved_simple_options(model, options_with_signal)),
    );

    let mut abort_fired = false;
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        if abort_fired {
            return;
        }
        match &event {
            AssistantMessageEvent::TextDelta { delta, .. }
            | AssistantMessageEvent::ThinkingDelta { delta, .. } => text.push_str(delta),
            _ => {}
        }
        if text.chars().count() >= 50 {
            signal.cancel();
            abort_fired = true;
        }
    }
    let message = stream.result().await;

    assert_eq!(message.stop_reason, StopReason::Aborted);
    assert!(!message.content.is_empty());

    context.messages.push(Message::Assistant(message));
    context
        .messages
        .push(user("Please continue, but only generate 5 names."));

    let follow_up = complete_simple(model, &context, options.clone()).await;
    assert_eq!(follow_up.stop_reason, StopReason::Stop);
    assert!(!follow_up.content.is_empty());
}

/// `testImmediateAbort(llm, options)`
async fn test_immediate_abort(model: &Model, options: &SimpleStreamOptions) {
    let signal = CancellationToken::new();
    signal.cancel();
    let mut options = options.clone();
    options.base.base.signal = Some(signal);

    let response = complete_simple(model, &context(None, vec![user("Hello")]), options).await;
    assert_eq!(response.stop_reason, StopReason::Aborted);
}

/// `testAbortThenNewMessage(llm, options)`
async fn test_abort_then_new_message(model: &Model, options: &SimpleStreamOptions) {
    let signal = CancellationToken::new();
    signal.cancel();
    let mut aborting = options.clone();
    aborting.base.base.signal = Some(signal);

    let mut context = context(None, vec![user("Hello, how are you?")]);
    let aborted = complete_simple(model, &context, aborting).await;
    assert_eq!(aborted.stop_reason, StopReason::Aborted);
    // Nothing had arrived when the abort fired, so the message is empty.
    assert_eq!(aborted.content.len(), 0);

    context.messages.push(Message::Assistant(aborted));
    context.messages.push(user("What is 2 + 2?"));

    let follow_up = complete_simple(model, &context, options.clone()).await;
    assert_eq!(follow_up.stop_reason, StopReason::Stop);
    assert!(!follow_up.content.is_empty());
}

async fn abort_pair(model: &Model, options: &SimpleStreamOptions) {
    test_abort_signal(model, options).await;
    test_immediate_abort(model, options).await;
}

fn thinking(level: ThinkingLevel) -> SimpleStreamOptions {
    SimpleStreamOptions {
        reasoning: Some(level),
        ..simple_options(None)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn google_provider_abort() {
    skip_unless!(env("GEMINI_API_KEY").is_some(), "GEMINI_API_KEY");
    abort_pair(
        &model("google", "gemini-2.5-flash"),
        &thinking(ThinkingLevel::Low),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_completions_provider_abort() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    let llm = Model {
        api: "openai-completions".to_string(),
        compat: None,
        ..model("openai", "gpt-4o-mini")
    };
    abort_pair(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_responses_provider_abort() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    abort_pair(&model("openai", "gpt-5-mini"), &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn azure_openai_responses_provider_abort() {
    skip_unless!(has_azure_openai_credentials(), "Azure OpenAI credentials");
    let llm = model("azure-openai-responses", "gpt-4o-mini");
    let _ = resolve_azure_deployment_name(&llm.id);
    abort_pair(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_provider_abort() {
    skip_unless!(
        env("ANTHROPIC_OAUTH_TOKEN").is_some(),
        "ANTHROPIC_OAUTH_TOKEN"
    );
    abort_pair(
        &model("anthropic", "claude-sonnet-4-6"),
        &thinking(ThinkingLevel::Low),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mistral_provider_abort() {
    skip_unless!(env("MISTRAL_API_KEY").is_some(), "MISTRAL_API_KEY");
    abort_pair(
        &model("mistral", "devstral-medium-latest"),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn together_provider_abort() {
    skip_unless!(env("TOGETHER_API_KEY").is_some(), "TOGETHER_API_KEY");
    abort_pair(
        &model("together", "moonshotai/Kimi-K2.6"),
        &thinking(ThinkingLevel::High),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn baseten_provider_abort() {
    skip_unless!(env("BASETEN_API_KEY").is_some(), "BASETEN_API_KEY");
    abort_pair(
        &model("baseten", "zai-org/GLM-5.2"),
        &thinking(ThinkingLevel::High),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn minimax_provider_abort() {
    skip_unless!(env("MINIMAX_API_KEY").is_some(), "MINIMAX_API_KEY");
    abort_pair(&model("minimax", "MiniMax-M2.7"), &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn xiaomi_provider_abort() {
    for (variable, provider) in [
        ("XIAOMI_API_KEY", "xiaomi"),
        ("XIAOMI_TOKEN_PLAN_CN_API_KEY", "xiaomi-token-plan-cn"),
        ("XIAOMI_TOKEN_PLAN_AMS_API_KEY", "xiaomi-token-plan-ams"),
        ("XIAOMI_TOKEN_PLAN_SGP_API_KEY", "xiaomi-token-plan-sgp"),
    ] {
        if env(variable).is_none() {
            println!("skipped: {variable}");
            continue;
        }
        abort_pair(&model(provider, "mimo-v2.5-pro"), &simple_options(None)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn qwen_token_plan_provider_abort() {
    for (variable, provider, model_id) in [
        ("QWEN_TOKEN_PLAN_API_KEY", "qwen-token-plan", "qwen3.7-max"),
        (
            "QWEN_TOKEN_PLAN_API_KEY",
            "qwen-token-plan-individual",
            "qwen3.8-max",
        ),
        (
            "QWEN_TOKEN_PLAN_CN_API_KEY",
            "qwen-token-plan-cn",
            "qwen3.7-max",
        ),
    ] {
        if env(variable).is_none() {
            println!("skipped: {variable}");
            continue;
        }
        abort_pair(&model(provider, model_id), &simple_options(None)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn kimi_for_coding_provider_abort() {
    skip_unless!(env("KIMI_API_KEY").is_some(), "KIMI_API_KEY");
    abort_pair(
        &model("kimi-coding", "kimi-for-coding"),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn vercel_ai_gateway_provider_abort() {
    skip_unless!(env("AI_GATEWAY_API_KEY").is_some(), "AI_GATEWAY_API_KEY");
    abort_pair(
        &model("vercel-ai-gateway", "google/gemini-2.5-flash"),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_codex_provider_abort() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    abort_pair(
        &model("openai-codex", "gpt-5.5"),
        &simple_options(Some(&token)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn amazon_bedrock_provider_abort() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    let llm = model(
        "amazon-bedrock",
        "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
    );
    test_abort_signal(&llm, &thinking(ThinkingLevel::Medium)).await;
    test_immediate_abort(&llm, &simple_options(None)).await;
    test_abort_then_new_message(&llm, &simple_options(None)).await;
}
