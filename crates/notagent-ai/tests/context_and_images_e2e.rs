mod e2e_support;

use e2e_support::*;
use notagent_ai::types::*;
use notagent_ai::utils::overflow::is_context_overflow;
use regex::Regex;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

const LOREM_IPSUM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum. ";

/// `generateOverflowContent(contextWindow)`
fn generate_overflow_content(context_window: u64) -> String {
    let target_tokens = context_window + 10_000;
    let target_chars = target_tokens as f64 * 4.0 * 1.5;
    let repetitions = (target_chars / LOREM_IPSUM.len() as f64).ceil() as usize;
    LOREM_IPSUM.repeat(repetitions)
}

/// `testContextOverflow(model, apiKey)` plus the shared assertions of every block.
async fn assert_context_overflow(
    model: &Model,
    options: SimpleStreamOptions,
    error_pattern: Option<&str>,
) {
    let context = context(
        Some("You are a helpful assistant."),
        vec![user(&generate_overflow_content(model.context_window))],
    );
    let response = complete_simple(model, &context, options).await;

    assert_eq!(
        response.stop_reason,
        StopReason::Error,
        "{}/{}: {:?}",
        model.provider,
        model.id,
        response.error_message
    );
    if let Some(pattern) = error_pattern {
        let error_message = response.error_message.clone().unwrap_or_default();
        assert!(
            Regex::new(&format!("(?i){pattern}"))
                .expect("valid pattern")
                .is_match(&error_message),
            "{}/{}: {error_message}",
            model.provider,
            model.id
        );
    }
    assert!(is_context_overflow(&response, Some(model.context_window)));
}

macro_rules! overflow_block {
    ($name:ident, $variable:literal, $provider:literal, $model:literal, $pattern:expr) => {
        #[tokio::test(flavor = "multi_thread")]
        async fn $name() {
            skip_unless!(env($variable).is_some(), $variable);
            assert_context_overflow(&model($provider, $model), simple_options(None), $pattern)
                .await;
        }
    };
}

overflow_block!(
    overflow_anthropic_api_key,
    "ANTHROPIC_API_KEY",
    "anthropic",
    "claude-haiku-4-5",
    Some(r"prompt is too long")
);
overflow_block!(
    overflow_openai_responses,
    "OPENAI_API_KEY",
    "openai",
    "gpt-4o",
    Some(r"exceeds the context window")
);
overflow_block!(
    overflow_google,
    "GEMINI_API_KEY",
    "google",
    "gemini-2.5-flash",
    Some(r"input token count.*exceeds the maximum")
);
overflow_block!(
    overflow_xai,
    "XAI_API_KEY",
    "xai",
    "grok-4.3",
    Some(r"maximum prompt length is \d+")
);
overflow_block!(
    overflow_groq,
    "GROQ_API_KEY",
    "groq",
    "llama-3.3-70b-versatile",
    Some(r"reduce the length of the messages")
);
overflow_block!(
    overflow_huggingface,
    "HF_TOKEN",
    "huggingface",
    "moonshotai/Kimi-K2.5",
    None
);
overflow_block!(
    overflow_together,
    "TOGETHER_API_KEY",
    "together",
    "moonshotai/Kimi-K2.6",
    None
);
overflow_block!(overflow_zai, "ZAI_API_KEY", "zai", "glm-5.2", None);
overflow_block!(
    overflow_mistral,
    "MISTRAL_API_KEY",
    "mistral",
    "devstral-medium-latest",
    Some(r"too large for model with \d+ maximum context length")
);
overflow_block!(
    overflow_minimax,
    "MINIMAX_API_KEY",
    "minimax",
    "MiniMax-M2.7",
    None
);
overflow_block!(
    overflow_xiaomi,
    "XIAOMI_API_KEY",
    "xiaomi",
    "mimo-v2.5-pro",
    None
);
overflow_block!(
    overflow_xiaomi_cn,
    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    "xiaomi-token-plan-cn",
    "mimo-v2.5-pro",
    None
);
overflow_block!(
    overflow_xiaomi_ams,
    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    "xiaomi-token-plan-ams",
    "mimo-v2.5-pro",
    None
);
overflow_block!(
    overflow_xiaomi_sgp,
    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
    "xiaomi-token-plan-sgp",
    "mimo-v2.5-pro",
    None
);
overflow_block!(
    overflow_qwen,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan",
    "qwen3.7-max",
    Some(r"input length")
);
overflow_block!(
    overflow_qwen_individual,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan-individual",
    "qwen3.8-max",
    Some(r"input length")
);
overflow_block!(
    overflow_qwen_cn,
    "QWEN_TOKEN_PLAN_CN_API_KEY",
    "qwen-token-plan-cn",
    "qwen3.7-max",
    Some(r"input length")
);
overflow_block!(
    overflow_kimi,
    "KIMI_API_KEY",
    "kimi-coding",
    "kimi-for-coding",
    None
);
overflow_block!(
    overflow_vercel_gateway,
    "AI_GATEWAY_API_KEY",
    "vercel-ai-gateway",
    "google/gemini-2.5-flash",
    None
);

#[tokio::test(flavor = "multi_thread")]
async fn overflow_anthropic_oauth() {
    let token = skip_unless_some!(env("ANTHROPIC_OAUTH_TOKEN"), "ANTHROPIC_OAUTH_TOKEN");
    assert_context_overflow(
        &model("anthropic", "claude-sonnet-4-6"),
        simple_options(Some(&token)),
        Some(r"prompt is too long"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn overflow_github_copilot_oauth() {
    let token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    if let Some(google_model) = notagent_ai::model_catalog::get_builtin_models("github-copilot")
        .into_iter()
        .find(|candidate| candidate.id.starts_with("gemini-"))
    {
        assert_context_overflow(
            &google_model,
            simple_options(Some(&token)),
            Some(r"exceeds the limit of \d+"),
        )
        .await;
    }
    assert_context_overflow(
        &model("github-copilot", "claude-sonnet-4.6"),
        simple_options(Some(&token)),
        Some(r"exceeds the limit of \d+|input is too long"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn overflow_openai_completions() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    assert_context_overflow(
        &model("openai", "gpt-4o-mini"),
        simple_options(None),
        Some(r"maximum context length"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn overflow_azure_openai_responses() {
    skip_unless!(has_azure_openai_credentials(), "Azure OpenAI credentials");
    let llm = model("azure-openai-responses", "gpt-4o-mini");
    let _ = resolve_azure_deployment_name(&llm.id);
    assert_context_overflow(&llm, simple_options(None), Some(r"context|maximum")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn overflow_cerebras() {
    skip_unless!(env("CEREBRAS_API_KEY").is_some(), "CEREBRAS_API_KEY");
    let llm = notagent_ai::model_catalog::get_builtin_models("cerebras")
        .into_iter()
        .next()
        .expect("a cerebras model");
    assert_context_overflow(
        &llm,
        simple_options(None),
        Some(r"4(00|13|29).*\(no body\)"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn overflow_openai_codex_oauth() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    assert_context_overflow(
        &model("openai-codex", "gpt-5.5"),
        simple_options(Some(&token)),
        None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn overflow_amazon_bedrock() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    assert_context_overflow(
        &model(
            "amazon-bedrock",
            "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
        ),
        simple_options(None),
        None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn overflow_openrouter() {
    skip_unless!(env("OPENROUTER_API_KEY").is_some(), "OPENROUTER_API_KEY");
    for model_id in [
        "anthropic/claude-sonnet-4",
        "deepseek/deepseek-v3.2",
        "mistralai/mistral-large-2512",
        "google/gemini-2.5-flash",
        "meta-llama/llama-4-scout",
    ] {
        assert_context_overflow(
            &model("openrouter", model_id),
            simple_options(None),
            Some(r"maximum context length is \d+ tokens"),
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

fn empty_tool(name: &str, description: &str) -> Tool {
    Tool {
        name: name.to_string(),
        description: description.to_string(),
        parameters: json!({ "type": "object", "properties": {} }),
        constrained_sampling: None,
    }
}

/// `handleToolWithImageResult(model, options)` and, with `text`, the text+image variant.
async fn handle_tool_with_image_result(
    model: &Model,
    options: &SimpleStreamOptions,
    with_text: Option<&str>,
) {
    if !model.input.contains(&Modality::Image) {
        println!(
            "Skipping tool image result test - model {} doesn't support images",
            model.id
        );
        return;
    }

    let tool_name = if with_text.is_some() {
        "get_circle_with_description"
    } else {
        "get_circle"
    };
    let mut context = context(
        Some("You are a helpful assistant that uses tools when asked."),
        vec![user(&format!(
            "Call the {tool_name} tool to get an image, and describe what you see, shapes, colors, etc."
        ))],
    );
    context.tools = Some(vec![empty_tool(
        tool_name,
        "Returns a circle image for visualization",
    )]);

    let first = complete_simple(model, &context, options.clone()).await;
    assert_eq!(
        first.stop_reason,
        StopReason::ToolUse,
        "{:?}",
        first.error_message
    );
    let tool_call = tool_calls(&first)
        .first()
        .cloned()
        .cloned()
        .expect("Expected tool call");
    assert_eq!(tool_call.name, tool_name);
    context.messages.push(Message::Assistant(first));

    let mut result_content = Vec::new();
    if let Some(text) = with_text {
        result_content.push(text_block(text));
    }
    result_content.push(image_block("image/png", &red_circle_png()));
    context
        .messages
        .push(Message::ToolResult(ToolResultMessage {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content: result_content,
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: now_ms(),
        }));

    let second = complete_simple(model, &context, options.clone()).await;
    assert_eq!(
        second.stop_reason,
        StopReason::Stop,
        "{:?}",
        second.error_message
    );
    assert_eq!(second.error_message, None);
    let text = message_text(&second).to_lowercase();
    assert!(text.contains("red"), "{text}");
    assert!(text.contains("circle"), "{text}");
}

async fn image_tool_result_suite(model: &Model, options: &SimpleStreamOptions) {
    handle_tool_with_image_result(model, options, None).await;
    handle_tool_with_image_result(model, options, Some("This is a circle in a bright color."))
        .await;
}

macro_rules! image_tool_block {
    ($name:ident, $variable:literal, $provider:literal, $model:literal) => {
        #[tokio::test(flavor = "multi_thread")]
        async fn $name() {
            skip_unless!(env($variable).is_some(), $variable);
            image_tool_result_suite(&model($provider, $model), &simple_options(None)).await;
        }
    };
}

image_tool_block!(
    image_tool_google_provider,
    "GEMINI_API_KEY",
    "google",
    "gemini-2.5-flash"
);
image_tool_block!(
    image_tool_openai_responses_provider,
    "OPENAI_API_KEY",
    "openai",
    "gpt-5-mini"
);
image_tool_block!(
    image_tool_anthropic_provider,
    "ANTHROPIC_API_KEY",
    "anthropic",
    "claude-haiku-4-5"
);
image_tool_block!(
    image_tool_openrouter_provider,
    "OPENROUTER_API_KEY",
    "openrouter",
    "z-ai/glm-4.5v"
);
image_tool_block!(
    image_tool_mistral_provider,
    "MISTRAL_API_KEY",
    "mistral",
    "pixtral-12b"
);
image_tool_block!(
    image_tool_together_provider,
    "TOGETHER_API_KEY",
    "together",
    "moonshotai/Kimi-K2.6"
);
image_tool_block!(
    image_tool_baseten_provider,
    "BASETEN_API_KEY",
    "baseten",
    "moonshotai/Kimi-K2.6"
);
image_tool_block!(
    image_tool_xiaomi_provider,
    "XIAOMI_API_KEY",
    "xiaomi",
    "mimo-v2.5-pro"
);
image_tool_block!(
    image_tool_xiaomi_cn_provider,
    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    "xiaomi-token-plan-cn",
    "mimo-v2.5-pro"
);
image_tool_block!(
    image_tool_xiaomi_ams_provider,
    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    "xiaomi-token-plan-ams",
    "mimo-v2.5-pro"
);
image_tool_block!(
    image_tool_xiaomi_sgp_provider,
    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
    "xiaomi-token-plan-sgp",
    "mimo-v2.5-pro"
);
image_tool_block!(
    image_tool_qwen_provider,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan",
    "qwen3.7-max"
);
image_tool_block!(
    image_tool_qwen_individual_provider,
    "QWEN_TOKEN_PLAN_API_KEY",
    "qwen-token-plan-individual",
    "qwen3.8-max"
);

#[tokio::test(flavor = "multi_thread")]
async fn image_tool_openai_completions_provider() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    let llm = Model {
        api: "openai-completions".to_string(),
        compat: None,
        ..model("openai", "gpt-4o-mini")
    };
    image_tool_result_suite(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn image_tool_azure_openai_responses_provider() {
    skip_unless!(has_azure_openai_credentials(), "Azure OpenAI credentials");
    let llm = model("azure-openai-responses", "gpt-4o-mini");
    let _ = resolve_azure_deployment_name(&llm.id);
    image_tool_result_suite(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn image_tool_amazon_bedrock_provider() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    image_tool_result_suite(
        &model(
            "amazon-bedrock",
            "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
        ),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn image_tool_anthropic_oauth_provider() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    image_tool_result_suite(
        &model("anthropic", "claude-haiku-4-5"),
        &simple_options(Some(&token)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn image_tool_github_copilot_provider() {
    let token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    image_tool_result_suite(
        &model("github-copilot", "claude-sonnet-4.6"),
        &simple_options(Some(&token)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn image_tool_openai_codex_provider() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    image_tool_result_suite(
        &model("openai-codex", "gpt-5.5"),
        &simple_options(Some(&token)),
    )
    .await;
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// Asserts that the replayed payload carries the tool result image inside
/// `function_call_output` rather than as a later user message.
async fn assert_tool_result_images_in_function_call_output(
    model: &Model,
    options: &SimpleStreamOptions,
) {
    let tool_text = "The image shows a shape.";
    let mut context = context(
        Some("You are a helpful assistant that uses tools when asked."),
        vec![user(
            "Call the get_circle tool to get an image, and describe what you see.",
        )],
    );
    context.tools = Some(vec![empty_tool(
        "get_circle",
        "Returns a circle image for visualization",
    )]);

    let first = complete_simple(model, &context, options.clone()).await;
    assert_eq!(
        first.stop_reason,
        StopReason::ToolUse,
        "Error: {:?}",
        first.error_message
    );
    let tool_call = tool_calls(&first)
        .first()
        .cloned()
        .cloned()
        .expect("a tool call");
    context.messages.push(Message::Assistant(first));
    context
        .messages
        .push(Message::ToolResult(ToolResultMessage {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content: vec![
                text_block(tool_text),
                image_block("image/png", &red_circle_png()),
            ],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: now_ms(),
        }));

    let mut second_options = options.clone();
    let payloads = capture_payloads(&mut second_options);
    let second = complete_simple(model, &context, second_options).await;
    assert_eq!(
        second.stop_reason,
        StopReason::Stop,
        "Error: {:?}",
        second.error_message
    );
    assert_eq!(second.error_message, None);

    let payload = payloads.first().expect("a captured payload");
    let input = payload["input"].as_array().expect("input array");
    let output_index = input
        .iter()
        .position(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
        .expect("a function_call_output item");
    let output = input[output_index]["output"]
        .as_array()
        .expect("an output array");
    let text_item = output
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("input_text"))
        .expect("an input_text item");
    let image_item = output
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("input_image"))
        .expect("an input_image item");
    assert!(
        text_item["text"]
            .as_str()
            .is_some_and(|text| text.contains(tool_text)),
        "{text_item}"
    );
    assert!(
        image_item["image_url"]
            .as_str()
            .is_some_and(|url| url.starts_with("data:image/png;base64,")),
        "{image_item}"
    );
    // The image must not be smuggled in as a later user message.
    assert_eq!(
        input[output_index + 1..]
            .iter()
            .filter(|item| item.get("role").and_then(Value::as_str) == Some("user"))
            .count(),
        0
    );

    let text = message_text(&second).to_lowercase();
    assert!(text.contains("red"), "{text}");
    assert!(text.contains("circle"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_responses_sends_tool_result_images_in_function_call_output() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    assert_tool_result_images_in_function_call_output(
        &model("openai", "gpt-5-mini"),
        &simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn azure_openai_responses_sends_tool_result_images_in_function_call_output() {
    skip_unless!(has_azure_openai_credentials(), "Azure OpenAI credentials");
    let llm = model("azure-openai-responses", "gpt-4o-mini");
    let _ = resolve_azure_deployment_name(&llm.id);
    assert_tool_result_images_in_function_call_output(&llm, &simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn github_copilot_sends_tool_result_images_in_function_call_output() {
    let token = skip_unless_some!(
        resolve_api_key("github-copilot").await,
        "github-copilot OAuth"
    );
    assert_tool_result_images_in_function_call_output(
        &model("github-copilot", "gpt-5-mini"),
        &simple_options(Some(&token)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_codex_sends_tool_result_images_in_function_call_output() {
    let token = skip_unless_some!(resolve_api_key("openai-codex").await, "openai-codex OAuth");
    assert_tool_result_images_in_function_call_output(
        &model("openai-codex", "gpt-5.5"),
        &simple_options(Some(&token)),
    )
    .await;
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// Runs a two-turn conversation whose first turn is produced by `first_model` and whose
/// second turn replays that history on `second_model` — the shape both suites share.
async fn assert_reasoning_replay(
    first_model: &Model,
    first_options: SimpleStreamOptions,
    second_model: &Model,
    second_options: SimpleStreamOptions,
) {
    let mut context = context(
        Some("You are a helpful assistant. Think before answering."),
        vec![user(
            "Think step by step: what is 6 times 7? Reply with just the number.",
        )],
    );

    let first = complete_simple(first_model, &context, first_options).await;
    assert_ne!(
        first.stop_reason,
        StopReason::Error,
        "Error: {:?}",
        first.error_message
    );
    context.messages.push(Message::Assistant(first));
    context
        .messages
        .push(user("Repeat that number and nothing else."));

    let second = complete_simple(second_model, &context, second_options).await;
    assert_ne!(
        second.stop_reason,
        StopReason::Error,
        "Error: {:?}",
        second.error_message
    );
    assert_eq!(second.error_message, None);
    assert!(!second.content.is_empty());
    assert!(
        message_text(&second).contains("42"),
        "{}",
        message_text(&second)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_responses_replays_its_own_reasoning() {
    skip_unless!(
        env("OPENAI_API_KEY").is_some() && env("ANTHROPIC_API_KEY").is_some(),
        "OPENAI_API_KEY + ANTHROPIC_API_KEY"
    );
    let llm = model("openai", "gpt-5-mini");
    assert_reasoning_replay(&llm, simple_options(None), &llm, simple_options(None)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_responses_replays_reasoning_across_models() {
    skip_unless!(
        env("OPENAI_API_KEY").is_some() && env("ANTHROPIC_API_KEY").is_some(),
        "OPENAI_API_KEY + ANTHROPIC_API_KEY"
    );
    assert_reasoning_replay(
        &model("openai", "gpt-5-mini"),
        simple_options(None),
        &model("openai", "gpt-5.5"),
        simple_options(None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_responses_replays_anthropic_history() {
    skip_unless!(
        env("OPENAI_API_KEY").is_some() && env("ANTHROPIC_API_KEY").is_some(),
        "OPENAI_API_KEY + ANTHROPIC_API_KEY"
    );
    assert_reasoning_replay(
        &model("anthropic", "claude-sonnet-4-5"),
        simple_options(None),
        &model("openai", "gpt-5.5"),
        simple_options(None),
    )
    .await;
}

/// the next without erroring.
#[tokio::test(flavor = "multi_thread")]
async fn cross_provider_handoff() {
    skip_unless!(e2e_enabled(), "NOTAGENT_AI_E2E");
    let mut configured: Vec<(Model, SimpleStreamOptions)> = Vec::new();
    for (variable, provider, model_id) in [
        ("ANTHROPIC_API_KEY", "anthropic", "claude-haiku-4-5"),
        ("OPENAI_API_KEY", "openai", "gpt-5-mini"),
        ("GEMINI_API_KEY", "google", "gemini-2.5-flash"),
        ("XAI_API_KEY", "xai", "grok-4.3"),
        ("MISTRAL_API_KEY", "mistral", "devstral-medium-latest"),
        ("OPENROUTER_API_KEY", "openrouter", "z-ai/glm-4.5v"),
    ] {
        if env(variable).is_some() {
            configured.push((model(provider, model_id), simple_options(None)));
        }
    }
    if let Some(token) = resolve_api_key("openai-codex").await {
        configured.push((
            model("openai-codex", "gpt-5.5"),
            simple_options(Some(&token)),
        ));
    }
    if let Some(token) = resolve_api_key("github-copilot").await {
        configured.push((
            model("github-copilot", "claude-sonnet-4.6"),
            simple_options(Some(&token)),
        ));
    }
    if has_bedrock_credentials() {
        configured.push((
            model(
                "amazon-bedrock",
                "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
            ),
            simple_options(None),
        ));
    }
    if configured.len() < 2 {
        println!("skipped: fewer than two providers configured");
        return;
    }

    for (source, source_options) in &configured {
        for (target, target_options) in &configured {
            if source.provider == target.provider {
                continue;
            }
            assert_reasoning_replay(
                source,
                source_options.clone(),
                target,
                target_options.clone(),
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// The two `NOTAGENT_CACHE_RETENTION` cases that need a live key. The variable is read
/// instead of mutating the process environment (which would race across tests).
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_cache_ttl_follows_notagent_cache_retention() {
    skip_unless!(env("ANTHROPIC_API_KEY").is_some(), "ANTHROPIC_API_KEY");
    let llm = model("anthropic", "claude-haiku-4-5");
    let context = context(Some("You are a helpful assistant."), vec![user("Hello")]);

    // Unset: cache_control without a ttl.
    let mut options = simple_options(None);
    let payloads = capture_payloads(&mut options);
    let _ = complete_simple(&llm, &context, options).await;
    let payload = payloads.first().expect("a captured payload");
    assert_eq!(
        payload["system"][0]["cache_control"],
        json!({ "type": "ephemeral" })
    );

    // `long`: cache_control with `ttl: "1h"`.
    let mut options = simple_options(None);
    options.base.base.env = Some(
        [("NOTAGENT_CACHE_RETENTION".to_string(), "long".to_string())]
            .into_iter()
            .collect(),
    );
    let payloads = capture_payloads(&mut options);
    let _ = complete_simple(&llm, &context, options).await;
    let payload = payloads.first().expect("a captured payload");
    assert_eq!(
        payload["system"][0]["cache_control"],
        json!({ "type": "ephemeral", "ttl": "1h" })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_responses_prompt_cache_retention_follows_notagent_cache_retention() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    let llm = model("openai", "gpt-5-mini");
    let context = context(Some("You are a helpful assistant."), vec![user("Hello")]);

    let mut options = simple_options(None);
    let payloads = capture_payloads(&mut options);
    let _ = complete_simple(&llm, &context, options).await;
    let payload = payloads.first().expect("a captured payload");
    assert_eq!(payload.get("prompt_cache_retention"), None);

    let mut options = simple_options(None);
    options.base.base.env = Some(
        [("NOTAGENT_CACHE_RETENTION".to_string(), "long".to_string())]
            .into_iter()
            .collect(),
    );
    let payloads = capture_payloads(&mut options);
    let _ = complete_simple(&llm, &context, options).await;
    let payload = payloads.first().expect("a captured payload");
    assert_eq!(payload["prompt_cache_retention"], "24h");
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn bedrock_uses_the_model_max_tokens_cap_instead_of_the_4096_default() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    let llm = Model {
        max_tokens: 6000,
        ..model("amazon-bedrock", "global.anthropic.claude-sonnet-4-6")
    };
    let context = context(
        Some("You are a deterministic text generator. Follow the requested output format exactly."),
        vec![user(
            "Output exactly 5200 repetitions of the token alpha, separated by single spaces. Do not number them. Do not use markdown. Do not add any other text.",
        )],
    );

    let response = complete_simple(
        &llm,
        &context,
        SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Low),
            ..simple_options(None)
        },
    )
    .await;

    assert_ne!(
        response.stop_reason,
        StopReason::Error,
        "{:?}",
        response.error_message
    );
    assert!(response.usage.output > 4096, "{}", response.usage.output);
}
