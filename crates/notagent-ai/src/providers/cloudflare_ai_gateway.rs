//! `cloudflareAIGatewayProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/cloudflare-ai-gateway.ts` (23 LOC).

use std::sync::Arc;

use crate::api::streams::{AnthropicMessagesApi, OpenAICompletionsApi, OpenAIResponsesApi};
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};
use crate::providers::cloudflare_auth::CloudflareAiGatewayAuth;
use crate::providers::cloudflare_stream::cloudflare_streams;
use crate::types::ProviderStreams;

/// `cloudflareAIGatewayProvider()`
pub fn cloudflare_ai_gateway_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "cloudflare-ai-gateway".to_string(),
        name: Some("Cloudflare AI Gateway".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(CloudflareAiGatewayAuth)),
            oauth: None,
        },
        models: get_builtin_models("cloudflare-ai-gateway"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::ByApi(
            [
                (
                    "anthropic-messages".to_string(),
                    cloudflare_streams(Arc::new(AnthropicMessagesApi)) as Arc<dyn ProviderStreams>,
                ),
                (
                    "openai-completions".to_string(),
                    cloudflare_streams(Arc::new(OpenAICompletionsApi)) as Arc<dyn ProviderStreams>,
                ),
                (
                    "openai-responses".to_string(),
                    cloudflare_streams(Arc::new(OpenAIResponsesApi)) as Arc<dyn ProviderStreams>,
                ),
            ]
            .into(),
        ),
    })
}
