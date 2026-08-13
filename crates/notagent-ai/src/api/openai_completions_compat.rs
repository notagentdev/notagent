//! Compatibility resolution for OpenAI-compatible completions endpoints.
//!
//! 1:1 port of `detectCompat`/`getCompat` from
//! `packages/ai/src/api/openai-completions.ts:1444-1577`. The workstream plan names this
//! matrix (24 fields, 11 thinking formats) as the biggest risk of the OpenAI family, so
//! it is ported and tested on its own before the request builder uses it.

use serde_json::{Map, Value};

use crate::types::{
    CacheControlFormat, DeferredToolsMode, MaxTokensField, Model, OpenAICompletionsCompat,
    OpenRouterRouting, SessionAffinityFormat, ThinkingFormat, VercelGatewayRouting,
};

/// `ResolvedOpenAICompletionsCompat` — every field with its effective value.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedOpenAICompletionsCompat {
    pub supports_store: bool,
    pub supports_developer_role: bool,
    pub supports_reasoning_effort: bool,
    pub supports_usage_in_streaming: bool,
    pub supports_finish_reason: bool,
    pub max_tokens_field: MaxTokensField,
    pub requires_tool_result_name: bool,
    pub requires_assistant_after_tool_result: bool,
    pub requires_thinking_as_text: bool,
    pub requires_reasoning_content_on_assistant_messages: bool,
    pub thinking_format: ThinkingFormat,
    pub open_router_routing: OpenRouterRouting,
    pub vercel_gateway_routing: VercelGatewayRouting,
    pub chat_template_kwargs: Map<String, Value>,
    pub chat_template_args: Map<String, Value>,
    pub zai_tool_stream: bool,
    pub supports_thinking_token_budget: bool,
    pub supports_strict_mode: bool,
    pub supports_openai_grammar_tools: bool,
    pub cache_control_format: Option<CacheControlFormat>,
    pub send_session_affinity_headers: bool,
    pub deferred_tools_mode: Option<DeferredToolsMode>,
    pub session_affinity_format: Option<SessionAffinityFormat>,
    pub supports_long_cache_retention: bool,
}

/// `detectCompat(model)` — URL and provider based auto-detection.
pub fn detect_compat(model: &Model) -> ResolvedOpenAICompletionsCompat {
    let provider = model.provider.as_str();
    let base_url = model.base_url.as_str();

    let is_zai = provider == "zai"
        || provider == "zai-coding-cn"
        || base_url.contains("api.z.ai")
        || base_url.contains("open.bigmodel.cn");
    let is_together = provider == "together"
        || base_url.contains("api.together.ai")
        || base_url.contains("api.together.xyz");
    let is_moonshot = provider == "moonshotai"
        || provider == "moonshotai-cn"
        || base_url.contains("api.moonshot.");
    let is_open_router = provider == "openrouter" || base_url.contains("openrouter.ai");
    let is_cloudflare_workers_ai =
        provider == "cloudflare-workers-ai" || base_url.contains("api.cloudflare.com");
    let is_cloudflare_ai_gateway =
        provider == "cloudflare-ai-gateway" || base_url.contains("gateway.ai.cloudflare.com");
    let is_nvidia = provider == "nvidia" || base_url.contains("integrate.api.nvidia.com");
    let is_ant_ling = provider == "ant-ling" || base_url.contains("api.ant-ling.com");
    let is_deepseek = provider == "deepseek" || base_url.to_lowercase().contains("deepseek.com");

    let is_non_standard = is_nvidia
        || provider == "cerebras"
        || base_url.contains("cerebras.ai")
        || provider == "xai"
        || base_url.contains("api.x.ai")
        || is_together
        || base_url.contains("chutes.ai")
        || is_deepseek
        || is_zai
        || is_moonshot
        || provider == "opencode"
        || base_url.contains("opencode.ai")
        || is_cloudflare_workers_ai
        || is_cloudflare_ai_gateway
        || is_ant_ling;

    let use_max_tokens = base_url.contains("chutes.ai")
        || is_deepseek
        || is_moonshot
        || is_cloudflare_ai_gateway
        || is_together
        || is_nvidia
        || is_ant_ling
        || is_zai;

    let is_grok = provider == "xai" || base_url.contains("api.x.ai");
    let is_open_router_developer_role_model =
        is_open_router && (model.id.starts_with("anthropic/") || model.id.starts_with("openai/"));
    let cache_control_format = if provider == "openrouter" && model.id.starts_with("anthropic/") {
        Some(CacheControlFormat::Anthropic)
    } else {
        None
    };

    ResolvedOpenAICompletionsCompat {
        supports_store: !is_non_standard,
        supports_developer_role: is_open_router_developer_role_model
            || (!is_non_standard && !is_open_router),
        supports_reasoning_effort: !is_grok
            && !is_zai
            && !is_moonshot
            && !is_together
            && !is_cloudflare_ai_gateway
            && !is_nvidia
            && !is_ant_ling,
        supports_usage_in_streaming: true,
        supports_finish_reason: true,
        max_tokens_field: if use_max_tokens {
            MaxTokensField::MaxTokens
        } else {
            MaxTokensField::MaxCompletionTokens
        },
        requires_tool_result_name: false,
        requires_assistant_after_tool_result: false,
        requires_thinking_as_text: false,
        requires_reasoning_content_on_assistant_messages: is_deepseek,
        thinking_format: if is_deepseek {
            ThinkingFormat::Deepseek
        } else if is_zai {
            ThinkingFormat::Zai
        } else if is_together {
            ThinkingFormat::Together
        } else if is_ant_ling {
            ThinkingFormat::AntLing
        } else if is_open_router {
            ThinkingFormat::Openrouter
        } else {
            ThinkingFormat::Openai
        },
        open_router_routing: OpenRouterRouting::default(),
        vercel_gateway_routing: VercelGatewayRouting::default(),
        chat_template_kwargs: Map::new(),
        chat_template_args: Map::new(),
        zai_tool_stream: false,
        supports_thinking_token_budget: false,
        supports_strict_mode: !is_moonshot
            && !is_together
            && !is_cloudflare_ai_gateway
            && !is_nvidia,
        supports_openai_grammar_tools: false,
        cache_control_format,
        send_session_affinity_headers: false,
        deferred_tools_mode: None,
        session_affinity_format: Some(if is_open_router {
            SessionAffinityFormat::Openrouter
        } else {
            SessionAffinityFormat::Openai
        }),
        supports_long_cache_retention: !(is_together
            || is_cloudflare_workers_ai
            || is_cloudflare_ai_gateway
            || is_nvidia
            || is_ant_ling),
    }
}

/// `getCompat(model)` — auto-detection overridden field by field by `model.compat`.
pub fn get_compat(model: &Model) -> ResolvedOpenAICompletionsCompat {
    let detected = detect_compat(model);
    let Some(compat) = model
        .compat
        .as_ref()
        .and_then(crate::types::ModelCompat::as_openai_completions)
    else {
        return detected;
    };
    resolve_compat(compat, detected)
}

fn resolve_compat(
    compat: &OpenAICompletionsCompat,
    detected: ResolvedOpenAICompletionsCompat,
) -> ResolvedOpenAICompletionsCompat {
    ResolvedOpenAICompletionsCompat {
        supports_store: compat.supports_store.unwrap_or(detected.supports_store),
        supports_developer_role: compat
            .supports_developer_role
            .unwrap_or(detected.supports_developer_role),
        supports_reasoning_effort: compat
            .supports_reasoning_effort
            .unwrap_or(detected.supports_reasoning_effort),
        supports_usage_in_streaming: compat
            .supports_usage_in_streaming
            .unwrap_or(detected.supports_usage_in_streaming),
        supports_finish_reason: compat
            .supports_finish_reason
            .unwrap_or(detected.supports_finish_reason),
        max_tokens_field: compat.max_tokens_field.unwrap_or(detected.max_tokens_field),
        requires_tool_result_name: compat
            .requires_tool_result_name
            .unwrap_or(detected.requires_tool_result_name),
        requires_assistant_after_tool_result: compat
            .requires_assistant_after_tool_result
            .unwrap_or(detected.requires_assistant_after_tool_result),
        requires_thinking_as_text: compat
            .requires_thinking_as_text
            .unwrap_or(detected.requires_thinking_as_text),
        requires_reasoning_content_on_assistant_messages: compat
            .requires_reasoning_content_on_assistant_messages
            .unwrap_or(detected.requires_reasoning_content_on_assistant_messages),
        thinking_format: compat.thinking_format.unwrap_or(detected.thinking_format),
        // TS falls back to `{}` here, not to the detected value.
        open_router_routing: compat.open_router_routing.clone().unwrap_or_default(),
        vercel_gateway_routing: compat
            .vercel_gateway_routing
            .clone()
            .unwrap_or(detected.vercel_gateway_routing),
        chat_template_kwargs: compat
            .chat_template_kwargs
            .clone()
            .unwrap_or(detected.chat_template_kwargs),
        chat_template_args: compat
            .chat_template_args
            .clone()
            .unwrap_or(detected.chat_template_args),
        zai_tool_stream: compat.zai_tool_stream.unwrap_or(detected.zai_tool_stream),
        supports_thinking_token_budget: compat
            .supports_thinking_token_budget
            .unwrap_or(detected.supports_thinking_token_budget),
        supports_strict_mode: compat
            .supports_strict_mode
            .unwrap_or(detected.supports_strict_mode),
        supports_openai_grammar_tools: compat
            .supports_openai_grammar_tools
            .unwrap_or(detected.supports_openai_grammar_tools),
        cache_control_format: compat
            .cache_control_format
            .or(detected.cache_control_format),
        send_session_affinity_headers: compat
            .send_session_affinity_headers
            .unwrap_or(detected.send_session_affinity_headers),
        deferred_tools_mode: compat.deferred_tools_mode.or(detected.deferred_tools_mode),
        session_affinity_format: compat
            .session_affinity_format
            .or(detected.session_affinity_format),
        supports_long_cache_retention: compat
            .supports_long_cache_retention
            .unwrap_or(detected.supports_long_cache_retention),
    }
}
