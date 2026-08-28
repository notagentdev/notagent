use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `moonshotai_cnProvider()`
pub fn moonshotai_cn_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "moonshotai-cn".to_string(),
        name: Some("Moonshot AI CN".to_string()),
        base_url: Some("https://api.moonshot.cn/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Moonshot AI API key",
                ["MOONSHOT_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("moonshotai-cn"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
