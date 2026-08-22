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
    pub models: Vec<MtplxModelSpec>,
}

impl MtplxProviderConfig {
    /// Whether the address stays on this machine. Decides both whether a real
    /// key is required and whether login may skip its prompt.
    pub fn is_loopback(&self) -> bool {
        let Some(rest) = self
            .base_url
            .split_once("://")
            .map(|(_, rest)| rest)
            .or(Some(self.base_url.as_str()))
        else {
            return false;
        };
        let host = rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or_else(|| rest.split(['/', '?', '#']).next().unwrap_or_default())
            .trim_matches(['[', ']'])
            .to_ascii_lowercase();
        host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "0:0:0:0:0:0:0:1"
    }
}

/// Builds one MTPLX provider from its config.
pub fn mtplx_provider(config: MtplxProviderConfig) -> Arc<BuiltProvider> {
    let loopback = config.is_loopback();
    let models = config
        .models
        .iter()
        .map(|spec| build_model(&config, spec))
        .collect();
    create_provider(CreateProviderOptions {
        id: config.id.clone(),
        name: Some(config.name.clone()),
        base_url: Some(config.base_url.clone()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(MtplxApiKeyAuth {
                name: format!("{} API key", config.name),
                env_var: "MTPLX_API_KEY".to_string(),
                // Only loopback gets a prompt-free login; a remote server
                // rejects a keyless start, so a placeholder would just move the
                // failure to the first request.
                placeholder: loopback.then(|| LOCAL_PLACEHOLDER_KEY.to_string()),
            })),
            oauth: None,
        },
        models,
        fetch_models: None,
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
