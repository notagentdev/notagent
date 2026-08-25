//! Providers for servers that speak the OpenAI completions API: the two common
//! local runtimes, plus one instance whose address the user names.
//!
//! What they share is that their catalogue cannot be shipped. Which models
//! exist depends on what the user pulled or loaded, so a static list would
//! offer models the server does not have and hide the ones it does. All of them
//! ask the server instead, and a server that is not running simply offers
//! nothing.
//!
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
use crate::types::{Api, Modality, Model, ModelCost, ProviderEnv};

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
///
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
        // Bracketed IPv6: the port sits outside the brackets, so the host is
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
///
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
}

/// Ollama's OpenAI-compatible surface.
pub fn ollama_provider() -> Arc<BuiltProvider> {
    openai_compatible_provider(OpenAICompatibleConfig {
        id: OLLAMA_PROVIDER_ID.to_string(),
        name: "Ollama".to_string(),
        base_url: Some(OLLAMA_BASE_URL.to_string()),
        key_env_var: "OLLAMA_API_KEY".to_string(),
        base_url_env_var: None,
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
            let signal = context.signal.clone();
            Box::pin(async move {
                let Some(base_url) = base_url else {
                    // Nobody named an address yet, so there is nothing to ask.
                    return Ok(Vec::new());
                };
                fetch_served_models(&config, &base_url, signal).await
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

/// Key handling.
///
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
                                message: "Enter the base URL (for example http://localhost:8080/v1)"
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
            // On loopback the server authorizes everything, so asking for a
            // secret that does not exist teaches the wrong thing.
            let key = if is_loopback(&base_url) {
                PLACEHOLDER_KEY.to_string()
            } else {
                interaction
                    .prompt(AuthPrompt {
                        signal: Some(interaction.signal.clone()),
                        kind: AuthPromptKind::Secret {
                            message: format!("Enter {}", self.name),
                            placeholder: None,
                        },
                    })
                    .await?
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
            if self.config.base_url.is_none()
                && self.ambient_base_url(&input).await.is_none()
            {
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

/// Asks the server which models it holds.
///
/// Not reachable is the normal case for a local runtime, and yields an empty
/// list rather than an error: someone who has not started Ollama should see no
/// Ollama models, not a warning. A server that answers but answers badly is a
/// real fault and is reported as one.
async fn fetch_served_models(
    config: &OpenAICompatibleConfig,
    base_url: &str,
    signal: tokio_util::sync::CancellationToken,
) -> Result<Vec<Model>, String> {
    let timeout = if is_loopback(base_url) {
        LOOPBACK_LIST_TIMEOUT
    } else {
        REMOTE_LIST_TIMEOUT
    };
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let request = reqwest::Client::new()
        .get(&url)
        .header("Accept", "application/json")
        .timeout(timeout);
    let response = tokio::select! {
        response = request.send() => response,
        _ = signal.cancelled() => return Ok(Vec::new()),
    };
    let response = match response {
        Ok(response) => response,
        // A timeout is not absence: a server deep in a long generation can miss
        // the deadline, and treating that as "not running" would publish and
        // persist an empty list while the model is visibly working.
        Err(error) if error.is_timeout() => {
            return Err(format!("{} model list at {url}: timed out", config.name));
        }
        // Refused or unresolvable: no server here — the ordinary case, and an
        // empty offer rather than a warning.
        Err(_) => return Ok(Vec::new()),
    };
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("{} model list at {url}: {error}", config.name))?;
    if !status.is_success() {
        return Err(format!(
            "{} model list at {url}: HTTP {}",
            config.name,
            status.as_u16()
        ));
    }
    let parsed: Value = serde_json::from_str(&body)
        .map_err(|error| format!("{} model list at {url}: {error}", config.name))?;
    models_from_listing(config, base_url, &parsed)
        .map_err(|error| format!("{} model list at {url}: {error}", config.name))
}

/// Turns one `/v1/models` body into the models this provider offers.
///
/// Split from the request so the shape handling is exercised without a server.
pub fn models_from_listing(
    config: &OpenAICompatibleConfig,
    base_url: &str,
    body: &Value,
) -> Result<Vec<Model>, String> {
    let entries = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "no \"data\" array".to_string())?;
    Ok(entries
        .iter()
        .filter_map(|entry| model_from_listing(config, base_url, entry))
        .collect())
}

fn model_from_listing(
    config: &OpenAICompatibleConfig,
    base_url: &str,
    entry: &Value,
) -> Option<Model> {
    let id = entry
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?;
    // An embedding model offered as a chat target can only end in an error, and
    // both runtimes list them alongside the chat models.
    let kind = entry
        .get("type")
        .or_else(|| entry.get("capability"))
        .and_then(Value::as_str);
    if kind.is_some_and(|kind| kind != "llm" && kind != "chat" && kind != "model") {
        return None;
    }
    // Reported under several names depending on the runtime and its version.
    let window = [
        "context_length",
        "max_context_length",
        "loaded_context_length",
        "max_model_len",
    ]
    .iter()
    .find_map(|key| entry.get(*key).and_then(Value::as_u64))
    .filter(|window| *window > 0)
    .unwrap_or(FALLBACK_CONTEXT_WINDOW);

    Some(Model {
        id: id.to_string(),
        name: display_name(id),
        api: Api::from(COMPLETIONS_API),
        provider: config.id.clone(),
        base_url: base_url.to_string(),
        // Which of these think cannot be told from a listing. Claiming the
        // capability costs nothing when it is unused — the thinking level is
        // off until someone raises it — while denying it would leave a
        // reasoning model with no way to be asked for reasoning.
        reasoning: true,
        thinking_level_map: None,
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
        context_window: window,
        // No separate output ceiling is reported, and inventing a smaller one
        // would truncate long generations the server was willing to produce.
        max_tokens: window,
        sampling_params: None,
        headers: None,
        compat: None,
    })
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
    fn a_listing_becomes_models_on_the_server_address() {
        let body = json!({
            "data": [
                { "id": "qwen3-coder:30b" },
                { "id": "nomic-embed-text", "type": "embedding" },
                { "id": "gpt-oss-20b", "max_context_length": 131072 },
            ]
        });
        let models = models_from_listing(&config(), OLLAMA_BASE_URL, &body).expect("models");
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["qwen3-coder:30b", "gpt-oss-20b"]);
        assert_eq!(models[0].base_url, OLLAMA_BASE_URL);
        assert_eq!(models[0].name, "Qwen3 Coder 30b");
        // Unreported windows take the conservative floor; a reported one wins.
        assert_eq!(models[0].context_window, FALLBACK_CONTEXT_WINDOW);
        assert_eq!(models[1].context_window, 131_072);
    }

    #[test]
    fn a_body_without_a_data_array_is_a_fault() {
        assert!(models_from_listing(&config(), OLLAMA_BASE_URL, &json!({})).is_err());
    }

    #[test]
    fn a_custom_instance_takes_its_address_from_the_credential() {
        let custom = OpenAICompatibleConfig {
            id: CUSTOM_PROVIDER_ID.to_string(),
            name: "Custom".to_string(),
            base_url: None,
            key_env_var: "CUSTOM_API_KEY".to_string(),
            base_url_env_var: Some("CUSTOM_BASE_URL".to_string()),
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
