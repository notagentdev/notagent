use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `zaiProvider()`
pub fn zai_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "zai".to_string(),
        name: Some("Z.AI".to_string()),
        base_url: Some("https://api.z.ai/api/coding/paas/v4".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Z.AI API key",
                ["ZAI_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("zai"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
