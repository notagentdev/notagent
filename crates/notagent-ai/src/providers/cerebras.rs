//! `cerebrasProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/cerebras.ts` (15 LOC). The generated
//! `cerebras.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `cerebrasProvider()`
pub fn cerebras_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "cerebras".to_string(),
        name: Some("Cerebras".to_string()),
        base_url: Some("https://api.cerebras.ai/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Cerebras API key",
                ["CEREBRAS_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("cerebras"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
