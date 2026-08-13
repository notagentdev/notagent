//! `basetenProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/baseten.ts` (15 LOC). The generated
//! `baseten.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `basetenProvider()`
pub fn baseten_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "baseten".to_string(),
        name: Some("Baseten".to_string()),
        base_url: Some("https://inference.baseten.co/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Baseten API key",
                ["BASETEN_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("baseten"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
