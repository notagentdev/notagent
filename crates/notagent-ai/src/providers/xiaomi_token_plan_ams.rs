//! `xiaomi_token_plan_amsProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/xiaomi-token-plan-ams.ts` (15 LOC). The generated
//! `xiaomi-token-plan-ams.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `xiaomi_token_plan_amsProvider()`
pub fn xiaomi_token_plan_ams_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "xiaomi-token-plan-ams".to_string(),
        name: Some("Xiaomi Token Plan AMS".to_string()),
        base_url: Some("https://token-plan-ams.xiaomimimo.com/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Xiaomi Token Plan AMS API key",
                ["XIAOMI_TOKEN_PLAN_AMS_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("xiaomi-token-plan-ams"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
