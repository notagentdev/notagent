//! Serde-Parität der Kerntypen: Die JSON-Form muss dem TS-Original entsprechen
//! (camelCase, optionale Felder weggelassen statt `null`).
//!
//! Oracle sind die Feldnamen aus `packages/ai/src/types.ts` und die von der TS-App
//! geschriebenen Session-Dateien.

use notagent_ai::auth::types::{AuthType, Credential, OAuthCredential};
use notagent_ai::types::*;
use serde_json::json;

fn assert_roundtrip<T>(value: serde_json::Value)
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let parsed: T = serde_json::from_value(value.clone()).expect("Deserialisierung");
    let reserialized = serde_json::to_value(&parsed).expect("Serialisierung");
    assert_eq!(reserialized, value, "Roundtrip verändert das JSON");
}

#[test]
fn text_content_omits_absent_signature() {
    let content = AssistantContent::Text(TextContent::new("hi"));
    assert_eq!(
        serde_json::to_string(&content).unwrap(),
        r#"{"type":"text","text":"hi"}"#
    );
}

#[test]
fn content_blocks_use_ts_discriminators() {
    assert_roundtrip::<AssistantContent>(
        json!({"type": "text", "text": "a", "textSignature": "sig"}),
    );
    assert_roundtrip::<AssistantContent>(
        json!({"type": "thinking", "thinking": "t", "thinkingSignature": "s", "redacted": true}),
    );
    assert_roundtrip::<AssistantContent>(json!({
        "type": "toolCall",
        "id": "call_1",
        "name": "read",
        "arguments": {"path": "a.txt"},
        "thoughtSignature": "ts",
        "namespace": "ns"
    }));
    assert_roundtrip::<TextOrImageContent>(
        json!({"type": "image", "data": "AAA", "mimeType": "image/png"}),
    );
}

#[test]
fn user_content_accepts_string_and_blocks() {
    assert_roundtrip::<Message>(json!({"role": "user", "content": "plain", "timestamp": 1}));
    assert_roundtrip::<Message>(
        json!({"role": "user", "content": [{"type": "text", "text": "block"}], "timestamp": 2}),
    );
}

#[test]
fn usage_serializes_integral_costs_like_javascript() {
    let usage = Usage {
        input: 10,
        output: 20,
        cache_read: 0,
        cache_write: 0,
        cache_write1h: None,
        reasoning: None,
        total_tokens: Some(30),
        cost: UsageCost::default(),
    };
    assert_eq!(
        serde_json::to_string(&usage).unwrap(),
        r#"{"input":10,"output":20,"cacheRead":0,"cacheWrite":0,"totalTokens":30,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}}"#
    );
}

#[test]
fn usage_keeps_optional_breakdowns() {
    assert_roundtrip::<Usage>(json!({
        "input": 1, "output": 2, "cacheRead": 3, "cacheWrite": 4,
        "cacheWrite1h": 2, "reasoning": 1, "totalTokens": 10,
        "cost": {"input": 0.5, "output": 0.25, "cacheRead": 0, "cacheWrite": 0, "total": 0.75}
    }));
}

#[test]
fn stop_reasons_match_ts_spelling() {
    let expected = [
        "pending", "stop", "length", "toolUse", "error", "aborted", "deferred",
    ];
    let values = [
        StopReason::Pending,
        StopReason::Stop,
        StopReason::Length,
        StopReason::ToolUse,
        StopReason::Error,
        StopReason::Aborted,
        StopReason::Deferred,
    ];
    for (value, name) in values.iter().zip(expected) {
        assert_eq!(serde_json::to_value(value).unwrap(), json!(name));
    }
}

#[test]
fn assistant_message_omits_absent_optionals() {
    let message = AssistantMessage {
        content: vec![],
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-opus-4-5".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 1_700_000_000_000,
    };
    let serialized = serde_json::to_value(&message).unwrap();
    let object = serialized.as_object().unwrap();
    for absent in [
        "responseModel",
        "responseId",
        "diagnostics",
        "deferred",
        "errorMessage",
        "rawStopReason",
        "endTurn",
    ] {
        assert!(
            !object.contains_key(absent),
            "{absent} darf nicht serialisiert werden"
        );
    }
    assert_eq!(object["stopReason"], json!("stop"));
}

#[test]
fn assistant_message_events_use_ts_event_names() {
    let partial = json!({
        "role": "assistant", "content": [], "api": "anthropic-messages", "provider": "anthropic",
        "model": "m", "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
        "stopReason": "pending", "timestamp": 1
    });
    let mut partial_without_role = partial.clone();
    partial_without_role.as_object_mut().unwrap().remove("role");

    for (name, extra) in [
        ("start", json!({})),
        ("text_start", json!({"contentIndex": 0})),
        ("text_delta", json!({"contentIndex": 0, "delta": "a"})),
        ("text_end", json!({"contentIndex": 0, "content": "ab"})),
        ("thinking_start", json!({"contentIndex": 1})),
        ("thinking_delta", json!({"contentIndex": 1, "delta": "t"})),
        ("thinking_end", json!({"contentIndex": 1, "content": "tt"})),
        ("toolcall_start", json!({"contentIndex": 2})),
        ("toolcall_delta", json!({"contentIndex": 2, "delta": "{"})),
    ] {
        let mut event = json!({"type": name, "partial": partial_without_role});
        for (key, value) in extra.as_object().unwrap() {
            event
                .as_object_mut()
                .unwrap()
                .insert(key.clone(), value.clone());
        }
        assert_roundtrip::<AssistantMessageEvent>(event);
    }

    assert_roundtrip::<AssistantMessageEvent>(json!({
        "type": "toolcall_end",
        "contentIndex": 2,
        "toolCall": {"type": "toolCall", "id": "1", "name": "read", "arguments": {}},
        "partial": partial_without_role
    }));
    assert_roundtrip::<AssistantMessageEvent>(
        json!({"type": "done", "reason": "toolUse", "message": partial_without_role}),
    );
    assert_roundtrip::<AssistantMessageEvent>(
        json!({"type": "error", "reason": "aborted", "error": partial_without_role}),
    );
}

fn model_json(api: &str, compat: serde_json::Value) -> serde_json::Value {
    json!({
        "id": "m", "name": "M", "api": api, "provider": "p", "baseUrl": "https://example.test",
        "reasoning": true, "input": ["text", "image"],
        "cost": {"input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75},
        "contextWindow": 200000, "maxTokens": 64000, "compat": compat
    })
}

#[test]
fn model_compat_is_selected_by_api() {
    let completions: Model = serde_json::from_value(model_json(
        "openai-completions",
        json!({"thinkingFormat": "deepseek"}),
    ))
    .unwrap();
    assert_eq!(
        completions
            .compat
            .as_ref()
            .and_then(ModelCompat::as_openai_completions)
            .unwrap()
            .thinking_format,
        Some(ThinkingFormat::Deepseek)
    );

    for api in [
        "openai-responses",
        "azure-openai-responses",
        "openai-codex-responses",
    ] {
        let model: Model =
            serde_json::from_value(model_json(api, json!({"supportsToolSearch": true}))).unwrap();
        assert_eq!(
            model
                .compat
                .as_ref()
                .and_then(ModelCompat::as_openai_responses)
                .unwrap()
                .supports_tool_search,
            Some(true)
        );
    }

    let anthropic: Model = serde_json::from_value(model_json(
        "anthropic-messages",
        json!({"forceAdaptiveThinking": true}),
    ))
    .unwrap();
    assert_eq!(
        anthropic
            .compat
            .as_ref()
            .and_then(ModelCompat::as_anthropic_messages)
            .unwrap()
            .force_adaptive_thinking,
        Some(true)
    );

    let bedrock: Model = serde_json::from_value(model_json(
        "bedrock-converse-stream",
        json!({"supportsStrictMode": true}),
    ))
    .unwrap();
    assert_eq!(
        bedrock
            .compat
            .as_ref()
            .and_then(ModelCompat::as_bedrock)
            .unwrap()
            .supports_strict_mode,
        Some(true)
    );

    // APIs ohne Compat-Zuordnung behalten den Rohwert (TS-Typ `never`, Laufzeit erhält ihn).
    let google: Model =
        serde_json::from_value(model_json("google-generative-ai", json!({"foo": 1}))).unwrap();
    assert!(matches!(google.compat, Some(ModelCompat::Other(_))));
}

#[test]
fn model_roundtrips_with_thinking_level_map() {
    assert_roundtrip::<Model>(json!({
        "id": "m", "name": "M", "api": "openai-completions", "provider": "p",
        "baseUrl": "https://example.test", "reasoning": true,
        "thinkingLevelMap": {"off": null, "minimal": "low", "max": null},
        "input": ["text"],
        "cost": {"input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0,
                 "tiers": [{"input": 2, "output": 4, "cacheRead": 0, "cacheWrite": 0, "inputTokensAbove": 200000}]},
        "contextWindow": 128000, "maxTokens": 8192,
        "samplingParams": {"top_p": 0.9},
        "headers": {"x-test": "1"},
        "compat": {"maxTokensField": "max_completion_tokens", "supportsStore": false}
    }));
}

#[test]
fn tool_and_context_match_ts_shape() {
    assert_roundtrip::<Context>(json!({
        "systemPrompt": "sys",
        "messages": [],
        "tools": [{
            "name": "read", "description": "reads",
            "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]},
            "constrainedSampling": {"type": "json_schema", "strict": "prefer"}
        }]
    }));
    assert_roundtrip::<Tool>(json!({
        "name": "grammar", "description": "d", "parameters": {"type": "object"},
        "constrainedSampling": {"type": "grammar", "variants": {"openai_lark": "start: ..."}}
    }));
    assert_roundtrip::<Tool>(json!({
        "name": "off", "description": "d", "parameters": {"type": "object"}, "constrainedSampling": false
    }));
}

#[test]
fn tool_result_message_roundtrips() {
    assert_roundtrip::<Message>(json!({
        "role": "toolResult",
        "toolCallId": "call_1",
        "toolName": "read",
        "content": [{"type": "text", "text": "ok"}],
        "details": {"lines": 3},
        "addedToolNames": ["write"],
        "isError": false,
        "timestamp": 5
    }));
}

#[test]
fn deferred_handle_and_request_options_roundtrip() {
    assert_roundtrip::<DeferredHandle>(json!({
        "provider": "faux", "modelId": "m", "api": "faux-api", "id": "handle-1",
        "expiresAt": 123, "pollAfterMs": 500, "data": {"rows": [1, 2]}
    }));
    assert_roundtrip::<DeferredRequest>(json!(true));
    assert_roundtrip::<DeferredRequest>(json!({"window": "24h"}));
}

#[test]
fn transport_and_cache_retention_spelling() {
    assert_eq!(
        serde_json::to_value(Transport::WebsocketCached).unwrap(),
        json!("websocket-cached")
    );
    assert_eq!(serde_json::to_value(Transport::Sse).unwrap(), json!("sse"));
    assert_eq!(
        serde_json::to_value(CacheRetention::Long).unwrap(),
        json!("long")
    );
    assert_eq!(
        serde_json::to_value(SessionAffinityFormat::OpenaiNosession).unwrap(),
        json!("openai-nosession")
    );
}

#[test]
fn thinking_levels_match_ts_values() {
    let levels = [
        (ThinkingLevel::Minimal, "minimal"),
        (ThinkingLevel::Low, "low"),
        (ThinkingLevel::Medium, "medium"),
        (ThinkingLevel::High, "high"),
        (ThinkingLevel::Xhigh, "xhigh"),
        (ThinkingLevel::Max, "max"),
    ];
    for (level, name) in levels {
        assert_eq!(serde_json::to_value(level).unwrap(), json!(name));
    }
    assert_eq!(
        serde_json::to_value(ModelThinkingLevel::Off).unwrap(),
        json!("off")
    );
}

#[test]
fn thinking_format_variants_cover_all_eleven() {
    let formats = [
        (ThinkingFormat::Openai, "openai"),
        (ThinkingFormat::Openrouter, "openrouter"),
        (ThinkingFormat::Deepseek, "deepseek"),
        (ThinkingFormat::Together, "together"),
        (ThinkingFormat::Baseten, "baseten"),
        (ThinkingFormat::Zai, "zai"),
        (ThinkingFormat::Qwen, "qwen"),
        (ThinkingFormat::ChatTemplate, "chat-template"),
        (ThinkingFormat::QwenChatTemplate, "qwen-chat-template"),
        (ThinkingFormat::StringThinking, "string-thinking"),
        (ThinkingFormat::AntLing, "ant-ling"),
    ];
    assert_eq!(formats.len(), 11);
    for (format, name) in formats {
        assert_eq!(serde_json::to_value(format).unwrap(), json!(name));
    }
}

#[test]
fn openrouter_routing_keeps_snake_case_fields() {
    assert_roundtrip::<OpenRouterRouting>(json!({
        "allow_fallbacks": true,
        "require_parameters": false,
        "data_collection": "deny",
        "zdr": true,
        "order": ["anthropic"],
        "only": ["openai"],
        "ignore": ["together"],
        "quantizations": ["fp8"],
        "sort": {"by": "price", "partition": null},
        "max_price": {"prompt": 1.5, "completion": "2"},
        "preferred_min_throughput": {"p50": 20.0},
        "preferred_max_latency": 3.0
    }));
    assert_roundtrip::<OpenRouterRouting>(json!({"sort": "throughput"}));
}

#[test]
fn chat_template_kwarg_values_roundtrip() {
    assert_roundtrip::<ChatTemplateKwargValue>(json!("text"));
    assert_roundtrip::<ChatTemplateKwargValue>(json!(1.5));
    assert_roundtrip::<ChatTemplateKwargValue>(json!(true));
    assert_roundtrip::<ChatTemplateKwargValue>(
        json!({"$var": "thinking.enabled", "omitWhenOff": true}),
    );
    assert_roundtrip::<ChatTemplateKwargValue>(json!({"$var": "thinking.effort"}));
}

#[test]
fn assistant_images_roundtrips() {
    assert_roundtrip::<AssistantImages>(json!({
        "api": "openrouter-images", "provider": "openrouter", "model": "img",
        "output": [{"type": "image", "data": "AA", "mimeType": "image/png"}],
        "responseId": "r1", "stopReason": "stop", "timestamp": 7
    }));
}

#[test]
fn images_model_omits_model_only_fields() {
    assert_roundtrip::<ImagesModel>(json!({
        "id": "img", "name": "Img", "api": "openrouter-images", "provider": "openrouter",
        "baseUrl": "https://example.test", "input": ["text"], "output": ["image"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
    }));
}

#[test]
fn diagnostics_roundtrip() {
    assert_roundtrip::<AssistantMessage>(json!({
        "content": [], "api": "a", "provider": "p", "model": "m",
        "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                  "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
        "stopReason": "error",
        "errorMessage": "boom",
        "diagnostics": [{"type": "provider_transport_failure", "timestamp": 1,
                         "error": {"name": "Error", "message": "m", "code": "ECONNRESET"},
                         "details": {"attempt": 1}}],
        "timestamp": 9
    }));
}

#[test]
fn content_discriminator_does_not_leak_into_extra() {
    let content: AssistantContent =
        serde_json::from_value(json!({"type": "text", "text": "a"})).unwrap();
    let AssistantContent::Text(text) = &content else {
        panic!("Text-Variante erwartet")
    };
    assert!(
        text.extra.is_empty(),
        "Der Enum-Diskriminator darf nicht in `extra` landen"
    );
    assert_eq!(
        serde_json::to_string(&content).unwrap(),
        r#"{"type":"text","text":"a"}"#
    );
}

#[test]
fn aborted_tool_call_keeps_partial_json_scratch_field() {
    // bug-compat: Bricht ein Stream vor `content_block_stop` ab, löscht die TS-Implementierung
    // `partialJson` nicht mehr (anthropic-messages.ts:698) — die Session-Datei enthält es dann.
    let original = json!({
        "type": "toolCall", "id": "toolu_1", "name": "bash",
        "arguments": {}, "partialJson": "{\"command\": \"ls"
    });
    let content: AssistantContent = serde_json::from_value(original.clone()).unwrap();
    let AssistantContent::ToolCall(tool_call) = &content else {
        panic!("ToolCall-Variante erwartet")
    };
    assert_eq!(
        tool_call.extra.get("partialJson").and_then(|v| v.as_str()),
        Some("{\"command\": \"ls")
    );
    assert_eq!(serde_json::to_value(&content).unwrap(), original);
}

#[test]
fn usage_without_total_tokens_roundtrips_unchanged() {
    // Historische Session-Dateien der TS-App enthalten `totalTokens` nicht.
    let original = json!({
        "input": 4, "output": 1, "cacheRead": 148801, "cacheWrite": 255,
        "cost": {"input": 0.000012, "output": 0.000015, "cacheRead": 0.0446403,
                 "cacheWrite": 0.0009562500000000001, "total": 0.04562355}
    });
    let usage: Usage = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(usage.total_tokens, None);
    assert_eq!(serde_json::to_value(usage).unwrap(), original);
}

/// The wire format of `auth.json` (interface request C-4): `Credential` and
/// `AuthType` serialize OAuth as `"oauth"`, exactly like
/// `packages/ai/src/auth/types.ts:33` and `:114`.
#[test]
fn credentials_use_the_typescript_oauth_tag() {
    let credential = Credential::OAuth(OAuthCredential {
        refresh: "refresh-token".to_string(),
        access: "access-token".to_string(),
        expires: 1_700_000_000_000,
        extra: serde_json::Map::new(),
    });
    assert_eq!(
        serde_json::to_value(&credential).unwrap(),
        serde_json::json!({
            "type": "oauth",
            "refresh": "refresh-token",
            "access": "access-token",
            "expires": 1_700_000_000_000_i64,
        })
    );

    // A file written by the TypeScript app has to parse.
    let parsed: Credential = serde_json::from_value(serde_json::json!({
        "type": "oauth",
        "refresh": "refresh-token",
        "access": "access-token",
        "expires": 1_700_000_000_000_i64,
    }))
    .expect("oauth credential parses");
    assert_eq!(parsed, credential);

    assert_eq!(
        serde_json::to_value(AuthType::OAuth).unwrap(),
        serde_json::json!("oauth")
    );
    assert_eq!(
        serde_json::to_value(AuthType::ApiKey).unwrap(),
        serde_json::json!("api_key")
    );
    assert_eq!(
        serde_json::from_value::<AuthType>(serde_json::json!("oauth")).unwrap(),
        AuthType::OAuth
    );
}

/// Port of `packages/ai/test/lax-message-content.test.ts` (67). TS normalizes `null`
/// content at the `transformMessages` choke point, because hand-built histories and old
/// session files carry it (issues #6259, #6276). The Rust types make an absent content
/// unrepresentable, so the leniency moves to deserialization: `null` and a missing field
/// both become empty content, which is the state TS reaches before a request.
#[test]
fn null_or_missing_message_content_deserializes_as_empty() {
    use notagent_ai::types::{AssistantContent, Message, TextOrImageContent, UserContent};

    let messages: Vec<Message> = serde_json::from_value(serde_json::json!([
        { "role": "user", "content": null, "timestamp": 1 },
        {
            "role": "assistant",
            "content": null,
            "api": "openai-completions",
            "provider": "openai",
            "model": "test-model",
            "usage": {
                "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
            },
            "stopReason": "stop",
            "timestamp": 1,
        },
        {
            "role": "toolResult",
            "toolCallId": "call_1",
            "toolName": "web_search",
            "isError": false,
            "timestamp": 1,
        },
    ]))
    .expect("lax message content");

    assert_eq!(messages.len(), 3);
    match &messages[0] {
        Message::User(message) => assert_eq!(message.content, UserContent::Blocks(Vec::new())),
        other => panic!("unexpected message: {other:?}"),
    }
    match &messages[1] {
        Message::Assistant(message) => assert_eq!(message.content, Vec::<AssistantContent>::new()),
        other => panic!("unexpected message: {other:?}"),
    }
    match &messages[2] {
        Message::ToolResult(message) => {
            assert_eq!(message.content, Vec::<TextOrImageContent>::new())
        }
        other => panic!("unexpected message: {other:?}"),
    }
}
