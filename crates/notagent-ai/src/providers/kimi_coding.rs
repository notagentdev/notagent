//! `kimi_codingProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/kimi-coding.ts` (24 LOC). The generated
//! `kimi-coding.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::AnthropicMessagesApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `kimi_codingProvider()`
pub fn kimi_coding_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "kimi-coding".to_string(),
        name: Some("Kimi For Coding".to_string()),
        base_url: Some("https://api.kimi.com/coding".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Kimi API key",
                ["KIMI_API_KEY"],
            ))),
            oauth: Some(crate::auth::oauth::kimi_coding::kimi_coding_oauth()),
        },
        models: get_builtin_models("kimi-coding"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(AnthropicMessagesApi)),
    })
}
