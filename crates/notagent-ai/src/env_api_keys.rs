//! Discovery of API keys in environment variables.
//!
//! 1:1 port of `packages/ai/src/env-api-keys.ts` (188 LOC).

use std::sync::{Mutex, OnceLock};

use crate::types::ProviderEnv;
use crate::utils::provider_env::get_provider_env_value;

pub const ANTHROPIC_AUTH_TOKEN_ENV: &str = "ANTHROPIC_AUTH_TOKEN";
pub const ANTHROPIC_OAUTH_TOKEN_ENV: &str = "ANTHROPIC_OAUTH_TOKEN";
pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";

/// `cachedVertexAdcCredentialsExists`
fn vertex_adc_cache() -> &'static Mutex<Option<bool>> {
    static CACHE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// `hasVertexAdcCredentials(env)`
fn has_vertex_adc_credentials(env: Option<&ProviderEnv>) -> bool {
    if let Some(explicit_credentials_path) = env
        .and_then(|env| env.get("GOOGLE_APPLICATION_CREDENTIALS"))
        .filter(|path| !path.is_empty())
    {
        return std::path::Path::new(explicit_credentials_path).exists();
    }

    let mut cache = vertex_adc_cache()
        .lock()
        .expect("vertex ADC cache poisoned");
    if let Some(cached) = *cache {
        return cached;
    }

    let exists = match get_provider_env_value("GOOGLE_APPLICATION_CREDENTIALS", env) {
        Some(path) => std::path::Path::new(&path).exists(),
        None => match dirs::home_dir() {
            Some(home) => home
                .join(".config")
                .join("gcloud")
                .join("application_default_credentials.json")
                .exists(),
            None => false,
        },
    };
    *cache = Some(exists);
    exists
}

/// `getApiKeyEnvVars(provider)`
fn api_key_env_vars(provider: &str) -> Option<Vec<&'static str>> {
    if provider == "github-copilot" {
        return Some(vec!["COPILOT_GITHUB_TOKEN"]);
    }
    // ANTHROPIC_AUTH_TOKEN takes part in discovery/status, but `get_env_api_key` skips
    // it because requests must send it as `Authorization: Bearer`.
    if provider == "anthropic" {
        return Some(vec![
            ANTHROPIC_AUTH_TOKEN_ENV,
            ANTHROPIC_OAUTH_TOKEN_ENV,
            ANTHROPIC_API_KEY_ENV,
        ]);
    }

    let env_var = match provider {
        "ant-ling" => "ANT_LING_API_KEY",
        "qwen-token-plan" => "QWEN_TOKEN_PLAN_API_KEY",
        "qwen-token-plan-cn" => "QWEN_TOKEN_PLAN_CN_API_KEY",
        "qwen-token-plan-individual" => "QWEN_TOKEN_PLAN_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "azure-openai-responses" => "AZURE_OPENAI_API_KEY",
        "nvidia" => "NVIDIA_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "google" => "GEMINI_API_KEY",
        "google-vertex" => "GOOGLE_CLOUD_API_KEY",
        "groq" => "GROQ_API_KEY",
        "cerebras" => "CEREBRAS_API_KEY",
        "xai" => "XAI_API_KEY",
        "radius" => "RADIUS_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "vercel-ai-gateway" => "AI_GATEWAY_API_KEY",
        "zai" => "ZAI_API_KEY",
        "zai-coding-cn" => "ZAI_CODING_CN_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "minimax" => "MINIMAX_API_KEY",
        "minimax-cn" => "MINIMAX_CN_API_KEY",
        "moonshotai" => "MOONSHOT_API_KEY",
        "moonshotai-cn" => "MOONSHOT_API_KEY",
        "huggingface" => "HF_TOKEN",
        "fireworks" => "FIREWORKS_API_KEY",
        "together" => "TOGETHER_API_KEY",
        "baseten" => "BASETEN_API_KEY",
        "opencode" => "OPENCODE_API_KEY",
        "opencode-go" => "OPENCODE_API_KEY",
        "kimi-coding" => "KIMI_API_KEY",
        "cline-pass" => "CLINE_API_KEY",
        "cloudflare-workers-ai" => "CLOUDFLARE_API_KEY",
        "cloudflare-ai-gateway" => "CLOUDFLARE_API_KEY",
        "xiaomi" => "XIAOMI_API_KEY",
        "xiaomi-token-plan-cn" => "XIAOMI_TOKEN_PLAN_CN_API_KEY",
        "xiaomi-token-plan-ams" => "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
        "xiaomi-token-plan-sgp" => "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
        _ => return None,
    };
    Some(vec![env_var])
}

/// `findEnvKeys(provider, env)` — configured env vars that can provide an API key.
///
/// Reports only actual API-key variables; ambient sources (AWS profiles, IAM, ADC) are
/// deliberately excluded.
pub fn find_env_keys(provider: &str, env: Option<&ProviderEnv>) -> Option<Vec<String>> {
    let env_vars = api_key_env_vars(provider)?;
    let found: Vec<String> = env_vars
        .into_iter()
        .filter(|env_var| get_provider_env_value(env_var, env).is_some())
        .map(str::to_string)
        .collect();
    if found.is_empty() { None } else { Some(found) }
}

/// `getEnvApiKey(provider, env)`
pub fn get_env_api_key(provider: &str, env: Option<&ProviderEnv>) -> Option<String> {
    if let Some(env_keys) = find_env_keys(provider, env)
        && !env_keys.is_empty()
    {
        let api_key_env = if provider == "anthropic" {
            env_keys.iter().find(|key| *key != ANTHROPIC_AUTH_TOKEN_ENV)
        } else {
            env_keys.first()
        };
        if let Some(api_key_env) = api_key_env
            && let Some(value) = get_provider_env_value(api_key_env, env)
        {
            return Some(value);
        }
    }

    // Vertex AI accepts either an explicit API key or Application Default Credentials.
    if provider == "google-vertex" {
        let has_credentials = has_vertex_adc_credentials(env);
        let has_project = get_provider_env_value("GOOGLE_CLOUD_PROJECT", env).is_some()
            || get_provider_env_value("GCLOUD_PROJECT", env).is_some();
        let has_location = get_provider_env_value("GOOGLE_CLOUD_LOCATION", env).is_some();
        if has_credentials && has_project && has_location {
            return Some("<authenticated>".to_string());
        }
    }

    if provider == "amazon-bedrock" {
        // 1. AWS_PROFILE, 2. IAM keys, 3. bearer token, 4./5. ECS task roles, 6. IRSA.
        let configured = get_provider_env_value("AWS_PROFILE", env).is_some()
            || (get_provider_env_value("AWS_ACCESS_KEY_ID", env).is_some()
                && get_provider_env_value("AWS_SECRET_ACCESS_KEY", env).is_some())
            || get_provider_env_value("AWS_BEARER_TOKEN_BEDROCK", env).is_some()
            || get_provider_env_value("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", env).is_some()
            || get_provider_env_value("AWS_CONTAINER_CREDENTIALS_FULL_URI", env).is_some()
            || get_provider_env_value("AWS_WEB_IDENTITY_TOKEN_FILE", env).is_some();
        if configured {
            return Some("<authenticated>".to_string());
        }
    }

    None
}
