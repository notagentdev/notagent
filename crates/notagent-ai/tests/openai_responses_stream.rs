use std::sync::Arc;

use notagent_ai::api::openai_responses::{OpenAIResponsesOptions, stream};
use notagent_ai::types::{AssistantMessageEvent, Context, Model, ProviderRequestOptions};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::Value;

/// A fetch that replies with one canned response, optionally chunked.
struct CannedFetch {
    status: u16,
    body: String,
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
    options: OpenAIResponsesOptions,
    body: String,
    status: u16,
    events: Vec<Value>,
}

fn options_from_fixture(raw: &Value) -> OpenAIResponsesOptions {
    let raw = raw.as_object().cloned().unwrap_or_default();
    OpenAIResponsesOptions {
        service_tier: raw
            .get("serviceTier")
            .and_then(Value::as_str)
            .map(str::to_string),
        ..OpenAIResponsesOptions::default()
    }
}

fn cases() -> Vec<Case> {
    include_str!("fixtures/openai-responses-stream.jsonl")
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
                options: options_from_fixture(&raw["options"]),
                body: raw["body"].as_str().expect("body").to_string(),
                status: raw["status"].as_u64().unwrap_or(200) as u16,
                events: raw["events"].as_array().cloned().unwrap_or_default(),
                name,
            }
        })
        .collect()
}

/// Drops `partial` and the volatile `timestamp`.
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

/// Same, plus the `role` discriminator: in Rust it lives on the `Message` enum
/// (`#[serde(tag = "role")]`), so a bare assistant message does not repeat it.
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
        case.options.clone(),
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
    assert!(cases.len() >= 30, "expected the full fixture set");
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
async fn text_items_carry_their_message_id_as_a_signature() {
    let events = events_of("text").await;
    let message = final_message(&events);
    assert_eq!(message["content"][0]["text"], "Hello");
    assert_eq!(
        message["content"][0]["textSignature"],
        "{\"v\":1,\"id\":\"msg_1\"}"
    );
    // `response.created` sets the id first, `response.completed` overwrites it.
    assert_eq!(message["responseId"], "resp_1");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_is_streamed_as_text() {
    let events = events_of("refusal").await;
    assert_eq!(final_message(&events)["content"][0]["text"], "I cannot");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_final_answer_phase_forces_the_stop_reason() {
    // The response says "incomplete/max_output_tokens", but the phase already set `stop`
    // — and `finalizeResponse` then overwrites it with the mapped status.
    let events = events_of("message-phase-final-answer").await;
    let last = events.last().expect("event");
    assert_eq!(last["reason"], "length");
    assert_eq!(
        final_message(&events)["content"][0]["textSignature"],
        "{\"v\":1,\"id\":\"msg_1\",\"phase\":\"final_answer\"}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reasoning_summaries_are_joined_by_blank_lines() {
    let events = events_of("reasoning-summary").await;
    let message = final_message(&events);
    assert_eq!(message["content"][0]["thinking"], "step one\n\nstep two");
    // The signature is the whole reasoning item, ready for replay.
    let signature = message["content"][0]["thinkingSignature"]
        .as_str()
        .expect("signature");
    assert!(
        signature.contains("\"encrypted_content\":\"enc\""),
        "{signature}"
    );

    // Without a summary the reasoning content is used.
    let text = events_of("reasoning-text").await;
    assert_eq!(
        final_message(&text)["content"][0]["thinking"],
        "raw thought"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_late_encrypted_content_is_backfilled_into_the_signature() {
    let events = events_of("reasoning-signature-backfill").await;
    let signature = final_message(&events)["content"][0]["thinkingSignature"]
        .as_str()
        .expect("signature");
    assert!(
        signature.contains("\"encrypted_content\":\"late-enc\""),
        "{signature}"
    );

    // An existing value is never replaced.
    let kept = events_of("reasoning-signature-backfill-skipped").await;
    let signature = final_message(&kept)["content"][0]["thinkingSignature"]
        .as_str()
        .expect("signature");
    assert!(
        signature.contains("\"encrypted_content\":\"original\""),
        "{signature}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn function_call_ids_combine_the_call_and_item_id() {
    let events = events_of("function-call").await;
    let message = final_message(&events);
    assert_eq!(message["content"][0]["id"], "call_1|fc_1");
    assert_eq!(message["content"][0]["arguments"]["path"], "a.txt");
    // The item can also arrive without an `added` event.
    let late = events_of("function-call-without-added-event").await;
    assert_eq!(final_message(&late)["content"][0]["arguments"]["path"], "a");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_arguments_done_event_only_emits_the_missing_tail() {
    let tail = events_of("function-call-arguments-done-adds-a-tail").await;
    let deltas: Vec<&str> = tail
        .iter()
        .filter(|event| event["type"] == "toolcall_delta")
        .map(|event| event["delta"].as_str().expect("delta"))
        .collect();
    assert_eq!(deltas, vec!["{\"path\":\"a", ".txt\"}"]);

    // When the final arguments are not a continuation, no delta is emitted at all.
    let diverged = events_of("function-call-arguments-done-diverges").await;
    let deltas: Vec<&str> = diverged
        .iter()
        .filter(|event| event["type"] == "toolcall_delta")
        .map(|event| event["delta"].as_str().expect("delta"))
        .collect();
    assert_eq!(deltas, vec!["{\"path\":\"WRONG"]);
    // The finished call still carries the corrected arguments.
    assert_eq!(
        final_message(&diverged)["content"][0]["arguments"]["path"],
        "right"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_namespace_survives_the_stream() {
    let events = events_of("function-call-namespace").await;
    assert_eq!(final_message(&events)["content"][0]["namespace"], "files");
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_tool_input_is_wrapped_into_the_tools_json_object() {
    let events = events_of("custom-tool-call").await;
    let deltas: Vec<&str> = events
        .iter()
        .filter(|event| event["type"] == "toolcall_delta")
        .map(|event| event["delta"].as_str().expect("delta"))
        .collect();
    assert_eq!(deltas, vec!["{\"input\":\"ab", "cd", "\"}"]);
    assert_eq!(
        final_message(&events)["content"][0]["arguments"]["input"],
        "abcd"
    );

    // An unknown tool falls back to the "input" property and keeps the seed input.
    let unknown = events_of("custom-tool-call-unknown-tool").await;
    assert_eq!(
        final_message(&unknown)["content"][0]["arguments"]["input"],
        "seedx"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn output_indices_keep_parallel_items_apart() {
    let events = events_of("parallel-output-items").await;
    let content = final_message(&events)["content"]
        .as_array()
        .expect("content");
    assert_eq!(content.len(), 3);
    assert_eq!(content[0]["thinking"], "thinking");
    assert_eq!(content[1]["text"], "text");
    assert_eq!(content[2]["name"], "read");
}

#[tokio::test(flavor = "multi_thread")]
async fn deltas_for_an_unknown_output_index_are_ignored() {
    let events = events_of("deltas-without-a-slot").await;
    let content = final_message(&events)["content"]
        .as_array()
        .expect("content");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["text"], "ok");
}

#[tokio::test(flavor = "multi_thread")]
async fn usage_subtracts_cache_reads_and_writes_from_the_input_tokens() {
    let events = events_of("usage").await;
    let usage = &final_message(&events)["usage"];
    assert_eq!(usage["input"], 60);
    assert_eq!(usage["output"], 20);
    assert_eq!(usage["cacheRead"], 30);
    assert_eq!(usage["cacheWrite"], 10);
    assert_eq!(usage["reasoning"], 7);
    assert_eq!(usage["totalTokens"], 120);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_service_tier_scales_the_cost_after_it_was_calculated() {
    let flex = events_of("usage-flex-tier").await;
    let flex_cost = &final_message(&flex)["usage"]["cost"];
    let priority = events_of("usage-priority-tier").await;
    let priority_cost = &final_message(&priority)["usage"]["cost"];
    // priority is 2x, flex 0.5x, so priority is exactly four times flex.
    let flex_total = flex_cost["total"].as_f64().expect("total");
    let priority_total = priority_cost["total"].as_f64().expect("total");
    assert!((priority_total - flex_total * 4.0).abs() < 1e-12);
}

#[tokio::test(flavor = "multi_thread")]
async fn response_statuses_map_to_stop_reasons() {
    assert_eq!(
        events_of("incomplete-max-output-tokens")
            .await
            .last()
            .expect("event")["reason"],
        "length"
    );
    let filtered = events_of("incomplete-content-filter").await;
    let last = filtered.last().expect("event");
    assert_eq!(last["type"], "error");
    assert_eq!(
        last["error"]["errorMessage"],
        "Response incomplete: content_filter"
    );
    // The raw reason is preserved alongside the mapped one.
    assert_eq!(last["error"]["rawStopReason"], "incomplete.content_filter");

    let without_reason = events_of("incomplete-without-reason").await;
    assert_eq!(
        without_reason.last().expect("event")["error"]["errorMessage"],
        "Response incomplete without a provider reason"
    );

    // cancelled has no message of its own.
    assert_eq!(
        events_of("status-cancelled").await.last().expect("event")["type"],
        "error"
    );
    // in_progress and a missing status both count as a clean stop.
    assert_eq!(
        events_of("status-in-progress").await.last().expect("event")["reason"],
        "stop"
    );
    assert_eq!(
        events_of("status-missing").await.last().expect("event")["reason"],
        "stop"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_call_turns_a_clean_stop_into_tool_use() {
    assert_eq!(
        events_of("tool-use-overrides-stop")
            .await
            .last()
            .expect("event")["reason"],
        "toolUse"
    );
}

/// The prefix only appears once a status and a body were extracted; a plain thrown
/// error keeps its message unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_response_reports_the_providers_reason() {
    for (name, message) in [
        ("response-failed-with-error", "server_error: boom"),
        ("response-failed-with-details", "incomplete: content_filter"),
        (
            "response-failed-without-details",
            "Unknown error (no error details in response)",
        ),
        ("error-event", "Error Code rate_limit: slow down"),
        (
            "no-terminal-event",
            "OpenAI Responses stream ended before a terminal response event",
        ),
    ] {
        let events = events_of(name).await;
        let last = events.last().expect("event");
        assert_eq!(last["type"], "error", "{name}");
        assert_eq!(last["error"]["errorMessage"], message, "{name}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn http_errors_are_prefixed_with_the_provider_name() {
    let events = events_of("http-error").await;
    assert_eq!(
        events.last().expect("event")["error"]["errorMessage"],
        "OpenAI API error (429): {\"message\":\"slow down\"}"
    );
    let plain = events_of("http-error-plain-text").await;
    assert_eq!(
        plain.last().expect("event")["error"]["errorMessage"],
        "OpenAI API error (500): 500 gateway exploded"
    );
}
