//! `minimax_cnProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/minimax-cn.ts` (15 LOC). The generated
//! `minimax-cn.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::AnthropicMessagesApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `minimax_cnProvider()`
pub fn minimax_cn_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "minimax-cn".to_string(),
        name: Some("MiniMax CN".to_string()),
        base_url: Some("https://api.minimaxi.com/anthropic".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "MiniMax CN API key",
                ["MINIMAX_CN_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("minimax-cn"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(AnthropicMessagesApi)),
    })
}
