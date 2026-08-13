//! `mistralProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/mistral.ts` (15 LOC). The generated
//! `mistral.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::MistralConversationsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `mistralProvider()`
pub fn mistral_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "mistral".to_string(),
        name: Some("Mistral".to_string()),
        base_url: Some("https://api.mistral.ai".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Mistral API key",
                ["MISTRAL_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("mistral"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(MistralConversationsApi)),
    })
}
