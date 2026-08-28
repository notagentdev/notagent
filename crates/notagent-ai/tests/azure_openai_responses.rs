use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use notagent_ai::api::azure_openai_responses::{
    AzureOpenAIResponsesOptions, build_client_headers, build_params, build_request_url,
    normalize_azure_base_url, parse_deployment_name_map, resolve_azure_config,
    resolve_deployment_name, stream,
};
use notagent_ai::api::constrained_sampling::create_grammar_tool_input_properties;
use notagent_ai::types::{
    AssistantMessageEvent, Context, Model, ProviderEnv, ProviderHeaders, ProviderRequestOptions,
    ThinkingLevel,
};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::Value;

fn thinking_level(value: &str) -> ThinkingLevel {
    match value {
        "minimal" => ThinkingLevel::Minimal,
        "low" => ThinkingLevel::Low,
        "medium" => ThinkingLevel::Medium,
        "high" => ThinkingLevel::High,
        "xhigh" => ThinkingLevel::Xhigh,
        "max" => ThinkingLevel::Max,
        other => panic!("unknown thinking level {other}"),
    }
}

fn options_from_fixture(raw: &Value) -> AzureOpenAIResponsesOptions {
    let raw = raw.as_object().cloned().unwrap_or_default();
    AzureOpenAIResponsesOptions {
        reasoning_effort: raw
            .get("reasoningEffort")
            .and_then(Value::as_str)
            .map(thinking_level),
        reasoning_summary: raw
            .get("reasoningSummary")
            .map(|value| value.as_str().map(str::to_string)),
        azure_api_version: raw
            .get("azureApiVersion")
            .and_then(Value::as_str)
            .map(str::to_string),
        azure_resource_name: raw
            .get("azureResourceName")
            .and_then(Value::as_str)
            .map(str::to_string),
        azure_base_url: raw
            .get("azureBaseUrl")
            .and_then(Value::as_str)
            .map(str::to_string),
        azure_deployment_name: raw
            .get("azureDeploymentName")
            .and_then(Value::as_str)
            .map(str::to_string),
        max_tokens: raw.get("maxTokens").and_then(Value::as_u64),
        temperature: raw.get("temperature").and_then(Value::as_f64),
        sampling_params: raw
            .get("samplingParams")
            .and_then(Value::as_object)
            .cloned(),
        session_id: raw
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        env: raw.get("env").and_then(Value::as_object).map(|env| {
            env.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect::<ProviderEnv>()
        }),
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
    options: AzureOpenAIResponsesOptions,
    request_headers: Option<ProviderHeaders>,
    payload: Value,
    request_url: Option<String>,
    sent_headers: BTreeMap<String, String>,
}

fn cases() -> Vec<Case> {
    include_str!("fixtures/azure-openai-responses.jsonl")
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
                name,
            }
        })
        .collect()
}

const TIMESTAMP: i64 = 1_700_000_000_000;

fn build(case: &Case) -> Value {
    let deployment_name = resolve_deployment_name(&case.model, &case.options);
    let supports_grammar_tools = case
        .model
        .compat
        .as_ref()
        .and_then(|compat| match compat {
            notagent_ai::types::ModelCompat::OpenAIResponses(compat) => {
                compat.supports_openai_grammar_tools
            }
            _ => None,
        })
        .unwrap_or(false);
    let grammar_tool_input_properties =
        create_grammar_tool_input_properties(case.context.tools.as_deref(), supports_grammar_tools)
            .unwrap_or_else(|error| panic!("{}: grammar properties: {error}", case.name));
    build_params(
        &case.model,
        &case.context,
        &case.options,
        &deployment_name,
        &grammar_tool_input_properties,
        TIMESTAMP,
    )
    .unwrap_or_else(|error| panic!("{}: build_params: {error}", case.name))
}

#[test]
fn every_captured_payload_is_reproduced_byte_for_byte() {
    let cases = cases();
    assert!(cases.len() >= 30, "expected the full fixture set");
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
        "{} of {} payloads differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

#[test]
fn every_captured_request_url_is_reproduced() {
    let mut failures = Vec::new();
    for case in cases() {
        let Some(expected) = case.request_url.clone() else {
            continue;
        };
        let (base_url, api_version) = resolve_azure_config(&case.model, &case.options)
            .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        let actual = build_request_url(&base_url, &api_version)
            .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        if actual != expected {
            failures.push(format!(
                "{}\n  expected: {expected}\n  actual:   {actual}",
                case.name
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn case_named(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("fixture {name} is missing"))
}

#[test]
fn azure_hosts_are_normalized_to_the_openai_v1_base_path() {
    for (name, expected) in [
        (
            "url-cognitive-root",
            "https://marc-quicktests-resource.cognitiveservices.azure.com/openai/v1",
        ),
        (
            "url-foundry-root",
            "https://marc-quicktests-resource.ai.azure.com/openai/v1",
        ),
        ("minimal", "https://my-resource.openai.azure.com/openai/v1"),
        (
            "url-openai-path",
            "https://my-resource.cognitiveservices.azure.com/openai/v1",
        ),
        (
            "url-openai-v1-path",
            "https://my-resource.cognitiveservices.azure.com/openai/v1",
        ),
        (
            "url-openai-v1-responses",
            "https://my-resource.services.ai.azure.com/openai/v1",
        ),
        (
            "url-trailing-slashes",
            "https://my-resource.openai.azure.com/openai/v1",
        ),
        // A non-Azure proxy keeps its path and its query.
        ("url-non-azure-proxy", "https://my-proxy.example.com/v1"),
        (
            "url-azure-query-stripped",
            "https://my-resource.openai.azure.com/openai/v1",
        ),
        (
            "url-non-azure-query-kept",
            "https://my-proxy.example.com/v1?custom=true",
        ),
    ] {
        let case = case_named(name);
        let (base_url, _) = resolve_azure_config(&case.model, &case.options).expect("config");
        assert_eq!(base_url, expected, "{name}");
    }
}

#[test]
fn an_unparsable_base_url_is_rejected_with_its_own_message() {
    let error = normalize_azure_base_url("not-a-url").expect_err("invalid");
    assert_eq!(
        error.to_string(),
        "Invalid Azure OpenAI base URL: not-a-url"
    );
}

#[test]
fn a_base_url_carrying_a_query_swallows_the_path_like_the_sdk_does() {
    // Bug-compat: the SDK concatenates baseURL and path as strings before parsing.
    assert_eq!(
        build_request_url("https://my-proxy.example.com/v1?custom=true", "v1").expect("url"),
        "https://my-proxy.example.com/v1?custom=true%2Fresponses&api-version=v1"
    );
}

#[test]
fn the_base_url_is_resolved_from_the_first_available_source() {
    for (name, expected) in [
        (
            "resource-name",
            "https://my-resource.openai.azure.com/openai/v1",
        ),
        (
            "resource-name-env",
            "https://env-resource.openai.azure.com/openai/v1",
        ),
        (
            "base-url-env",
            "https://env-resource.openai.azure.com/openai/v1",
        ),
        (
            "model-base-url",
            "https://model-resource.openai.azure.com/openai/v1",
        ),
    ] {
        let case = case_named(name);
        let (base_url, _) = resolve_azure_config(&case.model, &case.options).expect("config");
        assert_eq!(base_url, expected, "{name}");
    }

    let mut case = case_named("minimal");
    case.options.azure_base_url = None;
    case.model.base_url = String::new();
    let error = resolve_azure_config(&case.model, &case.options).expect_err("no base url");
    assert!(
        error
            .to_string()
            .starts_with("Azure OpenAI base URL is required."),
        "{error}"
    );
}

#[test]
fn the_api_version_falls_back_from_option_to_env_to_the_default() {
    let (_, version) = resolve_azure_config(
        &case_named("api-version").model,
        &case_named("api-version").options,
    )
    .expect("config");
    assert_eq!(version, "2024-12-01");

    let case = case_named("api-version-env");
    let (_, version) = resolve_azure_config(&case.model, &case.options).expect("config");
    assert_eq!(version, "2025-01-01");

    let case = case_named("minimal");
    let (_, version) = resolve_azure_config(&case.model, &case.options).expect("config");
    assert_eq!(version, "v1");
}

#[test]
fn the_deployment_name_prefers_the_option_then_the_map_then_the_model_id() {
    assert_eq!(
        resolve_deployment_name(
            &case_named("deployment-name-option").model,
            &case_named("deployment-name-option").options
        ),
        "my-deployment"
    );
    assert_eq!(
        resolve_deployment_name(
            &case_named("deployment-name-map").model,
            &case_named("deployment-name-map").options
        ),
        "mapped-deployment"
    );
    assert_eq!(
        resolve_deployment_name(
            &case_named("deployment-name-map-miss").model,
            &case_named("deployment-name-map-miss").options
        ),
        "gpt-4o-mini"
    );
    // The payload carries the deployment name as `model`.
    assert_eq!(
        build(&case_named("deployment-name-map"))["model"],
        "mapped-deployment"
    );
}

#[test]
fn the_deployment_name_map_drops_malformed_entries() {
    let map = parse_deployment_name_map(Some(" a=b , ,c , =d, e= , f=g=h "));
    assert_eq!(map.get("a").map(String::as_str), Some("b"));
    assert_eq!(map.get("c"), None);
    assert_eq!(map.get(""), None);
    assert_eq!(map.get("e"), None);
    // `split("=", 2)` keeps only the first two parts.
    assert_eq!(map.get("f").map(String::as_str), Some("g"));
    assert!(parse_deployment_name_map(None).is_empty());
}

#[test]
fn model_and_request_headers_are_merged() {
    let case = case_named("headers");
    let headers = build_client_headers(&case.model, case.request_headers.as_ref());
    assert_eq!(
        headers.get("x-model").cloned().flatten().as_deref(),
        Some("from-model")
    );
    assert_eq!(
        headers.get("x-request").cloned().flatten().as_deref(),
        Some("from-request")
    );
    assert_eq!(
        case.sent_headers.get("x-model").map(String::as_str),
        Some("from-model")
    );
    assert_eq!(
        case.sent_headers.get("x-request").map(String::as_str),
        Some("from-request")
    );
}

/// Records the request and answers with a complete stream.
struct RecordingFetch {
    seen: Arc<Mutex<Option<FetchRequest>>>,
}

impl FetchFn for RecordingFetch {
    fn fetch(
        &self,
        request: FetchRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FetchError>> + Send>,
    > {
        *self.seen.lock().expect("poisoned") = Some(request);
        Box::pin(async move {
            Ok(FetchResponse {
                status: 200,
                status_text: String::new(),
                headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
                body: FetchBody::Bytes(
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\ndata: [DONE]\n\n"
                        .as_bytes()
                        .to_vec(),
                ),
            })
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_request_authenticates_with_the_api_key_header() {
    let case = case_named("minimal");
    let seen = Arc::new(Mutex::new(None));
    let events = stream(
        case.model.clone(),
        case.context.clone(),
        ProviderRequestOptions {
            api_key: Some("k".to_string()),
            max_retries: Some(0),
            fetch: Some(Arc::new(RecordingFetch { seen: seen.clone() })),
            ..ProviderRequestOptions::default()
        },
        case.options.clone(),
    );
    let mut last = None;
    while let Some(event) = events.next().await {
        last = Some(event);
    }
    assert!(matches!(last, Some(AssistantMessageEvent::Done { .. })));

    let seen = seen.lock().expect("poisoned");
    let request = seen.as_ref().expect("request");
    assert_eq!(
        request.url,
        "https://my-resource.openai.azure.com/openai/v1/responses?api-version=v1"
    );
    // Azure authenticates with `api-key`, never with a bearer token.
    assert!(
        request
            .headers
            .iter()
            .any(|(key, value)| key == "api-key" && value == "k")
    );
    assert!(
        !request
            .headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("authorization"))
    );
    // The SDK sends `accept: application/json` even for a streaming request.
    assert_eq!(
        case.sent_headers.get("accept").map(String::as_str),
        Some("application/json")
    );
    assert!(
        request
            .headers
            .iter()
            .any(|(key, value)| key == "accept" && value == "application/json")
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
            max_retries: Some(0),
            fetch: Some(Arc::new(RecordingFetch {
                seen: Arc::new(Mutex::new(None)),
            })),
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
        Some("No API key for provider: azure-openai-responses")
    );
    // The api field is pinned to the literal, independent of `model.api`.
    assert_eq!(error.api, "azure-openai-responses");
}
