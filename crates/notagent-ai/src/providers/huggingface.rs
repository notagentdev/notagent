use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `huggingfaceProvider()`
pub fn huggingface_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "huggingface".to_string(),
        name: Some("Hugging Face".to_string()),
        base_url: Some("https://router.huggingface.co/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Hugging Face token",
                ["HF_TOKEN"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("huggingface"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
