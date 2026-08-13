//! `vercel_ai_gatewayProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/vercel-ai-gateway.ts` (15 LOC). The generated
//! `vercel-ai-gateway.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::AnthropicMessagesApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `vercel_ai_gatewayProvider()`
pub fn vercel_ai_gateway_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "vercel-ai-gateway".to_string(),
        name: Some("Vercel AI Gateway".to_string()),
        base_url: Some("https://ai-gateway.vercel.sh".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Vercel AI Gateway API key",
                ["AI_GATEWAY_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("vercel-ai-gateway"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(AnthropicMessagesApi)),
    })
}
