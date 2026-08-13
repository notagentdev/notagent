//! Differential test of the openai-completions stream state machine.
//!
//! `fixtures/openai-completions-stream.jsonl` holds the event sequence the TS
//! implementation emits for each scripted SSE body (see `fixtures/generators`). The
//! `partial` snapshots are stripped from both sides — they are the message under
//! construction and are compared through the final `done`/`error` message instead.

use std::collections::BTreeMap;
use std::sync::Arc;

use notagent_ai::api::constrained_sampling::create_grammar_tool_input_properties;
use notagent_ai::api::openai_completions::{OpenAICompletionsStreamState, stream};
use notagent_ai::api::openai_completions_compat::get_compat;
use notagent_ai::api::openai_completions_params::OpenAICompletionsOptions;
use notagent_ai::types::{AssistantMessageEvent, Context, Model, ProviderRequestOptions};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::{Map, Value};

/// A fetch that replies with one canned response.
struct CannedFetch {
    status: u16,
    body: String,
    /// Chunk boundaries: the SSE decoder has to survive splits inside an event.
    chunk_size: Option<usize>,
}

impl FetchFn for CannedFetch {
    fn fetch(
        &self,
        _request: FetchRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FetchError>> + Send>,
    > {
        let status = self.status;
        let body = self.body.clone();
        let chunk_size = self.chunk_size;
        Box::pin(async move {
            let content_type = if status == 200 {
                "text/event-stream"
            } else {
                "application/json"
            };
            let body = match chunk_size {
                None => FetchBody::Bytes(body.into_bytes()),
                Some(size) => {
                    let (sender, receiver) = tokio::sync::mpsc::channel(64);
                    let bytes = body.into_bytes();
                    tokio::spawn(async move {
                        for piece in bytes.chunks(size) {
                            if sender.send(Ok(piece.to_vec())).await.is_err() {
                                return;
                            }
                        }
                    });
                    FetchBody::Stream(receiver)
                }
            };
            Ok(FetchResponse {
                status,
                status_text: String::new(),
                headers: vec![("content-type".to_string(), content_type.to_string())],
                body,
            })
        })
    }
}

struct Case {
    name: String,
    model: Model,
    context: Context,
    body: String,
    status: u16,
    events: Vec<Value>,
}

fn cases() -> Vec<Case> {
    include_str!("fixtures/openai-completions-stream.jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let raw: Value = serde_json::from_str(line).expect("fixture line");
            let name = raw["name"].as_str().expect("name").to_string();
            Case {
                model: serde_json::from_value(raw["model"].clone())
                    .unwrap_or_else(|error| panic!("{name}: model: {error}")),
                context: serde_json::from_value(raw["context"].clone())
                    .unwrap_or_else(|error| panic!("{name}: context: {error}")),
                body: raw["body"].as_str().expect("body").to_string(),
                status: raw["status"].as_u64().unwrap_or(200) as u16,
                events: raw["events"].as_array().cloned().unwrap_or_default(),
                name,
            }
        })
        .collect()
}

/// Drops `partial` and the volatile `timestamp` so the comparison is about the shape of
/// the event and the finished message.
fn normalize(event: &AssistantMessageEvent) -> Value {
    let mut value = serde_json::to_value(event).expect("serialize event");
    if let Some(object) = value.as_object_mut() {
        object.remove("partial");
        for key in ["message", "error"] {
            if let Some(Value::Object(message)) = object.get_mut(key) {
                message.remove("timestamp");
            }
        }
    }
    value
}

/// Strips the same `timestamp` plus the `role` discriminator.
///
/// `AssistantMessage` carries `role: "assistant"` as a plain field in TS. In Rust the
/// discriminator lives on the `Message` enum (`#[serde(tag = "role")]`), so a bare
/// assistant message does not repeat it — adding it to the struct would emit it twice
/// for every session entry.
fn strip_expected(mut expected: Value) -> Value {
    if let Some(object) = expected.as_object_mut() {
        for key in ["message", "error"] {
            if let Some(Value::Object(message)) = object.get_mut(key) {
                message.remove("timestamp");
                message.remove("role");
            }
        }
    }
    expected
}

async fn run(case: &Case, chunk_size: Option<usize>) -> Vec<Value> {
    let events = stream(
        case.model.clone(),
        case.context.clone(),
        ProviderRequestOptions {
            api_key: Some("k".to_string()),
            max_retries: Some(0),
            fetch: Some(Arc::new(CannedFetch {
                status: case.status,
                body: case.body.clone(),
                chunk_size,
            })),
            ..ProviderRequestOptions::default()
        },
        OpenAICompletionsOptions::default(),
    );
    let mut collected = Vec::new();
    while let Some(event) = events.next().await {
        collected.push(normalize(&event));
    }
    collected
}

#[tokio::test(flavor = "multi_thread")]
async fn every_captured_event_sequence_is_reproduced() {
    let cases = cases();
    assert!(cases.len() >= 35, "expected the full fixture set");
    let mut failures = Vec::new();
    for case in &cases {
        let actual = run(case, None).await;
        let expected: Vec<Value> = case.events.iter().cloned().map(strip_expected).collect();
        if actual != expected {
            failures.push(format!(
                "{}\n  expected: {}\n  actual:   {}",
                case.name,
                serde_json::to_string(&expected).expect("serialize"),
                serde_json::to_string(&actual).expect("serialize"),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} sequences differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_decoder_survives_arbitrary_chunk_boundaries() {
    // Byte-wise delivery splits every SSE field; the event sequence must not change.
    for case in cases() {
        let whole = run(&case, None).await;
        let split = run(&case, Some(1)).await;
        assert_eq!(
            split, whole,
            "{} differs when fed one byte at a time",
            case.name
        );
    }
}

fn case_named(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("fixture {name} is missing"))
}

async fn events_of(name: &str) -> Vec<Value> {
    run(&case_named(name), None).await
}

fn final_message(events: &[Value]) -> &Value {
    let last = events.last().expect("at least one event");
    last.get("message")
        .or_else(|| last.get("error"))
        .expect("message")
}

#[tokio::test(flavor = "multi_thread")]
async fn usage_splits_cache_reads_and_writes_out_of_the_prompt_tokens() {
    let events = events_of("usage").await;
    let usage = &final_message(&events)["usage"];
    // 100 prompt tokens minus 40 cache reads minus 10 cache writes.
    assert_eq!(usage["input"], 50);
    assert_eq!(usage["output"], 20);
    assert_eq!(usage["cacheRead"], 40);
    assert_eq!(usage["cacheWrite"], 10);
    assert_eq!(usage["reasoning"], 5);
    assert_eq!(usage["totalTokens"], 120);

    // DeepSeek reports its hits under a different key.
    let deepseek = events_of("usage-deepseek-cache-hit").await;
    assert_eq!(final_message(&deepseek)["usage"]["cacheRead"], 30);

    // Moonshot puts usage on the choice instead of the chunk.
    let on_choice = events_of("usage-on-choice").await;
    assert_eq!(final_message(&on_choice)["usage"]["input"], 7);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_differing_response_model_is_recorded_once() {
    let events = events_of("response-model").await;
    assert_eq!(final_message(&events)["responseModel"], "gpt-5-2025");
    // The same id as the request means no `responseModel` at all.
    let same = events_of("response-model-same-as-request").await;
    assert_eq!(final_message(&same).get("responseModel"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_first_non_empty_reasoning_field_is_used() {
    // chutes.ai sends the same text under two keys; it must not be emitted twice.
    let events = events_of("reasoning-duplicate-fields").await;
    let deltas: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "thinking_delta")
        .collect();
    assert_eq!(deltas.len(), 1);
    let message = final_message(&events);
    assert_eq!(
        message["content"][0]["thinkingSignature"],
        "reasoning_content"
    );

    // opencode-go renames `reasoning` to the llama.cpp signature.
    let opencode = events_of("reasoning-opencode-go").await;
    assert_eq!(
        final_message(&opencode)["content"][0]["thinkingSignature"],
        "reasoning_content"
    );
    // Everything else keeps the field it arrived under.
    let plain = events_of("reasoning-field").await;
    assert_eq!(
        final_message(&plain)["content"][0]["thinkingSignature"],
        "reasoning"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_call_arguments_are_parsed_incrementally_and_finalized() {
    let events = events_of("tool-call").await;
    let deltas: Vec<&str> = events
        .iter()
        .filter(|event| event["type"] == "toolcall_delta")
        .map(|event| event["delta"].as_str().expect("delta"))
        .collect();
    assert_eq!(deltas, vec!["", "{\"path\":", "\"a.txt\"}"]);
    let end = events
        .iter()
        .find(|event| event["type"] == "toolcall_end")
        .expect("toolcall_end");
    assert_eq!(end["toolCall"]["arguments"]["path"], "a.txt");
    // The event carries the discriminator the session format expects.
    assert_eq!(end["toolCall"]["type"], "toolCall");

    // A stream cut off mid-JSON still yields the partially parsed arguments.
    let truncated = events_of("tool-call-truncated-arguments").await;
    let message = final_message(&truncated);
    assert_eq!(message["content"][0]["arguments"]["path"], "a");
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_calls_are_tracked_by_index_and_by_id() {
    let parallel = events_of("tool-call-two-parallel").await;
    let message = final_message(&parallel);
    assert_eq!(message["content"].as_array().expect("content").len(), 2);
    assert_eq!(message["content"][1]["name"], "write");

    // Without an index the id keeps the two deltas on the same block.
    let by_id = events_of("tool-call-without-index").await;
    assert_eq!(
        final_message(&by_id)["content"]
            .as_array()
            .expect("content")
            .len(),
        1
    );

    // An id that only arrives with the second delta is still adopted.
    let late = events_of("tool-call-id-arrives-late").await;
    assert_eq!(final_message(&late)["content"][0]["id"], "call_late");
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_tool_calls_stream_as_synthesized_json() {
    let events = events_of("custom-tool-call").await;
    let deltas: Vec<&str> = events
        .iter()
        .filter(|event| event["type"] == "toolcall_delta")
        .map(|event| event["delta"].as_str().expect("delta"))
        .collect();
    // The grammar input is wrapped into the JSON object the tool schema declares.
    assert_eq!(deltas, vec!["{\"input\":\"ab", "cd", "\"}"]);
    assert_eq!(
        final_message(&events)["content"][0]["arguments"]["input"],
        "abcd"
    );

    // An invented tool falls back to the "input" property.
    let unknown = events_of("custom-tool-call-unknown-tool").await;
    assert_eq!(
        final_message(&unknown)["content"][0]["arguments"]["input"],
        "x"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn encrypted_reasoning_details_attach_to_their_tool_call() {
    for name in [
        "reasoning-details-for-known-tool-call",
        "reasoning-details-before-tool-call",
    ] {
        let events = events_of(name).await;
        let signature = final_message(&events)["content"][0]["thoughtSignature"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: no thoughtSignature"));
        assert!(
            signature.contains("\"data\":\"enc\""),
            "{name}: {signature}"
        );
    }
    // Neither an id-only detail nor a non-encrypted one counts.
    let ignored = events_of("reasoning-details-ignored-when-incomplete").await;
    assert_eq!(
        final_message(&ignored)["content"][0].get("thoughtSignature"),
        None
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn every_finish_reason_maps_to_its_stop_reason() {
    for (name, reason) in [
        ("text", "stop"),
        ("finish-end", "stop"),
        ("finish-length", "length"),
        ("tool-call", "toolUse"),
        ("finish-function-call", "toolUse"),
    ] {
        let events = events_of(name).await;
        assert_eq!(events.last().expect("event")["reason"], reason, "{name}");
    }
    for (name, message) in [
        (
            "finish-content-filter",
            "Provider finish_reason: content_filter",
        ),
        (
            "finish-network-error",
            "Provider finish_reason: network_error",
        ),
        ("finish-unknown", "Provider finish_reason: weird_reason"),
    ] {
        let events = events_of(name).await;
        let last = events.last().expect("event");
        assert_eq!(last["type"], "error", "{name}");
        assert_eq!(last["error"]["errorMessage"], message, "{name}");
        // The raw value is preserved next to the mapped one.
        assert!(last["error"]["rawStopReason"].is_string(), "{name}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_finish_reason_is_an_error_unless_the_provider_omits_it() {
    let events = events_of("missing-finish-reason").await;
    let last = events.last().expect("event");
    assert_eq!(last["type"], "error");
    assert_eq!(
        last["error"]["errorMessage"],
        "Stream ended without finish_reason"
    );

    // With `supportsFinishReason: false` the stop reason is inferred from the content.
    let text = events_of("missing-finish-reason-tolerated").await;
    assert_eq!(text.last().expect("event")["reason"], "stop");
    let tool = events_of("missing-finish-reason-tolerated-tool-call").await;
    assert_eq!(tool.last().expect("event")["reason"], "toolUse");
}

#[tokio::test(flavor = "multi_thread")]
async fn http_errors_surface_status_and_body() {
    let events = events_of("http-error").await;
    let last = events.last().expect("event");
    assert_eq!(last["type"], "error");
    let message = last["error"]["errorMessage"].as_str().expect("message");
    assert!(message.starts_with("429: "), "{message}");
    // The OpenRouter metadata is already part of the body, so it is not appended twice.
    assert_eq!(message.matches("upstream said no").count(), 1);
    // No `start` event is emitted for a failed request.
    assert_eq!(events.len(), 1);

    let plain = events_of("http-error-plain-text").await;
    assert_eq!(
        plain.last().expect("event")["error"]["errorMessage"],
        "500 gateway exploded"
    );
    let nested = events_of("http-error-nested-message").await;
    assert_eq!(
        nested.last().expect("event")["error"]["errorMessage"],
        "400: {\"message\":\"bad request\"}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_field_inside_a_chunk_fails_the_stream() {
    let events = events_of("chunk-error-field").await;
    let last = events.last().expect("event");
    assert_eq!(last["type"], "error");
    assert_eq!(last["error"]["errorMessage"], "upstream blew up");
    // The text delivered before the error is kept on the partial message.
    assert_eq!(last["error"]["content"][0]["text"], "hi");

    let without_message = events_of("chunk-error-field-without-message").await;
    assert_eq!(
        without_message.last().expect("event")["error"]["errorMessage"],
        "{\"code\":500}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unparsable_chunk_ends_the_stream_with_an_error() {
    // Deviation class 1: TS reports V8's `JSON.parse` wording, which has no counterpart
    // here; only the failure itself is part of the contract.
    let case = Case {
        name: "unparsable".to_string(),
        model: case_named("text").model,
        context: case_named("text").context,
        body: "data: not json\n\ndata: [DONE]\n\n".to_string(),
        status: 200,
        events: Vec::new(),
    };
    let events = run(&case, None).await;
    let last = events.last().expect("event");
    assert_eq!(last["type"], "error");
    assert_eq!(last["error"]["stopReason"], "error");
    assert!(
        last["error"]["errorMessage"]
            .as_str()
            .expect("message")
            .contains("expected ident"),
        "{:?}",
        last["error"]["errorMessage"]
    );
}

/// The state machine can also be driven directly, which the responses APIs reuse.
#[test]
fn the_state_machine_starts_from_a_pending_message() {
    let model: Model = serde_json::from_value(serde_json::json!({
        "id": "gpt-5", "name": "gpt-5", "api": "openai-completions", "provider": "openai",
        "baseUrl": "https://api.openai.com/v1", "reasoning": true, "input": ["text"],
        "cost": { "input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 1000, "maxTokens": 100,
    }))
    .expect("model");
    let state = OpenAICompletionsStreamState::new(&model, get_compat(&model), BTreeMap::new(), 7);
    assert_eq!(
        state.output.stop_reason,
        notagent_ai::types::StopReason::Pending
    );
    assert_eq!(state.output.timestamp, 7);
    assert_eq!(state.output.usage.total_tokens, Some(0));
    assert!(state.output.content.is_empty());
    // Grammar properties are optional; an empty catalog is a valid starting point.
    assert!(
        create_grammar_tool_input_properties(None, true)
            .expect("properties")
            .is_empty()
    );
    let _ = Map::new();
}

#[tokio::test(flavor = "multi_thread")]
async fn null_chunks_from_openai_compatible_providers_are_skipped() {
    let events = events_of("null-chunks-are-ignored").await;
    let last = events.last().expect("event");
    assert_eq!(last["reason"], "stop");
    let message = &last["message"];
    assert_eq!(message.get("errorMessage"), None);
    // The first non-null chunk wins; `||=` never overwrites a set id.
    assert_eq!(message["responseId"], "chatcmpl-1");
    assert_eq!(message["usage"]["totalTokens"], 4);
    assert_eq!(message["content"][0]["text"], "OK");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_of_only_null_finish_reasons_is_an_error() {
    let events = events_of("only-null-finish-reasons").await;
    let last = events.last().expect("event");
    assert_eq!(last["type"], "error");
    assert_eq!(
        last["error"]["errorMessage"],
        "Stream ended without finish_reason"
    );
    // The text received so far is kept.
    assert_eq!(last["error"]["content"][0]["text"], "partial answer");
}

#[tokio::test(flavor = "multi_thread")]
async fn ids_that_change_mid_stream_stay_on_the_block_the_index_points_at() {
    // Kimi restates a different id for every delta of the same call.
    let events = events_of("tool-call-ids-mutate-mid-stream").await;
    let message = final_message(&events);
    let content = message["content"].as_array().expect("content");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["id"], "functions.read:0");
    assert_eq!(content[0]["arguments"]["path"], "README.md");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_custom_object_does_not_turn_a_function_call_into_a_custom_one() {
    let events = events_of("empty-custom-object-on-a-function-tool-call").await;
    let content = final_message(&events)["content"]
        .as_array()
        .expect("content");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["name"], "read");
    assert_eq!(content[0]["arguments"]["path"], "README.md");
}

#[tokio::test(flavor = "multi_thread")]
async fn text_reasoning_and_parallel_tool_calls_accumulate_independently() {
    let events = events_of("mixed-content-reasoning-and-parallel-tool-calls").await;
    let content = final_message(&events)["content"]
        .as_array()
        .expect("content");
    // Block order follows the order the first delta of each block arrived in.
    assert_eq!(content[0]["thinking"], "let me think");
    assert_eq!(content[1]["text"], "I will read two files");
    assert_eq!(content[2]["arguments"]["p"], "a");
    assert_eq!(content[3]["arguments"]["p"], "b");
    // Every block is opened once and closed once.
    for kind in ["thinking", "text", "toolcall"] {
        let starts = events
            .iter()
            .filter(|event| event["type"] == format!("{kind}_start"))
            .count();
        let ends = events
            .iter()
            .filter(|event| event["type"] == format!("{kind}_end"))
            .count();
        assert_eq!(starts, ends, "{kind}");
    }
}
