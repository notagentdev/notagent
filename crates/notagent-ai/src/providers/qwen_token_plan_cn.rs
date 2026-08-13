//! `qwen_token_plan_cnProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/qwen-token-plan-cn.ts` (15 LOC). The generated
//! `qwen-token-plan-cn.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `qwen_token_plan_cnProvider()`
pub fn qwen_token_plan_cn_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "qwen-token-plan-cn".to_string(),
        name: Some("Qwen Token Plan CN".to_string()),
        base_url: Some(
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1".to_string(),
        ),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Qwen Token Plan CN API key",
                ["QWEN_TOKEN_PLAN_CN_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("qwen-token-plan-cn"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
