use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `moonshotaiProvider()`
pub fn moonshotai_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "moonshotai".to_string(),
        name: Some("Moonshot AI".to_string()),
        base_url: Some("https://api.moonshot.ai/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Moonshot AI API key",
                ["MOONSHOT_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("moonshotai"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
