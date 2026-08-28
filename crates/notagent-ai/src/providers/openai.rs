use std::sync::Arc;

use crate::api::streams::OpenAIResponsesApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `openaiProvider()`
pub fn openai_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "openai".to_string(),
        name: Some("OpenAI".to_string()),
        base_url: Some("https://api.openai.com/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "OpenAI API key",
                ["OPENAI_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("openai"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAIResponsesApi)),
    })
}
