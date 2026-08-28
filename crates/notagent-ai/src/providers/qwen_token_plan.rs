use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `qwen_token_planProvider()`
pub fn qwen_token_plan_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "qwen-token-plan".to_string(),
        name: Some("Qwen Token Plan".to_string()),
        base_url: Some(
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1".to_string(),
        ),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Qwen Token Plan API key",
                ["QWEN_TOKEN_PLAN_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("qwen-token-plan"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
