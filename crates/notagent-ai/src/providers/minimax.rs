use std::sync::Arc;

use crate::api::streams::AnthropicMessagesApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `minimaxProvider()`
pub fn minimax_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "minimax".to_string(),
        name: Some("MiniMax".to_string()),
        base_url: Some("https://api.minimax.io/anthropic".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "MiniMax API key",
                ["MINIMAX_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("minimax"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(AnthropicMessagesApi)),
    })
}
