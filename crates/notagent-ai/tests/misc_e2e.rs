mod e2e_support;

use e2e_support::*;
use notagent_ai::types::*;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// `expectResponseId(model, options)`
async fn expect_response_id(model: &Model, options: SimpleStreamOptions) {
    let context = context(
        Some("You are a helpful assistant. Be concise."),
        vec![user("Reply with exactly: response id test")],
    );
    let response = complete_simple(model, &context, options).await;

    assert_ne!(
        response.stop_reason,
        StopReason::Error,
        "{:?}",
        response.error_message
    );
    assert!(
        response
            .response_id
            .as_deref()
            .is_some_and(|id| !id.is_empty()),
        "{:?}",
        response.response_id
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn response_id_google_provider() {
    skip_unless!(env("GEMINI_API_KEY").is_some(), "GEMINI_API_KEY");
    expect_response_id(&model("google", "gemini-2.5-flash"), simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn response_id_google_vertex_provider() {
    let configured = env("GOOGLE_CLOUD_PROJECT")
        .or_else(|| env("GCLOUD_PROJECT"))
        .is_some()
        && env("GOOGLE_CLOUD_LOCATION").is_some();
    let api_key = env("GOOGLE_CLOUD_API_KEY");
    skip_unless!(
        configured || api_key.is_some(),
        "GOOGLE_CLOUD_PROJECT + GOOGLE_CLOUD_LOCATION or GOOGLE_CLOUD_API_KEY"
    );
    let llm = model("google-vertex", "gemini-3-flash-preview");
    if configured {
        expect_response_id(&llm, simple_options(None)).await;
    }
    if let Some(api_key) = api_key {
        expect_response_id(&llm, simple_options(Some(&api_key))).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn response_id_openai_providers() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    let completions = Model {
        api: "openai-completions".to_string(),
        compat: None,
        ..model("openai", "gpt-4o-mini")
    };
    expect_response_id(&completions, simple_options(None)).await;
    expect_response_id(&model("openai", "gpt-5-mini"), simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn response_id_anthropic_provider() {
    skip_unless!(env("ANTHROPIC_API_KEY").is_some(), "ANTHROPIC_API_KEY");
    expect_response_id(
        &model("anthropic", "claude-sonnet-4-5"),
        simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn response_id_azure_openai_responses_provider() {
    skip_unless!(has_azure_openai_credentials(), "Azure OpenAI credentials");
    let llm = model("azure-openai-responses", "gpt-4o-mini");
    let _ = resolve_azure_deployment_name(&llm.id);
    expect_response_id(&llm, simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn response_id_mistral_provider() {
    skip_unless!(env("MISTRAL_API_KEY").is_some(), "MISTRAL_API_KEY");
    expect_response_id(
        &model("mistral", "devstral-medium-latest"),
        simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn response_id_github_copilot_provider() {
    let token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    expect_response_id(
        &model("github-copilot", "gpt-5.3-codex"),
        simple_options(Some(&token)),
    )
    .await;
    expect_response_id(
        &model("github-copilot", "claude-sonnet-4.6"),
        simple_options(Some(&token)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn response_id_openai_codex_provider() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    expect_response_id(
        &model("openai-codex", "gpt-5.5"),
        simple_options(Some(&token)),
    )
    .await;
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn opencode_models_smoke_test() {
    skip_unless!(env("OPENCODE_API_KEY").is_some(), "OPENCODE_API_KEY");
    for provider in ["opencode", "opencode-go"] {
        for model in notagent_ai::model_catalog::get_builtin_models(provider) {
            let response = complete_simple(
                &model,
                &context(None, vec![user("Say hello.")]),
                simple_options(None),
            )
            .await;
            assert!(!response.content.is_empty(), "{}", model.id);
            assert_eq!(response.stop_reason, StopReason::Stop, "{}", model.id);
        }
    }
}

// ---------------------------------------------------------------------------
// cache-affinity e2e
// ---------------------------------------------------------------------------

const CACHE_AFFINITY_SESSION_ID: &str = "0195d6e4-4cf9-7f44-a2d8-f8f7f49ee9d3";

fn cache_affinity_context(reply: &str) -> Context {
    context(
        Some("You are a helpful assistant. Reply exactly as requested."),
        vec![user(&format!("Reply with exactly: {reply}"))],
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_handles_sse_requests_with_aligned_cache_affinity_identifiers() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    let mut options = simple_options(Some(&token));
    options.base.session_id = Some(CACHE_AFFINITY_SESSION_ID.to_string());
    options.base.transport = Some(Transport::Sse);

    let response = complete_simple(
        &model("openai-codex", "gpt-5.5"),
        &cache_affinity_context("cache affinity e2e success"),
        options,
    )
    .await;

    assert_ne!(
        response.stop_reason,
        StopReason::Error,
        "{:?}",
        response.error_message
    );
    assert_eq!(response.error_message, None);
    assert!(message_text(&response).contains("cache affinity e2e success"));
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_responses_handles_aligned_cache_affinity_identifiers() {
    let api_key = skip_unless_some!(env("OPENAI_API_KEY"), "OPENAI_API_KEY");
    let mut options = simple_options(Some(&api_key));
    options.base.session_id = Some(CACHE_AFFINITY_SESSION_ID.to_string());

    let response = complete_simple(
        &model("openai", "gpt-5.4"),
        &cache_affinity_context("openai cache affinity e2e success"),
        options,
    )
    .await;

    assert_ne!(
        response.stop_reason,
        StopReason::Error,
        "{:?}",
        response.error_message
    );
    assert_eq!(response.error_message, None);
    assert!(message_text(&response).contains("openai cache affinity e2e success"));
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// `createLongSystemPrompt()` — the nonce keeps the prefix unique per run.
fn long_system_prompt() -> String {
    let body = std::iter::repeat_n(
        "Prompt-caching probe content. Keep this exact text stable across requests so the provider can reuse prefix tokens and report cache read and cache write usage.",
        80,
    )
    .collect::<Vec<_>>()
    .join("\n\n");
    format!(
        "You are a concise assistant.\nCache nonce: {}\n\n{body}",
        now_ms()
    )
}

fn mark_last_user_text_with_cache_control(payload: &mut Value) {
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages.iter_mut().rev() {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        if let Some(text) = message.get("content").and_then(Value::as_str) {
            message["content"] = json!([{
                "type": "text",
                "text": text,
                "cache_control": { "type": "ephemeral" },
            }]);
            return;
        }
        let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
            return;
        };
        for part in content.iter_mut().rev() {
            if part.get("type").and_then(Value::as_str) == Some("text") {
                part["cache_control"] = json!({ "type": "ephemeral" });
                return;
            }
        }
        return;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn openrouter_preserves_cache_write_tokens_on_the_completions_stream_path() {
    let api_key = skip_unless_some!(env("OPENROUTER_API_KEY"), "OPENROUTER_API_KEY");
    let llm = model("openrouter", "google/gemini-2.5-flash");
    let context = context(
        Some(&long_system_prompt()),
        vec![user("Reply with exactly: OK")],
    );

    let mut options = simple_options(Some(&api_key));
    options.base.max_tokens = Some(32);
    options.base.temperature = Some(0.0);
    options.base.base.on_payload = Some(std::sync::Arc::new(|payload: Value, _model: &Model| {
        let mut payload = payload;
        mark_last_user_text_with_cache_control(&mut payload);
        Box::pin(async move { Some(payload) })
    }));

    let first = complete_simple(&llm, &context, options.clone()).await;
    assert_eq!(
        first.stop_reason,
        StopReason::Stop,
        "{:?}",
        first.error_message
    );
    let second = complete_simple(&llm, &context, options).await;
    assert_eq!(
        second.stop_reason,
        StopReason::Stop,
        "{:?}",
        second.error_message
    );

    // With the cache_control marker at least one of the two calls has to create cache.
    assert!(first.usage.cache_write > 0 || second.usage.cache_write > 0);
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

fn count_pongs(text: &str) -> usize {
    text.to_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| *word == "pong")
        .count()
}

/// `expectThinkingDisabledE2E(model, expectations)`
async fn expect_thinking_disabled_e2e(
    model: &Model,
    options: SimpleStreamOptions,
    min_pongs: usize,
    max_output_tokens: Option<u64>,
) {
    let context = context(
        Some("You are a precise assistant. Follow the requested output format exactly."),
        vec![user(
            "Before replying, carefully solve 36863 * 5279 internally. Then reply with the word pong repeated exactly 40 times, separated by single spaces. Do not add any other text.",
        )],
    );
    let (events, response) = collect_stream_simple(model, &context, options).await;

    let mut thinking_event_count = 0usize;
    let mut thinking_char_count = 0usize;
    for event in &events {
        match event {
            AssistantMessageEvent::ThinkingStart { .. }
            | AssistantMessageEvent::ThinkingEnd { .. } => {
                thinking_event_count += 1;
            }
            AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                thinking_event_count += 1;
                thinking_char_count += delta.encode_utf16().count();
            }
            _ => {}
        }
    }

    assert_eq!(
        response.stop_reason,
        StopReason::Stop,
        "{:?}",
        response.error_message
    );
    assert_eq!(thinking_event_count, 0);
    assert_eq!(thinking_char_count, 0);
    assert!(!has_thinking(&response));
    assert!(count_pongs(message_text(&response).trim()) >= min_pongs);
    if let Some(max_output_tokens) = max_output_tokens {
        assert!(response.usage.output < max_output_tokens);
    }
}

fn disable_options(max_tokens: u64, temperature: Option<f64>) -> SimpleStreamOptions {
    let mut options = simple_options(None);
    options.base.max_tokens = Some(max_tokens);
    options.base.temperature = temperature;
    options
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_thinking_disable_for_budget_and_adaptive_models() {
    skip_unless!(env("ANTHROPIC_API_KEY").is_some(), "ANTHROPIC_API_KEY");
    for model_id in ["claude-sonnet-4-5", "claude-sonnet-4-6"] {
        expect_thinking_disabled_e2e(
            &model("anthropic", model_id),
            disable_options(320, Some(0.0)),
            35,
            None,
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn google_thinking_disable() {
    skip_unless!(env("GEMINI_API_KEY").is_some(), "GEMINI_API_KEY");
    for model_id in ["gemini-2.5-flash", "gemini-3-flash-preview"] {
        expect_thinking_disabled_e2e(
            &model("google", model_id),
            disable_options(160, Some(0.0)),
            35,
            None,
        )
        .await;
    }
    expect_thinking_disabled_e2e(
        &model("google", "gemini-3.1-pro-preview"),
        disable_options(512, Some(0.0)),
        20,
        None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn google_vertex_thinking_disable() {
    let api_key = env("GOOGLE_CLOUD_API_KEY");
    let configured = env("GOOGLE_CLOUD_PROJECT")
        .or_else(|| env("GCLOUD_PROJECT"))
        .is_some()
        && env("GOOGLE_CLOUD_LOCATION").is_some();
    skip_unless!(
        api_key.is_some() || configured,
        "GOOGLE_CLOUD_API_KEY or GOOGLE_CLOUD_PROJECT + GOOGLE_CLOUD_LOCATION"
    );
    for model_id in ["gemini-2.5-flash", "gemini-3-flash-preview"] {
        let mut options = disable_options(160, Some(0.0));
        options.base.base.api_key = api_key.clone();
        expect_thinking_disabled_e2e(&model("google-vertex", model_id), options, 35, None).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_thinking_disable_for_responses_reasoning_models() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    expect_thinking_disabled_e2e(
        &model("openai", "gpt-5.4-mini"),
        disable_options(160, None),
        35,
        None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openrouter_thinking_disable_for_qwen_reasoning_models() {
    skip_unless!(env("OPENROUTER_API_KEY").is_some(), "OPENROUTER_API_KEY");
    expect_thinking_disabled_e2e(
        &model("openrouter", "qwen/qwen3.5-plus-02-15"),
        disable_options(160, Some(0.0)),
        35,
        Some(100),
    )
    .await;
}
