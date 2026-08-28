use notagent_ai::types::{Model, ProviderHeaders};

use crate::core::settings_manager::SettingsManager;
use crate::core::telemetry::is_install_telemetry_enabled;

const OPENROUTER_HOST: &str = "openrouter.ai";
const NVIDIA_NIM_HOST: &str = "integrate.api.nvidia.com";
const CLOUDFLARE_API_HOST: &str = "api.cloudflare.com";
const CLOUDFLARE_AI_GATEWAY_HOST: &str = "gateway.ai.cloudflare.com";
const OPENCODE_HOST: &str = "opencode.ai";

/// `new URL(baseUrl).hostname === expectedHost`, with a parse failure reading
fn matches_host(base_url: &str, expected_host: &str) -> bool {
    let Some(rest) = base_url.split_once("://").map(|(_, rest)| rest) else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    host.eq_ignore_ascii_case(expected_host)
}

fn is_openrouter_model(model: &Model) -> bool {
    model.provider == "openrouter" || model.base_url.contains(OPENROUTER_HOST)
}

fn is_nvidia_nim_model(model: &Model) -> bool {
    model.provider == "nvidia" || matches_host(&model.base_url, NVIDIA_NIM_HOST)
}

fn is_cloudflare_model(model: &Model) -> bool {
    model.provider == "cloudflare-workers-ai"
        || model.provider == "cloudflare-ai-gateway"
        || matches_host(&model.base_url, CLOUDFLARE_API_HOST)
        || matches_host(&model.base_url, CLOUDFLARE_AI_GATEWAY_HOST)
}

fn default_attribution_headers(
    model: &Model,
    settings_manager: &SettingsManager,
) -> Vec<(String, String)> {
    if !is_install_telemetry_enabled(settings_manager) {
        return Vec::new();
    }

    if is_openrouter_model(model) {
        return vec![
            (
                "HTTP-Referer".to_string(),
                "https://notagent.dev".to_string(),
            ),
            ("X-OpenRouter-Title".to_string(), "notagent".to_string()),
            (
                "X-OpenRouter-Categories".to_string(),
                "cli-agent".to_string(),
            ),
        ];
    }

    if is_nvidia_nim_model(model) {
        return vec![(
            "X-BILLING-INVOKE-ORIGIN".to_string(),
            "Notagent".to_string(),
        )];
    }

    if is_cloudflare_model(model) {
        return vec![(
            "User-Agent".to_string(),
            "notagent-coding-agent".to_string(),
        )];
    }

    Vec::new()
}

fn session_headers(model: &Model, session_id: Option<&str>) -> Vec<(String, String)> {
    let Some(session_id) = session_id else {
        return Vec::new();
    };
    if model.provider != "opencode"
        && model.provider != "opencode-go"
        && !matches_host(&model.base_url, OPENCODE_HOST)
    {
        return Vec::new();
    }
    vec![
        ("x-opencode-session".to_string(), session_id.to_string()),
        ("x-opencode-client".to_string(), "notagent".to_string()),
    ]
}

/// Merges the attribution headers with whatever the caller already had, later
pub fn merge_provider_attribution_headers(
    model: &Model,
    settings_manager: &SettingsManager,
    session_id: Option<&str>,
    header_sources: &[Option<ProviderHeaders>],
) -> Option<ProviderHeaders> {
    let mut merged: ProviderHeaders = ProviderHeaders::new();
    for (name, value) in session_headers(model, session_id) {
        merged.insert(name, Some(value));
    }
    for (name, value) in default_attribution_headers(model, settings_manager) {
        merged.insert(name, Some(value));
    }
    for headers in header_sources.iter().flatten() {
        for (name, value) in headers {
            merged.insert(name.clone(), value.clone());
        }
    }
    (!merged.is_empty()).then_some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_host_out_of_a_base_url() {
        assert!(matches_host(
            "https://openrouter.ai/api/v1",
            "openrouter.ai"
        ));
        assert!(matches_host("https://user@host:8080/x", "host"));
        assert!(!matches_host("not a url", "openrouter.ai"));
        assert!(!matches_host("https://other.ai/v1", "openrouter.ai"));
    }

    #[test]
    fn the_opencode_session_header_is_not_telemetry() {
        // No provider match, so no session header either.
        let mut model = notagent_ai::types::Model {
            id: "m".to_string(),
            name: "m".to_string(),
            api: "openai-completions".to_string(),
            provider: "openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            reasoning: false,
            thinking_level_map: None,
            input: Vec::new(),
            cost: Default::default(),
            context_window: 1000,
            max_tokens: 100,
            sampling_params: None,
            headers: None,
            compat: None,
        };
        assert_eq!(session_headers(&model, Some("s1")), Vec::new());

        model.provider = "opencode".to_string();
        assert_eq!(
            session_headers(&model, Some("s1")),
            vec![
                ("x-opencode-session".to_string(), "s1".to_string()),
                ("x-opencode-client".to_string(), "notagent".to_string()),
            ]
        );
        assert_eq!(session_headers(&model, None), Vec::new());
    }
}
