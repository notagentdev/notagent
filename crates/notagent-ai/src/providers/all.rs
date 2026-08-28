use std::sync::Arc;

use crate::images_models::{ImagesModels, ImagesProvider};
use crate::models::{CreateModelsOptions, Models, Provider, create_models};
use crate::providers::radius::{RadiusProvider, RadiusProviderOptions, radius_provider};

pub use crate::model_catalog::{
    get_builtin_model, get_builtin_model_data_generated_at, get_builtin_models,
    get_builtin_providers,
};

/// signature's `options: RadiusProviderOptions = {}`.
pub fn default_radius_provider() -> Arc<RadiusProvider> {
    radius_provider(RadiusProviderOptions::default())
}

/// `builtinProviders()` — all built-in providers, freshly constructed.
pub fn builtin_providers() -> Vec<Arc<dyn Provider>> {
    vec![
        crate::providers::amazon_bedrock::amazon_bedrock_provider(),
        crate::providers::ant_ling::ant_ling_provider(),
        crate::providers::anthropic::anthropic_provider(),
        crate::providers::azure_openai_responses::azure_openai_responses_provider(),
        crate::providers::baseten::baseten_provider(),
        crate::providers::cerebras::cerebras_provider(),
        crate::providers::cloudflare_ai_gateway::cloudflare_ai_gateway_provider(),
        crate::providers::cline_pass::cline_pass_provider(),
        crate::providers::cloudflare_workers_ai::cloudflare_workers_ai_provider(),
        crate::providers::deepseek::deepseek_provider(),
        crate::providers::fireworks::fireworks_provider(),
        crate::providers::github_copilot::github_copilot_provider(),
        crate::providers::google::google_provider(),
        crate::providers::google_vertex::google_vertex_provider(),
        crate::providers::groq::groq_provider(),
        crate::providers::huggingface::huggingface_provider(),
        crate::providers::kimi_coding::kimi_coding_provider(),
        crate::providers::minimax::minimax_provider(),
        crate::providers::minimax_cn::minimax_cn_provider(),
        crate::providers::mistral::mistral_provider(),
        crate::providers::moonshotai::moonshotai_provider(),
        crate::providers::moonshotai_cn::moonshotai_cn_provider(),
        crate::providers::mtplx::mtplx_local_provider(),
        crate::providers::nvidia::nvidia_provider(),
        crate::providers::openai::openai_provider(),
        crate::providers::openai_compatible::custom_openai_provider(),
        crate::providers::openai_compatible::lmstudio_provider(),
        crate::providers::openai_compatible::ollama_provider(),
        crate::providers::openai_codex::openai_codex_provider(),
        crate::providers::opencode::opencode_provider(),
        crate::providers::opencode_go::opencode_go_provider(),
        crate::providers::openrouter::openrouter_provider(),
        crate::providers::qwen_token_plan::qwen_token_plan_provider(),
        crate::providers::qwen_token_plan_cn::qwen_token_plan_cn_provider(),
        crate::providers::qwen_token_plan_individual::qwen_token_plan_individual_provider(),
        default_radius_provider(),
        crate::providers::together::together_provider(),
        crate::providers::vercel_ai_gateway::vercel_ai_gateway_provider(),
        crate::providers::xai::xai_provider(),
        crate::providers::xiaomi::xiaomi_provider(),
        crate::providers::xiaomi_token_plan_ams::xiaomi_token_plan_ams_provider(),
        crate::providers::xiaomi_token_plan_cn::xiaomi_token_plan_cn_provider(),
        crate::providers::xiaomi_token_plan_sgp::xiaomi_token_plan_sgp_provider(),
        crate::providers::zai::zai_provider(),
        crate::providers::zai_coding_cn::zai_coding_cn_provider(),
    ]
}

/// `builtinModels(options?)` — a `Models` collection with every built-in provider.
pub fn builtin_models(options: Option<CreateModelsOptions>) -> Arc<Models> {
    let models = create_models(options);
    for provider in builtin_providers() {
        models.set_provider(provider);
    }
    models
}

/// `builtinImagesProviders()`
pub fn builtin_images_providers() -> Vec<Arc<dyn ImagesProvider>> {
    crate::images_models::builtin_images_providers()
}

/// `builtinImagesModels(options?)`
pub fn builtin_images_models(options: Option<CreateModelsOptions>) -> Arc<ImagesModels> {
    crate::images_models::builtin_images_models(options)
}
