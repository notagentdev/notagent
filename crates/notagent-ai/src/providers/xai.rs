//! `xaiProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/xai.ts` (28 LOC). The generated
//! `xai.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::{OpenAICompletionsApi, OpenAIResponsesApi};
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `xaiProvider()`
pub fn xai_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "xai".to_string(),
        name: Some("xAI".to_string()),
        base_url: Some("https://api.x.ai/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new("xAI API key", ["XAI_API_KEY"]))),
            oauth: Some(crate::auth::oauth::xai::xai_oauth()),
        },
        models: get_builtin_models("xai"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::ByApi(
            [
                (
                    "openai-completions".to_string(),
                    Arc::new(OpenAICompletionsApi) as Arc<dyn crate::types::ProviderStreams>,
                ),
                (
                    "openai-responses".to_string(),
                    Arc::new(OpenAIResponsesApi) as Arc<dyn crate::types::ProviderStreams>,
                ),
            ]
            .into(),
        ),
    })
}
