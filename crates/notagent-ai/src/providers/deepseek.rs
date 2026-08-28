use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `deepseekProvider()`
pub fn deepseek_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "deepseek".to_string(),
        name: Some("DeepSeek".to_string()),
        base_url: Some("https://api.deepseek.com".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "DeepSeek API key",
                ["DEEPSEEK_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("deepseek"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
