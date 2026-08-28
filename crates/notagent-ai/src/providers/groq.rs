use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `groqProvider()`
pub fn groq_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "groq".to_string(),
        name: Some("Groq".to_string()),
        base_url: Some("https://api.groq.com/openai/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Groq API key",
                ["GROQ_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("groq"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
