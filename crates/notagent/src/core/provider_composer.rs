use std::collections::BTreeMap;
use std::sync::Arc;

use notagent_ai::api::streams::get_api_provider;
use notagent_ai::auth::resolve::ModelsError;
use notagent_ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthCheck, AuthError, AuthPrompt,
    AuthPromptKind, AuthResult, AuthType, BoxFuture, Credential, ModelAuth, OAuthAuth,
    OAuthCredential, ProviderAuth, ProviderAuthInteraction,
};
use notagent_ai::lazy_stream;
use notagent_ai::models::{Provider, RefreshModelsContext};
use notagent_ai::types::{
    Context, DeferredCancelOptions, DeferredFetchOptions, DeferredHandle, Modality, Model,
    ModelCompat, ModelCost, ProviderHeaders, SimpleStreamOptions, StreamOptions,
};
use notagent_ai::utils::event_stream::AssistantMessageEventStream;
use serde_json::{Map, Value};

use crate::core::model_config::{
    ModelConfig, ModelsJsonModel, ModelsJsonModelOverride, ModelsJsonProvider,
};
use crate::core::resolve_config_value::{
    clear_config_value_cache, get_config_value_env_var_names, is_command_config_value,
    is_config_value_configured, resolve_config_value_or_throw, resolve_headers_or_throw,
};

/// `AuthStatus`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub configured: bool,
    pub source: Option<AuthStatusSource>,
    pub label: Option<String>,
}

impl AuthStatus {
    pub fn unconfigured() -> Self {
        AuthStatus {
            configured: false,
            source: None,
            label: None,
        }
    }

    pub fn configured(source: AuthStatusSource) -> Self {
        AuthStatus {
            configured: true,
            source: Some(source),
            label: None,
        }
    }
}

/// `"stored" | "runtime" | "environment" | "fallback" | "models_json_key" | "models_json_command"`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatusSource {
    Stored,
    Runtime,
    Environment,
    Fallback,
    ModelsJsonKey,
    ModelsJsonCommand,
}

/// `export const clearApiKeyCache = clearConfigValueCache`
pub fn clear_api_key_cache() {
    clear_config_value_cache();
}

// ---------------------------------------------------------------------------
// Model layering
// ---------------------------------------------------------------------------

/// The four compat keys that merge one level deep instead of being replaced.
const NESTED_COMPAT_KEYS: [&str; 4] = [
    "openRouterRouting",
    "vercelGatewayRouting",
    "chatTemplateKwargs",
    "chatTemplateArgs",
];

fn compat_to_value(compat: Option<&ModelCompat>) -> Option<Value> {
    compat.map(|compat| serde_json::to_value(compat).expect("compat serializes"))
}

/// `mergeCompat(base, override)` in JSON space; the result is reparsed against the
/// model's api by [`compat_from_value`].
fn merge_compat(base: Option<Value>, override_compat: Option<&Value>) -> Option<Value> {
    let Some(override_compat) = override_compat else {
        return base;
    };
    let mut merged: Map<String, Value> = base
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(object) = override_compat.as_object() {
        for (key, value) in object {
            merged.insert(key.clone(), value.clone());
        }
    }
    for key in NESTED_COMPAT_KEYS {
        let base_value = base
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|object| object.get(key));
        let override_value = override_compat
            .as_object()
            .and_then(|object| object.get(key));
        let base_object = base_value.filter(|value| value.is_object());
        let override_object = override_value.filter(|value| value.is_object());
        if base_object.is_none() && override_object.is_none() {
            continue;
        }
        let mut nested: Map<String, Value> = base_object
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if let Some(object) = override_object.and_then(Value::as_object) {
            for (key, value) in object {
                nested.insert(key.clone(), value.clone());
            }
        }
        merged.insert(key.to_owned(), Value::Object(nested));
    }
    Some(Value::Object(merged))
}

/// Reparse a merged compat object against the model api.
/// different api survive unread. Rust parses into the api's compat struct, which
/// drops keys the api never reads.
fn compat_from_value(api: &str, value: Option<Value>) -> Option<ModelCompat> {
    let value = value?;
    ModelCompat::from_api_value(api, value).ok()
}

fn merge_model_compat(model: &Model, override_compat: Option<&Value>) -> Option<ModelCompat> {
    compat_from_value(
        &model.api,
        merge_compat(compat_to_value(model.compat.as_ref()), override_compat),
    )
}

/// `applyModelOverride(model, override)`
fn apply_model_override(model: &Model, override_model: &ModelsJsonModelOverride) -> Model {
    let mut next = model.clone();
    if let Some(name) = &override_model.name {
        next.name = name.clone();
    }
    if let Some(reasoning) = override_model.reasoning {
        next.reasoning = reasoning;
    }
    if let Some(map) = &override_model.thinking_level_map {
        let mut merged = next.thinking_level_map.unwrap_or_default();
        for (level, value) in map {
            merged.insert(*level, value.clone());
        }
        next.thinking_level_map = Some(merged);
    }
    if let Some(input) = &override_model.input {
        next.input = input.clone();
    }
    if let Some(cost) = &override_model.cost {
        next.cost = ModelCost {
            input: cost.input.unwrap_or(model.cost.input),
            output: cost.output.unwrap_or(model.cost.output),
            cache_read: cost.cache_read.unwrap_or(model.cost.cache_read),
            cache_write: cost.cache_write.unwrap_or(model.cost.cache_write),
            tiers: cost.tiers.clone().or_else(|| model.cost.tiers.clone()),
        };
    }
    if let Some(context_window) = override_model.context_window {
        next.context_window = context_window as u64;
    }
    if let Some(max_tokens) = override_model.max_tokens {
        next.max_tokens = max_tokens as u64;
    }
    if let Some(sampling_params) = &override_model.sampling_params {
        let mut merged = next.sampling_params.unwrap_or_default();
        for (key, value) in sampling_params {
            merged.insert(key.clone(), value.clone());
        }
        next.sampling_params = Some(merged);
    }
    next.compat = merge_model_compat(model, override_model.compat.as_ref());
    next
}

/// `modelFromJson(providerId, definition, providerConfig, defaults)`
fn model_from_json(
    provider_id: &str,
    definition: &ModelsJsonModel,
    provider_config: &ModelsJsonProvider,
    defaults: Option<&Model>,
) -> Result<Model, String> {
    let api = definition
        .api
        .clone()
        .or_else(|| provider_config.api.clone())
        .or_else(|| defaults.map(|model| model.api.clone()));
    let Some(api) = api else {
        return Err(format!(
            "Provider {provider_id}, model {}: no \"api\" specified. Set at provider or model level.",
            definition.id
        ));
    };
    let base_url = definition
        .base_url
        .clone()
        .or_else(|| provider_config.base_url.clone())
        .or_else(|| defaults.map(|model| model.base_url.clone()));
    let Some(base_url) = base_url else {
        return Err(format!(
            "Provider {provider_id}: \"baseUrl\" is required when defining custom models."
        ));
    };
    if definition.context_window.is_some_and(|value| value <= 0.0) {
        return Err(format!(
            "Provider {provider_id}, model {}: invalid contextWindow",
            definition.id
        ));
    }
    if definition.max_tokens.is_some_and(|value| value <= 0.0) {
        return Err(format!(
            "Provider {provider_id}, model {}: invalid maxTokens",
            definition.id
        ));
    }
    let compat = compat_from_value(
        &api,
        merge_compat(provider_config.compat.clone(), definition.compat.as_ref()),
    );
    Ok(Model {
        id: definition.id.clone(),
        name: definition
            .name
            .clone()
            .unwrap_or_else(|| definition.id.clone()),
        api,
        provider: provider_id.to_owned(),
        base_url,
        reasoning: definition.reasoning.unwrap_or(false),
        thinking_level_map: definition.thinking_level_map.clone(),
        input: definition
            .input
            .clone()
            .unwrap_or_else(|| vec![Modality::Text]),
        cost: definition
            .cost
            .as_ref()
            .map(|cost| ModelCost {
                input: cost.input,
                output: cost.output,
                cache_read: cost.cache_read,
                cache_write: cost.cache_write,
                tiers: cost.tiers.clone(),
            })
            .unwrap_or(ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: None,
            }),
        context_window: definition
            .context_window
            .map_or(128_000, |value| value as u64),
        max_tokens: definition.max_tokens.map_or(16_384, |value| value as u64),
        sampling_params: definition.sampling_params.clone(),
        headers: None,
        compat,
    })
}

/// `applyModelsJson(providerId, baseModels, config)`
fn apply_models_json(
    provider_id: &str,
    base_models: &[Model],
    config: Option<&ModelsJsonProvider>,
) -> Result<Vec<Model>, String> {
    let Some(config) = config else {
        return Ok(base_models.to_vec());
    };
    if config.oauth.is_some() && config.base_url.is_none() {
        return Err(format!(
            "Provider {provider_id}: \"baseUrl\" is required when \"oauth\" is set."
        ));
    }
    let has_overrides = config
        .model_overrides
        .as_ref()
        .is_some_and(|overrides| !overrides.is_empty());
    if config
        .models
        .as_ref()
        .is_none_or(|models| models.is_empty())
        && config.base_url.is_none()
        && config.headers.is_none()
        && config.compat.is_none()
        && !has_overrides
        && config.api_key.is_none()
        && config.oauth.is_none()
        && config.auth_header.is_none()
    {
        return Err(format!(
            "Provider {provider_id}: must specify \"baseUrl\", \"headers\", \"compat\", \"modelOverrides\", or \"models\"."
        ));
    }

    let mut models: Vec<Model> = base_models
        .iter()
        .map(|model| {
            let mut next = model.clone();
            if config.oauth.as_deref() != Some("radius")
                && let Some(base_url) = &config.base_url
            {
                next.base_url = base_url.clone();
            }
            next.compat = merge_model_compat(model, config.compat.as_ref());
            next
        })
        .collect();
    for definition in config.models.iter().flatten() {
        let existing_index = models.iter().position(|model| model.id == definition.id);
        let defaults = existing_index
            .map(|index| models[index].clone())
            .or_else(|| models.first().cloned());
        let model = model_from_json(provider_id, definition, config, defaults.as_ref())?;
        match existing_index {
            Some(index) => models[index] = model,
            None => models.push(model),
        }
    }
    Ok(models)
}

// ---------------------------------------------------------------------------
// Auth layering
// ---------------------------------------------------------------------------

/// `withConfiguredAuth(auth, headers, authHeader)`
fn with_configured_auth(
    auth: ModelAuth,
    headers: Option<BTreeMap<String, String>>,
    auth_header: bool,
) -> Result<ModelAuth, String> {
    let mut merged: Option<ProviderHeaders> = match (&auth.headers, &headers) {
        (None, None) => None,
        _ => {
            let mut merged = auth.headers.clone().unwrap_or_default();
            for (name, value) in headers.into_iter().flatten() {
                merged.insert(name, Some(value));
            }
            Some(merged)
        }
    };
    if auth_header {
        let Some(api_key) = &auth.api_key else {
            return Err("authHeader requires a resolved API key".to_owned());
        };
        let mut headers = merged.unwrap_or_default();
        headers.insert(
            "Authorization".to_owned(),
            Some(format!("Bearer {api_key}")),
        );
        merged = Some(headers);
    }
    Ok(ModelAuth {
        headers: merged,
        ..auth
    })
}

/// `configuredApiKey(config, extension)` without the extension layer.
fn configured_api_key(config: Option<&ModelsJsonProvider>) -> Option<String> {
    config.and_then(|config| config.api_key.clone())
}

/// `configuredHeaders(config, extension)` without the extension layer.
fn configured_headers(config: Option<&ModelsJsonProvider>) -> Option<BTreeMap<String, String>> {
    config.and_then(|config| config.headers.clone())
}

/// `configContextEnv(values, ctx, explicit?)`
async fn config_context_env(
    values: &[String],
    ctx: &dyn notagent_ai::auth::types::AuthContext,
    explicit: Option<&BTreeMap<String, String>>,
) -> Option<BTreeMap<String, String>> {
    let mut env: BTreeMap<String, String> = explicit.cloned().unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    for value in values {
        for name in get_config_value_env_var_names(value) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    for name in names {
        if env.contains_key(&name) {
            continue;
        }
        if let Some(value) = ctx.env(&name).await {
            env.insert(name, value);
        }
    }
    if env.is_empty() { None } else { Some(env) }
}

fn auth_error(message: String) -> AuthError {
    AuthError(message)
}

/// `composeApiKeyAuth(providerId, base, config, extension)`
struct ComposedApiKeyAuth {
    provider_id: String,
    inherited: Option<Arc<dyn ApiKeyAuth>>,
    raw_key: Option<String>,
    raw_headers: Option<BTreeMap<String, String>>,
    auth_header: bool,
    name: String,
}

impl ComposedApiKeyAuth {
    fn input<'a>(input: &'a ApiKeyAuthInput<'a>) -> ApiKeyAuthInput<'a> {
        ApiKeyAuthInput {
            ctx: input.ctx,
            credential: input.credential.clone(),
            signal: input.signal.clone(),
        }
    }
}

impl ApiKeyAuth for ComposedApiKeyAuth {
    fn name(&self) -> &str {
        &self.name
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        if let Some(inherited) = &self.inherited
            && let Some(login) = inherited.login(interaction)
        {
            return Some(login);
        }
        Some(Box::pin(async move {
            let key = interaction
                .prompt(AuthPrompt {
                    signal: Some(interaction.signal.clone()),
                    kind: AuthPromptKind::Secret {
                        message: "Enter API key".to_owned(),
                        placeholder: None,
                    },
                })
                .await?;
            Ok(ApiKeyCredential {
                key: Some(key),
                env: None,
            })
        }))
    }

    fn check<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> Option<BoxFuture<'a, Result<Option<AuthCheck>, AuthError>>> {
        Some(Box::pin(async move {
            if input.credential.is_some() {
                if let Some(inherited) = &self.inherited
                    && let Some(check) = inherited.check(Self::input(&input))
                {
                    return check.await;
                }
                if input
                    .credential
                    .as_ref()
                    .is_some_and(|credential| credential.key.is_some())
                {
                    return Ok(Some(AuthCheck {
                        source: Some("stored credential".to_owned()),
                        check_type: AuthType::ApiKey,
                    }));
                }
                let resolved = match &self.inherited {
                    Some(inherited) => inherited.resolve(Self::input(&input)).await?,
                    None => None,
                };
                return Ok(resolved.map(|resolved| AuthCheck {
                    source: resolved.source,
                    check_type: AuthType::ApiKey,
                }));
            }
            if let Some(raw_key) = &self.raw_key {
                if is_command_config_value(raw_key) {
                    return Ok(Some(AuthCheck {
                        source: Some("configured API key".to_owned()),
                        check_type: AuthType::ApiKey,
                    }));
                }
                for name in get_config_value_env_var_names(raw_key) {
                    if input.ctx.env(&name).await.is_none() {
                        return Ok(None);
                    }
                }
                return Ok(Some(AuthCheck {
                    source: Some("configured API key".to_owned()),
                    check_type: AuthType::ApiKey,
                }));
            }
            if let Some(inherited) = &self.inherited
                && let Some(check) = inherited.check(Self::input(&input))
            {
                return check.await;
            }
            let resolved = match &self.inherited {
                Some(inherited) => inherited.resolve(Self::input(&input)).await?,
                None => None,
            };
            Ok(resolved.map(|resolved| AuthCheck {
                source: resolved.source,
                check_type: AuthType::ApiKey,
            }))
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            let result: Option<AuthResult> = if let Some(credential) = input.credential.clone() {
                match &self.inherited {
                    Some(inherited) => inherited.resolve(Self::input(&input)).await?,
                    None => credential.key.map(|key| AuthResult {
                        auth: ModelAuth {
                            api_key: Some(key),
                            headers: None,
                            base_url: None,
                        },
                        env: credential.env.clone(),
                        source: Some("stored credential".to_owned()),
                    }),
                }
            } else if let Some(raw_key) = &self.raw_key {
                let env = config_context_env(std::slice::from_ref(raw_key), input.ctx, None).await;
                let key = resolve_config_value_or_throw(
                    raw_key,
                    &format!("API key for provider \"{}\"", self.provider_id),
                    env.as_ref(),
                )
                .map_err(|error| auth_error(error.to_string()))?;
                match &self.inherited {
                    Some(inherited) => {
                        inherited
                            .resolve(ApiKeyAuthInput {
                                ctx: input.ctx,
                                credential: Some(ApiKeyCredential {
                                    key: Some(key),
                                    env: None,
                                }),
                                signal: input.signal.clone(),
                            })
                            .await?
                    }
                    None => Some(AuthResult {
                        auth: ModelAuth {
                            api_key: Some(key),
                            headers: None,
                            base_url: None,
                        },
                        env: None,
                        source: Some("configured API key".to_owned()),
                    }),
                }
            } else {
                match &self.inherited {
                    Some(inherited) => inherited.resolve(Self::input(&input)).await?,
                    None => None,
                }
            };
            let Some(result) = result else {
                return Ok(None);
            };
            let mut explicit_env: BTreeMap<String, String> = input
                .credential
                .as_ref()
                .and_then(|credential| credential.env.clone())
                .unwrap_or_default();
            for (name, value) in result.env.clone().unwrap_or_default() {
                explicit_env.insert(name, value);
            }
            let header_values: Vec<String> = self
                .raw_headers
                .as_ref()
                .map(|headers| headers.values().cloned().collect())
                .unwrap_or_default();
            let header_env =
                config_context_env(&header_values, input.ctx, Some(&explicit_env)).await;
            let headers = resolve_headers_or_throw(
                self.raw_headers.as_ref(),
                &format!("provider \"{}\"", self.provider_id),
                header_env.as_ref(),
            )
            .map_err(|error| auth_error(error.to_string()))?;
            let auth = with_configured_auth(result.auth.clone(), headers, self.auth_header)
                .map_err(auth_error)?;
            Ok(Some(AuthResult { auth, ..result }))
        })
    }
}

fn compose_api_key_auth(
    provider_id: &str,
    base: Option<&Arc<dyn Provider>>,
    config: Option<&ModelsJsonProvider>,
) -> Option<Arc<dyn ApiKeyAuth>> {
    let inherited = base.and_then(|base| base.auth().api_key.clone());
    let raw_key = configured_api_key(config);
    let oauth = base.and_then(|base| base.auth().oauth.clone());
    // OAuth-only providers get no fabricated API-key login method.
    if inherited.is_none() && raw_key.is_none() && oauth.is_some() {
        return None;
    }
    let name = inherited
        .as_ref()
        .map_or_else(|| "API key".to_owned(), |auth| auth.name().to_owned());
    Some(Arc::new(ComposedApiKeyAuth {
        provider_id: provider_id.to_owned(),
        inherited,
        raw_key,
        raw_headers: configured_headers(config),
        auth_header: config
            .and_then(|config| config.auth_header)
            .unwrap_or(false),
        name,
    }))
}

/// `composeOAuthAuth(providerId, base, config, extension)`
struct ComposedOAuthAuth {
    provider_id: String,
    inner: Arc<dyn OAuthAuth>,
    raw_headers: Option<BTreeMap<String, String>>,
    auth_header: bool,
}

impl OAuthAuth for ComposedOAuthAuth {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn is_subscription(&self) -> bool {
        self.inner.is_subscription()
    }

    fn login_label(&self) -> Option<&str> {
        self.inner.login_label()
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        self.inner.login(interaction)
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        signal: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        self.inner.refresh(credential, signal)
    }

    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
        Box::pin(async move {
            let env = credential.extra.get("env").and_then(|env| {
                env.as_object().map(|env| {
                    env.iter()
                        .filter_map(|(name, value)| {
                            value.as_str().map(|value| (name.clone(), value.to_owned()))
                        })
                        .collect::<BTreeMap<String, String>>()
                })
            });
            let auth = self.inner.to_auth(credential).await?;
            let headers = resolve_headers_or_throw(
                self.raw_headers.as_ref(),
                &format!("provider \"{}\"", self.provider_id),
                env.as_ref(),
            )
            .map_err(|error| auth_error(error.to_string()))?;
            with_configured_auth(auth, headers, self.auth_header).map_err(auth_error)
        })
    }
}

fn compose_oauth_auth(
    provider_id: &str,
    base: Option<&Arc<dyn Provider>>,
    config: Option<&ModelsJsonProvider>,
) -> Option<Arc<dyn OAuthAuth>> {
    let inner = base.and_then(|base| base.auth().oauth.clone())?;
    Some(Arc::new(ComposedOAuthAuth {
        provider_id: provider_id.to_owned(),
        inner,
        raw_headers: configured_headers(config),
        auth_header: config
            .and_then(|config| config.auth_header)
            .unwrap_or(false),
    }))
}

/// `rawModelHeaders(model, config, extension)`
fn raw_model_headers(
    model: &Model,
    config: Option<&ModelsJsonProvider>,
) -> Option<BTreeMap<String, String>> {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    if let Some(config) = config {
        if let Some(override_headers) = config
            .model_overrides
            .as_ref()
            .and_then(|overrides| overrides.get(&model.id))
            .and_then(|override_model| override_model.headers.as_ref())
        {
            headers.extend(override_headers.clone());
        }
        if let Some(definition_headers) = config
            .models
            .as_ref()
            .and_then(|models| models.iter().find(|entry| entry.id == model.id))
            .and_then(|definition| definition.headers.as_ref())
        {
            headers.extend(definition_headers.clone());
        }
    }
    if headers.is_empty() {
        None
    } else {
        Some(headers)
    }
}

// ---------------------------------------------------------------------------
// Composed provider
// ---------------------------------------------------------------------------

struct ComposedProvider {
    id: String,
    name: String,
    base_url: Option<String>,
    headers: Option<ProviderHeaders>,
    auth: ProviderAuth,
    base: Option<Arc<dyn Provider>>,
    config: Option<ModelsJsonProvider>,
}

impl ComposedProvider {
    fn layered_models(&self) -> Result<Vec<Model>, String> {
        let base_models = self
            .base
            .as_ref()
            .map(|base| base.get_models())
            .unwrap_or_default();
        let models = apply_models_json(&self.id, &base_models, self.config.as_ref())?;
        Ok(models
            .iter()
            .map(|model| {
                match self
                    .config
                    .as_ref()
                    .and_then(|config| config.model_overrides.as_ref())
                    .and_then(|overrides| overrides.get(&model.id))
                {
                    Some(override_model) => apply_model_override(model, override_model),
                    None => model.clone(),
                }
            })
            .collect())
    }

    fn supports_base_api(&self, model: &Model) -> bool {
        self.base
            .as_ref()
            .is_some_and(|base| base.get_models().iter().any(|entry| entry.api == model.api))
    }

    fn stream_with(
        &self,
        model: &Model,
        context: &Context,
        request: StreamRequest,
    ) -> AssistantMessageEventStream {
        let base = self
            .supports_base_api(model)
            .then(|| self.base.clone())
            .flatten();
        let model = model.clone();
        let context = context.clone();
        let setup_model = model.clone();
        lazy_stream(setup_model, move || async move {
            let streams: StreamTarget = match base {
                Some(base) => StreamTarget::Provider(base),
                None => match get_api_provider(&model.api) {
                    Some(api) => StreamTarget::Api(api),
                    None => {
                        return Err(format!("No API provider registered for api: {}", model.api));
                    }
                },
            };
            Ok(match request {
                StreamRequest::Full(options) => match streams {
                    StreamTarget::Provider(base) => base.stream(&model, &context, options),
                    StreamTarget::Api(api) => api.stream(&model, &context, options),
                },
                StreamRequest::Simple(options) => match streams {
                    StreamTarget::Provider(base) => base.stream_simple(&model, &context, options),
                    StreamTarget::Api(api) => api.stream_simple(&model, &context, options),
                },
            })
        })
    }
}

enum StreamRequest {
    Full(Option<StreamOptions>),
    Simple(Option<SimpleStreamOptions>),
}

enum StreamTarget {
    Provider(Arc<dyn Provider>),
    Api(Arc<dyn notagent_ai::types::ProviderStreams>),
}

impl Provider for ComposedProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    fn headers(&self) -> Option<&ProviderHeaders> {
        self.headers.as_ref()
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    /// Deviation (class 1): `getModels` must not fail in Rust, so a layering error
    /// that only appears after the base catalog changed yields an empty list; the
    /// same inputs are rejected eagerly by [`compose_model_provider`].
    fn get_models(&self) -> Vec<Model> {
        self.layered_models().unwrap_or_default()
    }

    fn is_dynamic(&self) -> bool {
        self.base.as_ref().is_some_and(|base| base.is_dynamic())
    }

    fn refresh_models<'a>(
        &'a self,
        context: RefreshModelsContext<'a>,
    ) -> Option<BoxFuture<'a, Result<(), String>>> {
        self.base.as_ref()?.refresh_models(context)
    }

    fn filter_models(&self, models: Vec<Model>, credential: Option<&Credential>) -> Vec<Model> {
        match &self.base {
            Some(base) => base.filter_models(models, credential),
            None => models,
        }
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        self.stream_with(model, context, StreamRequest::Full(options))
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        self.stream_with(model, context, StreamRequest::Simple(options))
    }

    fn fetch_deferred(
        &self,
        model: &Model,
        handle: &DeferredHandle,
        options: Option<DeferredFetchOptions>,
    ) -> Option<AssistantMessageEventStream> {
        self.base.as_ref()?.fetch_deferred(model, handle, options)
    }

    fn cancel_deferred<'a>(
        &'a self,
        model: &'a Model,
        handle: &'a DeferredHandle,
        options: Option<DeferredCancelOptions>,
    ) -> Option<BoxFuture<'a, Result<(), ModelsError>>> {
        self.base.as_ref()?.cancel_deferred(model, handle, options)
    }
}

/// Compose built-in and models.json layers without reading credentials.
pub fn compose_model_provider(
    provider_id: &str,
    base: Option<Arc<dyn Provider>>,
    model_config: &ModelConfig,
) -> Result<Arc<dyn Provider>, String> {
    let config = model_config.get_provider(provider_id).cloned();
    let provider = ComposedProvider {
        id: provider_id.to_owned(),
        name: config
            .as_ref()
            .and_then(|config| config.name.clone())
            .or_else(|| base.as_ref().map(|base| base.name().to_owned()))
            .unwrap_or_else(|| provider_id.to_owned()),
        base_url: config
            .as_ref()
            .and_then(|config| config.base_url.clone())
            .or_else(|| {
                base.as_ref()
                    .and_then(|base| base.base_url().map(str::to_owned))
            }),
        headers: base.as_ref().and_then(|base| base.headers().cloned()),
        auth: ProviderAuth::default(),
        base: base.clone(),
        config: config.clone(),
    };
    // Validate eagerly so registration/reload reports structural errors immediately.
    provider.layered_models()?;
    let api_key = compose_api_key_auth(provider_id, base.as_ref(), config.as_ref());
    let oauth = compose_oauth_auth(provider_id, base.as_ref(), config.as_ref());
    if api_key.is_none() && oauth.is_none() {
        return Err(format!(
            "Provider {provider_id}: no authentication method configured."
        ));
    }
    Ok(Arc::new(ComposedProvider {
        auth: ProviderAuth { api_key, oauth },
        ..provider
    }))
}

/// `resolveConfiguredModelHeaders(model, config, extension, env?)`
pub fn resolve_configured_model_headers(
    model: &Model,
    config: Option<&ModelsJsonProvider>,
    env: Option<&BTreeMap<String, String>>,
) -> Result<Option<BTreeMap<String, String>>, String> {
    resolve_headers_or_throw(
        raw_model_headers(model, config).as_ref(),
        &format!("model \"{}/{}\"", model.provider, model.id),
        env,
    )
    .map_err(|error| error.to_string())
}

/// `CompatibilityRequestConfig`
#[derive(Debug, Clone, Default)]
pub struct CompatibilityRequestConfig {
    pub headers: Option<ProviderHeaders>,
    pub auth_header: bool,
}

/// `resolveCompatibilityRequestConfig(model, config, extension)`
pub fn resolve_compatibility_request_config(
    model: &Model,
    config: Option<&ModelsJsonProvider>,
) -> Result<CompatibilityRequestConfig, String> {
    let mut raw: BTreeMap<String, String> = configured_headers(config).unwrap_or_default();
    raw.extend(raw_model_headers(model, config).unwrap_or_default());
    let configured = resolve_headers_or_throw(
        (!raw.is_empty()).then_some(&raw),
        &format!("model \"{}/{}\"", model.provider, model.id),
        None,
    )
    .map_err(|error| error.to_string())?;
    let headers = match (&model.headers, &configured) {
        (None, None) => None,
        _ => {
            let mut headers: ProviderHeaders = model
                .headers
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|(name, value)| (name, Some(value)))
                .collect();
            for (name, value) in configured.into_iter().flatten() {
                headers.insert(name, Some(value));
            }
            Some(headers)
        }
    };
    Ok(CompatibilityRequestConfig {
        headers,
        auth_header: config
            .and_then(|config| config.auth_header)
            .unwrap_or(false),
    })
}

/// `configuredRequestAuthStatus(config, extension)`
pub fn configured_request_auth_status(config: Option<&ModelsJsonProvider>) -> Option<AuthStatus> {
    let value = configured_api_key(config)?;
    if is_command_config_value(&value) {
        return Some(AuthStatus::configured(AuthStatusSource::ModelsJsonCommand));
    }
    let names = get_config_value_env_var_names(&value);
    if !names.is_empty() {
        return Some(if is_config_value_configured(&value, None) {
            AuthStatus {
                configured: true,
                source: Some(AuthStatusSource::Environment),
                label: Some(names.join(", ")),
            }
        } else {
            AuthStatus::unconfigured()
        });
    }
    // Without the extension layer only the models.json key can reach this point.
    Some(AuthStatus::configured(AuthStatusSource::ModelsJsonKey))
}
