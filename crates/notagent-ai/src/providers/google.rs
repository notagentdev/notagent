//! `googleProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/google.ts` (15 LOC). The generated
//! `google.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::GoogleGenerativeAIApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `googleProvider()`
pub fn google_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "google".to_string(),
        name: Some("Google".to_string()),
        base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Gemini API key",
                ["GEMINI_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("google"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(GoogleGenerativeAIApi)),
    })
}
