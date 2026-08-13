//! Port of `packages/ai/test/openai-responses-namespace.test.ts` (224 LOC) and
//! `packages/ai/test/openai-responses-partial-json-cleanup.test.ts` (106 LOC).
//!
//! Both drive `processResponsesStream` over a scripted event sequence and then replay the
//! resulting message through `convertResponsesMessages`.

use std::collections::{BTreeMap, BTreeSet};

use notagent_ai::api::openai_responses_shared::{
    ConvertResponsesMessagesOptions, ResponsesStreamOptions, ResponsesStreamState,
    convert_responses_messages,
};
use notagent_ai::types::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, Context, Message, Modality, Model,
    ModelCost, StopReason, ToolCall, Usage,
};
use serde_json::{Value, json};

const TIMESTAMP: i64 = 1_700_000_000_000;

fn model() -> Model {
    Model {
        id: "gpt-5.4".to_string(),
        name: "GPT-5.4".to_string(),
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 400_000,
        max_tokens: 128_000,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn create_output(model: &Model) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Pending,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: TIMESTAMP,
    }
}

/// Runs the events through the state machine and returns the final message plus every
/// event the TS implementation would have pushed onto the stream.
fn run(
    model: &Model,
    events: Vec<Value>,
    grammar_tool_input_properties: Option<&BTreeMap<String, String>>,
) -> (AssistantMessage, Vec<AssistantMessageEvent>) {
    let mut state = ResponsesStreamState::new(create_output(model), model);
    let options = ResponsesStreamOptions {
        grammar_tool_input_properties,
        ..ResponsesStreamOptions::default()
    };
    let mut emitted = Vec::new();
    for event in &events {
        emitted.extend(state.process_event(event, &options).expect("process"));
    }
    (state.output, emitted)
}

fn function_call_events(arguments_json: &str, namespace: Option<&str>) -> Vec<Value> {
    let mut done_item = json!({
        "type": "function_call",
        "id": "fc_test",
        "call_id": "call_test",
        "name": "lookup",
        "arguments": arguments_json,
    });
    if let Some(namespace) = namespace {
        done_item["namespace"] = json!(namespace);
    }
    vec![
        json!({
            "type": "response.output_item.added",
            "sequence_number": 0,
            "output_index": 0,
            "item": {
                "type": "function_call",
                "id": "fc_test",
                "call_id": "call_test",
                "name": "lookup",
                "arguments": "",
            },
        }),
        json!({
            "type": "response.output_item.done",
            "sequence_number": 1,
            "output_index": 0,
            "item": done_item,
        }),
        json!({
            "type": "response.completed",
            "sequence_number": 2,
            "response": { "id": "resp_test", "status": "completed" },
        }),
    ]
}

fn custom_tool_call_events() -> Vec<Value> {
    vec![
        json!({
            "type": "response.output_item.added",
            "sequence_number": 0,
            "output_index": 0,
            "item": {
                "type": "custom_tool_call",
                "id": "ctc_test",
                "call_id": "call_test",
                "name": "query",
                "input": "",
            },
        }),
        json!({
            "type": "response.output_item.done",
            "sequence_number": 1,
            "output_index": 0,
            "item": {
                "type": "custom_tool_call",
                "id": "ctc_test",
                "call_id": "call_test",
                "name": "query",
                "input": "hello",
                "namespace": "dynamic_tools",
            },
        }),
        json!({
            "type": "response.completed",
            "sequence_number": 2,
            "response": { "id": "resp_test", "status": "completed" },
        }),
    ]
}

fn tool_call_of(output: &AssistantMessage) -> &ToolCall {
    match output.content.first() {
        Some(AssistantContent::ToolCall(block)) => block,
        _ => panic!("Expected toolCall block"),
    }
}

fn openai_providers() -> BTreeSet<String> {
    ["openai".to_string()].into_iter().collect()
}

fn replay(
    model: &Model,
    output: &AssistantMessage,
    grammar: Option<&BTreeMap<String, String>>,
) -> Vec<Value> {
    convert_responses_messages(
        model,
        &Context {
            messages: vec![Message::Assistant(output.clone())],
            ..Context::default()
        },
        &openai_providers(),
        &ConvertResponsesMessagesOptions {
            grammar_tool_input_properties: grammar,
            ..ConvertResponsesMessagesOptions::default()
        },
        TIMESTAMP,
    )
    .expect("convert")
}

fn find_item<'a>(items: &'a [Value], item_type: &str) -> Option<&'a Value> {
    items
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some(item_type))
}

#[test]
fn round_trips_a_function_namespace_received_only_on_output_item_done() {
    let model = model();
    let (output, _) = run(
        &model,
        function_call_events("{\"value\":\"hello\"}", Some("dynamic_tools")),
        None,
    );

    let tool_call = tool_call_of(&output);
    assert_eq!(tool_call.id, "call_test|fc_test");
    assert_eq!(tool_call.name, "lookup");
    assert_eq!(
        Value::Object(tool_call.arguments.clone()),
        json!({ "value": "hello" })
    );
    assert_eq!(tool_call.namespace.as_deref(), Some("dynamic_tools"));

    let replayed = replay(&model, &output, None);
    let function_call = find_item(&replayed, "function_call").expect("function_call");
    assert_eq!(function_call["id"], "fc_test");
    assert_eq!(function_call["call_id"], "call_test");
    assert_eq!(function_call["name"], "lookup");
    assert_eq!(function_call["arguments"], "{\"value\":\"hello\"}");
    assert_eq!(function_call["namespace"], "dynamic_tools");
}

#[test]
fn round_trips_a_custom_tool_namespace_received_only_on_output_item_done() {
    let model = model();
    let grammar: BTreeMap<String, String> = [("query".to_string(), "input".to_string())]
        .into_iter()
        .collect();
    let (output, _) = run(&model, custom_tool_call_events(), Some(&grammar));

    let tool_call = tool_call_of(&output);
    assert_eq!(tool_call.id, "call_test|ctc_test");
    assert_eq!(tool_call.name, "query");
    assert_eq!(
        Value::Object(tool_call.arguments.clone()),
        json!({ "input": "hello" })
    );
    assert_eq!(tool_call.namespace.as_deref(), Some("dynamic_tools"));

    let replayed = replay(&model, &output, Some(&grammar));
    let custom = find_item(&replayed, "custom_tool_call").expect("custom_tool_call");
    assert_eq!(custom["id"], "ctc_test");
    assert_eq!(custom["call_id"], "call_test");
    assert_eq!(custom["name"], "query");
    assert_eq!(custom["input"], "hello");
    assert_eq!(custom["namespace"], "dynamic_tools");
}

#[test]
fn drops_namespaces_when_the_target_cannot_replay_their_load_items() {
    let model = model();
    let grammar: BTreeMap<String, String> = [("query".to_string(), "input".to_string())]
        .into_iter()
        .collect();
    let mut output = create_output(&model);
    output.content.push(AssistantContent::ToolCall(ToolCall {
        id: "call_function|fc_test".to_string(),
        name: "lookup".to_string(),
        arguments: json!({ "value": "hello" })
            .as_object()
            .cloned()
            .expect("object"),
        namespace: Some("dynamic_tools".to_string()),
        ..ToolCall::default()
    }));
    output.content.push(AssistantContent::ToolCall(ToolCall {
        id: "call_custom|ctc_test".to_string(),
        name: "query".to_string(),
        arguments: json!({ "input": "hello" })
            .as_object()
            .cloned()
            .expect("object"),
        namespace: Some("dynamic_tools".to_string()),
        ..ToolCall::default()
    }));

    let target_models = [
        Model {
            id: "gpt-5.2".to_string(),
            name: "GPT-5.2".to_string(),
            ..model.clone()
        },
        Model {
            provider: "azure-openai-responses".to_string(),
            ..model.clone()
        },
        Model {
            api: "openai-codex-responses".to_string(),
            provider: "openai-codex".to_string(),
            id: "gpt-5.3-codex-spark".to_string(),
            name: "GPT-5.3 Codex Spark".to_string(),
            ..model.clone()
        },
    ];

    for target_model in target_models {
        let replayed = replay(&target_model, &output, Some(&grammar));
        let function_call = find_item(&replayed, "function_call").expect("function_call");
        assert_eq!(function_call.get("namespace"), None, "{}", target_model.id);
        let custom = find_item(&replayed, "custom_tool_call").expect("custom_tool_call");
        assert_eq!(custom.get("namespace"), None, "{}", target_model.id);
    }
}

#[test]
fn does_not_add_a_namespace_to_ordinary_function_calls() {
    let model = model();
    let mut output = create_output(&model);
    output.content.push(AssistantContent::ToolCall(ToolCall {
        id: "call_test|fc_test".to_string(),
        name: "lookup".to_string(),
        arguments: json!({ "value": "hello" })
            .as_object()
            .cloned()
            .expect("object"),
        ..ToolCall::default()
    }));

    let replayed = replay(&model, &output, None);
    let function_call = find_item(&replayed, "function_call").expect("function_call");
    assert_eq!(function_call.get("namespace"), None);
}

// ---------------------------------------------------------------------------
// openai-responses-partial-json-cleanup.test.ts
// ---------------------------------------------------------------------------

#[test]
fn removes_partial_json_from_persisted_tool_call_blocks_at_output_item_done() {
    let model = Model {
        id: "gpt-5-mini".to_string(),
        name: "GPT-5 Mini".to_string(),
        ..model()
    };
    let arguments_json = "{\"path\":\"README.md\",\"content\":\"updated\"}";
    let mut events = vec![
        json!({
            "type": "response.output_item.added",
            "item": {
                "type": "function_call",
                "id": "fc_test",
                "call_id": "call_test",
                "name": "edit",
                "arguments": "",
            },
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "delta": "{\"path\":\"README.md\"",
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "delta": ",\"content\":\"updated\"}",
        }),
        json!({
            "type": "response.function_call_arguments.done",
            "arguments": arguments_json,
        }),
    ];
    events.push(json!({
        "type": "response.output_item.done",
        "item": {
            "type": "function_call",
            "id": "fc_test",
            "call_id": "call_test",
            "name": "edit",
            "arguments": arguments_json,
        },
    }));
    events.push(json!({
        "type": "response.completed",
        "sequence_number": 5,
        "response": { "id": "resp_test", "status": "completed" },
    }));

    let (output, emitted) = run(&model, events, None);

    assert_eq!(output.content.len(), 1);
    let persisted = tool_call_of(&output);
    assert_eq!(
        Value::Object(persisted.arguments.clone()),
        json!({ "path": "README.md", "content": "updated" })
    );
    assert_eq!(persisted.extra.get("partialJson"), None);

    let tool_call_end = emitted
        .iter()
        .find_map(|event| match event {
            AssistantMessageEvent::ToolcallEnd { tool_call, .. } => Some(tool_call),
            _ => None,
        })
        .expect("toolcall_end");
    assert_eq!(tool_call_end, persisted);
    assert_eq!(tool_call_end.extra.get("partialJson"), None);
}
