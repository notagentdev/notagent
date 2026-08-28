//! Providers for servers that speak the OpenAI completions API: the two common
//! local runtimes, plus one instance whose address the user names.
//! What they share is that their catalogue cannot be shipped. Which models
//! exist depends on what the user pulled or loaded, so a static list would
//! offer models the server does not have and hide the ones it does. All of them
//! ask the server instead, and a server that is not running simply offers
//! nothing.
//! They differ in one thing only: where the address comes from. The two local
//! runtimes listen on a documented port, so theirs is a constant and activation
//! needs no more than `/login`. A custom instance can be anywhere, so its
//! address is asked for at login and travels with the credential — which also
//! means `/logout` takes the endpoint away with the key, rather than leaving a
//! dead address configured.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthError, AuthPrompt, AuthPromptKind,
    AuthResult, BoxFuture, Credential, ModelAuth, ProviderAuth, ProviderAuthInteraction,
};
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};
use crate::types::{
    Api, Modality, Model, ModelCost, ModelThinkingLevel, ProviderEnv, ThinkingLevelMap,
};

/// These servers expose `/v1/chat/completions`, not `/v1/responses`.
const COMPLETIONS_API: &str = "openai-completions";

/// A server that is simply not running must not hold up a model refresh. On
/// loopback a running server answers this in single-digit milliseconds, so a
/// short deadline separates "not started" from "busy" without guessing.
const LOOPBACK_LIST_TIMEOUT: Duration = Duration::from_secs(2);

/// A custom instance may cross a network, where the same deadline would report
/// healthy servers as absent.
const REMOTE_LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// Stands in for a key where the server ignores it entirely. Most OpenAI
/// clients refuse to build a request without one.
const PLACEHOLDER_KEY: &str = "local";

/// What a discovered model is assumed to hold when the server does not say.
/// Deliberately small. A local runtime is bounded by the memory of the machine
/// it runs on, far below what the weights would allow, and a window assumed too
/// large defers compaction until the server is already failing. A server that
/// reports its own window overrides this.
const FALLBACK_CONTEXT_WINDOW: u64 = 32_768;

/// Where the credential keeps a custom instance's address.
const BASE_URL_ENV: &str = "baseUrl";

pub const OLLAMA_PROVIDER_ID: &str = "ollama";
pub const OLLAMA_BASE_URL: &str = "http://127.0.0.1:11434/v1";

pub const LMSTUDIO_PROVIDER_ID: &str = "lmstudio";
pub const LMSTUDIO_BASE_URL: &str = "http://127.0.0.1:1234/v1";

pub const CUSTOM_PROVIDER_ID: &str = "custom";

/// The ids whose provider offers nothing until someone activates it.
pub const OPENAI_COMPATIBLE_PROVIDER_IDS: [&str; 3] =
    [OLLAMA_PROVIDER_ID, LMSTUDIO_PROVIDER_ID, CUSTOM_PROVIDER_ID];

/// Whether an address stays on this machine, which decides whether a key is
/// worth asking for.
pub fn is_loopback(base_url: &str) -> bool {
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        // everything up to the closing bracket — with or without a port.
        bracketed.split(']').next().unwrap_or_default()
    } else {
        // Strip a trailing port only when what follows the last colon is a
        // plain number and the remainder has no colons of its own — an
        // unbracketed IPv6 host keeps all of them.
        match authority.rsplit_once(':') {
            Some((host, port))
                if !port.is_empty()
                    && port.bytes().all(|byte| byte.is_ascii_digit())
                    && !host.contains(':') =>
            {
                host
            }
            _ => authority,
        }
    };
    let host = host.to_ascii_lowercase();
    host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "0:0:0:0:0:0:0:1"
}

/// Normalises what a user typed into a base url this can request against.
/// The three mistakes worth absorbing are a missing scheme, a trailing slash,
/// and a missing `/v1` — the last because both runtimes document their address
/// without it while their OpenAI surface lives under it.
pub fn normalize_base_url(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let path = with_scheme
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    // Only a bare authority gets `/v1` appended. A url that already names a
    // path is taken as given: the user knows their deployment better than a
    // guess does.
    if path.contains('/') {
        Some(with_scheme)
    } else {
        Some(format!("{with_scheme}/v1"))
    }
}

/// Where a provider's model list comes from.
/// The OpenAI listing is the only one every server has, but it is also the
/// poorest: the specification has no field for a context window and none for
/// what a model is for, so both runtimes answer it with little more than a name.
/// Each therefore has a listing of its own, and this says which to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSource {
    /// `GET {base}/models` — the OpenAI shape, names only.
    OpenAI,
    /// `GET {host}/api/v0/models` — reports kind, state and context length.
    LmStudio,
    /// `GET {host}/api/tags` for the names, then `POST {host}/api/show` for
    /// what each one can do and how much context it holds.
    Ollama,
}

/// Everything that differs between one instance and another.
#[derive(Debug, Clone)]
pub struct OpenAICompatibleConfig {
    pub id: String,
    pub name: String,
    /// The fixed address, or `None` when the user names it at login.
    pub base_url: Option<String>,
    /// Environment variable consulted for a key when none is stored.
    pub key_env_var: String,
    /// Environment variable consulted for the address of a custom instance.
    pub base_url_env_var: Option<String>,
    pub catalog: CatalogSource,
}

/// Ollama's OpenAI-compatible surface.
pub fn ollama_provider() -> Arc<BuiltProvider> {
    openai_compatible_provider(OpenAICompatibleConfig {
        id: OLLAMA_PROVIDER_ID.to_string(),
        name: "Ollama".to_string(),
        base_url: Some(OLLAMA_BASE_URL.to_string()),
        key_env_var: "OLLAMA_API_KEY".to_string(),
        base_url_env_var: None,
        catalog: CatalogSource::Ollama,
    })
}

/// LM Studio's OpenAI-compatible surface.
pub fn lmstudio_provider() -> Arc<BuiltProvider> {
    openai_compatible_provider(OpenAICompatibleConfig {
        id: LMSTUDIO_PROVIDER_ID.to_string(),
        name: "LM Studio".to_string(),
        base_url: Some(LMSTUDIO_BASE_URL.to_string()),
        key_env_var: "LMSTUDIO_API_KEY".to_string(),
        base_url_env_var: None,
        catalog: CatalogSource::LmStudio,
    })
}

/// An instance whose address the user gives at login.
pub fn custom_openai_provider() -> Arc<BuiltProvider> {
    openai_compatible_provider(OpenAICompatibleConfig {
        id: CUSTOM_PROVIDER_ID.to_string(),
        name: "Custom (OpenAI-compatible)".to_string(),
        base_url: None,
        key_env_var: "CUSTOM_API_KEY".to_string(),
        base_url_env_var: Some("CUSTOM_BASE_URL".to_string()),
        catalog: CatalogSource::OpenAI,
    })
}

/// Builds one provider from its config.
pub fn openai_compatible_provider(config: OpenAICompatibleConfig) -> Arc<BuiltProvider> {
    let config = Arc::new(config);
    let auth_config = Arc::clone(&config);
    let fetch_config = Arc::clone(&config);
    create_provider(CreateProviderOptions {
        id: config.id.clone(),
        name: Some(config.name.clone()),
        base_url: config.base_url.clone(),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(OpenAICompatibleAuth {
                name: format!("{} API key", config.name),
                config: auth_config,
            })),
            oauth: None,
        },
        // A static list would claim models the server does not hold; which ones
        // exist is entirely up to what the user pulled or loaded.
        models: Vec::new(),
        fetch_models: Some(Arc::new(move |context| {
            let config = Arc::clone(&fetch_config);
            let base_url = resolved_base_url(&config, context.credential.as_ref());
            let api_key = credential_key(context.credential.as_ref());
            let signal = context.signal.clone();
            Box::pin(async move {
                let Some(base_url) = base_url else {
                    // Nobody named an address yet, so there is nothing to ask.
                    return Ok(Vec::new());
                };
                fetch_served_models(&config, &base_url, api_key.as_deref(), signal).await
            })
        })),
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}

/// The address this instance currently answers on.
fn resolved_base_url(
    config: &OpenAICompatibleConfig,
    credential: Option<&Credential>,
) -> Option<String> {
    if let Some(base_url) = &config.base_url {
        return Some(base_url.clone());
    }
    let Some(Credential::ApiKey(credential)) = credential else {
        return None;
    };
    credential
        .env
        .as_ref()
        .and_then(|env| env.get(BASE_URL_ENV))
        .filter(|base_url| !base_url.is_empty())
        .cloned()
}

/// The stored key, for a server that was switched to requiring one.
fn credential_key(credential: Option<&Credential>) -> Option<String> {
    let Some(Credential::ApiKey(credential)) = credential else {
        return None;
    };
    credential
        .key
        .as_ref()
        .filter(|key| !key.is_empty() && key.as_str() != PLACEHOLDER_KEY)
        .cloned()
}

/// The server's own root, below which its native API lives.
/// Both runtimes put their own listing beside the OpenAI one rather than under
/// it: `/api/v0/models` and `/api/tags` are siblings of `/v1`, not children.
fn server_root(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    trimmed.strip_suffix("/v1").unwrap_or(trimmed).to_string()
}

/// Key handling.
/// `resolve` has no unconditional fallback: an instance nobody activated
/// resolves to nothing, which keeps it out of the model picker until `/login`
/// stores a credential. The placeholder lives in `login`, so activation is an
/// explicit act with an explicit undo.
struct OpenAICompatibleAuth {
    name: String,
    config: Arc<OpenAICompatibleConfig>,
}

impl ApiKeyAuth for OpenAICompatibleAuth {
    fn name(&self) -> &str {
        &self.name
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            if interaction.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            // A fixed address needs no question; a custom one is the whole
            // point of the login and is asked for first.
            let base_url = match &self.config.base_url {
                Some(base_url) => base_url.clone(),
                None => {
                    let typed = interaction
                        .prompt(AuthPrompt {
                            signal: Some(interaction.signal.clone()),
                            kind: AuthPromptKind::Text {
                                message:
                                    "Enter the base URL (for example http://localhost:8080/v1)"
                                        .to_string(),
                                placeholder: None,
                            },
                        })
                        .await?;
                    normalize_base_url(&typed)
                        .ok_or_else(|| AuthError("No base URL was given".to_string()))?
                }
            };
            if interaction.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }

            let mut env = ProviderEnv::new();
            if self.config.base_url.is_none() {
                env.insert(BASE_URL_ENV.to_string(), base_url.clone());
            }
            // A local server authorizes everything until someone switches its
            // authentication on, which both runtimes offer. So the key is
            // asked for either way and the question says which case it is:
            // optional on loopback, where an empty answer stands a placeholder
            // in, and expected anywhere else.
            let optional = is_loopback(&base_url);
            let message = if optional {
                format!("Enter {} (leave empty if the server needs none)", self.name)
            } else {
                format!("Enter {}", self.name)
            };
            let typed = interaction
                .prompt(AuthPrompt {
                    signal: Some(interaction.signal.clone()),
                    kind: AuthPromptKind::Secret {
                        message,
                        placeholder: None,
                    },
                })
                .await?;
            let typed = typed.trim().to_string();
            let key = match (typed.is_empty(), optional) {
                (false, _) => typed,
                (true, true) => PLACEHOLDER_KEY.to_string(),
                (true, false) => {
                    return Err(AuthError("No API key was given".to_string()));
                }
            };
            if interaction.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            Ok(ApiKeyCredential {
                key: Some(key),
                env: (!env.is_empty()).then_some(env),
            })
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            if input.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            if let Some(credential) = &input.credential
                && let Some(key) = credential.key.as_ref().filter(|key| !key.is_empty())
            {
                return Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key.clone()),
                        ..Default::default()
                    },
                    env: credential.env.clone(),
                    source: Some("stored credential".to_string()),
                }));
            }
            // An ambient key activates a fixed instance, but not a custom one:
            // without an address there is nothing for the key to authorize.
            if self.config.base_url.is_none() && self.ambient_base_url(&input).await.is_none() {
                return Ok(None);
            }
            if let Some(value) = input
                .ctx
                .env(&self.config.key_env_var)
                .await
                .filter(|value| !value.is_empty())
            {
                let env = match self.ambient_base_url(&input).await {
                    Some(base_url) => {
                        let mut env = ProviderEnv::new();
                        env.insert(BASE_URL_ENV.to_string(), base_url);
                        Some(env)
                    }
                    None => None,
                };
                return Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(value),
                        ..Default::default()
                    },
                    env,
                    source: Some(self.config.key_env_var.clone()),
                }));
            }
            Ok(None)
        })
    }
}

impl OpenAICompatibleAuth {
    /// The address named by the environment, for a custom instance configured
    /// without a login.
    async fn ambient_base_url(&self, input: &ApiKeyAuthInput<'_>) -> Option<String> {
        let variable = self.config.base_url_env_var.as_ref()?;
        let value = input.ctx.env(variable).await?;
        normalize_base_url(&value)
    }
}

/// One model as a listing described it, before it becomes a [`Model`].
/// The three listings disagree on almost everything except that a model has a
/// name, so each parser reduces its own shape to this and the rest is shared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedModel {
    pub id: String,
    /// `None` when the listing does not say, which is the OpenAI shape's answer
    /// to every question beyond the name.
    pub context_window: Option<u64>,
    /// Whether the model thinks. `None` when the listing does not say, which is
    /// taken as "assume it can": offering a level that turns out to do nothing
    /// costs a wasted setting, while withholding one from a reasoning model
    /// leaves no way to ask it to think at all.
    pub reasoning: Option<bool>,
}

/// What a GET returned, or why there is nothing to read.
enum Fetched {
    Body(Value),
    /// Nothing is listening at that address.
    NoServer,
    /// A server answered, but does not have that route — how an older build
    /// answers a listing it predates.
    NoRoute,
}

/// Asks the server which models it holds.
/// This runs only for a provider somebody activated — the refresh reaches a
/// dynamic provider's fetch only once a credential resolves. So an address
/// nobody is listening at is worth reporting rather than swallowing: the
/// alternative is a picker that stays empty while the refresh claims success.
async fn fetch_served_models(
    config: &OpenAICompatibleConfig,
    base_url: &str,
    api_key: Option<&str>,
    signal: tokio_util::sync::CancellationToken,
) -> Result<Vec<Model>, String> {
    let client = reqwest::Client::new();
    let listed = match config.catalog {
        CatalogSource::LmStudio => {
            match fetch_lmstudio_models(config, &client, base_url, api_key, &signal).await? {
                Some(listed) => listed,
                // An older build has no `/api/v0`; the OpenAI listing still
                // names the models, it just cannot say anything about them.
                None => fetch_openai_models(config, &client, base_url, api_key, &signal).await?,
            }
        }
        CatalogSource::Ollama => {
            match fetch_ollama_models(config, &client, base_url, api_key, &signal).await? {
                Some(listed) => listed,
                None => fetch_openai_models(config, &client, base_url, api_key, &signal).await?,
            }
        }
        CatalogSource::OpenAI => {
            fetch_openai_models(config, &client, base_url, api_key, &signal).await?
        }
    };
    Ok(listed
        .into_iter()
        .map(|listed| build_model(config, base_url, &listed))
        .collect())
}

/// One GET, with the failures a local server actually produces sorted out.
async fn fetch_json(
    config: &OpenAICompatibleConfig,
    client: &reqwest::Client,
    url: &str,
    api_key: Option<&str>,
    loopback: bool,
    signal: &tokio_util::sync::CancellationToken,
) -> Result<Fetched, String> {
    let timeout = if loopback {
        LOOPBACK_LIST_TIMEOUT
    } else {
        REMOTE_LIST_TIMEOUT
    };
    let mut request = client
        .get(url)
        .header("Accept", "application/json")
        .timeout(timeout);
    // Both runtimes ignore a key until someone switches authentication on, at
    // which point they expect it as a bearer token like every OpenAI client.
    if let Some(api_key) = api_key {
        request = request.bearer_auth(api_key);
    }
    let response = tokio::select! {
        response = request.send() => response,
        // A cancelled refresh has no answer and no fault to report.
        _ = signal.cancelled() => return Ok(Fetched::NoRoute),
    };
    let response = match response {
        Ok(response) => response,
        // A timeout is not absence: a server deep in a long generation can miss
        // the deadline, and treating that as "not running" would publish and
        // persist an empty list while the model is visibly working.
        Err(error) if error.is_timeout() => {
            return Err(format!("{} at {url}: timed out", config.name));
        }
        // Refused or unresolvable: no server here.
        Err(_) => return Ok(Fetched::NoServer),
    };
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("{} model list at {url}: {error}", config.name))?;
    if status.as_u16() == 404 {
        // The route does not exist on this build, which the caller may have a
        // fallback for.
        return Ok(Fetched::NoRoute);
    }
    if !status.is_success() {
        return Err(format!(
            "{} model list at {url}: HTTP {}",
            config.name,
            status.as_u16()
        ));
    }
    serde_json::from_str(&body)
        .map(Fetched::Body)
        .map_err(|error| format!("{} model list at {url}: {error}", config.name))
}

/// `GET {base}/models` — the OpenAI listing every server has.
async fn fetch_openai_models(
    config: &OpenAICompatibleConfig,
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    signal: &tokio_util::sync::CancellationToken,
) -> Result<Vec<ListedModel>, String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let loopback = is_loopback(base_url);
    match fetch_json(config, client, &url, api_key, loopback, signal).await? {
        // The last listing anyone can try: there is no fallback left, so this
        // is where an unreachable address becomes something the user is told.
        Fetched::NoServer => Err(unreachable_message(config, base_url)),
        Fetched::NoRoute => Ok(Vec::new()),
        Fetched::Body(body) => models_from_openai_listing(&body)
            .map_err(|error| format!("{} model list at {url}: {error}", config.name)),
    }
}

/// What the picker says when the address answers nothing.
fn unreachable_message(config: &OpenAICompatibleConfig, base_url: &str) -> String {
    format!("{} is not reachable at {base_url}", config.name)
}

/// `GET {host}/api/v0/models` — LM Studio's own listing.
/// `Ok(None)` means the route is not there, which is how an older build
/// answers; the caller then falls back to the OpenAI listing.
async fn fetch_lmstudio_models(
    config: &OpenAICompatibleConfig,
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    signal: &tokio_util::sync::CancellationToken,
) -> Result<Option<Vec<ListedModel>>, String> {
    let url = format!("{}/api/v0/models", server_root(base_url));
    let loopback = is_loopback(base_url);
    match fetch_json(config, client, &url, api_key, loopback, signal).await? {
        // Both mean "ask the OpenAI listing instead"; whether the address is
        // dead is decided there, once, rather than in every branch.
        Fetched::NoServer | Fetched::NoRoute => Ok(None),
        Fetched::Body(body) => models_from_lmstudio_listing(&body)
            .map(Some)
            .map_err(|error| format!("{} model list at {url}: {error}", config.name)),
    }
}

/// `GET {host}/api/tags`, then `POST {host}/api/show` for each model.
/// The listing names the models and nothing else, so what each one can do and
/// how much context it holds has to be asked for separately. A model whose
/// details cannot be read is kept rather than dropped: a name that chats is
/// still usable, only its window is then the conservative floor.
async fn fetch_ollama_models(
    config: &OpenAICompatibleConfig,
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    signal: &tokio_util::sync::CancellationToken,
) -> Result<Option<Vec<ListedModel>>, String> {
    let root = server_root(base_url);
    let url = format!("{root}/api/tags");
    let loopback = is_loopback(base_url);
    let body = match fetch_json(config, client, &url, api_key, loopback, signal).await? {
        Fetched::NoServer | Fetched::NoRoute => return Ok(None),
        Fetched::Body(body) => body,
    };
    let names = names_from_ollama_tags(&body)
        .map_err(|error| format!("{} model list at {url}: {error}", config.name))?;

    let mut listed = Vec::new();
    for name in names {
        if signal.is_cancelled() {
            break;
        }
        match ollama_model_details(client, &root, &name, api_key, loopback, signal).await {
            // Only what the server says is an embedder is dropped. Silence is
            // not a verdict.
            Some(details) if !details.chats => continue,
            Some(details) => listed.push(ListedModel {
                id: name,
                context_window: details.context_window,
                reasoning: details.reasoning,
            }),
            None => listed.push(ListedModel {
                id: name,
                context_window: None,
                reasoning: None,
            }),
        }
    }
    Ok(Some(listed))
}

/// What `/api/show` says about one model.
pub struct OllamaDetails {
    pub chats: bool,
    pub context_window: Option<u64>,
    /// `None` when the build reports no capabilities at all.
    pub reasoning: Option<bool>,
}

async fn ollama_model_details(
    client: &reqwest::Client,
    root: &str,
    name: &str,
    api_key: Option<&str>,
    loopback: bool,
    signal: &tokio_util::sync::CancellationToken,
) -> Option<OllamaDetails> {
    let timeout = if loopback {
        LOOPBACK_LIST_TIMEOUT
    } else {
        REMOTE_LIST_TIMEOUT
    };
    let mut request = client
        .post(format!("{root}/api/show"))
        .header("Accept", "application/json")
        .json(&serde_json::json!({ "model": name }))
        .timeout(timeout);
    if let Some(api_key) = api_key {
        request = request.bearer_auth(api_key);
    }
    let response = tokio::select! {
        response = request.send() => response.ok()?,
        _ = signal.cancelled() => return None,
    };
    if !response.status().is_success() {
        return None;
    }
    let body: Value = serde_json::from_str(&response.text().await.ok()?).ok()?;
    Some(ollama_details_from_show(&body))
}

/// Reads one `/api/show` body.
/// Split from the request so the shape handling is exercised without a server.
pub fn ollama_details_from_show(body: &Value) -> OllamaDetails {
    // The capability list is the server's own answer to "what is this for".
    // A build that reports none is taken at its word as usable, because
    // dropping everything would empty the picker.
    let capabilities: Vec<&str> = body
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    // Keyed on the embedding capability rather than on the presence of
    // `completion`, which the server reports for everything it can run.
    let chats = !capabilities.contains(&"embedding");
    // `thinking` is the server's own verdict, so it is worth more than a guess
    // from the name — this is the one place a local model's effort levels can
    // be got right.
    let reasoning = (!capabilities.is_empty()).then(|| capabilities.contains(&"thinking"));

    // The window is keyed by architecture — `llama.context_length`,
    // `qwen3.context_length` — so the suffix is what identifies it.
    let context_window = body
        .get("model_info")
        .and_then(Value::as_object)
        .and_then(|info| {
            info.iter()
                .find(|(key, _)| key.ends_with(".context_length"))
                .and_then(|(_, value)| value.as_u64())
        })
        .filter(|window| *window > 0);

    OllamaDetails {
        chats,
        context_window,
        reasoning,
    }
}

/// Reads one `/api/tags` body: the models a server holds, by name.
pub fn names_from_ollama_tags(body: &Value) -> Result<Vec<String>, String> {
    let entries = body
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| "no \"models\" array".to_string())?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            // `model` is the tag a request has to name; `name` is the same
            // string on every build that reports both.
            entry
                .get("model")
                .or_else(|| entry.get("name"))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .collect())
}

/// Reads one `/api/v0/models` body: LM Studio's own listing.
pub fn models_from_lmstudio_listing(body: &Value) -> Result<Vec<ListedModel>, String> {
    let entries = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "no \"data\" array".to_string())?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())?;
            // `llm` and `vlm` both chat; `embeddings` does not. An unknown kind
            // is kept, since a future kind that chats should not vanish.
            if entry.get("type").and_then(Value::as_str) == Some("embeddings") {
                return None;
            }
            // A loaded model was given a window at load time, which is what it
            // will actually accept — `max_context_length` is only what it could
            // have been given.
            let context_window = ["loaded_context_length", "max_context_length"]
                .iter()
                .find_map(|key| entry.get(*key).and_then(Value::as_u64))
                .filter(|window| *window > 0);
            Some(ListedModel {
                id: id.to_string(),
                context_window,
                // The listing has no field for it, so this stays a guess.
                reasoning: None,
            })
        })
        .collect())
}

/// Reads one `/v1/models` body: the OpenAI listing, which is names only.
pub fn models_from_openai_listing(body: &Value) -> Result<Vec<ListedModel>, String> {
    let entries = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "no \"data\" array".to_string())?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())?;
            Some(ListedModel {
                id: id.to_string(),
                context_window: None,
                reasoning: None,
            })
        })
        .collect())
}

/// The effort levels a runtime accepts, as a map that marks the rest absent.
/// Both runtimes take `low`, `medium` and `high` on their OpenAI surface and
/// reject anything else, so `minimal` has to be struck: offering it would put a
/// setting in front of the user that the server answers with an error. `xhigh`
/// and `max` need no entry — a level is only offered when the map names it.
fn low_medium_high() -> ThinkingLevelMap {
    let mut map = ThinkingLevelMap::new();
    map.insert(ModelThinkingLevel::Minimal, None);
    map
}

fn build_model(config: &OpenAICompatibleConfig, base_url: &str, listed: &ListedModel) -> Model {
    Model {
        id: listed.id.clone(),
        name: display_name(&listed.id),
        api: Api::from(COMPLETIONS_API),
        provider: config.id.clone(),
        base_url: base_url.to_string(),
        reasoning: listed.reasoning.unwrap_or(true),
        // A server this knows the effort vocabulary of gets it; one it does not
        // — any endpoint the user pointed at — keeps the full range, since it
        // may be a service whose levels are the OpenAI ones.
        thinking_level_map: match config.catalog {
            CatalogSource::LmStudio | CatalogSource::Ollama => Some(low_medium_high()),
            CatalogSource::OpenAI => None,
        },
        input: vec![Modality::Text],
        // A local runtime bills nothing, and a custom endpoint bills something
        // this cannot know. Reporting real zeroes keeps cost accounting honest
        // rather than inventing a rate.
        cost: ModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            tiers: None,
        },
        context_window: listed.context_window.unwrap_or(FALLBACK_CONTEXT_WINDOW),
        // No separate output ceiling is reported, and inventing a smaller one
        // would truncate long generations the server was willing to produce.
        max_tokens: listed.context_window.unwrap_or(FALLBACK_CONTEXT_WINDOW),
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

/// `qwen3-coder:30b` reads better as `Qwen3 Coder 30b` in a model picker.
fn display_name(id: &str) -> String {
    let words: Vec<String> = id
        .split(['-', '_', '/', ':', '.'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect();
    if words.is_empty() {
        id.to_string()
    } else {
        words.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> OpenAICompatibleConfig {
        OpenAICompatibleConfig {
            id: "ollama".to_string(),
            name: "Ollama".to_string(),
            base_url: Some(OLLAMA_BASE_URL.to_string()),
            key_env_var: "OLLAMA_API_KEY".to_string(),
            base_url_env_var: None,
            catalog: CatalogSource::Ollama,
        }
    }

    #[test]
    fn loopback_covers_the_forms_an_address_takes() {
        assert!(is_loopback("http://127.0.0.1:11434/v1"));
        assert!(is_loopback("http://localhost:1234/v1"));
        assert!(is_loopback("http://[::1]:8080/v1"));
        assert!(!is_loopback("https://models.example.com/v1"));
        assert!(!is_loopback("http://192.168.1.10:1234/v1"));
    }

    #[test]
    fn a_typed_address_is_completed_rather_than_rejected() {
        assert_eq!(
            normalize_base_url("localhost:1234").as_deref(),
            Some("http://localhost:1234/v1")
        );
        assert_eq!(
            normalize_base_url("http://localhost:1234/").as_deref(),
            Some("http://localhost:1234/v1")
        );
        // A named path is taken as given.
        assert_eq!(
            normalize_base_url("https://gateway.example.com/openai/v1").as_deref(),
            Some("https://gateway.example.com/openai/v1")
        );
        assert_eq!(normalize_base_url("   "), None);
    }

    #[test]
    fn the_native_api_sits_beside_the_openai_one_not_under_it() {
        assert_eq!(
            server_root("http://127.0.0.1:1234/v1"),
            "http://127.0.0.1:1234"
        );
        assert_eq!(
            server_root("http://127.0.0.1:11434/v1/"),
            "http://127.0.0.1:11434"
        );
        // Nothing to strip: a gateway that is not mounted at /v1 keeps its path.
        assert_eq!(
            server_root("https://gateway.example.com/openai"),
            "https://gateway.example.com/openai"
        );
    }

    #[test]
    fn the_openai_listing_yields_names_and_nothing_else() {
        let body = json!({
            "object": "list",
            "data": [
                { "id": "qwen3-coder:30b", "object": "model", "owned_by": "library" },
                { "id": "", "object": "model" },
            ]
        });
        let listed = models_from_openai_listing(&body).expect("models");
        assert_eq!(
            listed,
            [ListedModel {
                id: "qwen3-coder:30b".to_string(),
                context_window: None,
                reasoning: None
            }]
        );
        assert!(models_from_openai_listing(&json!({})).is_err());
    }

    #[test]
    fn the_lmstudio_listing_reports_kind_and_window() {
        // The shape LM Studio's own `/api/v0/models` returns.
        let body = json!({
            "object": "list",
            "data": [
                {
                    "id": "qwen3-coder-30b",
                    "object": "model",
                    "type": "llm",
                    "publisher": "qwen",
                    "arch": "qwen3",
                    "compatibility_type": "gguf",
                    "quantization": "Q4_K_M",
                    "state": "not-loaded",
                    "max_context_length": 262144
                },
                {
                    "id": "text-embedding-nomic-embed-text-v1.5",
                    "object": "model",
                    "type": "embeddings",
                    "state": "not-loaded",
                    "max_context_length": 2048
                },
                {
                    "id": "qwen2-vl-7b",
                    "object": "model",
                    "type": "vlm",
                    "state": "loaded",
                    "max_context_length": 131072,
                    "loaded_context_length": 8192
                },
            ]
        });
        let listed = models_from_lmstudio_listing(&body).expect("models");
        assert_eq!(
            listed,
            [
                ListedModel {
                    id: "qwen3-coder-30b".to_string(),
                    context_window: Some(262_144),
                    // The listing has no field for it.
                    reasoning: None
                },
                // A vision model still chats; only the embedder is dropped.
                ListedModel {
                    id: "qwen2-vl-7b".to_string(),
                    // What it was actually loaded with wins over what it could
                    // have been given.
                    context_window: Some(8_192),
                    reasoning: None
                },
            ]
        );
        assert!(models_from_lmstudio_listing(&json!({})).is_err());
    }

    #[test]
    fn the_ollama_tags_listing_yields_the_names_a_request_can_use() {
        let body = json!({
            "models": [
                {
                    "name": "qwen3-coder:30b",
                    "model": "qwen3-coder:30b",
                    "modified_at": "2026-08-01T10:00:00Z",
                    "size": 18_000_000_000_u64,
                    "digest": "abc",
                    "details": { "format": "gguf", "family": "qwen3" }
                },
                { "name": "legacy-only-name" },
            ]
        });
        assert_eq!(
            names_from_ollama_tags(&body).expect("names"),
            ["qwen3-coder:30b", "legacy-only-name"]
        );
        assert!(names_from_ollama_tags(&json!({})).is_err());
    }

    #[test]
    fn ollama_details_name_the_window_by_architecture() {
        let body = json!({
            "capabilities": ["completion", "tools", "thinking"],
            "details": { "family": "qwen3" },
            "model_info": {
                "general.architecture": "qwen3",
                "qwen3.context_length": 262144,
                "qwen3.embedding_length": 5120
            }
        });
        let details = ollama_details_from_show(&body);
        assert!(details.chats);
        assert_eq!(details.context_window, Some(262_144));
        assert_eq!(details.reasoning, Some(true), "the server said it thinks");

        // `completion` is reported for everything the server can run, so the
        // embedding capability is what settles it.
        let embedder = ollama_details_from_show(&json!({
            "capabilities": ["completion", "embedding"],
            "model_info": { "nomic-bert.context_length": 2048 }
        }));
        assert!(!embedder.chats, "an embedder is not a chat target");

        let plain = ollama_details_from_show(&json!({
            "capabilities": ["completion", "tools"],
            "model_info": {}
        }));
        assert_eq!(
            plain.reasoning,
            Some(false),
            "a model without the thinking capability is not offered effort levels"
        );

        // An older build reports no capabilities at all; dropping everything
        // then would empty the picker, and the thinking question stays open.
        let silent = ollama_details_from_show(&json!({ "model_info": {} }));
        assert!(silent.chats);
        assert_eq!(silent.context_window, None);
        assert_eq!(silent.reasoning, None);
    }

    #[test]
    fn a_local_runtime_offers_only_the_effort_levels_it_accepts() {
        // Both runtimes take low, medium and high on their OpenAI surface and
        // reject `minimal`; `xhigh` and `max` are never offered without a map
        // entry naming them.
        let thinker = build_model(
            &config(),
            OLLAMA_BASE_URL,
            &ListedModel {
                id: "qwen3:8b".to_string(),
                context_window: None,
                reasoning: Some(true),
            },
        );
        assert!(thinker.reasoning);
        assert_eq!(
            crate::models::get_supported_thinking_levels(&thinker),
            [
                ModelThinkingLevel::Off,
                ModelThinkingLevel::Low,
                ModelThinkingLevel::Medium,
                ModelThinkingLevel::High,
            ]
        );

        let plain = build_model(
            &config(),
            OLLAMA_BASE_URL,
            &ListedModel {
                id: "llama3.2:3b".to_string(),
                context_window: None,
                reasoning: Some(false),
            },
        );
        assert!(!plain.reasoning);
        assert_eq!(
            crate::models::get_supported_thinking_levels(&plain),
            [ModelThinkingLevel::Off],
            "a model that does not think is offered no effort at all"
        );
    }

    #[test]
    fn an_endpoint_nobody_knows_keeps_the_full_range() {
        // A custom address may be any service, including one whose levels are
        // the OpenAI ones, so nothing is struck from it.
        let custom = OpenAICompatibleConfig {
            catalog: CatalogSource::OpenAI,
            ..config()
        };
        let model = build_model(
            &custom,
            OLLAMA_BASE_URL,
            &ListedModel {
                id: "gpt-5".to_string(),
                context_window: None,
                reasoning: None,
            },
        );
        assert!(
            crate::models::get_supported_thinking_levels(&model)
                .contains(&ModelThinkingLevel::Minimal)
        );
    }

    #[test]
    fn an_unreported_window_takes_the_conservative_floor() {
        let model = build_model(
            &config(),
            OLLAMA_BASE_URL,
            &ListedModel {
                id: "qwen3-coder:30b".to_string(),
                context_window: None,
                reasoning: None,
            },
        );
        assert_eq!(model.base_url, OLLAMA_BASE_URL);
        assert_eq!(model.name, "Qwen3 Coder 30b");
        assert_eq!(model.context_window, FALLBACK_CONTEXT_WINDOW);
        assert_eq!(model.cost.input, 0.0);
    }

    #[test]
    fn a_custom_instance_takes_its_address_from_the_credential() {
        let custom = OpenAICompatibleConfig {
            id: CUSTOM_PROVIDER_ID.to_string(),
            name: "Custom".to_string(),
            base_url: None,
            key_env_var: "CUSTOM_API_KEY".to_string(),
            base_url_env_var: Some("CUSTOM_BASE_URL".to_string()),
            catalog: CatalogSource::OpenAI,
        };
        assert_eq!(resolved_base_url(&custom, None), None);

        let mut env = ProviderEnv::new();
        env.insert(BASE_URL_ENV.to_string(), "http://host:9000/v1".to_string());
        let credential = Credential::ApiKey(ApiKeyCredential {
            key: Some("k".to_string()),
            env: Some(env),
        });
        assert_eq!(
            resolved_base_url(&custom, Some(&credential)).as_deref(),
            Some("http://host:9000/v1")
        );
        // A fixed address ignores whatever the credential carries.
        assert_eq!(
            resolved_base_url(&config(), Some(&credential)).as_deref(),
            Some(OLLAMA_BASE_URL)
        );
    }
}
