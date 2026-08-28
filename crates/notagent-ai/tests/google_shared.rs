use notagent_ai::api::google_shared::{
    convert_messages, is_thinking_part, retain_thought_signature,
};
use notagent_ai::types::*;
use serde_json::{Value, json};

const VALID_SIG: &str = "AAAAAAAAAAAAAAAAAAAAAA==";

fn model(api: &str, provider: &str, id: &str) -> Model {
    Model {
        id: id.to_string(),
        name: id.to_string(),
        api: api.to_string(),
        provider: provider.to_string(),
        base_url: "https://example.com".to_string(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 8192,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn assistant(source: &Model, content: Vec<AssistantContent>, stop_reason: StopReason) -> Message {
    Message::Assistant(AssistantMessage {
        content,
        api: source.api.clone(),
        provider: source.provider.clone(),
        model: source.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 1,
    })
}

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp: 1,
    })
}

fn tool_call(
    id: &str,
    name: &str,
    arguments: Value,
    thought_signature: Option<&str>,
) -> AssistantContent {
    AssistantContent::ToolCall(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.as_object().cloned().unwrap_or_default(),
        thought_signature: thought_signature.map(str::to_string),
        namespace: None,
        extra: Default::default(),
    })
}

fn tool_result(tool_call_id: &str, content: Vec<TextOrImageContent>) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: tool_call_id.to_string(),
        tool_name: "read".to_string(),
        content,
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 1,
    })
}

fn parts(content: &Value) -> Vec<Value> {
    content["parts"].as_array().cloned().unwrap_or_default()
}

fn model_turn(contents: &[Value]) -> Value {
    contents
        .iter()
        .find(|content| content["role"] == json!("model"))
        .cloned()
        .expect("a model turn")
}

// ---------------------------------------------------------------------------
// Gemini 3 unsigned tool calls
// ---------------------------------------------------------------------------

fn unsigned_context(source: &Model, thought_signature: Option<&str>) -> Context {
    Context {
        messages: vec![
            user("Hi"),
            assistant(
                source,
                vec![
                    tool_call(
                        "call_1",
                        "bash",
                        json!({ "command": "echo hi" }),
                        thought_signature,
                    ),
                    tool_call("call_2", "bash", json!({ "command": "ls -la" }), None),
                ],
                StopReason::ToolUse,
            ),
            tool_result(
                "call_1",
                vec![TextOrImageContent::Text(TextContent {
                    text: "hi".to_string(),
                    text_signature: None,
                    extra: Default::default(),
                })],
            ),
            tool_result(
                "call_2",
                vec![TextOrImageContent::Text(TextContent {
                    text: "files".to_string(),
                    text_signature: None,
                    extra: Default::default(),
                })],
            ),
        ],
        ..Default::default()
    }
}

#[test]
fn gemini3_history_keeps_the_tool_call_ids() {
    for target in [
        model("google-generative-ai", "google", "gemini-3-pro-preview"),
        model("google-generative-ai", "google", "gemini-3.6-flash"),
        model("google-vertex", "google-vertex", "gemini-3-pro-preview"),
    ] {
        let contents = convert_messages(&target, &unsigned_context(&target, None), 1);
        let call_ids: Vec<String> = contents
            .iter()
            .flat_map(parts)
            .filter_map(|part| part["functionCall"]["id"].as_str().map(str::to_string))
            .collect();
        let response_ids: Vec<String> = contents
            .iter()
            .flat_map(parts)
            .filter_map(|part| part["functionResponse"]["id"].as_str().map(str::to_string))
            .collect();
        assert_eq!(call_ids, vec!["call_1", "call_2"], "{}", target.id);
        assert_eq!(response_ids, vec!["call_1", "call_2"], "{}", target.id);
    }
}

#[test]
fn unsigned_tool_calls_get_no_signature_and_no_validator_marker() {
    // The assistant message comes from another model, so its signature is unusable.
    let target = model("google-generative-ai", "google", "gemini-3-pro-preview");
    let mut source = target.clone();
    source.id = "other-model".to_string();
    let contents = convert_messages(&target, &unsigned_context(&source, None), 1);
    let turn = model_turn(&contents);
    let calls: Vec<Value> = parts(&turn)
        .into_iter()
        .filter(|part| part.get("functionCall").is_some())
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].get("thoughtSignature"), None);
    assert_eq!(calls[1].get("thoughtSignature"), None);
    assert!(
        !turn
            .to_string()
            .contains("skip_thought_signature_validator")
    );
    assert!(!parts(&turn).iter().any(|part| {
        part["text"]
            .as_str()
            .is_some_and(|text| text.contains("Historical context"))
    }));

    let target = model("google-vertex", "google-vertex", "gemini-3-pro-preview");
    let contents = convert_messages(&target, &unsigned_context(&target, None), 1);
    let turn = model_turn(&contents);
    let calls: Vec<Value> = parts(&turn)
        .into_iter()
        .filter(|part| part.get("functionCall").is_some())
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].get("thoughtSignature"), None);
    assert_eq!(calls[1].get("thoughtSignature"), None);
    assert!(
        !turn
            .to_string()
            .contains("skip_thought_signature_validator")
    );
}

#[test]
fn a_valid_signature_survives_for_the_same_provider_and_model() {
    let target = model("google-generative-ai", "google", "gemini-3-pro-preview");
    let contents = convert_messages(&target, &unsigned_context(&target, Some(VALID_SIG)), 1);
    let turn = model_turn(&contents);
    let calls: Vec<Value> = parts(&turn)
        .into_iter()
        .filter(|part| part.get("functionCall").is_some())
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["thoughtSignature"], json!(VALID_SIG));
    assert_eq!(calls[1].get("thoughtSignature"), None);
}

// ---------------------------------------------------------------------------
// Signed empty blocks
// ---------------------------------------------------------------------------

fn signed_context(source: &Model, content: Vec<AssistantContent>) -> Context {
    Context {
        messages: vec![user("Hi"), assistant(source, content, StopReason::ToolUse)],
        ..Default::default()
    }
}

fn signed_parts(turn: &Value) -> Vec<Value> {
    parts(turn)
        .into_iter()
        .filter(|part| part["thoughtSignature"] == json!(VALID_SIG))
        .collect()
}

#[test]
fn signed_empty_blocks_are_echoed_back() {
    let target = model("google-generative-ai", "google", "gemini-3-pro-preview");

    let contents = convert_messages(
        &target,
        &signed_context(
            &target,
            vec![
                AssistantContent::Thinking(ThinkingContent {
                    thinking: String::new(),
                    thinking_signature: Some(VALID_SIG.to_string()),
                    redacted: None,
                    extra: Default::default(),
                }),
                tool_call("call_1", "bash", json!({ "command": "ls" }), None),
            ],
        ),
        1,
    );
    let turn = model_turn(&contents);
    let signed = signed_parts(&turn);
    assert_eq!(signed.len(), 1);
    assert_eq!(signed[0]["thought"], json!(true));

    let text = TextContent {
        text: String::new(),
        text_signature: Some(VALID_SIG.to_string()),
        extra: Default::default(),
    };
    let contents = convert_messages(
        &target,
        &signed_context(
            &target,
            vec![
                AssistantContent::Text(text),
                tool_call("call_1", "bash", json!({ "command": "ls" }), None),
            ],
        ),
        1,
    );
    assert_eq!(signed_parts(&model_turn(&contents)).len(), 1);
}

#[test]
fn unsigned_or_foreign_empty_blocks_are_dropped() {
    let target = model("google-generative-ai", "google", "gemini-3-pro-preview");

    // Unsigned empty blocks disappear.
    let contents = convert_messages(
        &target,
        &signed_context(
            &target,
            vec![
                AssistantContent::Thinking(ThinkingContent {
                    thinking: String::new(),
                    thinking_signature: None,
                    redacted: None,
                    extra: Default::default(),
                }),
                AssistantContent::Text(TextContent {
                    text: String::new(),
                    text_signature: None,
                    extra: Default::default(),
                }),
                tool_call("call_1", "bash", json!({ "command": "ls" }), None),
            ],
        ),
        1,
    );
    let turn = model_turn(&contents);
    assert_eq!(parts(&turn).len(), 1);
    assert!(parts(&turn)[0].get("functionCall").is_some());

    // A signature from another model is unusable and goes with the block.
    let mut source = target.clone();
    source.id = "other-model".to_string();
    let text = TextContent {
        text: String::new(),
        text_signature: Some(VALID_SIG.to_string()),
        extra: Default::default(),
    };
    let contents = convert_messages(
        &target,
        &signed_context(
            &source,
            vec![
                AssistantContent::Thinking(ThinkingContent {
                    thinking: String::new(),
                    thinking_signature: Some(VALID_SIG.to_string()),
                    redacted: None,
                    extra: Default::default(),
                }),
                AssistantContent::Text(text),
                tool_call("call_1", "bash", json!({ "command": "ls" }), None),
            ],
        ),
        1,
    );
    let turn = model_turn(&contents);
    assert_eq!(parts(&turn).len(), 1);
    assert!(parts(&turn)[0].get("functionCall").is_some());
    assert!(!turn.to_string().contains(VALID_SIG));
}

// ---------------------------------------------------------------------------
// Image tool results
// ---------------------------------------------------------------------------

/// The image-routing suite uses a model that accepts image input.
fn image_model(id: &str) -> Model {
    Model {
        input: vec![Modality::Text, Modality::Image],
        ..model("google-generative-ai", "google", id)
    }
}

fn image_context(source: &Model) -> Context {
    Context {
        messages: vec![
            user("Hi"),
            assistant(
                source,
                vec![
                    tool_call("call_a", "read", json!({}), None),
                    tool_call("call_img", "read", json!({}), None),
                    tool_call("call_b", "read", json!({}), None),
                ],
                StopReason::ToolUse,
            ),
            tool_result(
                "call_a",
                vec![TextOrImageContent::Text(TextContent {
                    text: "alpha text".to_string(),
                    text_signature: None,
                    extra: Default::default(),
                })],
            ),
            tool_result(
                "call_img",
                vec![TextOrImageContent::Image(ImageContent {
                    data: "abc".to_string(),
                    mime_type: "image/png".to_string(),
                })],
            ),
            tool_result(
                "call_b",
                vec![TextOrImageContent::Text(TextContent {
                    text: "beta text".to_string(),
                    text_signature: None,
                    extra: Default::default(),
                })],
            ),
        ],
        ..Default::default()
    }
}

#[test]
fn gemini2_keeps_a_separate_synthetic_image_turn() {
    let target = image_model("gemini-2.5-flash");
    let contents = convert_messages(&target, &image_context(&target), 1);

    assert_eq!(contents.len(), 5);
    assert!(
        parts(&contents[2])
            .iter()
            .all(|part| part.get("functionResponse").is_some())
    );
    assert_eq!(parts(&contents[3])[0]["text"], json!("Tool result image:"));
    assert!(parts(&contents[3])[1].get("inlineData").is_some());
    assert!(parts(&contents[4])[0].get("functionResponse").is_some());
}

#[test]
fn gemini3_nests_image_tool_results() {
    let target = image_model("gemini-3-pro-preview");
    let contents = convert_messages(&target, &image_context(&target), 1);

    assert_eq!(contents.len(), 3);
    let tool_turn = &contents[2];
    assert_eq!(parts(tool_turn).len(), 3);
    let image_response = &parts(tool_turn)[1]["functionResponse"];
    assert_eq!(image_response["parts"].as_array().expect("parts").len(), 1);
    assert!(image_response["parts"][0].get("inlineData").is_some());
}

// ---------------------------------------------------------------------------
// Thought signatures
// ---------------------------------------------------------------------------

#[test]
fn only_thought_true_marks_a_thinking_part() {
    assert!(is_thinking_part(&json!({ "thought": true })));
    assert!(is_thinking_part(
        &json!({ "thought": true, "thoughtSignature": "opaque-signature" })
    ));
    assert!(!is_thinking_part(
        &json!({ "thoughtSignature": "opaque-signature" })
    ));
    assert!(!is_thinking_part(
        &json!({ "thought": false, "thoughtSignature": "opaque-signature" })
    ));
    assert!(!is_thinking_part(&json!({})));
    assert!(!is_thinking_part(
        &json!({ "thought": false, "thoughtSignature": "" })
    ));
}

#[test]
fn a_signature_survives_deltas_that_omit_it() {
    let first = retain_thought_signature(None, Some("sig-1"));
    assert_eq!(first.as_deref(), Some("sig-1"));
    let second = retain_thought_signature(first, None);
    assert_eq!(second.as_deref(), Some("sig-1"));
    let third = retain_thought_signature(second, Some(""));
    assert_eq!(third.as_deref(), Some("sig-1"));
    assert_eq!(
        retain_thought_signature(Some("sig-1".to_string()), Some("sig-2")).as_deref(),
        Some("sig-2")
    );
}

// ---------------------------------------------------------------------------
// Raw finish reasons
//
// `GoogleStreamState`, so one state machine covers the Generative AI and the Vertex case.
// ---------------------------------------------------------------------------

#[test]
fn raw_gemini_finish_reasons_survive_into_the_error() {
    use notagent_ai::api::google_generative_ai::GoogleStreamState;

    for (api, provider, model_id, finish_reason) in [
        (
            "google-generative-ai",
            "google",
            "gemini-2.5-flash",
            "MALFORMED_FUNCTION_CALL",
        ),
        (
            "google-vertex",
            "google-vertex",
            "gemini-3-flash-preview",
            "SAFETY",
        ),
    ] {
        let target = model(api, provider, model_id);
        let mut state = GoogleStreamState::new_with_api(&target, api, 1);
        state.process_chunk(
            &json!({ "candidates": [{ "finishReason": finish_reason, "content": { "parts": [] } }] }),
            1,
        );
        let (_, result) = state.finish();

        assert_eq!(
            state.output.stop_reason,
            StopReason::Error,
            "{finish_reason}"
        );
        assert_eq!(
            state.output.raw_stop_reason.as_deref(),
            Some(finish_reason),
            "{finish_reason}"
        );
        assert_eq!(
            result.expect_err("an error").to_string(),
            format!("Provider stopped with: {finish_reason}")
        );
    }
}
