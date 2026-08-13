//! `openai_codexProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/openai-codex.ts` (22 LOC). The generated
//! `openai-codex.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::OpenAICodexResponsesApi;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `openai_codexProvider()`
pub fn openai_codex_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "openai-codex".to_string(),
        name: Some("OpenAI Codex".to_string()),
        base_url: Some("https://chatgpt.com/backend-api".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: None,
            oauth: Some(crate::auth::oauth::openai_codex::openai_codex_oauth()),
        },
        models: get_builtin_models("openai-codex"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICodexResponsesApi)),
    })
}
