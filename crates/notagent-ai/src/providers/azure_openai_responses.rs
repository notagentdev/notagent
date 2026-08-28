use std::sync::Arc;

use crate::api::streams::AzureOpenAIResponsesApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `azure_openai_responsesProvider()`
pub fn azure_openai_responses_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "azure-openai-responses".to_string(),
        name: Some("Azure OpenAI".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Azure OpenAI API key",
                ["AZURE_OPENAI_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("azure-openai-responses"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(AzureOpenAIResponsesApi)),
    })
}
