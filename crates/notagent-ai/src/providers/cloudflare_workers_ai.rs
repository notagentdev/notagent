//! `cloudflareWorkersAIProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/cloudflare-workers-ai.ts` (15 LOC).

use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};
use crate::providers::cloudflare_auth::CloudflareWorkersAiAuth;
use crate::providers::cloudflare_stream::cloudflare_streams;

/// `cloudflareWorkersAIProvider()`
pub fn cloudflare_workers_ai_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "cloudflare-workers-ai".to_string(),
        name: Some("Cloudflare Workers AI".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(CloudflareWorkersAiAuth)),
            oauth: None,
        },
        models: get_builtin_models("cloudflare-workers-ai"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(cloudflare_streams(Arc::new(OpenAICompletionsApi))),
    })
}
