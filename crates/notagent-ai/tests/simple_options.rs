use notagent_ai::api::simple_options::*;
use notagent_ai::api::transform_messages::transform_messages;
use notagent_ai::types::*;

fn model(context_window: u64, max_tokens: u64) -> Model {
    Model {
        id: "test-model".to_string(),
        name: "Test Model".to_string(),
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window,
        max_tokens,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn user(text: &str, timestamp: i64) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp,
    })
}

fn assistant(
    model_id: &str,
    provider: &str,
    content: Vec<AssistantContent>,
    stop: StopReason,
) -> Message {
    Message::Assistant(AssistantMessage {
        content,
        api: "openai-responses".to_string(),
        provider: provider.to_string(),
        model: model_id.to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 1,
    })
}

#[test]
fn clamps_max_tokens_against_the_remaining_context() {
    // (a newer user message precedes it), so the estimate is 1 005 tokens and
    // `buildBaseOptions(...).maxTokens` must come out as 4 899.
    let stale_assistant = Message::Assistant(AssistantMessage {
        content: vec![AssistantContent::Text(TextContent::new("kept"))],
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        model: "test-model".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage {
            input: 9_500,
            total_tokens: Some(9_500),
            ..Usage::default()
        },
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 100,
    });
    let context = Context {
        system_prompt: Some("system".to_string()),
        messages: vec![
            user("summary", 200),
            stale_assistant,
            user(&"x".repeat(4_000), 300),
        ],
        tools: None,
    };
    let model = model(10_000, 8_000);
    assert_eq!(
        notagent_ai::utils::estimate::estimate_context_tokens(&context).tokens,
        1_005
    );
    let base = build_base_options(&model, &context, None, None);
    assert_eq!(base.max_tokens, Some(4_899));
}

#[test]
fn a_zero_context_window_disables_clamping() {
    let model = model(0, 8_000);
    let context = Context::default();
    assert_eq!(clamp_max_tokens_to_context(&model, &context, 8_000), 8_000);
}

#[test]
fn an_overlong_context_still_leaves_one_token() {
    let model = model(100, 8_000);
    let context = Context {
        system_prompt: Some("x".repeat(10_000)),
        messages: vec![],
        tools: None,
    };
    assert_eq!(clamp_max_tokens_to_context(&model, &context, 8_000), 1);
}

#[test]
fn merges_model_and_request_sampling_params() {
    let mut model = model(10_000, 8_000);
    model.sampling_params = Some(
        serde_json::json!({"top_p": 0.9, "top_k": 40})
            .as_object()
            .unwrap()
            .clone(),
    );
    let options = SimpleStreamOptions {
        base: StreamOptions {
            sampling_params: Some(
                serde_json::json!({"top_p": 0.5})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            ..Default::default()
        },
        ..Default::default()
    };
    let base = build_base_options(&model, &Context::default(), Some(&options), None);
    let params = base.sampling_params.expect("merged sampling params");
    assert_eq!(
        params.get("top_p"),
        Some(&serde_json::json!(0.5)),
        "request params win per key"
    );
    assert_eq!(params.get("top_k"), Some(&serde_json::json!(40)));
}

#[test]
fn thinking_budgets_match_the_documented_defaults() {
    let cases = [
        (ThinkingLevel::Minimal, 1024),
        (ThinkingLevel::Low, 2048),
        (ThinkingLevel::Medium, 8192),
        (ThinkingLevel::High, 16384),
        // xhigh and max collapse to high for budget-based models.
        (ThinkingLevel::Xhigh, 16384),
        (ThinkingLevel::Max, 16384),
    ];
    for (level, expected) in cases {
        let adjusted = adjust_max_tokens_for_thinking(Some(32_000), 64_000, level, None);
        assert_eq!(adjusted.thinking_budget, expected, "{level:?}");
        assert_eq!(adjusted.max_tokens, 32_000 + expected);
    }
}

#[test]
fn thinking_budget_leaves_room_for_the_answer() {
    // Model cap below the budget: the budget shrinks so MIN_ANSWER_TOKENS remain.
    let adjusted = adjust_max_tokens_for_thinking(Some(1_000), 4_000, ThinkingLevel::High, None);
    assert_eq!(adjusted.max_tokens, 4_000);
    assert_eq!(adjusted.thinking_budget, 4_000 - MIN_ANSWER_TOKENS);

    // Without a caller cap the model cap applies.
    let adjusted = adjust_max_tokens_for_thinking(None, 20_000, ThinkingLevel::High, None);
    assert_eq!(adjusted.max_tokens, 20_000);
    assert_eq!(adjusted.thinking_budget, 16_384);
}

#[test]
fn custom_thinking_budgets_override_the_defaults() {
    let budgets = ThinkingBudgets {
        medium: Some(3_000),
        ..Default::default()
    };
    let adjusted =
        adjust_max_tokens_for_thinking(Some(10_000), 64_000, ThinkingLevel::Medium, Some(&budgets));
    assert_eq!(adjusted.thinking_budget, 3_000);
    // Levels without an override keep their default.
    let adjusted =
        adjust_max_tokens_for_thinking(Some(10_000), 64_000, ThinkingLevel::Low, Some(&budgets));
    assert_eq!(adjusted.thinking_budget, 2_048);
}

// ---------------------------------------------------------------------------
// transform-messages
// ---------------------------------------------------------------------------

#[test]
fn downgrades_images_for_text_only_models() {
    let model = model(10_000, 8_000);
    let messages = vec![Message::User(UserMessage {
        content: UserContent::Blocks(vec![
            TextOrImageContent::Text(TextContent::new("look")),
            TextOrImageContent::Image(ImageContent {
                data: "AA".to_string(),
                mime_type: "image/png".to_string(),
            }),
            TextOrImageContent::Image(ImageContent {
                data: "BB".to_string(),
                mime_type: "image/png".to_string(),
            }),
        ]),
        timestamp: 1,
    })];

    let transformed = transform_messages(&messages, &model, None, 0);
    let Message::User(user) = &transformed[0] else {
        panic!("user message")
    };
    let UserContent::Blocks(blocks) = &user.content else {
        panic!("blocks")
    };
    // Consecutive images collapse into a single placeholder.
    assert_eq!(blocks.len(), 2);
    assert_eq!(
        blocks[1],
        TextOrImageContent::Text(TextContent::new(
            "(image omitted: model does not support images)"
        ))
    );
}

#[test]
fn keeps_images_for_vision_models() {
    let mut model = model(10_000, 8_000);
    model.input = vec![Modality::Text, Modality::Image];
    let messages = vec![Message::User(UserMessage {
        content: UserContent::Blocks(vec![TextOrImageContent::Image(ImageContent {
            data: "AA".to_string(),
            mime_type: "image/png".to_string(),
        })]),
        timestamp: 1,
    })];
    assert_eq!(transform_messages(&messages, &model, None, 0), messages);
}

#[test]
fn drops_errored_and_aborted_assistant_turns() {
    let model = model(10_000, 8_000);
    let messages = vec![
        user("hi", 1),
        assistant(
            "test-model",
            "openai",
            vec![AssistantContent::Text(TextContent::new("partial"))],
            StopReason::Error,
        ),
        assistant(
            "test-model",
            "openai",
            vec![AssistantContent::Text(TextContent::new("ok"))],
            StopReason::Stop,
        ),
    ];
    let transformed = transform_messages(&messages, &model, None, 0);
    assert_eq!(transformed.len(), 2, "the errored turn is not replayed");
}

#[test]
fn synthesizes_results_for_orphaned_tool_calls() {
    let model = model(10_000, 8_000);
    let tool_call = ToolCall {
        id: "call_1".to_string(),
        name: "read".to_string(),
        ..Default::default()
    };
    let messages = vec![
        assistant(
            "test-model",
            "openai",
            vec![AssistantContent::ToolCall(tool_call)],
            StopReason::ToolUse,
        ),
        user("next", 2),
    ];

    let transformed = transform_messages(&messages, &model, None, 42);
    assert_eq!(transformed.len(), 3);
    let Message::ToolResult(result) = &transformed[1] else {
        panic!("synthetic tool result")
    };
    assert_eq!(result.tool_call_id, "call_1");
    assert!(result.is_error);
    assert_eq!(
        result.content,
        vec![TextOrImageContent::Text(TextContent::new(
            "No result provided"
        ))]
    );
    assert_eq!(result.timestamp, 42);
}

#[test]
fn normalizes_tool_call_ids_across_models_and_rewrites_results() {
    let model = model(10_000, 8_000);
    let tool_call = ToolCall {
        id: "call|with:specials".to_string(),
        name: "read".to_string(),
        ..Default::default()
    };
    let messages = vec![
        // A different model, so normalization applies.
        assistant(
            "other-model",
            "openai",
            vec![AssistantContent::ToolCall(tool_call)],
            StopReason::ToolUse,
        ),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "call|with:specials".to_string(),
            tool_name: "read".to_string(),
            content: vec![],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 3,
        }),
    ];

    let normalize =
        |id: &str, _source: &notagent_ai::types::AssistantMessage| id.replace(['|', ':'], "_");
    let transformed = transform_messages(&messages, &model, Some(&normalize), 0);
    let Message::Assistant(assistant) = &transformed[0] else {
        panic!("assistant")
    };
    let AssistantContent::ToolCall(tool_call) = &assistant.content[0] else {
        panic!("tool call")
    };
    assert_eq!(tool_call.id, "call_with_specials");
    let Message::ToolResult(result) = &transformed[1] else {
        panic!("tool result")
    };
    assert_eq!(
        result.tool_call_id, "call_with_specials",
        "the result follows the renamed call"
    );
}

#[test]
fn keeps_signed_thinking_for_the_same_model_and_converts_it_otherwise() {
    let model = model(10_000, 8_000);
    let thinking = AssistantContent::Thinking(ThinkingContent {
        thinking: "reasoned".to_string(),
        thinking_signature: Some("sig".to_string()),
        redacted: None,
        extra: Default::default(),
    });

    let same = transform_messages(
        &[assistant(
            "test-model",
            "openai",
            vec![thinking.clone()],
            StopReason::Stop,
        )],
        &model,
        None,
        0,
    );
    let Message::Assistant(assistant_message) = &same[0] else {
        panic!("assistant")
    };
    assert!(matches!(
        assistant_message.content[0],
        AssistantContent::Thinking(_)
    ));

    let cross = transform_messages(
        &[assistant(
            "other-model",
            "openai",
            vec![thinking],
            StopReason::Stop,
        )],
        &model,
        None,
        0,
    );
    let Message::Assistant(assistant_message) = &cross[0] else {
        panic!("assistant")
    };
    assert_eq!(
        assistant_message.content[0],
        AssistantContent::Text(TextContent::new("reasoned"))
    );
}

#[test]
fn drops_redacted_thinking_across_models() {
    let model = model(10_000, 8_000);
    let redacted = AssistantContent::Thinking(ThinkingContent {
        thinking: "[Reasoning redacted]".to_string(),
        thinking_signature: Some("opaque".to_string()),
        redacted: Some(true),
        extra: Default::default(),
    });

    let cross = transform_messages(
        &[assistant(
            "other-model",
            "openai",
            vec![redacted.clone()],
            StopReason::Stop,
        )],
        &model,
        None,
        0,
    );
    let Message::Assistant(assistant_message) = &cross[0] else {
        panic!("assistant")
    };
    assert!(
        assistant_message.content.is_empty(),
        "opaque payloads are model specific"
    );

    let same = transform_messages(
        &[assistant(
            "test-model",
            "openai",
            vec![redacted],
            StopReason::Stop,
        )],
        &model,
        None,
        0,
    );
    let Message::Assistant(assistant_message) = &same[0] else {
        panic!("assistant")
    };
    assert_eq!(assistant_message.content.len(), 1);
}
