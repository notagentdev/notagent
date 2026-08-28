use notagent_ai::utils::error_body::{
    MAX_PROVIDER_ERROR_BODY_CHARS, RawProviderError, format_provider_error,
    normalize_provider_error,
};
use serde_json::json;

#[test]
fn extracts_status_and_body_from_a_mistral_shaped_error() {
    let normalized = normalize_provider_error(&RawProviderError {
        status: Some(403),
        body_text: Some(r#"{"error":"blocked by gateway WAF"}"#.to_string()),
        body_json: None,
        message: "Mistral request failed".to_string(),
    });

    assert_eq!(normalized.status, Some(403));
    assert_eq!(
        normalized.body.as_deref(),
        Some(r#"{"error":"blocked by gateway WAF"}"#)
    );
    assert!(!normalized.message_carries_body);
}

#[test]
fn reads_the_parsed_body_when_the_message_is_opaque() {
    let normalized = normalize_provider_error(&RawProviderError {
        status: Some(403),
        body_text: None,
        body_json: Some(json!({ "error": "blocked by gateway WAF" })),
        message: "403 status code (no body)".to_string(),
    });

    assert_eq!(normalized.status, Some(403));
    assert_eq!(
        normalized.body.as_deref(),
        Some(r#"{"error":"blocked by gateway WAF"}"#)
    );
    assert!(!normalized.message_carries_body);
}

#[test]
fn preserves_a_message_that_already_carries_the_body() {
    let body = json!({ "error": { "code": 403, "message": "Permission denied" } }).to_string();
    let normalized = normalize_provider_error(&RawProviderError {
        status: Some(403),
        body_text: Some(body.clone()),
        body_json: None,
        message: body.clone(),
    });

    assert_eq!(normalized.status, Some(403));
    assert!(normalized.message_carries_body);
    assert_eq!(normalized.message, body);
}

#[test]
fn a_missing_body_leaves_the_message_in_charge() {
    // Bedrock response streams, SDK wrapper classes and class-instance `error` fields
    // never reach the normalizer as text, so the layer hands over no body at all.
    for (status, message) in [
        (
            400,
            "Invocation of model ID anthropic.claude-opus-5 with on-demand throughput isn't supported.",
        ),
        (400, "Input is too long for requested model."),
        (502, "TLS handshake failed"),
    ] {
        let normalized = normalize_provider_error(&RawProviderError {
            status: Some(status),
            body_text: None,
            body_json: None,
            message: message.to_string(),
        });
        assert_eq!(normalized.status, Some(status));
        assert_eq!(normalized.body, None);
        assert_eq!(normalized.message, message);
        assert!(normalized.message_carries_body);
    }
}

#[test]
fn surfaces_a_plain_parsed_json_body_object() {
    let normalized = normalize_provider_error(&RawProviderError {
        status: Some(400),
        body_text: None,
        body_json: Some(json!({ "message": "schema validation failed", "field": "tools[0]" })),
        message: "400 status code (no body)".to_string(),
    });

    assert_eq!(
        normalized.body.as_deref(),
        Some(r#"{"message":"schema validation failed","field":"tools[0]"}"#)
    );
    assert!(!normalized.message_carries_body);
}

#[test]
fn a_non_error_thrown_value_keeps_its_json_message() {
    let normalized = normalize_provider_error(&RawProviderError {
        status: None,
        body_text: None,
        body_json: None,
        message: r#"{"reason":"boom"}"#.to_string(),
    });

    assert_eq!(normalized.status, None);
    assert_eq!(normalized.body, None);
    assert_eq!(normalized.message, r#"{"reason":"boom"}"#);
    // Without a body there is nothing the message could be missing.
    assert!(normalized.message_carries_body);
    assert_eq!(
        format_provider_error(&normalized, None),
        r#"{"reason":"boom"}"#
    );
}

#[test]
fn an_empty_parsed_body_object_counts_as_no_body() {
    let normalized = normalize_provider_error(&RawProviderError {
        status: Some(403),
        body_text: None,
        body_json: Some(json!({})),
        message: "403 status code (no body)".to_string(),
    });

    assert_eq!(normalized.body, None);
    assert!(normalized.message_carries_body);
}

#[test]
fn the_body_is_truncated_at_the_cap() {
    let long_body = "x".repeat(MAX_PROVIDER_ERROR_BODY_CHARS + 50);
    let normalized = normalize_provider_error(&RawProviderError {
        status: Some(500),
        body_text: Some(long_body.clone()),
        body_json: None,
        message: "failed".to_string(),
    });

    let body = normalized.body.expect("a body");
    assert!(body.contains("... [truncated 50 chars]"));
    assert!(body.len() < long_body.len());
}

#[test]
fn a_message_that_contains_the_body_is_marked_as_carrying_it() {
    let normalized = normalize_provider_error(&RawProviderError {
        status: Some(500),
        body_text: Some("upstream exploded".to_string()),
        body_json: None,
        message: "500: upstream exploded".to_string(),
    });

    assert!(normalized.message_carries_body);
}

#[test]
fn the_formatter_surfaces_status_and_body_with_and_without_a_prefix() {
    let normalized = normalize_provider_error(&RawProviderError {
        status: Some(403),
        body_text: None,
        body_json: Some(json!({ "error": "blocked by gateway WAF" })),
        message: "403 status code (no body)".to_string(),
    });

    let formatted = format_provider_error(&normalized, None);
    assert!(formatted.contains("403"));
    assert!(formatted.contains("blocked by gateway WAF"));
    assert_ne!(formatted, "403 status code (no body)");
    assert_eq!(
        format_provider_error(&normalized, Some("OpenAI API error")),
        r#"OpenAI API error (403): {"error":"blocked by gateway WAF"}"#
    );

    // A message that already carries the body keeps it, prefix and status included.
    let body = json!({ "error": { "message": "Permission denied" } }).to_string();
    let normalized = normalize_provider_error(&RawProviderError {
        status: Some(403),
        body_text: Some(body.clone()),
        body_json: None,
        message: body.clone(),
    });
    assert_eq!(
        format_provider_error(&normalized, Some("OpenAI API error")),
        format!("OpenAI API error (403): {body}")
    );
}
