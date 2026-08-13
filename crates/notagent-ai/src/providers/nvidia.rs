//! `nvidiaProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/nvidia.ts` (15 LOC). The generated
//! `nvidia.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `nvidiaProvider()`
pub fn nvidia_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "nvidia".to_string(),
        name: Some("NVIDIA".to_string()),
        base_url: Some("https://integrate.api.nvidia.com/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "NVIDIA API key",
                ["NVIDIA_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("nvidia"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
