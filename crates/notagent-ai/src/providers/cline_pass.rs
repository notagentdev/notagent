use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `clinePassProvider()`
pub fn cline_pass_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "cline-pass".to_string(),
        name: Some("ClinePass".to_string()),
        base_url: Some("https://api.cline.bot/api/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Cline API key",
                ["CLINE_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("cline-pass"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
