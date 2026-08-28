use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `xiaomi_token_plan_cnProvider()`
pub fn xiaomi_token_plan_cn_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "xiaomi-token-plan-cn".to_string(),
        name: Some("Xiaomi Token Plan CN".to_string()),
        base_url: Some("https://token-plan-cn.xiaomimimo.com/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Xiaomi Token Plan CN API key",
                ["XIAOMI_TOKEN_PLAN_CN_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("xiaomi-token-plan-cn"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
