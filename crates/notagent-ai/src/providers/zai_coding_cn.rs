use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `zai_coding_cnProvider()`
pub fn zai_coding_cn_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "zai-coding-cn".to_string(),
        name: Some("Z.AI Coding CN".to_string()),
        base_url: Some("https://open.bigmodel.cn/api/coding/paas/v4".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Z.AI Coding CN API key",
                ["ZAI_CODING_CN_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("zai-coding-cn"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
