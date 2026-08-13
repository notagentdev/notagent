//! `opencode_goProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/opencode-go.ts` (20 LOC). The generated
//! `opencode-go.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::{AnthropicMessagesApi, OpenAICompletionsApi, OpenAIResponsesApi};
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `opencode_goProvider()`
pub fn opencode_go_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "opencode-go".to_string(),
        name: Some("OpenCode Go".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "OpenCode API key",
                ["OPENCODE_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("opencode-go"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::ByApi(
            [
                (
                    "anthropic-messages".to_string(),
                    Arc::new(AnthropicMessagesApi) as Arc<dyn crate::types::ProviderStreams>,
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
