use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use url::Url;

use crate::api::google_generative_ai::{
    GoogleError, GoogleStreamState, is_gemini3_flash_model, is_gemini3_pro_model,
};
use crate::api::google_shared::{
    convert_messages, convert_tools, resolve_google_function_calling_mode,
    supports_google_strict_tool_sampling,
};
use crate::api::simple_options::build_base_options;
use crate::api::sse::SseDecoder;
use crate::models::clamp_thinking_level;
use crate::types::{
    AssistantMessageEvent, Context, DoneReason, ErrorReason, Model, ModelThinkingLevel,
    ProviderEnv, ProviderHeaders, ProviderRequestOptions, SimpleStreamOptions, StopReason,
    ThinkingBudgets, ThinkingLevel,
};
use crate::utils::error_body::{RawProviderError, format_provider_error, normalize_provider_error};
use crate::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use crate::utils::fetch::{FetchBody, FetchFunction, FetchRequest, ReqwestFetch};
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::provider_retry::{
    ProviderRetryError, ProviderRetryOptions, retry_provider_request,
};
use crate::utils::sanitize_unicode::sanitize_surrogates;

pub const API_VERSION: &str = "v1";
const GCP_VERTEX_CREDENTIALS_MARKER: &str = "gcp-vertex-credentials";
/// `MULTI_REGIONAL_LOCATIONS` of the SDK.
const MULTI_REGIONAL_LOCATIONS: [&str; 2] = ["us", "eu"];

/// `GoogleVertexOptions extends StreamOptions`
#[derive(Debug, Clone, Default)]
pub struct GoogleVertexOptions {
    /// `"auto" | "none" | "any"`
    pub tool_choice: Option<String>,
    pub thinking: Option<crate::api::google_generative_ai::GoogleThinkingOptions>,
    pub project: Option<String>,
    pub location: Option<String>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub api_key: Option<String>,
    pub env: Option<ProviderEnv>,
}

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

/// `isPlaceholderApiKey(apiKey)`
fn is_placeholder_api_key(api_key: &str) -> bool {
    api_key.starts_with('<')
        && api_key.ends_with('>')
        && !api_key[1..api_key.len() - 1].contains('>')
}

/// `resolveApiKey(options)` — the credentials marker and `<placeholders>` mean ADC.
pub fn resolve_api_key(api_key: Option<&str>) -> Option<String> {
    let api_key = api_key.map(str::trim).filter(|key| !key.is_empty())?;
    if api_key == GCP_VERTEX_CREDENTIALS_MARKER || is_placeholder_api_key(api_key) {
        return None;
    }
    Some(api_key.to_string())
}

/// `resolveProject(options)`
pub fn resolve_project(
    project: Option<&str>,
    env: Option<&ProviderEnv>,
) -> Result<String, GoogleError> {
    project
        .filter(|project| !project.is_empty())
        .map(str::to_string)
        .or_else(|| get_provider_env_value("GOOGLE_CLOUD_PROJECT", env).filter(|value| !value.is_empty()))
        .or_else(|| get_provider_env_value("GCLOUD_PROJECT", env).filter(|value| !value.is_empty()))
        .ok_or_else(|| {
            GoogleError::from_message(
                "Vertex AI requires a project ID. Set GOOGLE_CLOUD_PROJECT/GCLOUD_PROJECT or pass project in options.",
            )
        })
}

/// `resolveLocation(options)`
pub fn resolve_location(
    location: Option<&str>,
    env: Option<&ProviderEnv>,
) -> Result<String, GoogleError> {
    location
        .filter(|location| !location.is_empty())
        .map(str::to_string)
        .or_else(|| {
            get_provider_env_value("GOOGLE_CLOUD_LOCATION", env).filter(|value| !value.is_empty())
        })
        .ok_or_else(|| {
            GoogleError::from_message(
                "Vertex AI requires a location. Set GOOGLE_CLOUD_LOCATION or pass location in options.",
            )
        })
}

/// `resolveCustomBaseUrl(baseUrl)` — a `{location}` template is not a usable base URL.
pub fn resolve_custom_base_url(base_url: &str) -> Option<String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() || trimmed.contains("{location}") {
        return None;
    }
    Some(trimmed.to_string())
}

/// `baseUrlIncludesApiVersion(baseUrl)`
pub fn base_url_includes_api_version(base_url: &str) -> bool {
    let pattern = regex::Regex::new(r"^v\d+(?:beta\d*)?$").expect("valid pattern");
    match Url::parse(base_url) {
        Ok(url) => url.path().split('/').any(|part| pattern.is_match(part)),
        Err(_) => regex::Regex::new(r"(?:^|/)v\d+(?:beta\d*)?(?:/|$)")
            .expect("valid pattern")
            .is_match(base_url),
    }
}

/// How the request is authenticated, which also decides the URL shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VertexEndpoint {
    /// A Vertex API key: the global host, no project path.
    ApiKey { api_key: String },
    /// ADC with project and location.
    ProjectLocation { project: String, location: String },
    /// A custom base URL without project or API key: no project path either.
    CustomBaseUrl,
}

/// The URL the SDK builds for `models.generateContentStream`.
pub fn build_request_url(model: &Model, endpoint: &VertexEndpoint) -> Result<String, GoogleError> {
    let custom_base_url = resolve_custom_base_url(&model.base_url);
    // `httpOptions.baseUrlResourceScope = COLLECTION` suppresses the project path.
    let scope_is_collection = custom_base_url.is_some();
    let api_version = match &custom_base_url {
        Some(base_url) if base_url_includes_api_version(base_url) => String::new(),
        _ => API_VERSION.to_string(),
    };

    let host = match (&custom_base_url, endpoint) {
        (Some(base_url), _) => base_url.trim_end_matches('/').to_string(),
        (None, VertexEndpoint::ApiKey { .. }) => "https://aiplatform.googleapis.com".to_string(),
        (None, VertexEndpoint::ProjectLocation { location, .. }) if location == "global" => {
            "https://aiplatform.googleapis.com".to_string()
        }
        (None, VertexEndpoint::ProjectLocation { location, .. })
            if MULTI_REGIONAL_LOCATIONS.contains(&location.as_str()) =>
        {
            format!("https://aiplatform.{location}.rep.googleapis.com")
        }
        (None, VertexEndpoint::ProjectLocation { location, .. }) => {
            format!("https://{location}-aiplatform.googleapis.com")
        }
        (None, VertexEndpoint::CustomBaseUrl) => {
            return Err(GoogleError::from_message(
                "Authentication is not set up. Please provide either a project and location, or an API key, or a custom base URL.",
            ));
        }
    };

    let mut segments: Vec<String> = vec![host];
    if !api_version.is_empty() {
        segments.push(api_version);
    }
    // `shouldPrependVertexProjectPath`: never with an API key or a COLLECTION-scoped
    // custom base URL.
    if !scope_is_collection && let VertexEndpoint::ProjectLocation { project, location } = endpoint
    {
        segments.push(format!("projects/{project}/locations/{location}"));
    }
    segments.push(format!(
        "publishers/google/models/{}:streamGenerateContent?alt=sse",
        model.id
    ));
    Ok(segments.join("/"))
}

/// `buildGoogleAuthOptions(env)` — the key file ADC should use, when one is configured.
pub fn google_application_credentials(env: Option<&ProviderEnv>) -> Option<String> {
    get_provider_env_value("GOOGLE_APPLICATION_CREDENTIALS", env).filter(|value| !value.is_empty())
}

/// The `httpOptions.headers` of `buildHttpOptions`, merged model-first.
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
// Thinking configuration
// ---------------------------------------------------------------------------

/// `getDisabledThinkingConfig(model)` — like Gemini's, but without the Gemma branch.
fn disabled_thinking_config(model_id: &str) -> Value {
    if is_gemini3_pro_model(model_id) {
        return json!({ "thinkingLevel": "LOW" });
    }
    if is_gemini3_flash_model(model_id) {
        return json!({ "thinkingLevel": "MINIMAL" });
    }
    json!({ "thinkingBudget": 0 })
}

/// `getGemini3ThinkingLevel(effort, model)`
pub fn gemini3_thinking_level(effort: ThinkingLevel, model_id: &str) -> &'static str {
    if is_gemini3_pro_model(model_id) {
        return match effort {
            ThinkingLevel::Minimal | ThinkingLevel::Low => "LOW",
            _ => "HIGH",
        };
    }
    match effort {
        ThinkingLevel::Minimal => "MINIMAL",
        ThinkingLevel::Low => "LOW",
        ThinkingLevel::Medium => "MEDIUM",
        _ => "HIGH",
    }
}

/// `getGoogleBudget(model, effort, customBudgets)` — Vertex has no flash-lite table.
pub fn google_budget(
    model_id: &str,
    effort: ThinkingLevel,
    custom_budgets: Option<ThinkingBudgets>,
) -> i64 {
    let custom = custom_budgets.and_then(|budgets| match effort {
        ThinkingLevel::Minimal => budgets.minimal,
        ThinkingLevel::Low => budgets.low,
        ThinkingLevel::Medium => budgets.medium,
        _ => budgets.high,
    });
    if let Some(custom) = custom {
        return custom as i64;
    }
    let pick = |minimal: i64, low: i64, medium: i64, high: i64| match effort {
        ThinkingLevel::Minimal => minimal,
        ThinkingLevel::Low => low,
        ThinkingLevel::Medium => medium,
        _ => high,
    };
    if model_id.contains("2.5-pro") {
        return pick(128, 2048, 8192, 32768);
    }
    if model_id.contains("2.5-flash") {
        return pick(128, 2048, 8192, 24576);
    }
    -1
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

/// `buildParams(model, context, options)`, flattened the way the SDK serializes it.
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: &GoogleVertexOptions,
    timestamp: i64,
) -> Result<Value, GoogleError> {
    let contents = convert_messages(model, context, timestamp);

    let mut generation_config = Map::new();
    if let Some(temperature) = options.temperature {
        generation_config.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_tokens) = options.max_tokens {
        generation_config.insert("maxOutputTokens".to_string(), json!(max_tokens));
    }

    let supports_strict_mode = supports_google_strict_tool_sampling(&model.id);
    let tools = context.tools.as_deref().unwrap_or_default();
    let function_calling_mode = if tools.is_empty() {
        None
    } else {
        resolve_google_function_calling_mode(
            tools,
            options.tool_choice.as_deref(),
            supports_strict_mode,
        )
        .map_err(|error| GoogleError::from_message(error.to_string()))?
    };

    let mut body = Map::new();
    body.insert("contents".to_string(), Value::Array(contents));
    if let Some(system_prompt) = context
        .system_prompt
        .as_deref()
        .filter(|prompt| !prompt.is_empty())
    {
        body.insert(
            "systemInstruction".to_string(),
            json!({ "parts": [{ "text": sanitize_surrogates(system_prompt) }], "role": "user" }),
        );
    }
    if !tools.is_empty()
        && let Some(converted) = convert_tools(tools, false, supports_strict_mode)
            .map_err(|error| GoogleError::from_message(error.to_string()))?
    {
        body.insert("tools".to_string(), converted);
    }
    if let Some(mode) = function_calling_mode {
        body.insert(
            "toolConfig".to_string(),
            json!({ "functionCallingConfig": { "mode": mode.as_str() } }),
        );
    }

    if model.reasoning
        && let Some(thinking) = &options.thinking
    {
        if thinking.enabled {
            let mut thinking_config = Map::new();
            thinking_config.insert("includeThoughts".to_string(), json!(true));
            if let Some(level) = &thinking.level {
                thinking_config.insert("thinkingLevel".to_string(), json!(level));
            } else if let Some(budget_tokens) = thinking.budget_tokens {
                thinking_config.insert("thinkingBudget".to_string(), json!(budget_tokens));
            }
            generation_config.insert("thinkingConfig".to_string(), Value::Object(thinking_config));
        } else {
            generation_config.insert(
                "thinkingConfig".to_string(),
                disabled_thinking_config(&model.id),
            );
        }
    }

    body.insert(
        "generationConfig".to_string(),
        Value::Object(generation_config),
    );
    Ok(Value::Object(body))
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// one. Without it a request that needs ADC fails with the SDK's own wording.
pub type VertexTokenProvider = Arc<
    dyn Fn() -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'static>,
        > + Send
        + Sync,
>;

/// `stream(model, context, options)`
pub fn stream(
    model: Model,
    context: Context,
    request: ProviderRequestOptions,
    options: GoogleVertexOptions,
    token_provider: Option<VertexTokenProvider>,
) -> AssistantMessageEventStream {
    let outer = create_assistant_message_event_stream();
    let stream = outer.clone();

    tokio::spawn(async move {
        let timestamp = crate::auth::resolve::now_ms();
        let mut state = GoogleStreamState::new_with_api(&model, "google-vertex", timestamp);
        let outcome = run_request(
            &model,
            &context,
            &request,
            &options,
            token_provider.as_ref(),
            &mut state,
            &stream,
            timestamp,
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
                state.output.error_message = Some(format_provider_error(
                    &normalize_provider_error(&RawProviderError {
                        status: error.status,
                        body_text: error.body.clone(),
                        body_json: None,
                        message: error.message.clone(),
                    }),
                    None,
                ));
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

/// Resolves how this request authenticates, mirroring the two `createClient` variants.
pub fn resolve_endpoint(
    model: &Model,
    api_key: Option<&str>,
    options: &GoogleVertexOptions,
) -> Result<VertexEndpoint, GoogleError> {
    if let Some(api_key) = resolve_api_key(api_key) {
        return Ok(VertexEndpoint::ApiKey { api_key });
    }
    let project = resolve_project(options.project.as_deref(), options.env.as_ref());
    let location = resolve_location(options.location.as_deref(), options.env.as_ref());
    match (project, location) {
        (Ok(project), Ok(location)) => Ok(VertexEndpoint::ProjectLocation { project, location }),
        // A custom base URL alone is enough for the SDK; without it the missing
        (project, location) => {
            if resolve_custom_base_url(&model.base_url).is_some() {
                Ok(VertexEndpoint::CustomBaseUrl)
            } else {
                Err(project.err().or(location.err()).expect("one failed"))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_request(
    model: &Model,
    context: &Context,
    request: &ProviderRequestOptions,
    options: &GoogleVertexOptions,
    token_provider: Option<&VertexTokenProvider>,
    state: &mut GoogleStreamState,
    stream: &AssistantMessageEventStream,
    timestamp: i64,
) -> Result<DoneReason, GoogleError> {
    let api_key = options.api_key.as_deref().or(request.api_key.as_deref());
    let endpoint = resolve_endpoint(model, api_key, options)?;

    let mut body = build_request_body(model, context, options, timestamp)?;
    if let Some(on_payload) = &request.on_payload
        && let Some(replacement) = on_payload(body.clone(), model).await
    {
        body = replacement;
    }

    let mut request_headers: Vec<(String, String)> =
        build_client_headers(model, request.headers.as_ref())
            .into_iter()
            .filter_map(|(key, value)| value.map(|value| (key, value)))
            .collect();
    request_headers.push(("content-type".to_string(), "application/json".to_string()));
    match &endpoint {
        VertexEndpoint::ApiKey { api_key } => {
            request_headers.push(("x-goog-api-key".to_string(), api_key.clone()));
        }
        _ => {
            let Some(token_provider) = token_provider else {
                return Err(GoogleError::from_message(
                    "Could not load the default credentials.",
                ));
            };
            let token = token_provider().await.map_err(GoogleError::from_message)?;
            request_headers.push(("authorization".to_string(), format!("Bearer {token}")));
        }
    }

    let fetch: FetchFunction = request
        .fetch
        .clone()
        .unwrap_or_else(|| Arc::new(ReqwestFetch::default()));
    let url = build_request_url(model, &endpoint)?;
    let payload =
        serde_json::to_vec(&body).map_err(|error| GoogleError::from_message(error.to_string()))?;

    let response = retry_provider_request(
        || {
            let fetch = fetch.clone();
            let url = url.clone();
            let request_headers = request_headers.clone();
            let payload = payload.clone();
            async move {
                let response = fetch
                    .fetch(FetchRequest {
                        method: "POST".to_string(),
                        url,
                        headers: request_headers,
                        body: Some(payload),
                    })
                    .await
                    .map_err(|error| GoogleError::from_message(error.to_string()))?;
                if response.status < 200 || response.status >= 300 {
                    let status = response.status;
                    let headers = response.headers.clone();
                    let body = read_body(response.body).await;
                    return Err(GoogleError {
                        message: format!("{status}: {body}"),
                        status: Some(status),
                        headers,
                        body: Some(body),
                    });
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
        ProviderRetryError::RetryDelayTooLong(message) => GoogleError::from_message(message),
        ProviderRetryError::Aborted => GoogleError::from_message("Request aborted"),
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

    let mut decoder = SseDecoder::new();
    let mut feed = |text: &str, state: &mut GoogleStreamState| {
        for event in decoder.feed(text) {
            if event.data.trim().is_empty() {
                continue;
            }
            let Ok(chunk) = serde_json::from_str::<Value>(&event.data) else {
                continue;
            };
            for emitted in state.process_chunk(&chunk, timestamp) {
                stream.push(emitted);
            }
        }
    };

    match response.body {
        FetchBody::Bytes(bytes) => feed(&String::from_utf8_lossy(&bytes), state),
        FetchBody::Stream(mut receiver) => {
            while let Some(chunk) = receiver.recv().await {
                if request
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.is_cancelled())
                {
                    return Err(GoogleError::from_message("Request was aborted"));
                }
                let chunk = chunk.map_err(|error| GoogleError::from_message(error.to_string()))?;
                feed(&String::from_utf8_lossy(&chunk), state);
            }
        }
    }
    for event in decoder.finish() {
        if event.data.trim().is_empty() {
            continue;
        }
        if let Ok(chunk) = serde_json::from_str::<Value>(&event.data) {
            for emitted in state.process_chunk(&chunk, timestamp) {
                stream.push(emitted);
            }
        }
    }

    let (emitted, outcome) = state.finish();
    for event in emitted {
        stream.push(event);
    }
    if request
        .signal
        .as_ref()
        .is_some_and(|signal| signal.is_cancelled())
    {
        return Err(GoogleError::from_message("Request was aborted"));
    }
    outcome
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

/// `streamSimple(model, context, options)`
pub fn stream_simple(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
    token_provider: Option<VertexTokenProvider>,
) -> AssistantMessageEventStream {
    let options = options.unwrap_or_default();
    let base = build_base_options(
        &model,
        &context,
        Some(&options),
        options.base.base.api_key.clone(),
    );
    let vertex_options =
        |thinking: Option<crate::api::google_generative_ai::GoogleThinkingOptions>| {
            GoogleVertexOptions {
                tool_choice: None,
                thinking,
                project: None,
                location: None,
                max_tokens: base.max_tokens,
                temperature: base.temperature,
                api_key: base.base.api_key.clone(),
                env: base.base.env.clone(),
            }
        };

    let Some(reasoning) = options.reasoning else {
        return stream(
            model,
            context,
            base.base.clone(),
            vertex_options(Some(
                crate::api::google_generative_ai::GoogleThinkingOptions {
                    enabled: false,
                    ..Default::default()
                },
            )),
            token_provider,
        );
    };

    let clamped = clamp_thinking_level(&model, reasoning.into());
    let effort = match clamped {
        ModelThinkingLevel::Off => ThinkingLevel::High,
        ModelThinkingLevel::Minimal => ThinkingLevel::Minimal,
        ModelThinkingLevel::Low => ThinkingLevel::Low,
        ModelThinkingLevel::Medium => ThinkingLevel::Medium,
        _ => ThinkingLevel::High,
    };

    let thinking = if is_gemini3_pro_model(&model.id) || is_gemini3_flash_model(&model.id) {
        crate::api::google_generative_ai::GoogleThinkingOptions {
            enabled: true,
            budget_tokens: None,
            level: Some(gemini3_thinking_level(effort, &model.id).to_string()),
        }
    } else {
        crate::api::google_generative_ai::GoogleThinkingOptions {
            enabled: true,
            budget_tokens: Some(google_budget(&model.id, effort, options.thinking_budgets)),
            level: None,
        }
    };

    stream(
        model,
        context,
        base.base.clone(),
        vertex_options(Some(thinking)),
        token_provider,
    )
}
