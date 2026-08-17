//! `clinePassProvider()`.
//!
//! Port addition (user decision 2026-08-17, v0.1.16), taken from
//! ../notagent-main-rust, where ClinePass is a provider entry in
//! `crates/notagent_repo/src/provider/provider.json`: an OpenAI-compatible
//! endpoint authenticated with `CLINE_API_KEY`. The model list and the
//! per-model thinking levels come from there verbatim; the catalog snapshot
//! lives in `data/cline-pass.json`, read through [`get_builtin_models`].
//!
//! ClinePass is a subscription — the plan covers the tokens, so its models
//! carry no per-token price and the footer reports none (see
//! `interactive/components/footer.rs`).

use std::sync::Arc;

use crate::api::streams::OpenAICompletionsApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::types::ProviderAuth;
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};

/// `clinePassProvider()`
pub fn cline_pass_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "cline-pass".to_string(),
        name: Some("ClinePass".to_string()),
        base_url: Some("https://api.cline.bot/api/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Cline API key",
                ["CLINE_API_KEY"],
            ))),
            oauth: None,
        },
        models: get_builtin_models("cline-pass"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(OpenAICompletionsApi)),
    })
}
