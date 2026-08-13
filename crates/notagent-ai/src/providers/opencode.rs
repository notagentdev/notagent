//! `opencodeProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/opencode.ts` (24 LOC). The generated
//! `opencode.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::{
    AnthropicMessagesApi, GoogleGenerativeAIApi, OpenAICompletionsApi, OpenAIResponsesApi,
};
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `opencodeProvider()`
pub fn opencode_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "opencode".to_string(),
        name: Some("OpenCode Zen".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "OpenCode API key",
                ["OPENCODE_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("opencode"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::ByApi(
            [
                (
                    "anthropic-messages".to_string(),
                    Arc::new(AnthropicMessagesApi) as Arc<dyn crate::types::ProviderStreams>,
                ),
                (
                    "google-generative-ai".to_string(),
                    Arc::new(GoogleGenerativeAIApi) as Arc<dyn crate::types::ProviderStreams>,
                ),
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
