//! Ports of the key-gated Anthropic-family suites:
//! `anthropic-opus-4-8-smoke.test.ts` (71), `anthropic-thinking-disable.test.ts` (172,
//! e2e half), `anthropic-tool-name-normalization.test.ts` (204),
//! `anthropic-eager-tool-input-e2e.test.ts` (155),
//! `anthropic-long-cache-retention-e2e.test.ts` (127),
//! `interleaved-thinking.test.ts` (144),
//! `xiaomi-token-plan-ams-anthropic-empty-signature-smoke.test.ts` (114) and
//! `xhigh.test.ts` (70).
//!
//! Gating follows `e2e_support`; the payload halves of these files are offline and live in
//! `anthropic_params.rs`.

mod e2e_support;

use e2e_support::*;
use notagent_ai::types::*;
use serde_json::json;

fn tool(name: &str, description: &str, property: &str, property_description: &str) -> Tool {
    Tool {
        name: name.to_string(),
        description: description.to_string(),
        parameters: json!({
            "type": "object",
            "properties": { property: { "type": "string", "description": property_description } },
            "required": [property],
        }),
        constrained_sampling: None,
    }
}

// ---------------------------------------------------------------------------
// anthropic-opus-4-8-smoke.test.ts
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn streams_claude_opus_4_8_with_reasoning_enabled() {
    skip_unless!(env("ANTHROPIC_API_KEY").is_some(), "ANTHROPIC_API_KEY");
    let model = model("anthropic", "claude-opus-4-8");
    let context = context(
        Some("You are a precise assistant. Follow the user's instructions exactly."),
        vec![user(
            "Compute 48291 * 7317 and 90844 - 17729, add the results, and determine whether the sum is divisible by 11. Reply with exactly this format and nothing else: sum=<sum>; divisibleBy11=<yes|no>",
        )],
    );
    let mut options = SimpleStreamOptions {
        reasoning: Some(ThinkingLevel::High),
        ..simple_options(None)
    };
    options.base.max_tokens = Some(1024);
    let payloads = capture_payloads(&mut options);

    let (events, response) = collect_stream_simple(&model, &context, options).await;
    let saw_thinking = events.iter().any(|event| {
        matches!(
            event,
            AssistantMessageEvent::ThinkingStart { .. }
                | AssistantMessageEvent::ThinkingDelta { .. }
                | AssistantMessageEvent::ThinkingEnd { .. }
        )
    });

    assert_eq!(
        response.stop_reason,
        StopReason::Stop,
        "{:?}",
        response.error_message
    );
    assert_eq!(response.error_message, None);
    let payload = payloads.first().expect("a captured payload");
    assert_eq!(payload["thinking"], json!({ "type": "adaptive" }));
    assert_eq!(payload["output_config"], json!({ "effort": "high" }));
    assert!(saw_thinking);

    let thinking = response
        .content
        .iter()
        .find_map(|block| match block {
            AssistantContent::Thinking(block) => Some(block),
            _ => None,
        })
        .expect("Expected thinking block from Claude Opus 4.8");
    let signature = thinking
        .thinking_signature
        .as_deref()
        .expect("Expected thinking signature from Claude Opus 4.8");
    assert!(!signature.is_empty());

    assert_eq!(
        message_text(&response).trim(),
        "sum=353418362; divisibleBy11=yes"
    );
}

// ---------------------------------------------------------------------------
// anthropic-thinking-disable.test.ts (E2E half)
// ---------------------------------------------------------------------------

fn count_pongs(text: &str) -> usize {
    text.to_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| *word == "pong")
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn disables_thinking_for_claude_reasoning_models() {
    skip_unless!(env("ANTHROPIC_API_KEY").is_some(), "ANTHROPIC_API_KEY");
    let model = model("anthropic", "claude-sonnet-4-5");
    let context = context(
        Some("You are a precise assistant. Follow the requested output format exactly."),
        vec![user(
            "Before replying, carefully solve 36863 * 5279 internally. Then reply with the word pong repeated exactly 40 times, separated by single spaces. Do not add any other text.",
        )],
    );
    // `reasoning` unset — `streamSimple` then disables thinking outright.
    let mut options = simple_options(None);
    options.base.temperature = Some(0.0);
    options.base.max_tokens = Some(160);

    let (events, response) = collect_stream_simple(&model, &context, options).await;

    let thinking_events = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AssistantMessageEvent::ThinkingStart { .. }
                    | AssistantMessageEvent::ThinkingDelta { .. }
                    | AssistantMessageEvent::ThinkingEnd { .. }
            )
        })
        .count();
    assert_eq!(thinking_events, 0);
    assert_eq!(thinking_text(&response).len(), 0);
    assert!(!has_thinking(&response));
    assert!(count_pongs(&message_text(&response)) >= 35);
}

// ---------------------------------------------------------------------------
// anthropic-tool-name-normalization.test.ts
// ---------------------------------------------------------------------------

/// The name the tool call carries when it comes back out of the stream.
async fn round_tripped_tool_name(
    token: &str,
    system_prompt: &str,
    prompt: &str,
    tool: Tool,
) -> Option<String> {
    let model = model("anthropic", "claude-sonnet-4-6");
    let mut context = context(Some(system_prompt), vec![user(prompt)]);
    context.tools = Some(vec![tool]);

    let (events, response) = collect_stream(&model, &context, stream_options(Some(token))).await;
    assert_eq!(
        response.stop_reason,
        StopReason::ToolUse,
        "Error: {:?}",
        response.error_message
    );

    events.iter().rev().find_map(|event| match event {
        AssistantMessageEvent::ToolcallEnd {
            content_index,
            partial,
            ..
        } => match &partial.content[*content_index] {
            AssistantContent::ToolCall(tool_call) => Some(tool_call.name.clone()),
            _ => None,
        },
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn normalizes_a_user_defined_tool_matching_a_claude_code_name() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    let name = round_tripped_tool_name(
        &token,
        "You are a helpful assistant. Use the todowrite tool when asked to add todos.",
        "Add a todo: buy milk. Use the todowrite tool.",
        tool("todowrite", "Write a todo item", "task", "The task to add"),
    )
    .await;
    // The tool call comes back with the ORIGINAL name, not the Claude Code casing.
    assert_eq!(name.as_deref(), Some("todowrite"));
}

#[tokio::test(flavor = "multi_thread")]
async fn handles_notagents_built_in_tools() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    let name = round_tripped_tool_name(
        &token,
        "You are a helpful assistant. Use the read tool to read files.",
        "Read the file /tmp/test.txt using the read tool.",
        tool("read", "Read a file", "path", "File path"),
    )
    .await;
    assert_eq!(name.as_deref(), Some("read"));
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_map_find_to_glob() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    let name = round_tripped_tool_name(
        &token,
        "You are a helpful assistant. Use the find tool to search for files.",
        "Find all .ts files using the find tool.",
        tool("find", "Find files by pattern", "pattern", "Glob pattern"),
    )
    .await;
    // `find` is not a Claude Code tool name, so nothing is mapped and the round trip works.
    assert_eq!(name.as_deref(), Some("find"));
}

#[tokio::test(flavor = "multi_thread")]
async fn handles_custom_tools_that_match_no_claude_code_name() {
    let token = skip_unless_some!(resolve_api_key("anthropic").await, "anthropic OAuth");
    let name = round_tripped_tool_name(
        &token,
        "You are a helpful assistant. Use my_custom_tool when asked.",
        "Use my_custom_tool with input 'hello'.",
        tool("my_custom_tool", "A custom tool", "input", "Input value"),
    )
    .await;
    assert_eq!(name.as_deref(), Some("my_custom_tool"));
}

// ---------------------------------------------------------------------------
// anthropic-eager-tool-input-e2e.test.ts / anthropic-long-cache-retention-e2e.test.ts
// ---------------------------------------------------------------------------

/// `getAnthropicMessagesModels(provider)` over every builtin provider.
fn anthropic_messages_cases() -> Vec<(String, Model)> {
    let mut cases = Vec::new();
    for provider in notagent_ai::providers::all::builtin_providers() {
        for model in provider.get_models() {
            if model.api == "anthropic-messages" {
                cases.push((format!("{}/{}", provider.id(), model.id), model));
            }
        }
    }
    cases
}

/// `getProbePriority(model)` — cheap current Claude 4 routes first.
fn probe_priority(model: &Model) -> f64 {
    let model_id = model.id.to_lowercase();
    let mut priority = model.cost.input + model.cost.output;
    if model_id.contains("haiku") && (model_id.contains("4-5") || model_id.contains("4.5")) {
        priority -= 1000.0;
    } else if model_id.contains("sonnet") && (model_id.contains("4-") || model_id.contains("4.")) {
        priority -= 750.0;
    } else if model_id.contains("claude") && (model_id.contains("4-") || model_id.contains("4.")) {
        priority -= 500.0;
    }
    priority
}

/// `selectOneCasePerProvider(cases)`
fn one_case_per_provider(cases: Vec<(String, Model)>) -> Vec<(String, Model)> {
    let mut by_provider: std::collections::BTreeMap<String, (String, Model)> =
        std::collections::BTreeMap::new();
    for (name, model) in cases {
        let entry = by_provider.entry(model.provider.clone());
        match entry {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((name, model));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if probe_priority(&model) < probe_priority(&entry.get().1) {
                    entry.insert((name, model));
                }
            }
        }
    }
    by_provider.into_values().collect()
}

/// `getE2EApiKey(provider)` — the Copilot token comes from auth.json, the rest from env.
async fn e2e_api_key(provider: &str) -> Option<String> {
    if provider == "github-copilot" {
        return resolve_api_key("github-copilot").await;
    }
    e2e_enabled()
        .then(|| notagent_ai::env_api_keys::get_env_api_key(provider, None))
        .flatten()
}

fn echo_tool() -> Tool {
    tool(
        "echo_value",
        "Echo a string value",
        "value",
        "The value to echo",
    )
}

/// `expectToolEnabledRequestAccepted(model, apiKey)`
async fn expect_tool_enabled_request_accepted(model: &Model, api_key: &str) {
    let mut context = context(
        Some("You are a concise assistant. Use tools when useful."),
        vec![user(
            "Call echo_value with value set to eager-input-streaming-compat.",
        )],
    );
    context.tools = Some(vec![echo_tool()]);
    let mut options = simple_options(Some(api_key));
    options.base.max_tokens = Some(128);

    let response = complete_simple(model, &context, options).await;
    assert_eq!(response.error_message, None, "{}", model.id);
    assert_ne!(response.stop_reason, StopReason::Error, "{}", model.id);
}

#[test]
fn eager_tool_input_covers_every_generated_anthropic_messages_model() {
    // The TS case list is built from the same catalog read, so this asserts the port sees
    // the same models rather than a stale copy.
    let cases = anthropic_messages_cases();
    assert!(!cases.is_empty());
    let mut names: Vec<&str> = cases.iter().map(|(name, _)| name.as_str()).collect();
    names.sort_unstable();
    let mut expected: Vec<String> = notagent_ai::providers::all::builtin_providers()
        .iter()
        .flat_map(|provider| {
            provider
                .get_models()
                .into_iter()
                .filter(|model| model.api == "anthropic-messages")
                .map(|model| format!("{}/{}", provider.id(), model.id))
                .collect::<Vec<_>>()
        })
        .collect();
    expected.sort();
    assert_eq!(names, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_provider_accepts_configured_tool_streaming() {
    skip_unless!(e2e_enabled(), "NOTAGENT_AI_E2E");
    for (name, model) in one_case_per_provider(anthropic_messages_cases()) {
        let Some(api_key) = e2e_api_key(&model.provider).await else {
            println!("skipped: {name}");
            continue;
        };
        expect_tool_enabled_request_accepted(&model, &api_key).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_provider_accepts_forced_eager_input_streaming() {
    skip_unless!(e2e_enabled(), "NOTAGENT_AI_E2E");
    for (name, model) in one_case_per_provider(anthropic_messages_cases()) {
        let Some(api_key) = e2e_api_key(&model.provider).await else {
            println!("skipped: {name}");
            continue;
        };
        // `withEagerToolInputStreaming(model)`
        let model = Model {
            compat: Some(
                ModelCompat::from_api_value(
                    "anthropic-messages",
                    json!({ "supportsEagerToolInputStreaming": true }),
                )
                .expect("compat"),
            ),
            ..model
        };
        expect_tool_enabled_request_accepted(&model, &api_key).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_provider_accepts_long_cache_retention() {
    skip_unless!(e2e_enabled(), "NOTAGENT_AI_E2E");
    for (name, model) in one_case_per_provider(anthropic_messages_cases()) {
        let Some(api_key) = e2e_api_key(&model.provider).await else {
            println!("skipped: {name}");
            continue;
        };
        // `withLongCacheRetention(model)`
        let model = Model {
            compat: Some(
                ModelCompat::from_api_value(
                    "anthropic-messages",
                    json!({ "supportsLongCacheRetention": true }),
                )
                .expect("compat"),
            ),
            ..model
        };
        let context = context(
            None,
            vec![user("Reply with exactly: long cache retention accepted")],
        );
        let mut options = simple_options(Some(&api_key));
        options.base.cache_retention = Some(CacheRetention::Long);
        options.base.max_tokens = Some(128);

        let response = complete_simple(&model, &context, options).await;
        assert_eq!(response.error_message, None, "{name}");
        assert_ne!(response.stop_reason, StopReason::Error, "{name}");
    }
}

// ---------------------------------------------------------------------------
// interleaved-thinking.test.ts
// ---------------------------------------------------------------------------

fn interleaved_calculator_tool() -> Tool {
    Tool {
        name: "calculator".to_string(),
        description: "Perform basic arithmetic operations".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "a": { "type": "number", "description": "First number" },
                "b": { "type": "number", "description": "Second number" },
                "operation": {
                    "type": "string",
                    "enum": ["add", "subtract", "multiply", "divide"],
                    "description": "The operation to perform.",
                },
            },
            "required": ["a", "b", "operation"],
        }),
        constrained_sampling: None,
    }
}

fn evaluate_calculator_call(tool_call: &ToolCall) -> f64 {
    let a = argument(tool_call, "a")
        .and_then(|value| value.as_f64())
        .expect("Invalid calculator arguments");
    let b = argument(tool_call, "b")
        .and_then(|value| value.as_f64())
        .expect("Invalid calculator arguments");
    match argument(tool_call, "operation")
        .and_then(|value| value.as_str().map(str::to_string))
        .as_deref()
    {
        Some("add") => a + b,
        Some("subtract") => a - b,
        Some("multiply") => a * b,
        Some("divide") => a / b,
        other => panic!("Invalid calculator arguments: {other:?}"),
    }
}

/// `assertSecondToolCallWithInterleavedThinking(llm, reasoning)`
async fn assert_second_tool_call_with_interleaved_thinking(
    model: &Model,
    reasoning: ThinkingLevel,
) {
    let mut context = context(
        Some(
            "You are a helpful assistant that must use tools for arithmetic. Always think before every tool call, not just the first one. Do not answer with plain text when a tool call is required.",
        ),
        vec![user(
            "Use calculator to calculate 328 * 29. You must call the calculator tool exactly once. Provide the final answer based on the best guess given the tool result, even if it seems unreliable. Start by thinking about the steps you will take to solve the problem.",
        )],
    );
    context.tools = Some(vec![interleaved_calculator_tool()]);
    let options = SimpleStreamOptions {
        reasoning: Some(reasoning),
        ..simple_options(None)
    };

    let first = complete_simple(model, &context, options.clone()).await;
    assert_eq!(
        first.stop_reason,
        StopReason::ToolUse,
        "Error: {:?}",
        first.error_message
    );
    assert!(has_thinking(&first));
    let first_tool_call = tool_calls(&first)
        .first()
        .cloned()
        .cloned()
        .expect("Expected first response to include a tool call");

    context.messages.push(Message::Assistant(first));

    let correct = evaluate_calculator_call(&first_tool_call);
    context
        .messages
        .push(Message::ToolResult(ToolResultMessage {
            tool_call_id: first_tool_call.id.clone(),
            tool_name: first_tool_call.name.clone(),
            content: vec![text_block(&format!(
                "The answer is {} or {}.",
                notagent_ai::utils::js_number::to_js_string(correct),
                notagent_ai::utils::js_number::to_js_string(correct * 2.0)
            ))],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: now_ms(),
        }));

    let second = complete_simple(model, &context, options).await;
    assert_eq!(
        second.stop_reason,
        StopReason::Stop,
        "Error: {:?}",
        second.error_message
    );
    assert!(has_thinking(&second));
    assert!(has_text(&second));
}

#[tokio::test(flavor = "multi_thread")]
async fn amazon_bedrock_interleaved_thinking() {
    skip_unless!(has_bedrock_credentials(), "AWS credentials");
    for model_id in [
        "global.anthropic.claude-opus-4-5-20251101-v1:0",
        "global.anthropic.claude-opus-4-6-v1",
    ] {
        assert_second_tool_call_with_interleaved_thinking(
            &model("amazon-bedrock", model_id),
            ThinkingLevel::High,
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_interleaved_thinking() {
    skip_unless!(env("ANTHROPIC_API_KEY").is_some(), "ANTHROPIC_API_KEY");
    for model_id in ["claude-opus-4-5", "claude-opus-4-6"] {
        assert_second_tool_call_with_interleaved_thinking(
            &model("anthropic", model_id),
            ThinkingLevel::High,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// xiaomi-token-plan-ams-anthropic-empty-signature-smoke.test.ts
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn xiaomi_reproduces_empty_thinking_signatures_and_preserves_them_for_replay() {
    let api_key = skip_unless_some!(
        env("XIAOMI_TOKEN_PLAN_AMS_API_KEY"),
        "XIAOMI_TOKEN_PLAN_AMS_API_KEY"
    );
    let model = model("xiaomi-token-plan-ams", "mimo-v2.5-pro");
    let mut first_context = context(
        Some("You are a precise assistant."),
        vec![user(
            "Think briefly, then reply with exactly this text and nothing else: first-ok",
        )],
    );
    let mut options = SimpleStreamOptions {
        reasoning: Some(ThinkingLevel::High),
        ..simple_options(Some(&api_key))
    };
    options.base.max_tokens = Some(512);

    let first = complete_simple(&model, &first_context, options.clone()).await;
    assert_eq!(
        first.stop_reason,
        StopReason::Stop,
        "{:?}",
        first.error_message
    );

    let thinking_blocks: Vec<&ThinkingContent> = first
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantContent::Thinking(block) => Some(block),
            _ => None,
        })
        .collect();
    assert!(!thinking_blocks.is_empty());
    assert!(
        thinking_blocks
            .iter()
            .any(|block| block.thinking_signature.as_deref() == Some(""))
    );
    let first_thinking = thinking_blocks[0].thinking.clone();

    first_context.messages.push(Message::Assistant(first));
    first_context.messages.push(user(
        "Reply with exactly this text and nothing else: second-ok",
    ));

    let mut replay_options = options;
    let payloads = capture_payloads(&mut replay_options);
    let _ = complete_simple(&model, &first_context, replay_options).await;

    let payload = payloads.first().expect("a captured payload");
    let assistant = payload["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("an assistant message");
    let content = assistant["content"].as_array().expect("content array");
    let replayed_thinking: Vec<&serde_json::Value> = content
        .iter()
        .filter(|block| block["type"] == "thinking")
        .collect();
    assert_eq!(
        replayed_thinking,
        vec![&json!({ "type": "thinking", "thinking": first_thinking, "signature": "" })]
    );
    assert!(
        !content
            .iter()
            .any(|block| block["type"] == "text" && block["text"] == json!(first_thinking))
    );
}

// ---------------------------------------------------------------------------
// xhigh.test.ts
// ---------------------------------------------------------------------------

fn xhigh_context() -> Context {
    let addend = now_ms() % 100;
    let other = (now_ms() / 7) % 100;
    context(
        None,
        vec![user(&format!(
            "What is {addend} + {other}? Think step by step."
        ))],
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn xhigh_works_with_openai_responses() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    let (events, response) = collect_stream_simple(
        &model("openai", "gpt-5.5"),
        &xhigh_context(),
        SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Xhigh),
            ..simple_options(None)
        },
    )
    .await;

    let has_thinking_event = events.iter().any(|event| {
        matches!(
            event,
            AssistantMessageEvent::ThinkingStart { .. }
                | AssistantMessageEvent::ThinkingDelta { .. }
        )
    });
    assert_eq!(
        response.stop_reason,
        StopReason::Stop,
        "Error: {:?}",
        response.error_message
    );
    assert!(has_text(&response));
    assert!(has_thinking_event || has_thinking(&response));
}

#[tokio::test(flavor = "multi_thread")]
async fn xhigh_errors_on_models_that_do_not_support_it() {
    skip_unless!(env("OPENAI_API_KEY").is_some(), "OPENAI_API_KEY");
    let responses_model = model("openai", "gpt-5-mini");
    let completions_model = Model {
        api: "openai-completions".to_string(),
        compat: None,
        ..responses_model.clone()
    };

    for model in [responses_model, completions_model] {
        let response = complete_simple(
            &model,
            &xhigh_context(),
            SimpleStreamOptions {
                reasoning: Some(ThinkingLevel::Xhigh),
                ..simple_options(None)
            },
        )
        .await;
        assert_eq!(response.stop_reason, StopReason::Error, "{}", model.api);
        assert!(
            response
                .error_message
                .as_deref()
                .is_some_and(|message| message.contains("xhigh")),
            "{:?}",
            response.error_message
        );
    }
}
