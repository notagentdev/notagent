//! `openrouterProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/openrouter.ts` (23 LOC). The generated
//! `openrouter.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `openrouterProvider()`
pub fn openrouter_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "openrouter".to_string(),
        name: Some("OpenRouter".to_string()),
        base_url: Some("https://openrouter.ai/api/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "OpenRouter API key",
                ["OPENROUTER_API_KEY"],
            ))),
            oauth: Some(crate::auth::oauth::openrouter::open_router_oauth()),
        },
        models: get_builtin_models("openrouter"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
