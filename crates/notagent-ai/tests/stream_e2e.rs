//! Port of `packages/ai/test/stream.test.ts` (1 747 LOC) — the provider matrix.
//!
//! TS applies six scenarios (`basicTextGeneration`, `handleToolCall`, `handleStreaming`,
//! `handleThinking`, `multiTurn`, `handleImage`) across ~35 `describe` blocks, one per
//! provider/model. The port keeps the scenarios as functions and drives them from a table,
//! one test per TS `describe` block; `Scenario` names the six exactly as TS does.
//!
//! Every block is key-gated like its TS counterpart (see `e2e_support`): without the
//! credential the test prints `skipped: …` and returns without making a request.

mod e2e_support;

use e2e_support::*;
use notagent_ai::types::*;
use serde_json::json;

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

/// `basicTextGeneration(model, options)`
async fn basic_text_generation(model: &Model, options: &SimpleStreamOptions) {
    let mut context = context(
        Some("You are a helpful assistant. Be concise."),
        vec![user("Reply with exactly: 'Hello test successful'")],
    );
    let response = complete_simple(model, &context, options.clone()).await;

    assert!(!response.content.is_empty(), "{:?}", response.error_message);
    assert!(response.usage.input + response.usage.cache_read > 0);
    assert!(response.usage.output > 0);
    assert_eq!(response.error_message, None);
    assert!(message_text(&response).contains("Hello test successful"));

    context.messages.push(Message::Assistant(response));
    context
        .messages
        .push(user("Now say 'Goodbye test successful'"));

    let second = complete_simple(model, &context, options.clone()).await;
    assert!(!second.content.is_empty(), "{:?}", second.error_message);
    assert!(second.usage.input + second.usage.cache_read > 0);
    assert!(second.usage.output > 0);
    assert_eq!(second.error_message, None);
    assert!(message_text(&second).contains("Goodbye test successful"));
}

/// `handleToolCall(model, options)`
async fn handle_tool_call(model: &Model, options: &SimpleStreamOptions) {
    let mut context = context(
        Some("You are a helpful assistant that uses tools when asked."),
        vec![user("Calculate 15 + 27 using the math_operation tool.")],
    );
    context.tools = Some(vec![calculator_tool()]);

    let (events, response) = collect_stream_simple(model, &context, options.clone()).await;

    let mut has_tool_start = false;
    let mut has_tool_delta = false;
    let mut has_tool_end = false;
    let mut accumulated = String::new();
    let mut index = 0usize;
    for event in &events {
        match event {
            AssistantMessageEvent::ToolcallStart {
                content_index,
                partial,
            } => {
                has_tool_start = true;
                index = *content_index;
                let AssistantContent::ToolCall(tool_call) = &partial.content[*content_index] else {
                    panic!("expected a toolCall block");
                };
                assert_eq!(tool_call.name, "math_operation");
                assert!(!tool_call.id.is_empty());
            }
            AssistantMessageEvent::ToolcallDelta {
                content_index,
                delta,
                partial,
            } => {
                has_tool_delta = true;
                assert_eq!(*content_index, index);
                let AssistantContent::ToolCall(tool_call) = &partial.content[*content_index] else {
                    panic!("expected a toolCall block");
                };
                assert_eq!(tool_call.name, "math_operation");
                accumulated.push_str(delta);
            }
            AssistantMessageEvent::ToolcallEnd {
                content_index,
                partial,
                ..
            } => {
                has_tool_end = true;
                assert_eq!(*content_index, index);
                let AssistantContent::ToolCall(tool_call) = &partial.content[*content_index] else {
                    panic!("expected a toolCall block");
                };
                assert_eq!(tool_call.name, "math_operation");
                serde_json::from_str::<serde_json::Value>(&accumulated).expect("valid JSON");
                assert_eq!(argument(tool_call, "a"), Some(json!(15)));
                assert_eq!(argument(tool_call, "b"), Some(json!(27)));
                let operation = argument(tool_call, "operation").expect("operation");
                assert!(
                    ["add", "subtract", "multiply", "divide"]
                        .contains(&operation.as_str().unwrap_or_default()),
                    "{operation}"
                );
            }
            _ => {}
        }
    }

    assert!(has_tool_start);
    assert!(has_tool_delta);
    assert!(has_tool_end);
    assert_eq!(response.stop_reason, StopReason::ToolUse);
    let calls = tool_calls(&response);
    assert!(!calls.is_empty(), "No tool call found in response");
    assert_eq!(calls[0].name, "math_operation");
    assert!(!calls[0].id.is_empty());
}

/// `handleStreaming(model, options)`
async fn handle_streaming(model: &Model, options: &SimpleStreamOptions) {
    let context = context(
        Some("You are a helpful assistant."),
        vec![user("Count from 1 to 3")],
    );
    let (events, response) = collect_stream_simple(model, &context, options.clone()).await;

    let mut text_started = false;
    let mut text_chunks = String::new();
    let mut text_completed = false;
    for event in &events {
        match event {
            AssistantMessageEvent::TextStart { .. } => text_started = true,
            AssistantMessageEvent::TextDelta { delta, .. } => text_chunks.push_str(delta),
            AssistantMessageEvent::TextEnd { .. } => text_completed = true,
            _ => {}
        }
    }

    assert!(text_started);
    assert!(!text_chunks.is_empty());
    assert!(text_completed);
    assert!(has_text(&response));
}

/// `handleThinking(model, options)`
async fn handle_thinking(model: &Model, options: &SimpleStreamOptions) {
    // TS randomizes the addend so the provider cannot serve a cached answer; the port
    // uses the clock for the same effect (scripts get no RNG).
    let addend = (now_ms() % 255) as u32;
    let context = context(
        Some("You are a helpful assistant."),
        vec![user(&format!(
            "Think long and hard about {addend} + 27. Think step by step. Then output the result."
        ))],
    );
    let (events, response) = collect_stream_simple(model, &context, options.clone()).await;

    let mut thinking_started = false;
    let mut thinking_chunks = String::new();
    let mut thinking_completed = false;
    for event in &events {
        match event {
            AssistantMessageEvent::ThinkingStart { .. } => thinking_started = true,
            AssistantMessageEvent::ThinkingDelta { delta, .. } => thinking_chunks.push_str(delta),
            AssistantMessageEvent::ThinkingEnd { .. } => thinking_completed = true,
            _ => {}
        }
    }

    assert_eq!(
        response.stop_reason,
        StopReason::Stop,
        "Error: {:?}",
        response.error_message
    );
    assert!(thinking_started);
    assert!(!thinking_chunks.is_empty());
    assert!(thinking_completed);
    assert!(has_thinking(&response));
}

/// `handleImage(model, options)`
async fn handle_image(model: &Model, options: &SimpleStreamOptions) {
    if !model.input.contains(&Modality::Image) {
        println!(
            "Skipping image test - model {} doesn't support images",
            model.id
        );
        return;
    }

    let context = context(
        Some("You are a helpful assistant."),
        vec![user_blocks(vec![
            text_block(
                "What do you see in this image? Please describe the shape (circle, rectangle, square, triangle, ...) and color (red, blue, green, ...). You MUST reply in English.",
            ),
            image_block("image/png", &red_circle_png()),
        ])],
    );
    let response = complete_simple(model, &context, options.clone()).await;

    assert!(!response.content.is_empty(), "{:?}", response.error_message);
    let text = message_text(&response).to_lowercase();
    assert!(text.contains("red"), "{text}");
    assert!(text.contains("circle"), "{text}");
}

/// `multiTurn(model, options)`
async fn multi_turn(model: &Model, options: &SimpleStreamOptions) {
    let mut context = context(
        Some("You are a helpful assistant that can use tools to answer questions."),
        vec![user(
            "Think about this briefly, then calculate 42 * 17 and 453 + 434 using the math_operation tool.",
        )],
    );
    context.tools = Some(vec![calculator_tool()]);

    let mut all_text = String::new();
    let mut seen_thinking = false;
    let mut seen_tool_calls = false;

    for _turn in 0..5 {
        let response = complete_simple(model, &context, options.clone()).await;
        context.messages.push(Message::Assistant(response.clone()));

        let mut results = Vec::new();
        for block in &response.content {
            match block {
                AssistantContent::Text(block) => all_text.push_str(&block.text),
                AssistantContent::Thinking(_) => seen_thinking = true,
                AssistantContent::ToolCall(tool_call) => {
                    seen_tool_calls = true;
                    assert_eq!(tool_call.name, "math_operation");
                    assert!(!tool_call.id.is_empty());

                    let a = argument(tool_call, "a")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let b = argument(tool_call, "b")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let result = match argument(tool_call, "operation")
                        .and_then(|value| value.as_str().map(str::to_string))
                        .as_deref()
                    {
                        Some("add") => a + b,
                        Some("multiply") => a * b,
                        _ => 0.0,
                    };
                    results.push(Message::ToolResult(ToolResultMessage {
                        tool_call_id: tool_call.id.clone(),
                        tool_name: tool_call.name.clone(),
                        content: vec![text_block(&notagent_ai::utils::js_number::to_js_string(
                            result,
                        ))],
                        details: None,
                        usage: None,
                        added_tool_names: None,
                        is_error: false,
                        timestamp: now_ms(),
                    }));
                }
            }
        }
        context.messages.extend(results);

        assert_ne!(
            response.stop_reason,
            StopReason::Error,
            "Error: {:?}",
            response.error_message
        );
        if response.stop_reason == StopReason::Stop {
            break;
        }
    }

    assert!(seen_thinking || seen_tool_calls);
    assert!(!all_text.is_empty());
    assert!(all_text.contains("714"), "{all_text}");
    assert!(all_text.contains("887"), "{all_text}");
}

// ---------------------------------------------------------------------------
// The provider matrix
// ---------------------------------------------------------------------------

/// The six scenarios, named as in TS.
#[derive(Clone, Copy)]
enum Scenario {
    BasicText,
    ToolCall,
    Streaming,
    Thinking,
    MultiTurn,
    Image,
}

async fn run(model: &Model, options: &SimpleStreamOptions, scenarios: &[Scenario]) {
    for scenario in scenarios {
        match scenario {
            Scenario::BasicText => basic_text_generation(model, options).await,
            Scenario::ToolCall => handle_tool_call(model, options).await,
            Scenario::Streaming => handle_streaming(model, options).await,
            Scenario::Thinking => handle_thinking(model, options).await,
            Scenario::MultiTurn => multi_turn(model, options).await,
            Scenario::Image => handle_image(model, options).await,
        }
    }
}

use Scenario::{BasicText, Image, MultiTurn, Streaming, Thinking, ToolCall};

/// Options with a thinking level, for the blocks that ask for reasoning.
fn thinking_options(level: ThinkingLevel) -> SimpleStreamOptions {
    SimpleStreamOptions {
        reasoning: Some(level),
        ..simple_options(None)
    }
}

fn options_with_header(name: &str, value: &str) -> SimpleStreamOptions {
    let mut options = simple_options(None);
    options.base.base.headers = Some(
        [(name.to_string(), Some(value.to_string()))]
            .into_iter()
            .collect(),
    );
    options
}

#[tokio::test(flavor = "multi_thread")]
async fn gemini_provider_gemini_2_5_flash() {
    skip_unless!(env("GEMINI_API_KEY").is_some(), "GEMINI_API_KEY");
    run(
        &model("google", "gemini-2.5-flash"),
        &simple_options(None),
        &[BasicText, ToolCall, Streaming, MultiTurn, Image],
    )
    .await;
    run(
        &model("google", "gemini-2.5-flash"),
        &thinking_options(ThinkingLevel::Low),
        &[Thinking],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn google_vertex_provider_gemini_3_flash_preview() {
    let configured = env("GOOGLE_CLOUD_PROJECT")
        .or_else(|| env("GCLOUD_PROJECT"))
        .is_some()
        && env("GOOGLE_CLOUD_LOCATION").is_some();
    skip_unless!(configured, "GOOGLE_CLOUD_PROJECT + GOOGLE_CLOUD_LOCATION");
    let model = model("google-vertex", "gemini-3-flash-preview");
    run(
        &model,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming, MultiTurn, Image],
    )
    .await;
    run(&model, &thinking_options(ThinkingLevel::Low), &[Thinking]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn google_vertex_provider_with_api_key() {
    skip_unless!(
        env("GOOGLE_CLOUD_API_KEY").is_some(),
        "GOOGLE_CLOUD_API_KEY"
    );
    let api_key = env("GOOGLE_CLOUD_API_KEY");
    run(
        &model("google-vertex", "gemini-3-flash-preview"),
        &simple_options(api_key.as_deref()),
        &[BasicText],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_completions_provider_gpt_4o_mini() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    // TS drops `compat` and pins the api to openai-completions.
    let llm = Model {
        api: "openai-completions".to_string(),
        compat: None,
        ..model("openai", "gpt-4o-mini")
    };
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming, Image],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deepseek_provider_deepseek_v4_flash() {
    skip_unless!(env("DEEPSEEK_API_KEY").is_some(), "DEEPSEEK_API_KEY");
    let llm = model("deepseek", "deepseek-v4-flash");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
    run(
        &llm,
        &thinking_options(ThinkingLevel::Medium),
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_responses_provider_gpt_5_4() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    let llm = model("openai", "gpt-5.4");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming, Image],
    )
    .await;
    run(
        &llm,
        &thinking_options(ThinkingLevel::Medium),
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_provider_claude_haiku_4_5() {
    skip_unless!(env("ANTHROPIC_API_KEY").is_some(), "ANTHROPIC_API_KEY");
    run(
        &model("anthropic", "claude-haiku-4-5"),
        &simple_options(None),
        &[BasicText, ToolCall, Streaming, Image],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn azure_openai_responses_provider_gpt_4o_mini() {
    skip_unless!(has_azure_openai_credentials(), "Azure OpenAI credentials");
    let llm = model("azure-openai-responses", "gpt-4o-mini");
    // TS passes `azureDeploymentName` when the map configures one; the Rust adapter reads
    // the same map from the environment, so no extra option is needed.
    let _ = resolve_azure_deployment_name(&llm.id);
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming, Image],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn xai_provider_grok_4_3() {
    skip_unless!(env("XAI_API_KEY").is_some(), "XAI_API_KEY");
    let llm = model("xai", "grok-4.3");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
    run(
        &llm,
        &thinking_options(ThinkingLevel::Medium),
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn groq_provider_gpt_oss_20b() {
    skip_unless!(env("GROQ_API_KEY").is_some(), "GROQ_API_KEY");
    let llm = model("groq", "openai/gpt-oss-20b");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
    run(
        &llm,
        &thinking_options(ThinkingLevel::Medium),
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cerebras_provider_gpt_oss_120b() {
    skip_unless!(env("CEREBRAS_API_KEY").is_some(), "CEREBRAS_API_KEY");
    let llm = model("cerebras", "gpt-oss-120b");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
    run(
        &llm,
        &thinking_options(ThinkingLevel::Medium),
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cloudflare_workers_ai_provider_kimi_k2_6() {
    skip_unless!(
        has_cloudflare_workers_ai_credentials(),
        "Cloudflare Workers AI credentials"
    );
    let llm = model("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
    run(
        &llm,
        &thinking_options(ThinkingLevel::Medium),
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cloudflare_ai_gateway_workers_ai_kimi_k2_6() {
    skip_unless!(
        has_cloudflare_ai_gateway_credentials(),
        "Cloudflare AI Gateway credentials"
    );
    let llm = model(
        "cloudflare-ai-gateway",
        "workers-ai/@cf/moonshotai/kimi-k2.6",
    );
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
    run(
        &llm,
        &thinking_options(ThinkingLevel::Medium),
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cloudflare_ai_gateway_openai_byok_gpt_5_1() {
    skip_unless!(
        has_cloudflare_ai_gateway_credentials() && env("OPENAI_API_KEY").is_some(),
        "Cloudflare AI Gateway + OPENAI_API_KEY"
    );
    let llm = model("cloudflare-ai-gateway", "gpt-5.1");
    let options = options_with_header(
        "Authorization",
        &format!("Bearer {}", env("OPENAI_API_KEY").unwrap_or_default()),
    );
    run(&llm, &options, &[BasicText, ToolCall, Streaming]).await;
    run(
        &llm,
        &SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Medium),
            ..options
        },
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cloudflare_ai_gateway_anthropic_byok_claude_sonnet_4_5() {
    skip_unless!(
        has_cloudflare_ai_gateway_credentials() && env("ANTHROPIC_API_KEY").is_some(),
        "Cloudflare AI Gateway + ANTHROPIC_API_KEY"
    );
    let llm = model("cloudflare-ai-gateway", "claude-sonnet-4-5");
    let options = options_with_header(
        "Authorization",
        &format!("Bearer {}", env("ANTHROPIC_API_KEY").unwrap_or_default()),
    );
    run(&llm, &options, &[BasicText, ToolCall, Streaming]).await;
    run(
        &llm,
        &SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..options
        },
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn huggingface_provider_kimi_k2_5() {
    skip_unless!(env("HF_TOKEN").is_some(), "HF_TOKEN");
    let llm = model("huggingface", "moonshotai/Kimi-K2.5");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn together_provider_kimi_k2_6() {
    skip_unless!(env("TOGETHER_API_KEY").is_some(), "TOGETHER_API_KEY");
    let llm = model("together", "moonshotai/Kimi-K2.6");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn baseten_provider_glm_5_2() {
    skip_unless!(env("BASETEN_API_KEY").is_some(), "BASETEN_API_KEY");
    let llm = model("baseten", "zai-org/GLM-5.2");
    let options = thinking_options(ThinkingLevel::High);
    run(
        &llm,
        &options,
        &[BasicText, ToolCall, Streaming, Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nvidia_provider_nemotron_3_super() {
    skip_unless!(env("NVIDIA_API_KEY").is_some(), "NVIDIA_API_KEY");
    let llm = model("nvidia", "nvidia/nemotron-3-super-120b-a12b");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openrouter_provider_glm_4_5v() {
    skip_unless!(env("OPENROUTER_API_KEY").is_some(), "OPENROUTER_API_KEY");
    let llm = model("openrouter", "z-ai/glm-4.5v");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming, Image],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn vercel_ai_gateway_providers() {
    skip_unless!(env("AI_GATEWAY_API_KEY").is_some(), "AI_GATEWAY_API_KEY");
    for model_id in [
        "google/gemini-2.5-flash",
        "anthropic/claude-opus-4.5",
        "openai/gpt-5.1-codex-max",
    ] {
        let llm = model("vercel-ai-gateway", model_id);
        run(
            &llm,
            &simple_options(None),
            &[BasicText, ToolCall, Streaming],
        )
        .await;
        run(
            &llm,
            &thinking_options(ThinkingLevel::Medium),
            &[Thinking, MultiTurn],
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn zai_provider_glm_5_2() {
    skip_unless!(env("ZAI_API_KEY").is_some(), "ZAI_API_KEY");
    let llm = model("zai", "glm-5.2");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
    run(
        &llm,
        &thinking_options(ThinkingLevel::Medium),
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mistral_provider() {
    skip_unless!(env("MISTRAL_API_KEY").is_some(), "MISTRAL_API_KEY");
    run(
        &model("mistral", "devstral-medium-latest"),
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
    run(
        &model("mistral", "mistral-small-2603"),
        &thinking_options(ThinkingLevel::Medium),
        &[Thinking, MultiTurn],
    )
    .await;
    run(
        &model("mistral", "pixtral-12b"),
        &simple_options(None),
        &[Image],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn minimax_provider_m2_7() {
    skip_unless!(env("MINIMAX_API_KEY").is_some(), "MINIMAX_API_KEY");
    let llm = model("minimax", "MiniMax-M2.7");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn kimi_coding_provider() {
    skip_unless!(env("KIMI_API_KEY").is_some(), "KIMI_API_KEY");
    let llm = model("kimi-coding", "kimi-for-coding");
    run(
        &llm,
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn xiaomi_providers_mimo_v2_5_pro() {
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
        let llm = model(provider, "mimo-v2.5-pro");
        run(
            &llm,
            &simple_options(None),
            &[BasicText, ToolCall, Streaming],
        )
        .await;
        run(
            &llm,
            &thinking_options(ThinkingLevel::Medium),
            &[Thinking, MultiTurn],
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn qwen_token_plan_providers() {
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
        let llm = model(provider, model_id);
        run(
            &llm,
            &simple_options(None),
            &[BasicText, ToolCall, Streaming],
        )
        .await;
        run(
            &llm,
            &thinking_options(ThinkingLevel::Medium),
            &[Thinking, MultiTurn],
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ant_ling_provider() {
    skip_unless!(env("ANT_LING_API_KEY").is_some(), "ANT_LING_API_KEY");
    run(
        &model("ant-ling", "Ling-2.6-flash"),
        &simple_options(None),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
    run(
        &model("ant-ling", "Ring-2.6-1T"),
        &thinking_options(ThinkingLevel::Medium),
        &[Thinking],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_oauth_provider_claude_sonnet_4_6() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    run(
        &model("anthropic", "claude-sonnet-4-6"),
        &simple_options(Some(&token)),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_oauth_provider_claude_opus_4_6_adaptive_thinking() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    run(
        &model("anthropic", "claude-opus-4-6"),
        &SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..simple_options(Some(&token))
        },
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn github_copilot_provider_gpt_5_3_codex() {
    let token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    run(
        &model("github-copilot", "gpt-5.3-codex"),
        &simple_options(Some(&token)),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
    run(
        &model("github-copilot", "gpt-5-mini"),
        &SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Medium),
            ..simple_options(Some(&token))
        },
        &[Thinking, MultiTurn],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn github_copilot_provider_claude_sonnet_4_6() {
    let token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    run(
        &model("github-copilot", "claude-sonnet-4.6"),
        &simple_options(Some(&token)),
        &[BasicText, ToolCall, Streaming],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_codex_provider() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    for model_id in ["gpt-5.4", "gpt-5.5"] {
        run(
            &model("openai-codex", model_id),
            &simple_options(Some(&token)),
            &[BasicText, ToolCall, Streaming],
        )
        .await;
        run(
            &model("openai-codex", model_id),
            &SimpleStreamOptions {
                reasoning: Some(ThinkingLevel::Medium),
                ..simple_options(Some(&token))
            },
            &[Thinking, MultiTurn],
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_codex_provider_websocket() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    let mut options = simple_options(Some(&token));
    options.base.transport = Some(Transport::Websocket);
    run(
        &model("openai-codex", "gpt-5.5"),
        &options,
        &[BasicText, ToolCall, Streaming],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn amazon_bedrock_provider_claude_sonnet_4_5() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    run(
        &model(
            "amazon-bedrock",
            "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
        ),
        &simple_options(None),
        &[BasicText, ToolCall, Streaming, Image],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn amazon_bedrock_provider_claude_opus_4_6_interleaved_thinking() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    run(
        &model("amazon-bedrock", "global.anthropic.claude-opus-4-6-v1"),
        &thinking_options(ThinkingLevel::High),
        &[Thinking, MultiTurn],
    )
    .await;
    run(
        &model(
            "amazon-bedrock",
            "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
        ),
        &thinking_options(ThinkingLevel::Medium),
        &[Thinking],
    )
    .await;
}
