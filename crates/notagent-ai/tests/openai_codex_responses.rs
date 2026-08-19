//! Differential test of the OpenAI Codex Responses adapter.
//!
//! `fixtures/openai-codex-responses.jsonl` records, for each case, the request body the
//! TS implementation builds, the URL and headers it sends, and the event sequence it
//! emits for the scripted SSE body (see `fixtures/generators`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use notagent_ai::api::constrained_sampling::create_grammar_tool_input_properties;
use notagent_ai::api::openai_codex_responses::{
    CodexReasoningEffort, CodexSseParser, OpenAICodexResponsesOptions, build_request_body,
    build_sse_headers, build_websocket_headers, compress_request_body_zstd, extract_account_id,
    is_retryable_error, is_terminal_rate_limit_error, parse_error_response,
    resolve_codex_service_tier, resolve_codex_url, resolve_codex_websocket_url,
    retry_after_delay_ms, service_tier_cost_multiplier, stream, validate_retry_delay_ms,
};
use notagent_ai::types::{
    AssistantMessageEvent, CacheRetention, Context, Message, Model, ProviderHeaders,
    ProviderRequestOptions, ThinkingLevel, Transport, UserContent, UserMessage,
};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::Value;

fn reasoning_effort(value: &str) -> CodexReasoningEffort {
    match value {
        "none" => CodexReasoningEffort::None,
        "minimal" => CodexReasoningEffort::Level(ThinkingLevel::Minimal),
        "low" => CodexReasoningEffort::Level(ThinkingLevel::Low),
        "medium" => CodexReasoningEffort::Level(ThinkingLevel::Medium),
        "high" => CodexReasoningEffort::Level(ThinkingLevel::High),
        "xhigh" => CodexReasoningEffort::Level(ThinkingLevel::Xhigh),
        "max" => CodexReasoningEffort::Level(ThinkingLevel::Max),
        other => panic!("unknown effort {other}"),
    }
}

fn options_from_fixture(raw: &Value) -> OpenAICodexResponsesOptions {
    let raw = raw.as_object().cloned().unwrap_or_default();
    OpenAICodexResponsesOptions {
        reasoning_effort: raw
            .get("reasoningEffort")
            .and_then(Value::as_str)
            .map(reasoning_effort),
        reasoning_summary: raw
            .get("reasoningSummary")
            .map(|value| value.as_str().map(str::to_string)),
        service_tier: raw
            .get("serviceTier")
            .and_then(Value::as_str)
            .map(str::to_string),
        text_verbosity: raw
            .get("textVerbosity")
            .and_then(Value::as_str)
            .map(str::to_string),
        tool_choice: raw
            .get("toolChoice")
            .and_then(Value::as_str)
            .map(str::to_string),
        temperature: raw.get("temperature").and_then(Value::as_f64),
        transport: Some(Transport::Sse),
        cache_retention: raw.get("cacheRetention").and_then(Value::as_str).map(
            |value| match value {
                "none" => CacheRetention::None,
                "long" => CacheRetention::Long,
                _ => CacheRetention::Short,
            },
        ),
        session_id: raw
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        websocket_connect_timeout_ms: None,
        env: None,
    }
}

fn headers_from_fixture(raw: &Value) -> Option<ProviderHeaders> {
    raw.get("headers")
        .and_then(Value::as_object)
        .map(|headers| {
            headers
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().map(str::to_string)))
                .collect()
        })
}

struct Case {
    name: String,
    model: Model,
    context: Context,
    options: OpenAICodexResponsesOptions,
    request_headers: Option<ProviderHeaders>,
    payload: Value,
    request_url: Option<String>,
    sent_headers: BTreeMap<String, String>,
    request_body_encoding: Option<String>,
    status: u16,
    body: String,
    events: Vec<Value>,
}

fn cases() -> Vec<Case> {
    include_str!("fixtures/openai-codex-responses.jsonl")
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
                request_headers: headers_from_fixture(&raw["options"]),
                payload: raw["payload"].clone(),
                request_url: raw["requestUrl"].as_str().map(str::to_string),
                sent_headers: raw["requestHeaders"]
                    .as_object()
                    .map(|headers| {
                        headers
                            .iter()
                            .filter_map(|(key, value)| {
                                value.as_str().map(|value| (key.clone(), value.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                request_body_encoding: raw["requestBodyEncoding"].as_str().map(str::to_string),
                status: raw["status"].as_u64().unwrap_or(200) as u16,
                body: raw["body"].as_str().unwrap_or_default().to_string(),
                events: raw["events"].as_array().cloned().unwrap_or_default(),
                name,
            }
        })
        .collect()
}

const TIMESTAMP: i64 = 1_700_000_000_000;
/// The JWT the generator signs; its payload carries `chatgpt_account_id: acct_123`.
fn token() -> String {
    let payload = serde_json::json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "acct_123" }
    });
    let encoded = base64_url(&serde_json::to_vec(&payload).expect("json"));
    format!("header.{encoded}.signature")
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn build(case: &Case) -> Value {
    let grammar_tool_input_properties: BTreeMap<String, String> =
        create_grammar_tool_input_properties(case.context.tools.as_deref(), false)
            .unwrap_or_else(|error| panic!("{}: grammar properties: {error}", case.name));
    let cache_session_id = if case.options.cache_retention == Some(CacheRetention::None) {
        None
    } else {
        case.options.session_id.as_deref()
    };
    let clamped = cache_session_id.map(|session_id| {
        notagent_ai::api::openai_prompt_cache::clamp_openai_prompt_cache_key(Some(session_id))
            .expect("clamped")
    });
    build_request_body(
        &case.model,
        &case.context,
        &case.options,
        clamped.as_deref(),
        &grammar_tool_input_properties,
        TIMESTAMP,
    )
    .unwrap_or_else(|error| panic!("{}: build_request_body: {error}", case.name))
}

#[test]
fn every_captured_request_body_is_reproduced_byte_for_byte() {
    let cases = cases();
    assert!(cases.len() >= 40, "expected the full fixture set");
    let mut failures = Vec::new();
    for case in &cases {
        let built = build(case);
        if serde_json::to_string(&built).expect("serialize")
            != serde_json::to_string(&case.payload).expect("serialize")
        {
            failures.push(format!(
                "{}\n  expected: {}\n  actual:   {}",
                case.name,
                serde_json::to_string(&case.payload).expect("serialize"),
                serde_json::to_string(&built).expect("serialize"),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} bodies differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// A fetch that replies with one canned response and records the request.
struct CannedFetch {
    status: u16,
    body: String,
    seen: Arc<Mutex<Option<FetchRequest>>>,
}

impl FetchFn for CannedFetch {
    fn fetch(
        &self,
        request: FetchRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FetchError>> + Send>,
    > {
        *self.seen.lock().expect("poisoned") = Some(request);
        let status = self.status;
        let body = self.body.clone();
        Box::pin(async move {
            Ok(FetchResponse {
                status,
                status_text: String::new(),
                headers: vec![(
                    "content-type".to_string(),
                    if status == 200 {
                        "text/event-stream".to_string()
                    } else {
                        "application/json".to_string()
                    },
                )],
                body: FetchBody::Bytes(body.into_bytes()),
            })
        })
    }
}

fn normalize(event: &AssistantMessageEvent) -> Value {
    let mut value = serde_json::to_value(event).expect("serialize event");
    if let Some(object) = value.as_object_mut() {
        object.remove("partial");
        for key in ["message", "error"] {
            if let Some(Value::Object(message)) = object.get_mut(key) {
                message.remove("timestamp");
                // The diagnostics carry their own wall-clock timestamp.
                message.remove("diagnostics");
            }
        }
    }
    value
}

fn strip_expected(mut expected: Value) -> Value {
    if let Some(object) = expected.as_object_mut() {
        for key in ["message", "error"] {
            if let Some(Value::Object(message)) = object.get_mut(key) {
                message.remove("timestamp");
                message.remove("diagnostics");
                // In Rust the `role` discriminator lives on the `Message` enum.
                message.remove("role");
            }
        }
    }
    expected
}

async fn run(case: &Case) -> (Vec<Value>, Option<FetchRequest>) {
    let seen = Arc::new(Mutex::new(None));
    let events = stream(
        case.model.clone(),
        case.context.clone(),
        ProviderRequestOptions {
            api_key: Some(token()),
            max_retries: Some(0),
            headers: case.request_headers.clone(),
            fetch: Some(Arc::new(CannedFetch {
                status: case.status,
                body: case.body.clone(),
                seen: seen.clone(),
            })),
            ..ProviderRequestOptions::default()
        },
        case.options.clone(),
    );
    let mut collected = Vec::new();
    while let Some(event) = events.next().await {
        collected.push(normalize(&event));
    }
    let request = seen.lock().expect("poisoned").clone();
    (collected, request)
}

#[tokio::test(flavor = "multi_thread")]
async fn every_captured_event_sequence_is_reproduced() {
    let mut failures = Vec::new();
    for case in cases() {
        let (actual, _) = run(&case).await;
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
        "{} sequences differ:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn every_captured_request_url_is_reproduced() {
    for case in cases() {
        let Some(expected) = case.request_url.clone() else {
            continue;
        };
        let (_, request) = run(&case).await;
        let request = request.expect("request");
        assert_eq!(request.url, expected, "{}", case.name);
    }
}

fn case_named(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("fixture {name} is missing"))
}

#[test]
fn the_codex_url_appends_the_missing_path_segments() {
    assert_eq!(
        resolve_codex_url(""),
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://proxy.example.com/api"),
        "https://proxy.example.com/api/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://proxy.example.com/api/codex"),
        "https://proxy.example.com/api/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://proxy.example.com/api/codex/responses"),
        "https://proxy.example.com/api/codex/responses"
    );
    // Trailing slashes are stripped first.
    assert_eq!(
        resolve_codex_url("https://proxy.example.com/api///"),
        "https://proxy.example.com/api/codex/responses"
    );
}

#[test]
fn the_websocket_url_swaps_the_scheme() {
    assert_eq!(
        resolve_codex_websocket_url("https://chatgpt.com/backend-api").expect("url"),
        "wss://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        resolve_codex_websocket_url("http://localhost:8080/api").expect("url"),
        "ws://localhost:8080/api/codex/responses"
    );
}

#[test]
fn the_account_id_comes_from_the_jwt_claim() {
    assert_eq!(
        extract_account_id(&token()).expect("account id"),
        "acct_123"
    );
    for invalid in ["", "a.b", "a.b.c.d", "header.not-base64!.signature"] {
        assert_eq!(
            extract_account_id(invalid).expect_err("invalid").message(),
            "Failed to extract accountId from token"
        );
    }
    // A well-formed token without the claim fails too.
    let payload = base64_url(b"{\"sub\":\"x\"}");
    assert!(extract_account_id(&format!("a.{payload}.c")).is_err());
}

#[test]
fn the_sse_headers_carry_the_account_the_beta_flag_and_the_session() {
    let headers = build_sse_headers(None, None, "acct_1", "token", Some("session-1"), "ua");
    assert_eq!(headers.get("authorization"), Some("Bearer token"));
    assert_eq!(headers.get("chatgpt-account-id"), Some("acct_1"));
    assert_eq!(headers.get("originator"), Some("notagent"));
    assert_eq!(headers.get("user-agent"), Some("ua"));
    assert_eq!(headers.get("openai-beta"), Some("responses=experimental"));
    assert_eq!(headers.get("accept"), Some("text/event-stream"));
    assert_eq!(headers.get("session-id"), Some("session-1"));
    assert_eq!(headers.get("x-client-request-id"), Some("session-1"));

    // Without a session id neither session header appears.
    let headers = build_sse_headers(None, None, "acct_1", "token", None, "ua");
    assert_eq!(headers.get("session-id"), None);
}

#[test]
fn the_websocket_headers_replace_the_beta_flag_and_drop_the_body_headers() {
    let headers = build_websocket_headers(None, None, "acct_1", "token", "req-1", "ua");
    assert_eq!(
        headers.get("openai-beta"),
        Some("responses_websockets=2026-02-06")
    );
    assert_eq!(headers.get("accept"), None);
    assert_eq!(headers.get("content-type"), None);
    assert_eq!(headers.get("x-client-request-id"), Some("req-1"));
    assert_eq!(headers.get("session-id"), Some("req-1"));
}

#[test]
fn a_null_request_header_deletes_a_default() {
    let case = case_named("headers");
    let headers = build_sse_headers(
        case.model.headers.as_ref(),
        case.request_headers.as_ref(),
        "acct_1",
        "token",
        None,
        "ua",
    );
    assert_eq!(headers.get("x-model"), Some("from-model"));
    assert_eq!(headers.get("x-request"), Some("from-request"));
    // `originator: null` deletes it — and the adapter then sets it again.
    assert_eq!(headers.get("originator"), Some("notagent"));
    assert_eq!(
        case.sent_headers.get("originator").map(String::as_str),
        Some("notagent")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_request_body_is_zstd_compressed() {
    let case = case_named("minimal");
    let (_, request) = run(&case).await;
    let request = request.expect("request");
    assert!(
        request
            .headers
            .iter()
            .any(|(key, value)| key == "content-encoding" && value == "zstd")
    );
    // The TS implementation compresses too.
    assert_eq!(case.request_body_encoding.as_deref(), Some("zstd"));
    // The compressed bytes decode back to the JSON body.
    let body = request.body.expect("body");
    let decoded = zstd::stream::decode_all(body.as_slice()).expect("zstd");
    let parsed: Value = serde_json::from_slice(&decoded).expect("json");
    assert_eq!(parsed["model"], "gpt-5-codex");
}

#[test]
fn compression_round_trips() {
    let compressed = compress_request_body_zstd("{\"a\":1}").expect("compressed");
    assert_eq!(
        zstd::stream::decode_all(compressed.as_slice()).expect("zstd"),
        b"{\"a\":1}"
    );
}

#[test]
fn terminal_rate_limits_are_not_retried() {
    for text in [
        "GoUsageLimitError",
        "FreeUsageLimitError",
        "Monthly usage limit reached",
        "available balance too low",
        "insufficient_quota",
        "you are out of budget",
        "quota exceeded",
        "billing problem",
    ] {
        assert!(is_terminal_rate_limit_error(text), "{text}");
        assert!(!is_retryable_error(429, text), "{text}");
    }
    assert!(!is_terminal_rate_limit_error("temporarily overloaded"));
    for status in [429, 500, 502, 503, 504] {
        assert!(is_retryable_error(status, "transient"), "{status}");
    }
    assert!(!is_retryable_error(400, "bad request"));
    // The message alone can make a non-5xx retryable.
    for text in [
        "rate limit",
        "rate-limit",
        "overloaded",
        "service unavailable",
        "upstream connect error",
        "connection refused",
    ] {
        assert!(is_retryable_error(400, text), "{text}");
    }
}

#[test]
fn the_retry_delay_comes_from_the_headers_in_order() {
    let now = 1_700_000_000_000;
    assert_eq!(
        retry_after_delay_ms(&[("retry-after-ms".to_string(), "1500".to_string())], now),
        Some(1500.0)
    );
    // Seconds when only `retry-after` is present.
    assert_eq!(
        retry_after_delay_ms(&[("retry-after".to_string(), "2".to_string())], now),
        Some(2000.0)
    );
    // Negative values clamp to zero.
    assert_eq!(
        retry_after_delay_ms(&[("retry-after-ms".to_string(), "-5".to_string())], now),
        Some(0.0)
    );
    // An HTTP date is turned into a delta.
    let date = chrono::DateTime::from_timestamp_millis(now + 30_000)
        .expect("date")
        .to_rfc2822();
    assert_eq!(
        retry_after_delay_ms(&[("retry-after".to_string(), date)], now),
        Some(30_000.0)
    );
    assert_eq!(retry_after_delay_ms(&[], now), None);
    assert_eq!(
        retry_after_delay_ms(
            &[("retry-after".to_string(), "not-a-date".to_string())],
            now
        ),
        None
    );
}

#[test]
fn a_server_delay_beyond_the_limit_is_rejected() {
    assert_eq!(
        validate_retry_delay_ms(1000.0, Some(60_000)).expect("allowed"),
        1000.0
    );
    let error = validate_retry_delay_ms(120_000.0, Some(60_000)).expect_err("too long");
    assert_eq!(
        error.message(),
        "Server requested 120s retry delay (max: 60s)"
    );
    // A limit of zero disables the check.
    assert!(validate_retry_delay_ms(999_999.0, Some(0)).is_ok());
}

#[test]
fn usage_limit_errors_get_a_friendly_message() {
    let now = 1_700_000_000_000;
    let (message, friendly) = parse_error_response(
        429,
        "Too Many Requests",
        "{\"error\":{\"code\":\"usage_limit_reached\",\"plan_type\":\"PLUS\"}}",
        now,
    );
    assert_eq!(
        friendly.as_deref(),
        Some("You have hit your ChatGPT usage limit (plus plan).")
    );
    // Without an error message the friendly one is used.
    assert_eq!(
        message,
        "You have hit your ChatGPT usage limit (plus plan)."
    );

    // `resets_at` adds the remaining minutes.
    let resets_at = (now as f64 / 1000.0) + 600.0;
    let (_, friendly) = parse_error_response(
        429,
        "",
        &format!("{{\"error\":{{\"code\":\"usage_limit_reached\",\"resets_at\":{resets_at}}}}}"),
        now,
    );
    assert_eq!(
        friendly.as_deref(),
        Some("You have hit your ChatGPT usage limit. Try again in ~10 min.")
    );

    // A plain error keeps its own message and gets no friendly one.
    let (message, friendly) = parse_error_response(
        400,
        "",
        "{\"error\":{\"code\":\"bad\",\"message\":\"nope\"}}",
        now,
    );
    assert_eq!(message, "nope");
    assert_eq!(friendly, None);

    // A non-JSON body falls back to the raw text, then the status text.
    assert_eq!(
        parse_error_response(500, "Server Error", "boom", now).0,
        "boom"
    );
    assert_eq!(
        parse_error_response(500, "Server Error", "", now).0,
        "Server Error"
    );
    assert_eq!(parse_error_response(500, "", "", now).0, "Request failed");
}

#[test]
fn the_service_tier_resolution_prefers_the_request_over_a_default_response() {
    assert_eq!(
        resolve_codex_service_tier(Some("default"), Some("flex")).as_deref(),
        Some("flex")
    );
    assert_eq!(
        resolve_codex_service_tier(Some("default"), Some("priority")).as_deref(),
        Some("priority")
    );
    // Anything else keeps the response value.
    assert_eq!(
        resolve_codex_service_tier(Some("flex"), Some("priority")).as_deref(),
        Some("flex")
    );
    assert_eq!(
        resolve_codex_service_tier(Some("default"), Some("auto")).as_deref(),
        Some("default")
    );
    assert_eq!(
        resolve_codex_service_tier(None, Some("flex")).as_deref(),
        Some("flex")
    );
    assert_eq!(resolve_codex_service_tier(None, None), None);

    assert_eq!(
        service_tier_cost_multiplier("gpt-5-codex", Some("flex")),
        0.5
    );
    assert_eq!(
        service_tier_cost_multiplier("gpt-5-codex", Some("priority")),
        2.0
    );
    assert_eq!(
        service_tier_cost_multiplier("gpt-5.5", Some("priority")),
        2.5
    );
    assert_eq!(service_tier_cost_multiplier("gpt-5-codex", None), 1.0);
}

#[test]
fn the_codex_sse_parser_joins_data_lines_and_skips_the_terminator() {
    let mut parser = CodexSseParser::new();
    let events = parser
        .feed("data: {\"type\":\"a\"}\n\nignored line\n\ndata: [DONE]\n\n")
        .expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], "a");

    // Multiple data lines of one event are joined with newlines; only lines that start
    // with `data:` count.
    let mut parser = CodexSseParser::new();
    let events = parser
        .feed("data: {\"type\":\ndata: \"b\"}\n\n")
        .expect("events");
    assert_eq!(events[0]["type"], "b");

    // Split feeds keep the buffer.
    let mut parser = CodexSseParser::new();
    assert!(parser.feed("data: {\"ty").expect("events").is_empty());
    let events = parser.feed("pe\":\"c\"}\n\n").expect("events");
    assert_eq!(events[0]["type"], "c");

    // Unparsable JSON is a protocol error.
    let mut parser = CodexSseParser::new();
    let error = parser.feed("data: not json\n\n").expect_err("invalid");
    assert!(
        error.message().starts_with("Invalid Codex SSE JSON:"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_api_key_ends_the_stream_with_an_error() {
    let case = case_named("minimal");
    let events = stream(
        case.model.clone(),
        case.context.clone(),
        ProviderRequestOptions {
            api_key: None,
            ..ProviderRequestOptions::default()
        },
        case.options.clone(),
    );
    let mut last = None;
    while let Some(event) = events.next().await {
        last = Some(event);
    }
    let Some(AssistantMessageEvent::Error { error, .. }) = last else {
        panic!("expected an error event");
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("No API key for provider: openai-codex")
    );
    assert_eq!(error.api, "openai-codex-responses");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_end_turn_flag_is_taken_from_the_terminal_response() {
    let (events, _) = run(&case_named("stream-response-done-is-mapped-to-completed")).await;
    let last = events.last().expect("event");
    assert_eq!(last["message"]["endTurn"], true);
    let (events, _) = run(&case_named("stream-end-turn-false")).await;
    assert_eq!(events.last().expect("event")["message"]["endTurn"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_response_status_is_dropped_before_the_shared_mapping() {
    let (events, _) = run(&case_named("stream-unknown-status-is-dropped")).await;
    let last = events.last().expect("event");
    // Without a status the shared mapping falls back to `stop`.
    assert_eq!(last["reason"], "stop");
    assert_eq!(last["message"].get("rawStopReason"), None);
}

// ---------------------------------------------------------------------------
// WebSocket transport
// ---------------------------------------------------------------------------

/// A local WebSocket server that answers one `response.create` frame with a scripted
/// event sequence and records the frame it received.
async fn spawn_codex_websocket_server(
    events: Vec<String>,
    close_before_completion: bool,
) -> (
    String,
    Arc<Mutex<Vec<String>>>,
    Arc<Mutex<Vec<(String, String)>>>,
) {
    use futures::{SinkExt, StreamExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let frames = Arc::new(Mutex::new(Vec::new()));
    let headers = Arc::new(Mutex::new(Vec::new()));
    let recorded_frames = frames.clone();
    let recorded_headers = headers.clone();

    tokio::spawn(async move {
        let Ok((connection, _)) = listener.accept().await else {
            return;
        };
        let captured = recorded_headers.clone();
        // The callback's error type is the tungstenite response, which clippy calls
        // large; the callback never fails here.
        #[allow(clippy::result_large_err)]
        let socket = tokio_tungstenite::accept_hdr_async(
            connection,
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
             response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                let mut captured = captured.lock().expect("poisoned");
                for (name, value) in request.headers() {
                    captured.push((
                        name.as_str().to_string(),
                        value.to_str().unwrap_or_default().to_string(),
                    ));
                }
                Ok(response)
            },
        )
        .await;
        let Ok(mut socket) = socket else {
            return;
        };
        if let Some(Ok(message)) = socket.next().await {
            recorded_frames
                .lock()
                .expect("poisoned")
                .push(message.to_text().unwrap_or_default().to_string());
        }
        for event in events {
            if socket
                .send(tokio_tungstenite::tungstenite::Message::Text(event.into()))
                .await
                .is_err()
            {
                return;
            }
        }
        if close_before_completion {
            let _ = socket.close(None).await;
        }
    });

    (format!("http://127.0.0.1:{port}"), frames, headers)
}

fn codex_model(base_url: &str) -> Model {
    let mut model = case_named("minimal").model;
    model.base_url = base_url.to_string();
    model
}

#[tokio::test(flavor = "multi_thread")]
async fn the_websocket_transport_streams_a_full_response() {
    let (base_url, frames, headers) = spawn_codex_websocket_server(
        vec![
            serde_json::json!({
                "type": "response.output_item.added", "output_index": 0,
                "item": { "type": "message", "id": "msg_1", "role": "assistant", "content": [], "status": "in_progress" }
            })
            .to_string(),
            serde_json::json!({ "type": "response.output_text.delta", "output_index": 0, "delta": "hello" })
                .to_string(),
            serde_json::json!({
                "type": "response.output_item.done", "output_index": 0,
                "item": { "type": "message", "id": "msg_1", "role": "assistant", "status": "completed",
                          "content": [{ "type": "output_text", "text": "hello", "annotations": [] }] }
            })
            .to_string(),
            serde_json::json!({
                "type": "response.done",
                "response": { "id": "resp_ws", "status": "completed", "end_turn": true }
            })
            .to_string(),
        ],
        false,
    )
    .await;

    let events = stream(
        codex_model(&base_url),
        case_named("minimal").context,
        ProviderRequestOptions {
            api_key: Some(token()),
            ..ProviderRequestOptions::default()
        },
        OpenAICodexResponsesOptions {
            transport: Some(Transport::Websocket),
            ..OpenAICodexResponsesOptions::default()
        },
    );
    let mut collected = Vec::new();
    while let Some(event) = events.next().await {
        collected.push(normalize(&event));
    }

    let last = collected.last().expect("event");
    assert_eq!(last["type"], "done", "{collected:?}");
    assert_eq!(last["message"]["content"][0]["text"], "hello");
    assert_eq!(last["message"]["responseId"], "resp_ws");
    assert_eq!(last["message"]["endTurn"], true);

    // The frame is the request body with a leading `type: response.create`.
    let frames = frames.lock().expect("poisoned");
    let frame: Value = serde_json::from_str(frames.first().expect("frame")).expect("json");
    assert_eq!(frame["type"], "response.create");
    assert_eq!(frame["model"], "gpt-5-codex");
    assert_eq!(frame["stream"], true);

    // The handshake carries the WebSocket beta flag, not the SSE one.
    let headers = headers.lock().expect("poisoned");
    let beta = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("openai-beta"));
    // TS deletes OpenAI-Beta from the socket's connect headers.
    assert_eq!(beta, None, "{headers:?}");
    assert!(headers.iter().any(
        |(name, value)| name.eq_ignore_ascii_case("chatgpt-account-id") && value == "acct_123"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_websocket_that_closes_early_falls_back_to_sse() {
    // The server closes before any event: nothing was emitted, so the adapter records
    // the failure and falls back to SSE. Had an event already gone out, TS would rethrow
    // instead of falling back.
    let (base_url, _, _) = spawn_codex_websocket_server(Vec::new(), true).await;

    let seen = Arc::new(Mutex::new(None));
    let events = stream(
        codex_model(&base_url),
        case_named("minimal").context,
        ProviderRequestOptions {
            api_key: Some(token()),
            max_retries: Some(0),
            fetch: Some(Arc::new(CannedFetch {
                status: 200,
                body: "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_sse\",\"status\":\"completed\"}}\n\ndata: [DONE]\n\n".to_string(),
                seen: seen.clone(),
            })),
            ..ProviderRequestOptions::default()
        },
        OpenAICodexResponsesOptions {
            transport: Some(Transport::Auto),
            session_id: Some("ws-fallback-session".to_string()),
            ..OpenAICodexResponsesOptions::default()
        },
    );
    let mut collected = Vec::new();
    while let Some(event) = events.next().await {
        collected.push(event);
    }

    // The SSE attempt succeeded, so the stream finishes.
    let last = collected.last().expect("event");
    let AssistantMessageEvent::Done { message, .. } = last else {
        panic!("expected a done event, got {last:?}");
    };
    assert_eq!(message.response_id.as_deref(), Some("resp_sse"));
    // The fallback is recorded as a diagnostic on the message.
    let diagnostics = message.diagnostics.as_ref().expect("diagnostics");
    assert_eq!(diagnostics[0].r#type, "provider_transport_failure");
    let details = diagnostics[0].details.as_ref().expect("details");
    assert_eq!(details["configuredTransport"], "auto");
    assert_eq!(details["fallbackTransport"], "sse");
    assert_eq!(details["eventsEmitted"], false);
    assert_eq!(details["phase"], "before_message_stream_start");
    // The SSE request really went out.
    assert!(seen.lock().expect("poisoned").is_some());

    // The session is now pinned to SSE, and the stats say so.
    let stats = notagent_ai::api::openai_codex_responses::openai_codex_websocket_debug_stats(
        "ws-fallback-session",
    )
    .expect("stats");
    assert_eq!(stats.websocket_failures, 1);
    assert_eq!(stats.sse_fallbacks, 1);
    assert_eq!(stats.websocket_fallback_active, Some(true));
    notagent_ai::api::openai_codex_responses::reset_openai_codex_websocket_debug_stats(Some(
        "ws-fallback-session",
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_error_event_over_the_websocket_is_not_retried_over_sse() {
    let (base_url, _, _) = spawn_codex_websocket_server(
        vec![
            serde_json::json!({ "type": "error", "code": "invalid_request", "message": "nope" })
                .to_string(),
        ],
        true,
    )
    .await;

    let seen = Arc::new(Mutex::new(None));
    let events = stream(
        codex_model(&base_url),
        case_named("minimal").context,
        ProviderRequestOptions {
            api_key: Some(token()),
            fetch: Some(Arc::new(CannedFetch {
                status: 200,
                body: String::new(),
                seen: seen.clone(),
            })),
            ..ProviderRequestOptions::default()
        },
        OpenAICodexResponsesOptions {
            transport: Some(Transport::Websocket),
            ..OpenAICodexResponsesOptions::default()
        },
    );
    let mut collected = Vec::new();
    while let Some(event) = events.next().await {
        collected.push(event);
    }
    let last = collected.last().expect("event");
    let AssistantMessageEvent::Error { error, .. } = last else {
        panic!("expected an error event, got {last:?}");
    };
    assert_eq!(error.error_message.as_deref(), Some("Codex error: nope"));
    // A CodexApiError is not a transport failure, so SSE is never tried.
    assert!(seen.lock().expect("poisoned").is_none());
}

/// A WebSocket server that answers every `response.create` frame on every connection
/// with a scripted text response; frames are recorded as (connection index, frame).
async fn spawn_codex_websocket_reuse_server() -> (String, Arc<Mutex<Vec<(usize, String)>>>) {
    use futures::{SinkExt, StreamExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let frames = Arc::new(Mutex::new(Vec::new()));
    let recorded_frames = frames.clone();

    tokio::spawn(async move {
        let mut connection_index = 0usize;
        loop {
            let Ok((connection, _)) = listener.accept().await else {
                return;
            };
            let index = connection_index;
            connection_index += 1;
            let recorded_frames = recorded_frames.clone();
            tokio::spawn(async move {
                let Ok(mut socket) = tokio_tungstenite::accept_async(connection).await else {
                    return;
                };
                let mut response_index = 0usize;
                while let Some(Ok(message)) = socket.next().await {
                    let Ok(text) = message.to_text() else {
                        continue;
                    };
                    if text.is_empty() {
                        continue;
                    }
                    recorded_frames
                        .lock()
                        .expect("poisoned")
                        .push((index, text.to_string()));
                    response_index += 1;
                    let response_id = format!("resp_c{index}_{response_index}");
                    let events = vec![
                        serde_json::json!({
                            "type": "response.output_item.added", "output_index": 0,
                            "item": { "type": "message", "id": "msg_1", "role": "assistant", "content": [], "status": "in_progress" }
                        }),
                        serde_json::json!({ "type": "response.output_text.delta", "output_index": 0, "delta": "hello" }),
                        serde_json::json!({
                            "type": "response.output_item.done", "output_index": 0,
                            "item": { "type": "message", "id": "msg_1", "role": "assistant", "status": "completed",
                                      "content": [{ "type": "output_text", "text": "hello", "annotations": [] }] }
                        }),
                        serde_json::json!({
                            "type": "response.done",
                            "response": { "id": response_id, "status": "completed" }
                        }),
                    ];
                    for event in events {
                        if socket
                            .send(tokio_tungstenite::tungstenite::Message::Text(
                                event.to_string().into(),
                            ))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            });
        }
    });

    (format!("http://127.0.0.1:{port}"), frames)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_follow_up_request_reuses_the_websocket_and_continues_via_previous_response_id() {
    let (base_url, frames) = spawn_codex_websocket_reuse_server().await;
    let session = "ws-reuse-session";
    let request = || ProviderRequestOptions {
        api_key: Some(token()),
        ..ProviderRequestOptions::default()
    };
    let options = || OpenAICodexResponsesOptions {
        transport: Some(Transport::Auto),
        session_id: Some(session.to_string()),
        ..OpenAICodexResponsesOptions::default()
    };

    let context = case_named("minimal").context;
    let events = stream(codex_model(&base_url), context.clone(), request(), options());
    let mut assistant = None;
    while let Some(event) = events.next().await {
        if let AssistantMessageEvent::Done { message, .. } = event {
            assistant = Some(message);
        }
    }
    let assistant = assistant.expect("first response");
    assert_eq!(assistant.response_id.as_deref(), Some("resp_c0_1"));

    // The follow-up extends the transcript with the reply and a new user message.
    let mut follow_up = context.clone();
    follow_up.messages.push(Message::Assistant(assistant));
    follow_up.messages.push(Message::User(UserMessage {
        content: UserContent::Text("again".to_string()),
        timestamp: 2,
    }));
    let events = stream(codex_model(&base_url), follow_up, request(), options());
    let mut second = None;
    while let Some(event) = events.next().await {
        if let AssistantMessageEvent::Done { message, .. } = event {
            second = Some(message);
        }
    }
    let second = second.expect("second response");
    // `resp_c0_2`: connection 0 answered both requests — the socket was reused.
    // Codex keeps `previous_response_id` state per connection, so a delta over a
    // fresh connection would be rejected with "Invalid `previous_response_id`".
    assert_eq!(second.response_id.as_deref(), Some("resp_c0_2"));

    let frames = frames.lock().expect("poisoned");
    assert_eq!(
        frames.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        vec![0, 0],
        "both requests must ride the same connection"
    );
    let first_frame: Value = serde_json::from_str(&frames[0].1).expect("json");
    assert_eq!(first_frame.get("previous_response_id"), None);
    let second_frame: Value = serde_json::from_str(&frames[1].1).expect("json");
    assert_eq!(second_frame["previous_response_id"], "resp_c0_1");
    // The delta carries only the new user message, not the replayed transcript.
    assert_eq!(second_frame["input"].as_array().expect("input").len(), 1);
    assert_eq!(
        second_frame["input"][0]["content"][0]["text"],
        "again",
        "{second_frame}"
    );

    let stats =
        notagent_ai::api::openai_codex_responses::openai_codex_websocket_debug_stats(session)
            .expect("stats");
    assert_eq!(stats.connections_created, 1);
    assert_eq!(stats.connections_reused, 1);
    assert_eq!(stats.full_context_requests, 1);
    assert_eq!(stats.delta_requests, 1);
    assert_eq!(stats.last_previous_response_id.as_deref(), Some("resp_c0_1"));

    notagent_ai::api::openai_codex_responses::close_openai_codex_websocket_sessions(Some(session));
    notagent_ai::api::openai_codex_responses::reset_openai_codex_websocket_debug_stats(Some(
        session,
    ));
}
