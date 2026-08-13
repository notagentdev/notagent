//! Azure OpenAI Responses API.
//!
//! 1:1 port of `packages/ai/src/api/azure-openai-responses.ts`. The message, tool and
//! stream handling come from [`crate::api::openai_responses_shared`]; what is specific
//! here is the deployment-name resolution, the base-URL normalization and the `api-key`
//! authentication the `AzureOpenAI` client performs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Map, Value, json};
use url::Url;

use crate::api::constrained_sampling::create_grammar_tool_input_properties;
use crate::api::openai_prompt_cache::clamp_openai_prompt_cache_key;
use crate::api::openai_responses_shared::{
    ConvertResponsesMessagesOptions, ConvertResponsesToolsOptions, ResponsesStreamOptions,
    ResponsesStreamState, convert_responses_messages, convert_responses_tools,
};
use crate::api::simple_options::build_base_options;
use crate::api::sse::SseDecoder;
use crate::models::clamp_thinking_level;
use crate::types::{
    AssistantMessage, AssistantMessageEvent, Context, DoneReason, ErrorReason, Model,
    ModelThinkingLevel, ProviderEnv, ProviderHeaders, ProviderRequestOptions, SimpleStreamOptions,
    StopReason, ThinkingLevel, Usage,
};
use crate::utils::error_body::{RawProviderError, format_provider_error, normalize_provider_error};
use crate::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use crate::utils::fetch::{FetchBody, FetchFunction, FetchRequest, ReqwestFetch};
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::provider_retry::{
    ProviderErrorInfo, ProviderRetryError, ProviderRetryOptions, retry_provider_request,
};

pub const DEFAULT_AZURE_API_VERSION: &str = "v1";

/// `AZURE_TOOL_CALL_PROVIDERS`
pub fn azure_tool_call_providers() -> BTreeSet<String> {
    [
        "openai",
        "openai-codex",
        "opencode",
        "azure-openai-responses",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Responses rejects `max_output_tokens` below 16 (notagentdev/notagent#6265).
pub const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS: u64 = 16;

/// Error of this provider.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AzureOpenAIError(pub String);

/// `AzureOpenAIResponsesOptions extends StreamOptions`
#[derive(Debug, Clone, Default)]
pub struct AzureOpenAIResponsesOptions {
    pub reasoning_effort: Option<ThinkingLevel>,
    /// `"auto" | "detailed" | "concise" | null`; `Some(None)` is an explicit `null`.
    pub reasoning_summary: Option<Option<String>>,
    pub azure_api_version: Option<String>,
    pub azure_resource_name: Option<String>,
    pub azure_base_url: Option<String>,
    pub azure_deployment_name: Option<String>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub sampling_params: Option<Map<String, Value>>,
    pub session_id: Option<String>,
    pub env: Option<ProviderEnv>,
}

/// `parseDeploymentNameMap(value)` — `modelId=deployment` pairs, comma separated.
pub fn parse_deployment_name_map(value: Option<&str>) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Some(value) = value else {
        return map;
    };
    for entry in value.split(',') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        // `split("=", 2)` in JS keeps only the first two parts and drops the rest.
        let mut parts = trimmed.split('=');
        let (Some(model_id), Some(deployment_name)) = (parts.next(), parts.next()) else {
            continue;
        };
        if model_id.is_empty() || deployment_name.is_empty() {
            continue;
        }
        map.insert(
            model_id.trim().to_string(),
            deployment_name.trim().to_string(),
        );
    }
    map
}

/// `resolveDeploymentName(model, options)`
pub fn resolve_deployment_name(model: &Model, options: &AzureOpenAIResponsesOptions) -> String {
    if let Some(name) = options
        .azure_deployment_name
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        return name.to_string();
    }
    let mapped = parse_deployment_name_map(
        get_provider_env_value("AZURE_OPENAI_DEPLOYMENT_NAME_MAP", options.env.as_ref()).as_deref(),
    )
    .get(&model.id)
    .cloned();
    mapped
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| model.id.clone())
}

/// `normalizeAzureBaseUrl(baseUrl)`
pub fn normalize_azure_base_url(base_url: &str) -> Result<String, AzureOpenAIError> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let mut url = Url::parse(trimmed)
        .map_err(|_| AzureOpenAIError(format!("Invalid Azure OpenAI base URL: {base_url}")))?;

    let host = url.host_str().unwrap_or_default().to_string();
    let is_azure_host = host.ends_with(".openai.azure.com")
        || host.ends_with(".cognitiveservices.azure.com")
        || host.ends_with(".ai.azure.com");
    let normalized_path = url.path().trim_end_matches('/').to_string();

    // Azure hosts need /openai/v1 as the base path so the SDK can append
    // /deployments/<model>/... and ?api-version=v1 correctly.
    if is_azure_host
        && (normalized_path.is_empty()
            || normalized_path == "/"
            || normalized_path == "/openai"
            || normalized_path == "/openai/v1/responses")
    {
        url.set_path("/openai/v1");
        url.set_query(None);
    }

    Ok(url.to_string().trim_end_matches('/').to_string())
}

/// `buildURL(path, query)` of the SDK for `path = "/responses"`.
///
/// The base URL and the path are concatenated as plain strings before parsing, so a base
/// URL that carries a query swallows the path into that query — the SDK then re-encodes
/// it (`?custom=true%2Fresponses`). That quirk is part of the wire behaviour and is
/// reproduced here.
pub fn build_request_url(base_url: &str, api_version: &str) -> Result<String, AzureOpenAIError> {
    let mut url = Url::parse(&format!("{base_url}/responses"))
        .map_err(|_| AzureOpenAIError(format!("Invalid Azure OpenAI base URL: {base_url}")))?;
    let mut query: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .filter(|(key, _)| key != "api-version")
        .collect();
    query.push(("api-version".to_string(), api_version.to_string()));
    url.query_pairs_mut().clear().extend_pairs(query);
    Ok(url.to_string())
}

fn build_default_base_url(resource_name: &str) -> String {
    format!("https://{resource_name}.openai.azure.com/openai/v1")
}

/// `resolveAzureConfig(model, options)`
pub fn resolve_azure_config(
    model: &Model,
    options: &AzureOpenAIResponsesOptions,
) -> Result<(String, String), AzureOpenAIError> {
    let api_version = options
        .azure_api_version
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            get_provider_env_value("AZURE_OPENAI_API_VERSION", options.env.as_ref())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| DEFAULT_AZURE_API_VERSION.to_string());

    let mut resolved_base_url = options
        .azure_base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            get_provider_env_value("AZURE_OPENAI_BASE_URL", options.env.as_ref())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        });

    let resource_name = options
        .azure_resource_name
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            get_provider_env_value("AZURE_OPENAI_RESOURCE_NAME", options.env.as_ref())
                .filter(|value| !value.is_empty())
        });

    if resolved_base_url.is_none()
        && let Some(resource_name) = &resource_name
    {
        resolved_base_url = Some(build_default_base_url(resource_name));
    }
    if resolved_base_url.is_none() && !model.base_url.is_empty() {
        resolved_base_url = Some(model.base_url.clone());
    }
    let Some(resolved_base_url) = resolved_base_url else {
        return Err(AzureOpenAIError(
            "Azure OpenAI base URL is required. Set AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME, or pass azureBaseUrl, azureResourceName, or model.baseUrl."
                .to_string(),
        ));
    };

    Ok((normalize_azure_base_url(&resolved_base_url)?, api_version))
}

fn model_supports_grammar_tools(model: &Model) -> bool {
    match &model.compat {
        Some(crate::types::ModelCompat::OpenAIResponses(compat)) => {
            compat.supports_openai_grammar_tools.unwrap_or(false)
        }
        _ => false,
    }
}

fn model_supports_strict_mode(model: &Model) -> bool {
    match &model.compat {
        Some(crate::types::ModelCompat::OpenAIResponses(compat)) => {
            compat.supports_strict_mode.unwrap_or(true)
        }
        _ => true,
    }
}

fn effort_str(effort: ThinkingLevel) -> &'static str {
    match effort {
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

/// `buildParams(model, context, options, deploymentName, grammarToolInputProperties)`
pub fn build_params(
    model: &Model,
    context: &Context,
    options: &AzureOpenAIResponsesOptions,
    deployment_name: &str,
    grammar_tool_input_properties: &BTreeMap<String, String>,
    timestamp: i64,
) -> Result<Value, crate::api::constrained_sampling::ConstrainedSamplingError> {
    let messages = convert_responses_messages(
        model,
        context,
        &azure_tool_call_providers(),
        &ConvertResponsesMessagesOptions {
            grammar_tool_input_properties: Some(grammar_tool_input_properties),
            ..ConvertResponsesMessagesOptions::default()
        },
        timestamp,
    )?;

    let mut params = Map::new();
    params.insert("model".to_string(), json!(deployment_name));
    params.insert("input".to_string(), Value::Array(messages));
    params.insert("stream".to_string(), json!(true));
    if let Some(key) = clamp_openai_prompt_cache_key(options.session_id.as_deref()) {
        params.insert("prompt_cache_key".to_string(), json!(key));
    }
    params.insert("store".to_string(), json!(false));

    if let Some(max_tokens) = options.max_tokens.filter(|value| *value != 0) {
        params.insert(
            "max_output_tokens".to_string(),
            json!(max_tokens.max(OPENAI_RESPONSES_MIN_OUTPUT_TOKENS)),
        );
    }
    if let Some(temperature) = options.temperature {
        params.insert("temperature".to_string(), json!(temperature));
    }

    if let Some(tools) = context.tools.as_ref().filter(|tools| !tools.is_empty()) {
        params.insert(
            "tools".to_string(),
            Value::Array(convert_responses_tools(
                tools,
                &ConvertResponsesToolsOptions {
                    supports_strict_mode: Some(model_supports_strict_mode(model)),
                    supports_openai_grammar_tools: Some(model_supports_grammar_tools(model)),
                    ..ConvertResponsesToolsOptions::default()
                },
            )?),
        );
    }

    if model.reasoning {
        let has_summary = options
            .reasoning_summary
            .as_ref()
            .is_some_and(|summary| summary.as_deref().is_some_and(|value| !value.is_empty()));
        if options.reasoning_effort.is_some() || has_summary {
            let effort = match options.reasoning_effort {
                Some(effort) => model
                    .thinking_level_map
                    .as_ref()
                    .and_then(|map| map.get(&effort.into()))
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| effort_str(effort).to_string()),
                None => "medium".to_string(),
            };
            let summary = options
                .reasoning_summary
                .clone()
                .flatten()
                .filter(|summary| !summary.is_empty())
                .unwrap_or_else(|| "auto".to_string());
            params.insert(
                "reasoning".to_string(),
                json!({ "effort": effort, "summary": summary }),
            );
            params.insert(
                "include".to_string(),
                json!(["reasoning.encrypted_content"]),
            );
        } else if !matches!(
            model
                .thinking_level_map
                .as_ref()
                .and_then(|map| map.get(&ModelThinkingLevel::Off)),
            Some(None)
        ) {
            let effort = model
                .thinking_level_map
                .as_ref()
                .and_then(|map| map.get(&ModelThinkingLevel::Off))
                .cloned()
                .flatten()
                .unwrap_or_else(|| "none".to_string());
            params.insert("reasoning".to_string(), json!({ "effort": effort }));
        }
    }

    // Last, so custom keys override the named request fields.
    if let Some(sampling_params) = &options.sampling_params {
        for (key, value) in sampling_params {
            params.insert(key.clone(), value.clone());
        }
    }

    Ok(Value::Object(params))
}

/// The `defaultHeaders` half of `createClient(model, apiKey, options)`.
pub fn build_client_headers(
    model: &Model,
    options_headers: Option<&ProviderHeaders>,
) -> BTreeMap<String, Option<String>> {
    let mut headers: BTreeMap<String, Option<String>> = model
        .headers
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, Some(value)))
        .collect();
    if let Some(options_headers) = options_headers {
        for (key, value) in options_headers {
            headers.insert(key.clone(), value.clone());
        }
    }
    headers
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// The HTTP half of an `APIError`; boxed so the error itself stays small.
#[derive(Debug, Clone)]
pub struct AzureApiErrorResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct AzureTransportError {
    pub message: String,
    pub response: Option<Box<AzureApiErrorResponse>>,
}

impl AzureTransportError {
    fn message(message: impl Into<String>) -> Self {
        AzureTransportError {
            message: message.into(),
            response: None,
        }
    }
}

impl ProviderErrorInfo for AzureTransportError {
    fn status(&self) -> Option<u16> {
        self.response.as_ref().map(|response| response.status)
    }

    fn header(&self, name: &str) -> Option<String> {
        self.response.as_ref().and_then(|response| {
            response
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
        })
    }

    fn message(&self) -> String {
        self.message.clone()
    }
}

/// `formatAzureOpenAIError(error)`
fn format_azure_openai_error(error: &AzureTransportError) -> String {
    format_provider_error(
        &normalize_provider_error(&RawProviderError {
            status: error.response.as_ref().map(|response| response.status),
            body_text: None,
            body_json: error
                .response
                .as_ref()
                .and_then(|response| response.body.clone()),
            message: error.message.clone(),
        }),
        Some("Azure OpenAI API error"),
    )
}

fn empty_output(model: &Model, timestamp: i64) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        // TS pins the api to the literal, independent of `model.api`.
        api: "azure-openai-responses".to_string(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage {
            total_tokens: Some(0),
            ..Usage::default()
        },
        stop_reason: StopReason::Pending,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp,
    }
}

/// `stream(model, context, options)`
pub fn stream(
    model: Model,
    context: Context,
    request: ProviderRequestOptions,
    options: AzureOpenAIResponsesOptions,
) -> AssistantMessageEventStream {
    let outer = create_assistant_message_event_stream();
    let stream = outer.clone();

    tokio::spawn(async move {
        let timestamp = crate::auth::resolve::now_ms();
        let mut state = ResponsesStreamState::new(empty_output(&model, timestamp), &model);

        let outcome = run_request(
            &model, &context, &request, &options, &mut state, &stream, timestamp,
        )
        .await;

        match outcome {
            Ok(reason) => {
                stream.push(AssistantMessageEvent::Done {
                    reason,
                    message: state.output.clone(),
                });
                stream.end(Some(state.output));
            }
            Err(error) => {
                let aborted = request
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.is_cancelled());
                state.output.stop_reason = if aborted {
                    StopReason::Aborted
                } else {
                    StopReason::Error
                };
                state.output.error_message = Some(format_azure_openai_error(&error));
                stream.push(AssistantMessageEvent::Error {
                    reason: if aborted {
                        ErrorReason::Aborted
                    } else {
                        ErrorReason::Error
                    },
                    error: state.output.clone(),
                });
                stream.end(Some(state.output));
            }
        }
    });

    outer
}

async fn run_request(
    model: &Model,
    context: &Context,
    request: &ProviderRequestOptions,
    options: &AzureOpenAIResponsesOptions,
    state: &mut ResponsesStreamState,
    stream: &AssistantMessageEventStream,
    timestamp: i64,
) -> Result<DoneReason, AzureTransportError> {
    let deployment_name = resolve_deployment_name(model, options);
    let Some(api_key) = request.api_key.as_deref().filter(|key| !key.is_empty()) else {
        return Err(AzureTransportError::message(format!(
            "No API key for provider: {}",
            model.provider
        )));
    };

    let (base_url, api_version) = resolve_azure_config(model, options)
        .map_err(|error| AzureTransportError::message(error.to_string()))?;

    let grammar_tool_input_properties = create_grammar_tool_input_properties(
        context.tools.as_deref(),
        model_supports_grammar_tools(model),
    )
    .map_err(|error| AzureTransportError::message(error.to_string()))?;

    let mut params = build_params(
        model,
        context,
        options,
        &deployment_name,
        &grammar_tool_input_properties,
        timestamp,
    )
    .map_err(|error| AzureTransportError::message(error.to_string()))?;
    if let Some(on_payload) = &request.on_payload
        && let Some(replacement) = on_payload(params.clone(), model).await
    {
        params = replacement;
    }

    let headers = build_client_headers(model, request.headers.as_ref());
    let mut request_headers: Vec<(String, String)> = headers
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .collect();
    request_headers.push(("content-type".to_string(), "application/json".to_string()));
    // The SDK sends `accept: application/json` even for streaming requests.
    request_headers.push(("accept".to_string(), "application/json".to_string()));
    // The AzureOpenAI client authenticates with `api-key`, not a bearer token.
    if !request_headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("api-key"))
    {
        request_headers.push(("api-key".to_string(), api_key.to_string()));
    }

    let fetch: FetchFunction = request
        .fetch
        .clone()
        .unwrap_or_else(|| Arc::new(ReqwestFetch::default()));
    // `/responses` is not one of the SDK's deployment endpoints, so no `/deployments/…`
    // segment is inserted; the api-version rides along as a default query parameter.
    let url = build_request_url(&base_url, &api_version)
        .map_err(|error| AzureTransportError::message(error.to_string()))?;
    let body = serde_json::to_vec(&params)
        .map_err(|error| AzureTransportError::message(error.to_string()))?;

    let response = retry_provider_request(
        || {
            let fetch = fetch.clone();
            let url = url.clone();
            let request_headers = request_headers.clone();
            let body = body.clone();
            async move {
                let response = fetch
                    .fetch(FetchRequest {
                        method: "POST".to_string(),
                        url,
                        headers: request_headers,
                        body: Some(body),
                    })
                    .await
                    .map_err(|error| AzureTransportError::message(error.to_string()))?;
                if response.status < 200 || response.status >= 300 {
                    let status = response.status;
                    let headers = response.headers.clone();
                    let body = read_body(response.body).await;
                    return Err(api_error(status, headers, &body));
                }
                Ok(response)
            }
        },
        ProviderRetryOptions {
            max_retries: request.max_retries,
            max_retry_delay_ms: request.max_retry_delay_ms,
            signal: request.signal.clone(),
        },
    )
    .await
    .map_err(|error| match error {
        ProviderRetryError::Request(error) => error,
        ProviderRetryError::RetryDelayTooLong(message) => AzureTransportError::message(message),
        ProviderRetryError::Aborted => AzureTransportError::message("Request aborted"),
    })?;

    if let Some(on_response) = &request.on_response {
        on_response(
            crate::types::ProviderResponse {
                status: response.status,
                headers: response.headers.iter().cloned().collect(),
            },
            model,
        )
        .await;
    }

    stream.push(AssistantMessageEvent::Start {
        partial: state.output.clone(),
    });

    let stream_options = ResponsesStreamOptions {
        grammar_tool_input_properties: Some(&grammar_tool_input_properties),
        ..ResponsesStreamOptions::default()
    };

    let mut decoder = SseDecoder::new();
    let pump = |events: Vec<crate::api::sse::ServerSentEvent>,
                state: &mut ResponsesStreamState|
     -> Result<(), AzureTransportError> {
        for event in events {
            if event.data.starts_with("[DONE]") || event.data.trim().is_empty() {
                continue;
            }
            let parsed = serde_json::from_str::<Value>(&event.data)
                .map_err(|error| AzureTransportError::message(error.to_string()))?;
            for emitted in state
                .process_event(&parsed, &stream_options)
                .map_err(|error| AzureTransportError::message(error.to_string()))?
            {
                stream.push(emitted);
            }
        }
        Ok(())
    };

    match response.body {
        FetchBody::Bytes(bytes) => {
            let text = String::from_utf8_lossy(&bytes).to_string();
            let events: Vec<_> = decoder
                .feed(&text)
                .into_iter()
                .chain(decoder.finish())
                .collect();
            pump(events, state)?;
        }
        FetchBody::Stream(mut body) => {
            while let Some(chunk) = body.recv().await {
                if request
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.is_cancelled())
                {
                    return Err(AzureTransportError::message("Request was aborted"));
                }
                let chunk =
                    chunk.map_err(|error| AzureTransportError::message(error.to_string()))?;
                let text = String::from_utf8_lossy(&chunk).to_string();
                pump(decoder.feed(&text), state)?;
            }
            pump(decoder.finish(), state)?;
        }
    }

    if !state.saw_terminal_response_event() {
        return Err(AzureTransportError::message(
            "OpenAI Responses stream ended before a terminal response event",
        ));
    }
    if request
        .signal
        .as_ref()
        .is_some_and(|signal| signal.is_cancelled())
    {
        return Err(AzureTransportError::message("Request was aborted"));
    }
    if state.output.stop_reason == StopReason::Pending {
        return Err(AzureTransportError::message(
            "Azure OpenAI Responses stream ended without a stop reason",
        ));
    }
    if state.output.stop_reason == StopReason::Aborted
        || state.output.stop_reason == StopReason::Error
    {
        return Err(AzureTransportError::message(
            state
                .output
                .error_message
                .clone()
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| "An unknown error occurred".to_string()),
        ));
    }
    Ok(match state.output.stop_reason {
        StopReason::Length => DoneReason::Length,
        StopReason::ToolUse => DoneReason::ToolUse,
        StopReason::Deferred => DoneReason::Deferred,
        _ => DoneReason::Stop,
    })
}

/// The openai SDK's `APIError.generate(status, errJSON, errText, headers)`.
fn api_error(status: u16, headers: Vec<(String, String)>, body: &str) -> AzureTransportError {
    let err_json = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|body| body.get("error").cloned());
    let message = match err_json.as_ref().filter(|json| !json.is_null()) {
        Some(json) => match json.get("message").filter(|message| !message.is_null()) {
            Some(Value::String(message)) if !message.is_empty() => message.clone(),
            Some(Value::String(_)) | None => json.to_string(),
            Some(message) => message.to_string(),
        },
        None => body.to_string(),
    };
    let message = if message.is_empty() {
        format!("{status} status code (no body)")
    } else {
        format!("{status} {message}")
    };
    AzureTransportError {
        message,
        response: Some(Box::new(AzureApiErrorResponse {
            status,
            headers,
            body: err_json,
        })),
    }
}

async fn read_body(body: FetchBody) -> String {
    match body {
        FetchBody::Bytes(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        FetchBody::Stream(mut receiver) => {
            let mut collected = Vec::new();
            while let Some(Ok(chunk)) = receiver.recv().await {
                collected.extend(chunk);
            }
            String::from_utf8_lossy(&collected).to_string()
        }
    }
}

/// `streamSimple(model, context, options)` — throws without an api key, like TS.
pub fn stream_simple(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
) -> Result<AssistantMessageEventStream, AzureOpenAIError> {
    let options = options.unwrap_or_default();
    let Some(api_key) = options
        .base
        .base
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
    else {
        return Err(AzureOpenAIError(format!(
            "No API key for provider: {}",
            model.provider
        )));
    };

    let base = build_base_options(&model, &context, Some(&options), Some(api_key.to_string()));
    let clamped_reasoning = options
        .reasoning
        .map(|reasoning| clamp_thinking_level(&model, reasoning.into()));
    let reasoning_effort = match clamped_reasoning {
        Some(ModelThinkingLevel::Minimal) => Some(ThinkingLevel::Minimal),
        Some(ModelThinkingLevel::Low) => Some(ThinkingLevel::Low),
        Some(ModelThinkingLevel::Medium) => Some(ThinkingLevel::Medium),
        Some(ModelThinkingLevel::High) => Some(ThinkingLevel::High),
        Some(ModelThinkingLevel::Xhigh) => Some(ThinkingLevel::Xhigh),
        Some(ModelThinkingLevel::Max) => Some(ThinkingLevel::Max),
        Some(ModelThinkingLevel::Off) | None => None,
    };

    Ok(stream(
        model,
        context,
        base.base.clone(),
        AzureOpenAIResponsesOptions {
            reasoning_effort,
            reasoning_summary: None,
            azure_api_version: None,
            azure_resource_name: None,
            azure_base_url: None,
            azure_deployment_name: None,
            max_tokens: base.max_tokens,
            temperature: base.temperature,
            sampling_params: base.sampling_params.clone(),
            session_id: base.session_id.clone(),
            env: base.base.env.clone(),
        },
    ))
}
