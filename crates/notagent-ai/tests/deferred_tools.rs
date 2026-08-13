//! Port of `packages/ai/test/deferred-tools.test.ts` (550 LOC).
//!
//! TS captures the request body through `onPayload` on `streamSimple`; the port calls the
//! request builders those streams call, so the assertions stay on the payload.

use std::collections::BTreeMap;

use notagent_ai::api::anthropic_params::{
    AnthropicOptions, build_params as build_anthropic_params,
};
use notagent_ai::api::openai_codex_responses::{
    OpenAICodexResponsesOptions, build_request_body as build_codex_params,
};
use notagent_ai::api::openai_completions_compat::get_compat as get_completions_compat;
use notagent_ai::api::openai_completions_params::{
    ConvertCompletionsMessagesOptions, convert_messages as convert_completions_messages,
};
use notagent_ai::api::openai_responses::{
    OpenAIResponsesOptions, build_params as build_responses_params,
    get_compat as get_responses_compat,
};
use notagent_ai::model_catalog::get_builtin_model;
use notagent_ai::types::*;
use notagent_ai::utils::estimate::estimate_context_tokens;
use serde_json::{Map, Value, json};

const TIMESTAMP: i64 = 1_700_000_000_000;

fn catalog_model(provider: &str, id: &str) -> Model {
    get_builtin_model(provider, id)
        .unwrap_or_else(|| panic!("{provider}/{id} is missing from the catalog"))
}

fn make_tool(name: &str) -> Tool {
    Tool {
        name: name.to_string(),
        description: format!("The {name} tool"),
        parameters: json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"],
        }),
        constrained_sampling: None,
    }
}

fn user_message(timestamp: i64) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text("Hello".to_string()),
        timestamp,
    })
}

fn assistant_with(
    content: Vec<AssistantContent>,
    api: &str,
    provider: &str,
    model: &str,
) -> Message {
    Message::Assistant(AssistantMessage {
        content,
        api: api.to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 2,
    })
}

fn tool_call(id: &str, name: &str) -> AssistantContent {
    AssistantContent::ToolCall(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: Map::new(),
        ..ToolCall::default()
    })
}

fn make_assistant_tool_call() -> Message {
    assistant_with(
        vec![tool_call("call_1", "base_tool")],
        "anthropic-messages",
        "anthropic",
        "claude-opus-4-6",
    )
}

fn tool_result(
    tool_call_id: &str,
    tool_name: &str,
    content: Vec<TextOrImageContent>,
    added_tool_names: Vec<String>,
) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        content,
        details: None,
        usage: None,
        added_tool_names: Some(added_tool_names),
        is_error: false,
        timestamp: 3,
    })
}

fn text_block(text: &str) -> TextOrImageContent {
    TextOrImageContent::Text(TextContent {
        text: text.to_string(),
        ..TextContent::default()
    })
}

fn make_tool_result(added_tool_names: &[&str]) -> Message {
    tool_result(
        "call_1",
        "base_tool",
        vec![text_block("done")],
        added_tool_names
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
    )
}

fn make_context(tools: Vec<Tool>, added_tool_names: &[&str]) -> Context {
    Context {
        system_prompt: None,
        messages: vec![
            user_message(1),
            make_assistant_tool_call(),
            make_tool_result(added_tool_names),
            user_message(4),
        ],
        tools: Some(tools),
    }
}

fn default_context(tools: Vec<Tool>) -> Context {
    make_context(tools, &["late_tool"])
}

// ---------------------------------------------------------------------------
// Payload builders — one per API the suite exercises
// ---------------------------------------------------------------------------

fn anthropic_payload(model: &Model, context: &Context, api_key: &str) -> Value {
    build_anthropic_params(
        model,
        context,
        notagent_ai::api::anthropic_params::is_oauth_token(api_key),
        &AnthropicOptions {
            thinking_enabled: Some(false),
            ..AnthropicOptions::default()
        },
        TIMESTAMP,
    )
}

fn responses_payload(model: &Model, context: &Context) -> Value {
    build_responses_params(
        model,
        context,
        &OpenAIResponsesOptions::default(),
        &get_responses_compat(model),
        &BTreeMap::new(),
        TIMESTAMP,
    )
    .expect("params")
}

fn codex_payload(model: &Model, context: &Context) -> Value {
    build_codex_params(
        model,
        context,
        &OpenAICodexResponsesOptions::default(),
        None,
        &BTreeMap::new(),
        TIMESTAMP,
    )
    .expect("params")
}

fn anthropic_tools(payload: &Value) -> Vec<&Value> {
    payload["tools"]
        .as_array()
        .map(|tools| tools.iter().collect())
        .unwrap_or_default()
}

fn anthropic_tool_names(payload: &Value) -> Vec<&str> {
    anthropic_tools(payload)
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect()
}

/// The content array of the message that carries a `tool_result` block.
fn anthropic_tool_result_content(payload: &Value) -> &Vec<Value> {
    payload["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find_map(|message| {
            let content = message["content"].as_array()?;
            content
                .iter()
                .any(|block| block["type"] == "tool_result")
                .then_some(content)
        })
        .expect("No tool result in payload")
}

fn anthropic_tool_result(payload: &Value) -> &Value {
    anthropic_tool_result_content(payload)
        .iter()
        .find(|block| block["type"] == "tool_result")
        .expect("No tool result in payload")
}

fn openai_tool_names(payload: &Value) -> Vec<String> {
    payload["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| {
                    tool.get("name")
                        .or_else(|| {
                            tool.get("function")
                                .and_then(|function| function.get("name"))
                        })
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn input_items<'a>(payload: &'a Value, item_type: &str) -> Vec<&'a Value> {
    payload["input"]
        .as_array()
        .map(|input| {
            input
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some(item_type))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

#[test]
fn loads_an_anthropic_tool_at_its_tool_result_marker() {
    let context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    let payload = anthropic_payload(
        &catalog_model("anthropic", "claude-opus-4-6"),
        &context,
        "fake-key",
    );

    let tools = anthropic_tools(&payload);
    assert_eq!(tools[0]["name"], "base_tool");
    assert_eq!(tools[1]["name"], "late_tool");
    assert_eq!(tools[1]["defer_loading"], true);
    assert_eq!(
        anthropic_tool_result(&payload)["content"],
        json!([{ "type": "tool_reference", "tool_name": "late_tool" }])
    );
}

#[test]
fn preserves_tool_output_as_sibling_content_after_emitting_references() {
    let mut context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    context.messages[1] = assistant_with(
        vec![
            tool_call("call_1", "base_tool"),
            tool_call("call_2", "base_tool"),
        ],
        "anthropic-messages",
        "anthropic",
        "claude-opus-4-6",
    );
    context.messages[2] = tool_result(
        "call_1",
        "base_tool",
        vec![
            text_block("work completed"),
            TextOrImageContent::Image(ImageContent {
                mime_type: "image/png".to_string(),
                data: "aW1hZ2U=".to_string(),
            }),
        ],
        vec!["late_tool".to_string()],
    );
    context.messages.insert(
        3,
        tool_result(
            "call_2",
            "base_tool",
            vec![text_block("second result")],
            Vec::new(),
        ),
    );

    let payload = anthropic_payload(
        &catalog_model("anthropic", "claude-opus-4-6"),
        &context,
        "fake-key",
    );
    let content = anthropic_tool_result_content(&payload);

    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "call_1");
    assert_eq!(
        content[0]["content"],
        json!([{ "type": "tool_reference", "tool_name": "late_tool" }])
    );
    assert_eq!(content[1]["type"], "tool_result");
    assert_eq!(content[1]["tool_use_id"], "call_2");
    assert_eq!(content[1]["content"], "second result");
    assert_eq!(content[2]["type"], "text");
    assert_eq!(content[2]["text"], "work completed");
    assert_eq!(content[3]["type"], "image");
    assert_eq!(
        content[3]["source"],
        json!({ "type": "base64", "media_type": "image/png", "data": "aW1hZ2U=" })
    );
}

#[test]
fn loads_a_tool_introduced_by_openai_history_after_switching_to_anthropic() {
    let mut context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    context.messages[1] = assistant_with(
        vec![tool_call("call_1", "base_tool")],
        "openai-responses",
        "openai",
        "gpt-5.4",
    );

    let payload = anthropic_payload(
        &catalog_model("anthropic", "claude-opus-4-8"),
        &context,
        "fake-key",
    );

    let tools = anthropic_tools(&payload);
    assert_eq!(tools[0]["name"], "base_tool");
    assert_eq!(tools[1]["name"], "late_tool");
    assert_eq!(tools[1]["defer_loading"], true);
    assert_eq!(
        anthropic_tool_result(&payload)["content"],
        json!([{ "type": "tool_reference", "tool_name": "late_tool" }])
    );
}

#[test]
fn does_not_resurrect_a_marked_tool_missing_from_context_tools() {
    let context = default_context(vec![make_tool("base_tool")]);
    let payload = anthropic_payload(
        &catalog_model("anthropic", "claude-opus-4-6"),
        &context,
        "fake-key",
    );

    assert_eq!(anthropic_tool_names(&payload), vec!["base_tool"]);
    let content = &anthropic_tool_result(&payload)["content"];
    assert!(
        !content
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_reference")),
        "{content}"
    );
}

#[test]
fn keeps_a_tool_immediate_when_it_was_used_before_its_marker() {
    let mut context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    context.messages[1] = assistant_with(
        vec![tool_call("call_1", "late_tool")],
        "anthropic-messages",
        "anthropic",
        "claude-opus-4-6",
    );

    let payload = anthropic_payload(
        &catalog_model("anthropic", "claude-opus-4-6"),
        &context,
        "fake-key",
    );

    assert_eq!(
        anthropic_tool_names(&payload),
        vec!["base_tool", "late_tool"]
    );
    assert!(
        anthropic_tools(&payload)
            .iter()
            .all(|tool| tool.get("defer_loading").is_none())
    );
}

#[test]
fn normalizes_oauth_names_before_checking_prior_tool_usage() {
    let mut context = make_context(vec![make_tool("base_tool"), make_tool("read")], &["read"]);
    context.messages[1] = assistant_with(
        vec![tool_call("call_1", "Read")],
        "anthropic-messages",
        "anthropic",
        "claude-opus-4-6",
    );

    let payload = anthropic_payload(
        &catalog_model("anthropic", "claude-opus-4-6"),
        &context,
        "sk-ant-oat-fake",
    );

    assert_eq!(anthropic_tool_names(&payload), vec!["base_tool", "Read"]);
    assert!(
        anthropic_tools(&payload)
            .iter()
            .all(|tool| tool.get("defer_loading").is_none())
    );
    let content = &anthropic_tool_result(&payload)["content"];
    assert!(
        !content
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_reference")),
        "{content}"
    );
}

#[test]
fn matches_oauth_canonicalized_markers_to_active_tools() {
    let context = make_context(vec![make_tool("base_tool"), make_tool("read")], &["Read"]);
    let payload = anthropic_payload(
        &catalog_model("anthropic", "claude-opus-4-6"),
        &context,
        "sk-ant-oat-fake",
    );

    let tools = anthropic_tools(&payload);
    assert_eq!(tools[0]["name"], "base_tool");
    assert_eq!(tools[1]["name"], "Read");
    assert_eq!(tools[1]["defer_loading"], true);
    let content = &anthropic_tool_result(&payload)["content"];
    assert!(
        content.as_array().is_some_and(|blocks| blocks
            .iter()
            .any(|block| block["type"] == "tool_reference" && block["tool_name"] == "Read")),
        "{content}"
    );
}

#[test]
fn deduplicates_active_tools_after_oauth_canonicalization() {
    let context = Context {
        system_prompt: None,
        messages: vec![user_message(1)],
        tools: Some(vec![
            make_tool("read"),
            Tool {
                description: "Canonical definition".to_string(),
                ..make_tool("Read")
            },
        ]),
    };
    let payload = anthropic_payload(
        &catalog_model("anthropic", "claude-opus-4-6"),
        &context,
        "sk-ant-oat-fake",
    );

    let tools = anthropic_tools(&payload);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "Read");
    assert_eq!(tools[0]["description"], "Canonical definition");
}

#[test]
fn uses_the_normal_tool_list_when_anthropic_tool_references_are_unsupported() {
    let context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    let models = [
        catalog_model("anthropic", "claude-haiku-4-5"),
        Model {
            id: "claude-sonnet-4-20250514".to_string(),
            ..catalog_model("anthropic", "claude-opus-4-6")
        },
    ];

    for model in models {
        let payload = anthropic_payload(&model, &context, "fake-key");
        assert_eq!(
            anthropic_tool_names(&payload),
            vec!["base_tool", "late_tool"],
            "{}",
            model.id
        );
        assert!(
            anthropic_tools(&payload)
                .iter()
                .all(|tool| tool.get("defer_loading").is_none()),
            "{}",
            model.id
        );
    }
}

#[test]
fn keeps_one_immediate_anthropic_tool_when_every_current_tool_is_marked() {
    let context = default_context(vec![make_tool("late_tool")]);
    let payload = anthropic_payload(
        &catalog_model("anthropic", "claude-opus-4-6"),
        &context,
        "fake-key",
    );

    let tools = anthropic_tools(&payload);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "late_tool");
    assert_eq!(tools[0].get("defer_loading"), None);
    let content = &anthropic_tool_result(&payload)["content"];
    assert!(
        !content
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_reference")),
        "{content}"
    );
}

#[test]
fn supports_explicit_anthropic_compatibility_overrides() {
    let model = Model {
        provider: "anthropic-proxy".to_string(),
        compat: Some(
            ModelCompat::from_api_value(
                "anthropic-messages",
                json!({ "supportsToolReferences": true }),
            )
            .expect("compat"),
        ),
        ..catalog_model("anthropic", "claude-opus-4-6")
    };
    let context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    let payload = anthropic_payload(&model, &context, "fake-key");

    let late = anthropic_tools(&payload)
        .into_iter()
        .find(|tool| tool["name"] == "late_tool")
        .expect("late_tool");
    assert_eq!(late["defer_loading"], true);
}

// ---------------------------------------------------------------------------
// Kimi (openai-completions)
// ---------------------------------------------------------------------------

fn kimi_model(deferred_tools_mode: Option<&str>) -> Model {
    let mut raw = json!({
        "id": "deferred-tools-model",
        "name": "Deferred Tools Model",
        "api": "openai-completions",
        "provider": "moonshotai",
        "baseUrl": "http://127.0.0.1:9/v1",
        "reasoning": false,
        "input": ["text"],
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 128000,
        "maxTokens": 4096,
    });
    if let Some(mode) = deferred_tools_mode {
        raw["compat"] = json!({ "deferredToolsMode": mode });
    }
    serde_json::from_value(raw).expect("model")
}

fn completions_messages(model: &Model, context: &Context) -> Vec<Value> {
    convert_completions_messages(
        model,
        context,
        &get_completions_compat(model),
        &ConvertCompletionsMessagesOptions {
            grammar_tool_input_properties: None,
        },
        TIMESTAMP,
    )
    .expect("messages")
}

#[test]
fn serializes_kimi_deferred_tools_as_system_tool_definitions() {
    let context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    let model = kimi_model(Some("kimi"));
    let messages = completions_messages(&model, &context);

    let tool_result_index = messages
        .iter()
        .position(|message| message["role"] == "tool")
        .expect("a tool message");
    let system_tool_index = messages
        .iter()
        .position(|message| message.get("tools").is_some())
        .expect("a system tools message");
    assert!(system_tool_index > tool_result_index);
    assert_eq!(
        messages[system_tool_index]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["function"]["name"].as_str().expect("name"))
            .collect::<Vec<_>>(),
        vec!["late_tool"]
    );
}

#[test]
fn emits_kimi_deferred_schemas_after_all_tool_results_in_a_batch() {
    let mut context = default_context(vec![
        make_tool("base_tool"),
        make_tool("late_tool"),
        make_tool("later_tool"),
    ]);
    context.messages.insert(
        3,
        tool_result(
            "call_2",
            "base_tool",
            vec![text_block("done")],
            vec!["later_tool".to_string()],
        ),
    );

    let messages = completions_messages(&kimi_model(Some("kimi")), &context);

    assert_eq!(
        messages
            .iter()
            .map(|message| message["role"].as_str().expect("role"))
            .collect::<Vec<_>>(),
        vec!["user", "assistant", "tool", "tool", "system", "user"]
    );
    assert_eq!(
        messages[4]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["function"]["name"].as_str().expect("name"))
            .collect::<Vec<_>>(),
        vec!["late_tool", "later_tool"]
    );
}

#[test]
fn leaves_openai_completions_tools_unchanged_without_kimi_mode() {
    let context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    let model = kimi_model(None);
    let messages = completions_messages(&model, &context);
    let active: Vec<&Tool> = context.tools.iter().flatten().collect();
    let tools = notagent_ai::api::openai_completions_params::convert_tools(
        &active,
        &get_completions_compat(&model),
    )
    .expect("tools");

    assert_eq!(
        tools
            .iter()
            .map(|tool| tool["function"]["name"].as_str().expect("name"))
            .collect::<Vec<_>>(),
        vec!["base_tool", "late_tool"]
    );
    assert!(
        messages
            .iter()
            .all(|message| message.get("tools").is_none())
    );
}

// ---------------------------------------------------------------------------
// OpenAI Responses
// ---------------------------------------------------------------------------

#[test]
fn loads_an_openai_responses_tool_through_additional_tools() {
    let context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    let payload = responses_payload(&catalog_model("openai", "gpt-5.4"), &context);
    let additional = input_items(&payload, "additional_tools");

    assert_eq!(openai_tool_names(&payload), vec!["base_tool"]);
    assert_eq!(additional.len(), 1);
    assert_eq!(additional[0]["role"], "developer");
    let tools = additional[0]["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["name"], "late_tool");
    assert!(tools.iter().all(|tool| tool.get("defer_loading").is_none()));
    assert!(input_items(&payload, "tool_search_call").is_empty());
    assert!(input_items(&payload, "tool_search_output").is_empty());
}

#[test]
fn preserves_an_additional_tools_marker_after_the_loaded_tool_is_used() {
    let mut context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    context.messages.insert(
        3,
        assistant_with(
            vec![tool_call("call_late|fc_late", "late_tool")],
            "openai-responses",
            "openai",
            "gpt-5.4",
        ),
    );
    context.messages.insert(
        4,
        tool_result(
            "call_late|fc_late",
            "late_tool",
            vec![text_block("done")],
            vec!["late_tool".to_string()],
        ),
    );

    let payload = responses_payload(&catalog_model("openai", "gpt-5.4"), &context);
    let input = payload["input"].as_array().expect("input");
    let additional_indexes: Vec<usize> = input
        .iter()
        .enumerate()
        .filter(|(_, item)| item.get("type").and_then(Value::as_str) == Some("additional_tools"))
        .map(|(index, _)| index)
        .collect();
    let late_call_index = input
        .iter()
        .position(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("name").and_then(Value::as_str) == Some("late_tool")
        })
        .expect("the late function call");

    assert_eq!(additional_indexes.len(), 1);
    assert!(additional_indexes[0] < late_call_index);
    assert_eq!(openai_tool_names(&payload), vec!["base_tool"]);
}

#[test]
fn falls_back_to_client_tool_search_when_additional_tools_is_unsupported() {
    let model = Model {
        provider: "openai-proxy".to_string(),
        compat: Some(
            ModelCompat::from_api_value(
                "openai-responses",
                json!({ "supportsAdditionalTools": false, "supportsToolSearch": true }),
            )
            .expect("compat"),
        ),
        ..catalog_model("openai", "gpt-5.4")
    };
    let context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    let payload = responses_payload(&model, &context);
    let search_call = input_items(&payload, "tool_search_call");
    let search_output = input_items(&payload, "tool_search_output");

    assert_eq!(openai_tool_names(&payload), vec!["base_tool"]);
    assert_eq!(search_call.len(), 1);
    assert_eq!(search_call[0]["execution"], "client");
    assert_eq!(search_call[0]["status"], "completed");
    assert_eq!(search_output.len(), 1);
    assert_eq!(search_output[0]["call_id"], search_call[0]["call_id"]);
    let tools = search_output[0]["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["name"], "late_tool");
    assert_eq!(tools[0]["defer_loading"], true);
    assert!(input_items(&payload, "additional_tools").is_empty());
}

#[test]
fn uses_the_normal_tool_list_for_unsupported_openai_models() {
    for model_id in ["gpt-5.2", "gpt-5.4-nano", "gpt-5.5-pro"] {
        let context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
        let payload = responses_payload(&catalog_model("openai", model_id), &context);

        assert_eq!(
            openai_tool_names(&payload),
            vec!["base_tool", "late_tool"],
            "{model_id}"
        );
        assert!(
            input_items(&payload, "tool_search_output").is_empty(),
            "{model_id}"
        );
    }
}

#[test]
fn uses_the_normal_tool_list_when_openai_tool_search_is_explicitly_disabled() {
    let model = Model {
        provider: "openai-proxy".to_string(),
        compat: Some(
            ModelCompat::from_api_value("openai-responses", json!({ "supportsToolSearch": false }))
                .expect("compat"),
        ),
        ..catalog_model("openai", "gpt-5.4")
    };
    let context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    let payload = responses_payload(&model, &context);

    assert_eq!(openai_tool_names(&payload), vec!["base_tool", "late_tool"]);
    assert!(input_items(&payload, "tool_search_output").is_empty());
}

#[test]
fn selects_additional_tools_tool_search_or_top_level_tools_for_codex_models() {
    let context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    let additional = codex_payload(&catalog_model("openai-codex", "gpt-5.6-sol"), &context);
    let tool_search = codex_payload(&catalog_model("openai-codex", "gpt-5.4"), &context);
    let top_level = codex_payload(
        &catalog_model("openai-codex", "gpt-5.3-codex-spark"),
        &context,
    );

    assert_eq!(openai_tool_names(&additional), vec!["base_tool"]);
    assert!(!input_items(&additional, "additional_tools").is_empty());
    assert!(input_items(&additional, "tool_search_output").is_empty());
    assert_eq!(openai_tool_names(&tool_search), vec!["base_tool"]);
    assert!(!input_items(&tool_search, "tool_search_output").is_empty());
    assert_eq!(
        openai_tool_names(&top_level),
        vec!["base_tool", "late_tool"]
    );
    assert!(input_items(&top_level, "additional_tools").is_empty());
    assert!(input_items(&top_level, "tool_search_output").is_empty());
}

#[test]
fn leaves_providers_without_deferred_loading_unchanged() {
    let context = default_context(vec![make_tool("base_tool"), make_tool("late_tool")]);
    let model = catalog_model("groq", "llama-3.3-70b-versatile");
    let active: Vec<&Tool> = context.tools.iter().flatten().collect();
    let tools = notagent_ai::api::openai_completions_params::convert_tools(
        &active,
        &get_completions_compat(&model),
    )
    .expect("tools");

    assert_eq!(
        tools
            .iter()
            .map(|tool| tool["function"]["name"].as_str().expect("name"))
            .collect::<Vec<_>>(),
        vec!["base_tool", "late_tool"]
    );
}

// ---------------------------------------------------------------------------
// Context estimate
// ---------------------------------------------------------------------------

#[test]
fn counts_definitions_marked_after_the_latest_usage_checkpoint() {
    let assistant = Message::Assistant(AssistantMessage {
        content: vec![AssistantContent::Text(TextContent {
            text: "done".to_string(),
            ..TextContent::default()
        })],
        usage: Usage {
            input: 50,
            output: 50,
            total_tokens: Some(100),
            ..Usage::default()
        },
        stop_reason: StopReason::Stop,
        ..match make_assistant_tool_call() {
            Message::Assistant(message) => message,
            _ => unreachable!(),
        }
    });

    let plain = estimate_context_tokens(&Context {
        messages: vec![assistant.clone(), user_message(4)],
        tools: Some(Vec::new()),
        ..Context::default()
    });
    let late_tool = Tool {
        description: "x".repeat(4000),
        ..make_tool("late_tool")
    };
    let marked = estimate_context_tokens(&Context {
        messages: vec![assistant, make_tool_result(&["late_tool"])],
        tools: Some(vec![late_tool]),
        ..Context::default()
    });

    assert!(
        marked.tokens > plain.tokens + 500,
        "{} vs {}",
        marked.tokens,
        plain.tokens
    );
    assert!(
        marked.trailing_tokens > plain.trailing_tokens + 500,
        "{} vs {}",
        marked.trailing_tokens,
        plain.trailing_tokens
    );
}
