use notagent_ai::api::openai_completions_compat::{
    ResolvedOpenAICompletionsCompat, detect_compat, get_compat,
};
use notagent_ai::types::*;
use serde_json::json;

fn model(provider: &str, base_url: &str, id: &str) -> Model {
    Model {
        id: id.to_string(),
        name: id.to_string(),
        api: "openai-completions".to_string(),
        provider: provider.to_string(),
        base_url: base_url.to_string(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 4096,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn detect(provider: &str, base_url: &str) -> ResolvedOpenAICompletionsCompat {
    detect_compat(&model(provider, base_url, "some-model"))
}

#[test]
fn openai_itself_is_the_standard_profile() {
    let compat = detect("openai", "https://api.openai.com/v1");
    assert!(compat.supports_store);
    assert!(compat.supports_developer_role);
    assert!(compat.supports_reasoning_effort);
    assert_eq!(compat.max_tokens_field, MaxTokensField::MaxCompletionTokens);
    assert_eq!(compat.thinking_format, ThinkingFormat::Openai);
    assert_eq!(
        compat.session_affinity_format,
        Some(SessionAffinityFormat::Openai)
    );
    assert!(compat.supports_strict_mode);
    assert!(compat.supports_long_cache_retention);
    assert_eq!(compat.cache_control_format, None);
}

#[test]
fn non_standard_providers_lose_store_and_the_developer_role() {
    for (provider, base_url) in [
        ("nvidia", "https://integrate.api.nvidia.com/v1"),
        ("cerebras", "https://api.cerebras.ai/v1"),
        ("xai", "https://api.x.ai/v1"),
        ("together", "https://api.together.ai/v1"),
        ("deepseek", "https://api.deepseek.com/v1"),
        ("zai", "https://api.z.ai/v1"),
        ("moonshotai", "https://api.moonshot.cn/v1"),
        ("opencode", "https://opencode.ai/zen/v1"),
        (
            "cloudflare-workers-ai",
            "https://api.cloudflare.com/client/v4",
        ),
        (
            "cloudflare-ai-gateway",
            "https://gateway.ai.cloudflare.com/v1",
        ),
        ("ant-ling", "https://api.ant-ling.com/v1"),
        ("custom", "https://llm.chutes.ai/v1"),
    ] {
        let compat = detect(provider, base_url);
        assert!(
            !compat.supports_store,
            "{provider} must not advertise store"
        );
        assert!(
            !compat.supports_developer_role,
            "{provider} must not advertise the developer role"
        );
    }
}

#[test]
fn detection_works_from_the_base_url_alone() {
    // A custom provider id with a known endpoint still gets the right profile.
    let compat = detect("my-proxy", "https://api.z.ai/v1");
    assert_eq!(compat.thinking_format, ThinkingFormat::Zai);
    assert_eq!(compat.max_tokens_field, MaxTokensField::MaxTokens);
    assert!(!compat.supports_reasoning_effort);
}

#[test]
fn thinking_formats_follow_the_provider() {
    let cases = [
        (
            "deepseek",
            "https://api.deepseek.com/v1",
            ThinkingFormat::Deepseek,
        ),
        ("zai", "https://api.z.ai/v1", ThinkingFormat::Zai),
        (
            "together",
            "https://api.together.ai/v1",
            ThinkingFormat::Together,
        ),
        (
            "ant-ling",
            "https://api.ant-ling.com/v1",
            ThinkingFormat::AntLing,
        ),
        (
            "openrouter",
            "https://openrouter.ai/api/v1",
            ThinkingFormat::Openrouter,
        ),
        (
            "openai",
            "https://api.openai.com/v1",
            ThinkingFormat::Openai,
        ),
        (
            "groq",
            "https://api.groq.com/openai/v1",
            ThinkingFormat::Openai,
        ),
    ];
    for (provider, base_url, expected) in cases {
        assert_eq!(
            detect(provider, base_url).thinking_format,
            expected,
            "{provider}"
        );
    }
}

#[test]
fn max_tokens_field_follows_the_endpoint() {
    for (provider, base_url) in [
        ("deepseek", "https://api.deepseek.com/v1"),
        ("moonshotai", "https://api.moonshot.cn/v1"),
        (
            "cloudflare-ai-gateway",
            "https://gateway.ai.cloudflare.com/v1",
        ),
        ("together", "https://api.together.ai/v1"),
        ("nvidia", "https://integrate.api.nvidia.com/v1"),
        ("ant-ling", "https://api.ant-ling.com/v1"),
        ("zai", "https://api.z.ai/v1"),
        ("custom", "https://llm.chutes.ai/v1"),
    ] {
        assert_eq!(
            detect(provider, base_url).max_tokens_field,
            MaxTokensField::MaxTokens,
            "{provider}"
        );
    }
    for (provider, base_url) in [
        ("openai", "https://api.openai.com/v1"),
        ("openrouter", "https://openrouter.ai/api/v1"),
    ] {
        assert_eq!(
            detect(provider, base_url).max_tokens_field,
            MaxTokensField::MaxCompletionTokens,
            "{provider}"
        );
    }
}

#[test]
fn reasoning_effort_support_excludes_the_known_offenders() {
    for (provider, base_url) in [
        ("xai", "https://api.x.ai/v1"),
        ("zai", "https://api.z.ai/v1"),
        ("moonshotai", "https://api.moonshot.cn/v1"),
        ("together", "https://api.together.ai/v1"),
        (
            "cloudflare-ai-gateway",
            "https://gateway.ai.cloudflare.com/v1",
        ),
        ("nvidia", "https://integrate.api.nvidia.com/v1"),
        ("ant-ling", "https://api.ant-ling.com/v1"),
    ] {
        assert!(
            !detect(provider, base_url).supports_reasoning_effort,
            "{provider}"
        );
    }
    assert!(detect("deepseek", "https://api.deepseek.com/v1").supports_reasoning_effort);
}

#[test]
fn openrouter_developer_role_depends_on_the_model_prefix() {
    let openrouter = "https://openrouter.ai/api/v1";
    assert!(
        detect_compat(&model(
            "openrouter",
            openrouter,
            "anthropic/claude-opus-4.5"
        ))
        .supports_developer_role
    );
    assert!(
        detect_compat(&model("openrouter", openrouter, "openai/gpt-5")).supports_developer_role
    );
    assert!(!detect_compat(&model("openrouter", openrouter, "meta/llama")).supports_developer_role);
}

#[test]
fn openrouter_anthropic_models_use_anthropic_cache_control() {
    let openrouter = "https://openrouter.ai/api/v1";
    assert_eq!(
        detect_compat(&model(
            "openrouter",
            openrouter,
            "anthropic/claude-opus-4.5"
        ))
        .cache_control_format,
        Some(CacheControlFormat::Anthropic)
    );
    assert_eq!(
        detect_compat(&model("openrouter", openrouter, "openai/gpt-5")).cache_control_format,
        None
    );
    // Only the provider id counts here, not the base URL.
    assert_eq!(
        detect_compat(&model("proxy", openrouter, "anthropic/claude")).cache_control_format,
        None
    );
}

#[test]
fn strict_mode_and_long_cache_retention_have_their_own_exclusions() {
    for (provider, base_url) in [
        ("moonshotai", "https://api.moonshot.cn/v1"),
        ("together", "https://api.together.ai/v1"),
        (
            "cloudflare-ai-gateway",
            "https://gateway.ai.cloudflare.com/v1",
        ),
        ("nvidia", "https://integrate.api.nvidia.com/v1"),
    ] {
        assert!(
            !detect(provider, base_url).supports_strict_mode,
            "{provider}"
        );
    }
    // deepseek and zai are non-standard but keep strict mode.
    assert!(detect("deepseek", "https://api.deepseek.com/v1").supports_strict_mode);
    assert!(detect("zai", "https://api.z.ai/v1").supports_strict_mode);

    for (provider, base_url) in [
        ("together", "https://api.together.ai/v1"),
        (
            "cloudflare-workers-ai",
            "https://api.cloudflare.com/client/v4",
        ),
        (
            "cloudflare-ai-gateway",
            "https://gateway.ai.cloudflare.com/v1",
        ),
        ("nvidia", "https://integrate.api.nvidia.com/v1"),
        ("ant-ling", "https://api.ant-ling.com/v1"),
    ] {
        assert!(
            !detect(provider, base_url).supports_long_cache_retention,
            "{provider}"
        );
    }
    assert!(detect("openai", "https://api.openai.com/v1").supports_long_cache_retention);
}

#[test]
fn session_affinity_format_is_openrouter_only_for_openrouter() {
    assert_eq!(
        detect("openrouter", "https://openrouter.ai/api/v1").session_affinity_format,
        Some(SessionAffinityFormat::Openrouter)
    );
    assert_eq!(
        detect("openai", "https://api.openai.com/v1").session_affinity_format,
        Some(SessionAffinityFormat::Openai)
    );
}

#[test]
fn explicit_model_compat_overrides_detection_field_by_field() {
    let mut model = model("deepseek", "https://api.deepseek.com/v1", "deepseek-chat");
    model.compat = Some(
        ModelCompat::from_api_value(
            "openai-completions",
            json!({
                "supportsStore": true,
                "maxTokensField": "max_completion_tokens",
                "thinkingFormat": "qwen",
                "supportsOpenAIGrammarTools": true
            }),
        )
        .unwrap(),
    );
    let compat = get_compat(&model);
    assert!(compat.supports_store, "overridden");
    assert_eq!(
        compat.max_tokens_field,
        MaxTokensField::MaxCompletionTokens,
        "overridden"
    );
    assert_eq!(compat.thinking_format, ThinkingFormat::Qwen, "overridden");
    assert!(compat.supports_openai_grammar_tools, "overridden");
    // Everything not overridden keeps the detected value.
    assert!(
        compat.requires_reasoning_content_on_assistant_messages,
        "deepseek detection survives"
    );
    assert!(compat.supports_strict_mode);
}

#[test]
fn all_eleven_thinking_formats_are_accepted_as_overrides() {
    let formats = [
        ("openai", ThinkingFormat::Openai),
        ("openrouter", ThinkingFormat::Openrouter),
        ("deepseek", ThinkingFormat::Deepseek),
        ("together", ThinkingFormat::Together),
        ("baseten", ThinkingFormat::Baseten),
        ("zai", ThinkingFormat::Zai),
        ("qwen", ThinkingFormat::Qwen),
        ("chat-template", ThinkingFormat::ChatTemplate),
        ("qwen-chat-template", ThinkingFormat::QwenChatTemplate),
        ("string-thinking", ThinkingFormat::StringThinking),
        ("ant-ling", ThinkingFormat::AntLing),
    ];
    assert_eq!(formats.len(), 11);
    for (name, expected) in formats {
        let mut model = model("custom", "https://example.test/v1", "m");
        model.compat = Some(
            ModelCompat::from_api_value("openai-completions", json!({ "thinkingFormat": name }))
                .unwrap(),
        );
        assert_eq!(get_compat(&model).thinking_format, expected, "{name}");
    }
}

#[test]
fn open_router_routing_falls_back_to_empty_not_to_detection() {
    let mut model = model(
        "openrouter",
        "https://openrouter.ai/api/v1",
        "anthropic/claude",
    );
    model.compat = Some(
        ModelCompat::from_api_value("openai-completions", json!({"supportsStore": false})).unwrap(),
    );
    assert_eq!(
        get_compat(&model).open_router_routing,
        OpenRouterRouting::default()
    );

    model.compat = Some(
        ModelCompat::from_api_value(
            "openai-completions",
            json!({"openRouterRouting": {"order": ["anthropic"], "allow_fallbacks": false}}),
        )
        .unwrap(),
    );
    let routing = get_compat(&model).open_router_routing;
    assert_eq!(routing.order, Some(vec!["anthropic".to_string()]));
    assert_eq!(routing.allow_fallbacks, Some(false));
}
