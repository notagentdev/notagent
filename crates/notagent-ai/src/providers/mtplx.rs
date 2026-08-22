//! MTPLX providers — a local server on loopback, plus remote instances.
//!
//! One factory, one config struct, two kinds of instance. The two differ only
//! in where their config comes from and whether the address is on loopback;
//! everything of value (session header, client header, auth handling) is
//! identical and lives here once.
//!
//! Three details make this more than a base-url override:
//!
//! - `x-mtplx-session-id` binds a request to the server's warm-prefix session
//!   bank. Without it the server infers the session from a prompt prefix scan
//!   on every request, so a long agent conversation re-prefills work it
//!   already holds. Sent via [`SessionAffinityFormat::Mtplx`].
//! - `x-mtplx-client` decides whether the server or the client owns the
//!   sampler. Left unset, the server classifies by user-agent, which would
//!   make our sampler ownership depend on a version string.
//! - The key is a placeholder on loopback. The server authorizes every request
//!   when it has no key configured, but most OpenAI clients refuse to build a
//!   request without one, so a constant stands in.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthError, AuthPrompt, AuthPromptKind,
    AuthResult, BoxFuture, ModelAuth, ProviderAuth, ProviderAuthInteraction,
};
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};
use crate::types::{
    Api, Modality, Model, ModelCompat, ModelCost, OpenAICompletionsCompat, SessionAffinityFormat,
};

/// The api id MTPLX serves: `/v1/chat/completions`, not `/v1/responses`.
const MTPLX_API: &str = "openai-completions";

/// A server that is simply not running must not hold up a model refresh. On
/// loopback a running server answers this in single-digit milliseconds, so a
/// short deadline separates "not started" from "busy" without guessing.
const LOOPBACK_LIST_TIMEOUT: Duration = Duration::from_secs(2);

/// A remote instance crosses a network, so the same deadline would report
/// healthy servers as absent.
const REMOTE_LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// Stands in for a key on loopback, where the server ignores it entirely.
const LOCAL_PLACEHOLDER_KEY: &str = "mtplx-local";

/// What one MTPLX model offers. The provider-dependent fields (provider id,
/// base url, headers, compat) are filled in by the factory, so a caller
/// describes only what is genuinely per-model.
#[derive(Debug, Clone)]
pub struct MtplxModelSpec {
    pub id: String,
    pub name: String,
    pub context_window: u64,
    pub max_tokens: u64,
    pub reasoning: bool,
}

impl MtplxModelSpec {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        MtplxModelSpec {
            id: id.into(),
            name: name.into(),
            // A served model's real window comes from the checkpoint; this is a
            // conservative floor that every catalogued MTPLX model exceeds.
            context_window: 131_072,
            // Deliberately not a smaller default: a client-side ceiling the user
            // never chose would silently truncate long generations, and the
            // server owns the generation contract.
            max_tokens: 131_072,
            reasoning: true,
        }
    }
}

/// Everything that differs between one MTPLX instance and another.
///
/// There is deliberately no "requires a key" field: that follows from
/// [`base_url`](Self::base_url) via [`Self::is_loopback`]. A separate flag
/// could be set wrong and would then wave a keyless remote instance through.
#[derive(Debug, Clone)]
pub struct MtplxProviderConfig {
    pub id: String,
    pub name: String,
    pub base_url: String,
    /// Value for `x-mtplx-client`.
    pub client_hint: String,
}

impl MtplxProviderConfig {
    /// Whether the address stays on this machine. Decides both whether a real
    /// key is required and whether login may skip its prompt.
    pub fn is_loopback(&self) -> bool {
        let rest = self
            .base_url
            .split_once("://")
            .map_or(self.base_url.as_str(), |(_, rest)| rest);
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let authority = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let host = if let Some(bracketed) = authority.strip_prefix('[') {
            // Bracketed IPv6: the port sits outside the brackets, so the host
            // is everything up to the closing bracket — with or without a port.
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
}

/// Provider id of the built-in local instance. Short because it is typed:
/// `/model mtplx/<model>`.
pub const LOCAL_PROVIDER_ID: &str = "mtplx";

/// Where `mtplx serve` listens unless told otherwise.
pub const LOCAL_BASE_URL: &str = "http://127.0.0.1:8000/v1";

/// How we name ourselves to the server.
///
/// Deliberately not one of the names the server treats as its own surface:
/// those hand it ownership of the sampler, and our temperature and top-p would
/// then be dropped without a trace. Unset is not an option either, because the
/// server would classify us by user-agent and our sampler ownership would ride
/// on a version string.
pub const CLIENT_HINT: &str = "notagent";

/// The built-in local instance.
///
/// It offers nothing until someone activates it with `/login mtplx`, and
/// nothing again after `/logout` — see [`MtplxApiKeyAuth`]. That keeps an
/// unused local server out of everyone else's model picker.
pub fn mtplx_local_provider() -> Arc<BuiltProvider> {
    mtplx_provider(MtplxProviderConfig {
        id: LOCAL_PROVIDER_ID.to_string(),
        name: "MTPLX (local)".to_string(),
        base_url: LOCAL_BASE_URL.to_string(),
        client_hint: CLIENT_HINT.to_string(),
    })
}

/// Builds one MTPLX provider from its config.
pub fn mtplx_provider(config: MtplxProviderConfig) -> Arc<BuiltProvider> {
    let loopback = config.is_loopback();
    let timeout = if loopback {
        LOOPBACK_LIST_TIMEOUT
    } else {
        REMOTE_LIST_TIMEOUT
    };
    let config = Arc::new(MtplxRuntimeConfig { config, timeout });
    let (id, name, base_url, auth_name) = (
        config.config.id.clone(),
        config.config.name.clone(),
        config.config.base_url.clone(),
        format!("{} API key", config.config.name),
    );
    create_provider(CreateProviderOptions {
        id,
        name: Some(name),
        base_url: Some(base_url),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(MtplxApiKeyAuth {
                name: auth_name,
                env_var: "MTPLX_API_KEY".to_string(),
                // Only loopback gets a prompt-free login; a remote server
                // rejects a keyless start, so a placeholder would just move the
                // failure to the first request.
                placeholder: loopback.then(|| LOCAL_PLACEHOLDER_KEY.to_string()),
            })),
            oauth: None,
        },
        // Static entries would claim models the server is not serving: it holds
        // exactly one chat model, chosen at startup, and reports a
        // model-specific id for it. The list is asked for instead, so a server
        // that is not running leaves the provider empty rather than wrong.
        models: Vec::new(),
        fetch_models: Some(Arc::new(move |context| {
            let config = config.clone();
            let signal = context.signal.clone();
            Box::pin(async move { fetch_served_models(config, signal).await })
        })),
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}

fn build_model(config: &MtplxProviderConfig, spec: &MtplxModelSpec) -> Model {
    let mut headers = BTreeMap::new();
    headers.insert("x-mtplx-client".to_string(), config.client_hint.clone());
    Model {
        id: spec.id.clone(),
        name: spec.name.clone(),
        api: Api::from(MTPLX_API),
        provider: config.id.clone(),
        base_url: config.base_url.clone(),
        reasoning: spec.reasoning,
        thinking_level_map: None,
        input: vec![Modality::Text],
        // A local server bills nothing. Reporting real zeroes keeps cost
        // accounting honest rather than inventing a rate.
        cost: ModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            tiers: None,
        },
        context_window: spec.context_window,
        max_tokens: spec.max_tokens,
        sampling_params: None,
        headers: Some(headers),
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            send_session_affinity_headers: Some(true),
            session_affinity_format: Some(SessionAffinityFormat::Mtplx),
            ..Default::default()
        })),
    }
}

/// Key handling for an MTPLX instance.
///
/// `resolve` deliberately has no unconditional fallback: an instance nobody
/// activated resolves to nothing, which keeps it out of the model list until
/// `/login` stores a credential. The placeholder lives in `login`, so
/// activation is an explicit act with an explicit undo (`/logout`).
struct MtplxApiKeyAuth {
    name: String,
    env_var: String,
    placeholder: Option<String>,
}

impl ApiKeyAuth for MtplxApiKeyAuth {
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
            if let Some(placeholder) = &self.placeholder {
                // No prompt: the server ignores the value, and asking for a
                // secret that does not exist teaches the wrong thing.
                return Ok(ApiKeyCredential {
                    key: Some(placeholder.clone()),
                    env: None,
                });
            }
            let key = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Secret {
                        message: format!("Enter {}", self.name),
                        placeholder: None,
                    },
                })
                .await?;
            if interaction.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            Ok(ApiKeyCredential {
                key: Some(key),
                env: None,
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
            if let Some(value) = input
                .ctx
                .env(&self.env_var)
                .await
                .filter(|value| !value.is_empty())
            {
                return Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(value),
                        ..Default::default()
                    },
                    env: None,
                    source: Some(self.env_var.clone()),
                }));
            }
            Ok(None)
        })
    }
}

/// The config plus what the factory derived from it, shared into the fetch
/// closure.
struct MtplxRuntimeConfig {
    config: MtplxProviderConfig,
    timeout: Duration,
}

/// Asks the server which model it is currently serving.
///
/// A server holds exactly one chat model and names it with a model-specific
/// id, so this is the only honest source for the list. Not reachable is the
/// normal case for a local server, and yields an empty list rather than an
/// error: an agent that has not started MTPLX should see no MTPLX models, not
/// a warning. A server that answers but answers badly is a real fault and is
/// reported as one.
async fn fetch_served_models(
    runtime: Arc<MtplxRuntimeConfig>,
    signal: tokio_util::sync::CancellationToken,
) -> Result<Vec<Model>, String> {
    let url = format!("{}/models", runtime.config.base_url.trim_end_matches('/'));
    let request = reqwest::Client::new()
        .get(&url)
        .header("Accept", "application/json")
        .timeout(runtime.timeout);
    let response = tokio::select! {
        response = request.send() => response,
        _ = signal.cancelled() => return Ok(Vec::new()),
    };
    let response = match response {
        Ok(response) => response,
        // A timeout is not absence: a server deep in a long generation can
        // miss the deadline, and treating that as "not running" would publish
        // and persist an empty list while the model is visibly working.
        // Failing instead keeps the restored list on offer.
        Err(error) if error.is_timeout() => {
            return Err(format!("MTPLX model list at {url}: timed out"));
        }
        // Refused or unresolvable: no server here — the ordinary case for a
        // local instance, and an empty offer rather than a warning.
        Err(_) => return Ok(Vec::new()),
    };
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("MTPLX model list at {url}: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "MTPLX model list at {url}: HTTP {}",
            status.as_u16()
        ));
    }
    let parsed: Value = serde_json::from_str(&body)
        .map_err(|error| format!("MTPLX model list at {url}: {error}"))?;
    models_from_listing(&runtime.config, &parsed)
        .map_err(|error| format!("MTPLX model list at {url}: {error}"))
}

/// Turns one `/v1/models` body into the models this provider offers.
///
/// Split from the request so the shape handling is exercised without a server.
pub fn models_from_listing(
    config: &MtplxProviderConfig,
    body: &Value,
) -> Result<Vec<Model>, String> {
    let entries = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "no \"data\" array".to_string())?;
    Ok(entries
        .iter()
        .filter_map(|entry| model_from_listing(config, entry))
        .collect())
}

fn model_from_listing(config: &MtplxProviderConfig, entry: &Value) -> Option<Model> {
    // Embedders and rerankers are listed only on request, but a future server
    // could widen the default listing; a retrieval model offered as a chat
    // target can only end in a 400.
    let capability = entry.get("capability").and_then(Value::as_str);
    if capability.is_some_and(|capability| capability != "chat") {
        return None;
    }
    let id = entry
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?;
    let window = ["context_length", "max_model_len", "max_context_length"]
        .iter()
        .find_map(|key| entry.get(*key).and_then(Value::as_u64))
        .filter(|window| *window > 0);
    let mut spec = MtplxModelSpec::new(id, display_name(id));
    if let Some(window) = window {
        spec.context_window = window;
        // No separate output ceiling is reported, and inventing a smaller one
        // would truncate long generations the server was willing to produce.
        spec.max_tokens = window;
    }
    Some(build_model(config, &spec))
}

/// `mtplx-qwen35-4b-optimized-speed` reads better as `Qwen35 4b Optimized Speed`
/// in a model picker than as its wire id.
fn display_name(id: &str) -> String {
    let words: Vec<String> = id
        .trim_start_matches("mtplx-")
        .split(['-', '_'])
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
