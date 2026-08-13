//! `qwen_token_plan_individualProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/qwen-token-plan-individual.ts` (15 LOC). The generated
//! `qwen-token-plan-individual.models.ts` is the catalog snapshot in `data/`, read through
//! [`get_builtin_models`].

use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `qwen_token_plan_individualProvider()`
pub fn qwen_token_plan_individual_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "qwen-token-plan-individual".to_string(),
        name: Some("Qwen Token Plan Individual".to_string()),
        base_url: Some(
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1".to_string(),
        ),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Qwen Token Plan Individual API key",
                ["QWEN_TOKEN_PLAN_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("qwen-token-plan-individual"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
