use std::sync::{Arc, Mutex};

use notagent_ai::auth::resolve::now_ms;
use notagent_ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthCheck, AuthError, AuthPrompt,
    AuthPromptKind, AuthResult, AuthType, BoxFuture, Credential, ModelAuth, ProviderAuth,
    ProviderAuthInteraction,
};
use notagent_ai::models::{ModelsPublication, Provider, RefreshModelsContext};
use notagent_ai::models_store::ModelsStoreEntry;
use notagent_ai::types::{
    Context, MaxTokensField, Modality, Model, ModelCompat, ModelCost, OpenAICompletionsCompat,
    ProviderEnv, SimpleStreamOptions, StreamOptions,
};
use notagent_ai::utils::event_stream::AssistantMessageEventStream;

use super::client::{
    LLAMA_STATUS_LOADED, LlamaClient, LlamaError, LlamaModelInfo, llama_inference_url,
    normalize_llama_server_url,
};

pub const LLAMA_PROVIDER_ID: &str = "llama.cpp";
pub const DEFAULT_LLAMA_SERVER_URL: &str = "http://127.0.0.1:8080";

/// `credentialServerUrl(credential)`
fn credential_server_url(
    credential: Option<&ApiKeyCredential>,
) -> Result<Option<String>, LlamaError> {
    let value = credential
        .and_then(|credential| credential.env.as_ref())
        .and_then(|env| env.get("LLAMA_BASE_URL"));
    match value {
        Some(value) if !value.trim().is_empty() => Ok(Some(normalize_llama_server_url(value)?)),
        _ => Ok(None),
    }
}

/// `resolveServerUrl(ctx, credential)`
async fn resolve_server_url(input: &ApiKeyAuthInput<'_>) -> Result<Option<String>, LlamaError> {
    if let Some(configured) = credential_server_url(input.credential.as_ref())? {
        return Ok(Some(configured));
    }
    let from_env = input.ctx.env("LLAMA_BASE_URL").await;
    let configured = from_env.as_deref().map(str::trim).unwrap_or("");
    if configured.is_empty() {
        return Ok(None);
    }
    Ok(Some(normalize_llama_server_url(configured)?))
}

/// `toPiModel(model, serverUrl)`
fn to_pi_model(model: &LlamaModelInfo, server_url: &str) -> Result<Model, LlamaError> {
    let reported_context_window = model
        .meta
        .as_ref()
        .and_then(|meta| meta.n_ctx.or(meta.n_ctx_train));
    let context_window = match reported_context_window {
        Some(reported) if reported > 0 => reported,
        _ => 128_000,
    };
    let has_image = model
        .architecture
        .as_ref()
        .and_then(|architecture| architecture.input_modalities.as_ref())
        .is_some_and(|modalities| modalities.iter().any(|entry| entry == "image"));
    Ok(Model {
        id: model.id.clone(),
        name: model.id.clone(),
        api: "openai-completions".to_owned(),
        provider: LLAMA_PROVIDER_ID.to_owned(),
        base_url: llama_inference_url(server_url)?,
        reasoning: false,
        thinking_level_map: None,
        input: if has_image {
            vec![Modality::Text, Modality::Image]
        } else {
            vec![Modality::Text]
        },
        cost: ModelCost::default(),
        context_window,
        max_tokens: context_window,
        sampling_params: None,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_reasoning_effort: Some(false),
            supports_usage_in_streaming: Some(true),
            supports_strict_mode: Some(false),
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            ..OpenAICompletionsCompat::default()
        })),
    })
}

/// The loaded entries of a router catalog as notagent models.
fn loaded_models(catalog: &[LlamaModelInfo], server_url: &str) -> Result<Vec<Model>, LlamaError> {
    catalog
        .iter()
        .filter(|model| model.status.value == LLAMA_STATUS_LOADED)
        .map(|model| to_pi_model(model, server_url))
        .collect()
}

/// `provider.auth.apiKey` of the llama.cpp provider.
struct LlamaApiKeyAuth;

impl ApiKeyAuth for LlamaApiKeyAuth {
    fn name(&self) -> &str {
        "llama.cpp server"
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            let entered_url = interaction
                .prompt(AuthPrompt {
                    signal: Some(interaction.signal.clone()),
                    kind: AuthPromptKind::Text {
                        message: "llama.cpp server URL".to_owned(),
                        placeholder: Some(
                            std::env::var("LLAMA_BASE_URL")
                                .unwrap_or_else(|_| DEFAULT_LLAMA_SERVER_URL.to_owned()),
                        ),
                    },
                })
                .await?;
            let entered_url = entered_url.trim();
            let fallback = std::env::var("LLAMA_BASE_URL").unwrap_or_default();
            let server_url = normalize_llama_server_url(if !entered_url.is_empty() {
                entered_url
            } else if fallback.is_empty() {
                DEFAULT_LLAMA_SERVER_URL
            } else {
                &fallback
            })
            .map_err(|error| AuthError(error.0))?;
            let api_key = interaction
                .prompt(AuthPrompt {
                    signal: Some(interaction.signal.clone()),
                    kind: AuthPromptKind::Secret {
                        message: "API key (optional)".to_owned(),
                        placeholder: None,
                    },
                })
                .await?;
            let api_key = api_key.trim();
            let api_key = (!api_key.is_empty()).then(|| api_key.to_owned());
            // Reachability check: a server that does not answer fails the login.
            LlamaClient::new(&server_url, api_key.as_deref())
                .map_err(|error| AuthError(error.0))?
                .list(false, Some(&interaction.signal))
                .await
                .map_err(|error| AuthError(error.0))?;
            Ok(ApiKeyCredential {
                key: api_key,
                env: Some(ProviderEnv::from([(
                    "LLAMA_BASE_URL".to_owned(),
                    server_url,
                )])),
            })
        }))
    }

    fn check<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> Option<BoxFuture<'a, Result<Option<AuthCheck>, AuthError>>> {
        Some(Box::pin(async move {
            let stored = input.credential.is_some();
            let server_url = resolve_server_url(&input)
                .await
                .map_err(|error| AuthError(error.0))?;
            Ok(server_url.map(|_| AuthCheck {
                source: Some(
                    if stored {
                        "stored credential"
                    } else {
                        "LLAMA_BASE_URL"
                    }
                    .to_owned(),
                ),
                check_type: AuthType::ApiKey,
            }))
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            let Some(server_url) = resolve_server_url(&input)
                .await
                .map_err(|error| AuthError(error.0))?
            else {
                return Ok(None);
            };
            let credential = input.credential.clone();
            let api_key = match credential
                .as_ref()
                .and_then(|credential| credential.key.clone())
            {
                Some(key) => key,
                None => input
                    .ctx
                    .env("LLAMA_API_KEY")
                    .await
                    .unwrap_or_else(|| "local".to_owned()),
            };
            let mut env = credential
                .as_ref()
                .and_then(|credential| credential.env.clone())
                .unwrap_or_default();
            env.insert("LLAMA_BASE_URL".to_owned(), server_url.clone());
            Ok(Some(AuthResult {
                auth: ModelAuth {
                    api_key: Some(api_key),
                    headers: None,
                    base_url: Some(
                        llama_inference_url(&server_url).map_err(|error| AuthError(error.0))?,
                    ),
                },
                env: Some(env),
                source: Some(
                    if credential.is_some() {
                        "stored credential"
                    } else {
                        "LLAMA_BASE_URL"
                    }
                    .to_owned(),
                ),
            }))
        })
    }
}

/// `LlamaProviderController` — the provider plus the catalog seam the `/llama`
/// command writes to after loading or unloading a model.
pub struct LlamaProviderController {
    pub provider: Arc<LlamaProvider>,
}

impl LlamaProviderController {
    /// `setCatalog(models, serverUrl)`
    pub fn set_catalog(
        &self,
        catalog: &[LlamaModelInfo],
        server_url: &str,
    ) -> Result<(), LlamaError> {
        let models = loaded_models(catalog, server_url)?;
        *self.provider.models.lock().expect("poisoned") = models;
        Ok(())
    }
}

/// The llama.cpp provider.
pub struct LlamaProvider {
    models: Mutex<Vec<Model>>,
    auth: ProviderAuth,
    base_url: String,
    streams: Arc<dyn notagent_ai::types::ProviderStreams>,
}

/// `createLlamaProvider()`
pub fn create_llama_provider() -> LlamaProviderController {
    LlamaProviderController {
        provider: Arc::new(LlamaProvider {
            models: Mutex::new(Vec::new()),
            auth: ProviderAuth {
                api_key: Some(Arc::new(LlamaApiKeyAuth)),
                oauth: None,
            },
            base_url: llama_inference_url(DEFAULT_LLAMA_SERVER_URL)
                .expect("the default server URL parses"),
            streams: Arc::new(notagent_ai::api::streams::OpenAICompletionsApi),
        }),
    }
}

impl LlamaProvider {
    async fn refresh<'a>(&'a self, context: RefreshModelsContext<'a>) -> Result<(), String> {
        if let Some(stored) = &context.stored {
            let restored: Vec<Model> = stored
                .models
                .iter()
                .filter(|model| {
                    model.provider == LLAMA_PROVIDER_ID && model.api == "openai-completions"
                })
                .cloned()
                .collect();
            let published = (context.publish)(ModelsPublication {
                persist: None,
                update: Some(Box::new(move || {
                    *self.models.lock().expect("poisoned") = restored;
                })),
            })
            .await;
            if !published {
                return Ok(());
            }
        }

        if !context.allow_network || context.signal.is_cancelled() {
            return Ok(());
        }
        let Some(Credential::ApiKey(credential)) = &context.credential else {
            return Ok(());
        };
        let Some(server_url) = credential_server_url(Some(credential)).map_err(|error| error.0)?
        else {
            return Ok(());
        };
        let catalog = LlamaClient::new(&server_url, credential.key.as_deref())
            .map_err(|error| error.0)?
            .list(false, Some(&context.signal))
            .await
            .map_err(|error| error.0)?;
        if context.signal.is_cancelled() {
            return Ok(());
        }
        let refreshed = loaded_models(&catalog, &server_url).map_err(|error| error.0)?;
        let persisted = refreshed.clone();
        (context.publish)(ModelsPublication {
            persist: Some(Some(ModelsStoreEntry {
                models: persisted,
                checked_at: Some(now_ms()),
                last_modified: None,
                etag: None,
            })),
            update: Some(Box::new(move || {
                *self.models.lock().expect("poisoned") = refreshed;
            })),
        })
        .await;
        Ok(())
    }
}

impl Provider for LlamaProvider {
    fn id(&self) -> &str {
        LLAMA_PROVIDER_ID
    }

    fn name(&self) -> &str {
        "llama.cpp"
    }

    fn base_url(&self) -> Option<&str> {
        Some(&self.base_url)
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Vec<Model> {
        self.models.lock().expect("poisoned").clone()
    }

    fn is_dynamic(&self) -> bool {
        true
    }

    fn refresh_models<'a>(
        &'a self,
        context: RefreshModelsContext<'a>,
    ) -> Option<BoxFuture<'a, Result<(), String>>> {
        Some(Box::pin(self.refresh(context)))
    }

    /// of a non-builtin provider resolves the API registry entry of `model.api`
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        self.streams.stream(model, context, options)
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        self.streams.stream_simple(model, context, options)
    }
}
