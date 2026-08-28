#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use notagent_ai::auth::types::{Credential, OAuthCredential};
use notagent_ai::models::Provider;

use notagent_ai::providers::all::builtin_providers;
use notagent_ai::types::*;
use serde_json::{Map, Value, json};

/// `it.skipIf(!condition)` — returns from the test when the credential is missing.
#[macro_export]
macro_rules! skip_unless {
    ($condition:expr, $reason:expr) => {
        if !$condition {
            println!("skipped: {}", $reason);
            return;
        }
    };
}

/// `it.skipIf(!value)` binding the resolved credential.
#[macro_export]
macro_rules! skip_unless_some {
    ($value:expr, $reason:expr) => {
        match $value {
            Some(value) => value,
            None => {
                println!("skipped: {}", $reason);
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

fn auth_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".notagent")
        .join("agent")
        .join("auth.json")
}

fn load_auth_storage() -> Map<String, Value> {
    let Ok(content) = std::fs::read_to_string(auth_path()) else {
        return Map::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_auth_storage(storage: &Map<String, Value>) {
    let path = auth_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(serialized) = serde_json::to_string_pretty(storage) {
        let _ = std::fs::write(&path, serialized);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

/// `resolveApiKey(provider)` — reads `~/.notagent/agent/auth.json`, refreshing and
pub async fn resolve_api_key(provider: &str) -> Option<String> {
    if !e2e_enabled() {
        return None;
    }
    let mut storage = load_auth_storage();
    let entry = storage.get(provider)?.clone();
    let credential: Credential = serde_json::from_value(entry).ok()?;

    let oauth_credential: OAuthCredential = match credential {
        Credential::ApiKey(credential) => return credential.key,
        Credential::OAuth(credential) => credential,
    };

    let oauth = builtin_providers()
        .into_iter()
        .find(|candidate| candidate.id() == provider)?
        .auth()
        .oauth
        .clone()?;

    let mut credential = oauth_credential;
    if notagent_ai::auth::resolve::now_ms() >= credential.expires {
        match oauth.refresh(credential.clone(), Default::default()).await {
            Ok(refreshed) => credential = refreshed,
            Err(error) => {
                println!("{error}");
                return None;
            }
        }
    }

    storage.insert(
        provider.to_string(),
        serde_json::to_value(Credential::OAuth(credential.clone())).ok()?,
    );
    save_auth_storage(&storage);
    oauth.to_auth(credential).await.ok()?.api_key
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// Whether the e2e suites may look at credentials at all — see the module docs.
pub fn e2e_enabled() -> bool {
    std::env::var("NOTAGENT_AI_E2E")
        .is_ok_and(|value| !value.is_empty() && value != "0" && value != "false")
}

/// A provider credential from the environment, `None` unless e2e is enabled.
pub fn env(name: &str) -> Option<String> {
    if !e2e_enabled() {
        return None;
    }
    raw_env(name)
}

/// The raw variable, without the e2e gate — for values that only shape a request.
pub fn raw_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// `hasBedrockCredentials()`
pub fn has_bedrock_credentials() -> bool {
    env("AWS_PROFILE").is_some()
        || (env("AWS_ACCESS_KEY_ID").is_some() && env("AWS_SECRET_ACCESS_KEY").is_some())
        || env("AWS_BEARER_TOKEN_BEDROCK").is_some()
}

/// `hasAzureOpenAICredentials()`
pub fn has_azure_openai_credentials() -> bool {
    env("AZURE_OPENAI_API_KEY").is_some()
        && (env("AZURE_OPENAI_BASE_URL").is_some() || env("AZURE_OPENAI_RESOURCE_NAME").is_some())
}

/// `resolveAzureDeploymentName(modelId)`
pub fn resolve_azure_deployment_name(model_id: &str) -> Option<String> {
    let map_value = env("AZURE_OPENAI_DEPLOYMENT_NAME_MAP")?;
    map_value.split(',').find_map(|entry| {
        let entry = entry.trim();
        let (id, deployment) = entry.split_once('=')?;
        (id.trim() == model_id).then(|| deployment.trim().to_string())
    })
}

/// `hasCloudflareWorkersAICredentials()`
pub fn has_cloudflare_workers_ai_credentials() -> bool {
    env("CLOUDFLARE_API_KEY").is_some() && env("CLOUDFLARE_ACCOUNT_ID").is_some()
}

/// `hasCloudflareAiGatewayCredentials()`
pub fn has_cloudflare_ai_gateway_credentials() -> bool {
    has_cloudflare_workers_ai_credentials() && env("CLOUDFLARE_GATEWAY_ID").is_some()
}

// ---------------------------------------------------------------------------
// Model, context and dispatch helpers
// ---------------------------------------------------------------------------

pub fn model(provider: &str, id: &str) -> Model {
    notagent_ai::model_catalog::get_builtin_model(provider, id)
        .unwrap_or_else(|| panic!("{provider}/{id} is missing from the catalog"))
}

pub fn now_ms() -> i64 {
    notagent_ai::auth::resolve::now_ms()
}

pub fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp: now_ms(),
    })
}

pub fn user_blocks(content: Vec<TextOrImageContent>) -> Message {
    Message::User(UserMessage {
        content: UserContent::Blocks(content),
        timestamp: now_ms(),
    })
}

pub fn text_block(text: &str) -> TextOrImageContent {
    TextOrImageContent::Text(TextContent {
        text: text.to_string(),
        ..TextContent::default()
    })
}

pub fn image_block(mime_type: &str, data: &str) -> TextOrImageContent {
    TextOrImageContent::Image(ImageContent {
        mime_type: mime_type.to_string(),
        data: data.to_string(),
    })
}

/// `test/data/red-circle.png`, base64-encoded.
pub fn red_circle_png() -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(include_bytes!("../fixtures/red-circle.png"))
}

pub fn context(system_prompt: Option<&str>, messages: Vec<Message>) -> Context {
    Context {
        system_prompt: system_prompt.map(str::to_string),
        messages,
        tools: None,
    }
}

/// The `math_operation` tool the stream suite shares. `StringEnum` because Google rejects
/// the `anyOf`/`const` shape `Type.Enum` generates.
pub fn calculator_tool() -> Tool {
    Tool {
        name: "math_operation".to_string(),
        description: "Perform basic arithmetic operations".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "a": { "type": "number", "description": "First number" },
                "b": { "type": "number", "description": "Second number" },
                "operation": {
                    "type": "string",
                    "enum": ["add", "subtract", "multiply", "divide"],
                    "description": "The operation to perform. One of 'add', 'subtract', 'multiply', 'divide'.",
                },
            },
            "required": ["a", "b", "operation"],
        }),
        constrained_sampling: None,
    }
}

/// Dispatches through the provider that owns `model`, without the `Models` auth
/// `model.api` and injects `getEnvApiKey(model.provider)` when the caller passed no key.
/// `BuiltProvider::stream*` is that same dispatch.
pub fn provider_for(model: &Model) -> Arc<dyn Provider> {
    builtin_providers()
        .into_iter()
        .find(|provider| provider.id() == model.provider)
        .unwrap_or_else(|| panic!("no builtin provider {}", model.provider))
}

pub fn request_options(api_key: Option<&str>) -> ProviderRequestOptions {
    ProviderRequestOptions {
        api_key: api_key.map(str::to_string),
        ..ProviderRequestOptions::default()
    }
}

pub fn stream_options(api_key: Option<&str>) -> StreamOptions {
    StreamOptions {
        base: request_options(api_key),
        ..StreamOptions::default()
    }
}

pub fn simple_options(api_key: Option<&str>) -> SimpleStreamOptions {
    SimpleStreamOptions {
        base: stream_options(api_key),
        ..SimpleStreamOptions::default()
    }
}

/// `apiKey || getEnvApiKey(model.provider)` of the compat dispatcher.
fn with_env_api_key<T>(model: &Model, api_key: &mut Option<String>, value: T) -> T {
    if api_key.is_none() {
        *api_key = e2e_enabled()
            .then(|| notagent_ai::env_api_keys::get_env_api_key(&model.provider, None))
            .flatten();
    }
    value
}

pub fn resolved_simple_options(
    model: &Model,
    mut options: SimpleStreamOptions,
) -> SimpleStreamOptions {
    with_env_api_key(model, &mut options.base.base.api_key, ());
    options
}

pub fn resolved_stream_options(model: &Model, mut options: StreamOptions) -> StreamOptions {
    with_env_api_key(model, &mut options.base.api_key, ());
    options
}

pub async fn complete(
    model: &Model,
    context: &Context,
    options: StreamOptions,
) -> AssistantMessage {
    provider_for(model)
        .stream(
            model,
            context,
            Some(resolved_stream_options(model, options)),
        )
        .result()
        .await
}

pub async fn complete_simple(
    model: &Model,
    context: &Context,
    options: SimpleStreamOptions,
) -> AssistantMessage {
    provider_for(model)
        .stream_simple(
            model,
            context,
            Some(resolved_simple_options(model, options)),
        )
        .result()
        .await
}

/// the handle the captured payloads land in.
pub fn capture_payloads(options: &mut SimpleStreamOptions) -> CapturedPayloads {
    let captured = CapturedPayloads::default();
    let sink = Arc::clone(&captured.0);
    options.base.base.on_payload = Some(Arc::new(move |payload: Value, _model: &Model| {
        sink.lock().expect("poisoned").push(payload);
        Box::pin(async move { None })
    }));
    captured
}

/// The payloads an [`capture_payloads`] hook recorded, in request order.
#[derive(Clone, Default)]
pub struct CapturedPayloads(Arc<std::sync::Mutex<Vec<Value>>>);

impl CapturedPayloads {
    pub fn first(&self) -> Option<Value> {
        self.0.lock().expect("poisoned").first().cloned()
    }

    pub fn all(&self) -> Vec<Value> {
        self.0.lock().expect("poisoned").clone()
    }

    pub fn len(&self) -> usize {
        self.0.lock().expect("poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Collects every event plus the final message, like `for await (const event of s)`
/// followed by `await s.result()`.
pub async fn collect_stream(
    model: &Model,
    context: &Context,
    options: StreamOptions,
) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
    let stream = provider_for(model).stream(
        model,
        context,
        Some(resolved_stream_options(model, options)),
    );
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    (events, stream.result().await)
}

pub async fn collect_stream_simple(
    model: &Model,
    context: &Context,
    options: SimpleStreamOptions,
) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
    let stream = provider_for(model).stream_simple(
        model,
        context,
        Some(resolved_simple_options(model, options)),
    );
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    (events, stream.result().await)
}

// ---------------------------------------------------------------------------
// Assertions on a finished message
// ---------------------------------------------------------------------------

pub fn message_text(message: &AssistantMessage) -> String {
    message
        .content
        .iter()
        .map(|block| match block {
            AssistantContent::Text(block) => block.text.as_str(),
            _ => "",
        })
        .collect()
}

pub fn thinking_text(message: &AssistantMessage) -> String {
    message
        .content
        .iter()
        .map(|block| match block {
            AssistantContent::Thinking(block) => block.thinking.as_str(),
            _ => "",
        })
        .collect()
}

pub fn tool_calls(message: &AssistantMessage) -> Vec<&ToolCall> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantContent::ToolCall(block) => Some(block),
            _ => None,
        })
        .collect()
}

pub fn has_text(message: &AssistantMessage) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, AssistantContent::Text(_)))
}

pub fn has_thinking(message: &AssistantMessage) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, AssistantContent::Thinking(_)))
}

pub fn argument(tool_call: &ToolCall, name: &str) -> Option<Value> {
    tool_call.arguments.get(name).cloned()
}
