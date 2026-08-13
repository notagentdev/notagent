//! `ant_lingProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/ant-ling.ts` (15 LOC). The generated
//! `ant-ling.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `ant_lingProvider()`
pub fn ant_ling_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "ant-ling".to_string(),
        name: Some("Ant Ling".to_string()),
        base_url: Some("https://api.ant-ling.com/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Ant Ling API key",
                ["ANT_LING_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("ant-ling"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
