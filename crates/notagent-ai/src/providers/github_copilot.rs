//! `githubCopilotProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/github-copilot.ts` (34 LOC). Copilot narrows
//! the catalog to the model ids the OAuth credential advertises.

use std::sync::Arc;

use crate::api::streams::{AnthropicMessagesApi, OpenAICompletionsApi, OpenAIResponsesApi};
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::{Credential, ProviderAuth};
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};
use crate::types::Model;

/// `filterModels(models, credential)` — an api-key credential keeps every model, and so
/// does an OAuth credential whose `availableModelIds` is not a string array.
pub fn filter_models(models: Vec<Model>, credential: Option<&Credential>) -> Vec<Model> {
    let Some(Credential::OAuth(credential)) = credential else {
        return models;
    };
    let Some(available) = credential
        .extra
        .get("availableModelIds")
        .and_then(serde_json::Value::as_array)
    else {
        return models;
    };
    // TS requires every entry to be a string and otherwise keeps the whole catalog;
    // `available_model_ids` would silently drop non-strings instead.
    if !available.iter().all(serde_json::Value::is_string) {
        return models;
    }
    let available: Vec<&str> = available
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    models
        .into_iter()
        .filter(|model| available.contains(&model.id.as_str()))
        .collect()
}

/// `githubCopilotProvider()`
pub fn github_copilot_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "github-copilot".to_string(),
        name: Some("GitHub Copilot".to_string()),
        base_url: Some("https://api.individual.githubcopilot.com".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "GitHub Copilot token",
                ["COPILOT_GITHUB_TOKEN"],
            ))),
            oauth: Some(crate::auth::oauth::github_copilot::github_copilot_oauth()),
        },
        models: get_builtin_models("github-copilot"),
        fetch_models: None,
        filter_models: Some(Arc::new(filter_models)),
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
