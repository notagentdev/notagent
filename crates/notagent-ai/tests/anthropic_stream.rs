use notagent_ai::api::anthropic_messages::{
    ANTHROPIC_MESSAGE_EVENTS, AnthropicStreamState, map_stop_reason,
};
use notagent_ai::api::sse::SseDecoder;
use notagent_ai::types::*;
use serde_json::{Value, json};

fn model() -> Model {
    Model {
        id: "claude-opus-4-5".to_string(),
        name: "Claude Opus 4.5".to_string(),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![Modality::Text, Modality::Image],
        cost: ModelCost {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
            tiers: None,
        },
        context_window: 200_000,
        max_tokens: 64_000,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn sse_body(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(event, data)| format!("event: {event}\ndata: {data}\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Feeds an SSE body through the decoder and the state machine.
fn run(
    events: &[(&str, Value)],
    tool_names: Vec<String>,
    is_oauth: bool,
) -> (
    Vec<AssistantMessageEvent>,
    AssistantMessage,
    Result<DoneReason, String>,
) {
    let mut decoder = SseDecoder::new();
    let mut state = AnthropicStreamState::new(&model(), is_oauth, tool_names, 1_700_000_000_000);
    let mut emitted = vec![AssistantMessageEvent::Start {
        partial: state.output.clone(),
    }];

    let body = sse_body(events);
    let mut sse_events = decoder.feed(&body);
    sse_events.extend(decoder.finish());

    for sse in sse_events {
        let Some(name) = sse.event.as_deref() else {
            continue;
        };
        if !ANTHROPIC_MESSAGE_EVENTS.contains(&name) {
            continue;
        }
        let parsed: Value = serde_json::from_str(&sse.data).expect("event data is JSON");
        emitted.extend(state.process_event(&parsed).expect("event processing"));
    }

    let done = state.finish().map_err(|error| error.to_string());
    (emitted, state.output.clone(), done)
}

fn minimal_events() -> Vec<(&'static str, Value)> {
    vec![
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "msg_test", "usage": {
                "input_tokens": 12, "output_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0
            }}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {
                "input_tokens": 12, "output_tokens": 5, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0
            }}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]
}

#[test]
fn parses_a_minimal_text_stream() {
    let (events, message, done) = run(&minimal_events(), vec![], false);

    let names: Vec<&str> = events.iter().map(event_name).collect();
    assert_eq!(names, ["start", "text_start", "text_delta", "text_end"]);
    assert_eq!(done, Ok(DoneReason::Stop));
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(message.raw_stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(message.response_id.as_deref(), Some("msg_test"));
    assert_eq!(
        message.content,
        vec![AssistantContent::Text(TextContent::new("Hello"))]
    );
    assert_eq!(message.usage.input, 12);
    assert_eq!(message.usage.output, 5);
    assert_eq!(message.usage.total_tokens, Some(17));
    // 12 input tokens at $5/M plus 5 output tokens at $25/M.
    assert_eq!(message.usage.cost.input, 5.0 / 1_000_000.0 * 12.0);
    assert_eq!(message.usage.cost.output, 25.0 / 1_000_000.0 * 5.0);
    assert_eq!(
        message.usage.cost.total,
        message.usage.cost.input + message.usage.cost.output
    );
}

#[test]
fn assembles_tool_calls_from_input_json_deltas() {
    let events = vec![
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "m", "usage": {}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {}}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"path\":"}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": " \"a.txt\"}"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ];
    let (emitted, message, done) = run(&events, vec![], false);

    assert_eq!(done, Ok(DoneReason::ToolUse));
    let AssistantContent::ToolCall(tool_call) = &message.content[0] else {
        panic!("tool call")
    };
    assert_eq!(tool_call.id, "toolu_1");
    assert_eq!(tool_call.name, "read");
    assert_eq!(tool_call.arguments.get("path"), Some(&json!("a.txt")));
    assert!(
        tool_call.extra.is_empty(),
        "the scratch buffer must not reach the content block"
    );

    let first_delta = emitted
        .iter()
        .find(|event| event_name(event) == "toolcall_delta")
        .expect("delta");
    let AssistantMessageEvent::ToolcallDelta { partial, .. } = first_delta else {
        panic!("delta")
    };
    let AssistantContent::ToolCall(partial_call) = &partial.content[0] else {
        panic!("tool call")
    };
    assert!(partial_call.arguments.is_empty() || partial_call.arguments.contains_key("path"));
}

#[test]
fn maps_thinking_blocks_including_signatures_and_redaction() {
    let events = vec![
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "m", "usage": {}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "step"}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig-"}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "part2"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "redacted_thinking", "data": "opaque"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 1}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ];
    let (_, message, done) = run(&events, vec![], false);
    assert_eq!(done, Ok(DoneReason::Stop));

    let AssistantContent::Thinking(thinking) = &message.content[0] else {
        panic!("thinking")
    };
    assert_eq!(thinking.thinking, "step");
    assert_eq!(
        thinking.thinking_signature.as_deref(),
        Some("sig-part2"),
        "signature deltas concatenate"
    );
    assert_eq!(thinking.redacted, None);

    let AssistantContent::Thinking(redacted) = &message.content[1] else {
        panic!("redacted thinking")
    };
    assert_eq!(redacted.thinking, "[Reasoning redacted]");
    assert_eq!(redacted.thinking_signature.as_deref(), Some("opaque"));
    assert_eq!(redacted.redacted, Some(true));
}

#[test]
fn maps_claude_code_tool_names_back_for_oauth_requests() {
    let events = vec![
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "m", "usage": {}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t", "name": "Read", "input": {}}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ];

    let (_, message, _) = run(&events, vec!["read".to_string()], true);
    let AssistantContent::ToolCall(tool_call) = &message.content[0] else {
        panic!("tool call")
    };
    assert_eq!(
        tool_call.name, "read",
        "the caller's casing wins for OAuth requests"
    );

    // Without OAuth the provider name is kept verbatim.
    let (_, message, _) = run(&events, vec!["read".to_string()], false);
    let AssistantContent::ToolCall(tool_call) = &message.content[0] else {
        panic!("tool call")
    };
    assert_eq!(tool_call.name, "Read");
}

#[test]
fn keeps_input_tokens_when_message_delta_omits_them() {
    let mut events = minimal_events();
    events[4] = (
        "message_delta",
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 7}}),
    );
    let (_, message, _) = run(&events, vec![], false);
    assert_eq!(
        message.usage.input, 12,
        "proxies may omit input_tokens in message_delta"
    );
    assert_eq!(message.usage.output, 7);
}

#[test]
fn reads_the_1h_cache_write_split_and_thinking_tokens() {
    let events = vec![
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "m", "usage": {
                "input_tokens": 10, "cache_creation_input_tokens": 100,
                "cache_creation": {"ephemeral_1h_input_tokens": 40}
            }}}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"},
                   "usage": {"output_tokens": 20, "output_tokens_details": {"thinking_tokens": 12}}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ];
    let (_, message, _) = run(&events, vec![], false);
    assert_eq!(message.usage.cache_write, 100);
    assert_eq!(message.usage.cache_write1h, Some(40));
    assert_eq!(message.usage.reasoning, Some(12));
    // 60 short writes at 6.25 plus 40 long writes at twice the input rate.
    assert_eq!(
        message.usage.cost.cache_write,
        (6.25 * 60.0 + 5.0 * 2.0 * 40.0) / 1_000_000.0
    );
}

#[test]
fn a_stream_without_a_stop_reason_is_an_error() {
    let events = vec![
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "m", "usage": {}}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ];
    let (_, _, done) = run(&events, vec![], false);
    assert_eq!(
        done,
        Err("Anthropic stream ended without a stop reason".to_string())
    );
}

#[test]
fn detects_a_stream_that_ends_before_message_stop() {
    let mut decoder = SseDecoder::new();
    let mut state = AnthropicStreamState::new(&model(), false, vec![], 0);
    let body = sse_body(&[(
        "message_start",
        json!({"type": "message_start", "message": {"id": "m", "usage": {}}}),
    )]);
    let mut sse_events = decoder.feed(&body);
    sse_events.extend(decoder.finish());
    for sse in sse_events {
        let parsed: Value = serde_json::from_str(&sse.data).expect("JSON");
        state.process_event(&parsed).expect("processing");
    }
    assert!(state.ended_without_message_stop());
}

#[test]
fn maps_every_documented_stop_reason() {
    assert_eq!(
        map_stop_reason("end_turn", None).unwrap(),
        (StopReason::Stop, None)
    );
    assert_eq!(
        map_stop_reason("max_tokens", None).unwrap(),
        (StopReason::Length, None)
    );
    assert_eq!(
        map_stop_reason("tool_use", None).unwrap(),
        (StopReason::ToolUse, None)
    );
    assert_eq!(
        map_stop_reason("pause_turn", None).unwrap(),
        (StopReason::Stop, None)
    );
    assert_eq!(
        map_stop_reason("stop_sequence", None).unwrap(),
        (StopReason::Stop, None)
    );
    assert_eq!(
        map_stop_reason("sensitive", None).unwrap(),
        (
            StopReason::Error,
            Some("Provider stopped with: sensitive".to_string())
        )
    );
    assert_eq!(
        map_stop_reason("refusal", None).unwrap(),
        (
            StopReason::Error,
            Some("The model refused to complete the request".to_string())
        )
    );
    assert_eq!(
        map_stop_reason("refusal", Some(&json!({"explanation": "why"}))).unwrap(),
        (StopReason::Error, Some("why".to_string()))
    );
    // Unknown values fail loudly so new API values are noticed.
    assert!(map_stop_reason("brand_new", None).is_err());
}

#[test]
fn refusals_surface_as_stream_errors() {
    let events = vec![
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "m", "usage": {}}}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "refusal", "stop_details": {"explanation": "not allowed"}}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ];
    let (_, message, done) = run(&events, vec![], false);
    assert_eq!(done, Err("not allowed".to_string()));
    assert_eq!(message.stop_reason, StopReason::Error);
}

#[test]
fn content_indices_follow_the_order_blocks_arrive_in() {
    // their index and reports the array position.
    let events = vec![
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "m", "usage": {}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 5, "content_block": {"type": "text", "text": ""}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 9, "content_block": {"type": "text", "text": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 9, "delta": {"type": "text_delta", "text": "second"}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 5, "delta": {"type": "text_delta", "text": "first"}}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ];
    let (emitted, message, _) = run(&events, vec![], false);

    let deltas: Vec<(usize, String)> = emitted
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
                ..
            } => Some((*content_index, delta.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        deltas,
        [(1, "second".to_string()), (0, "first".to_string())]
    );
    assert_eq!(message.content.len(), 2);
}

fn event_name(event: &AssistantMessageEvent) -> &'static str {
    match event {
        AssistantMessageEvent::Start { .. } => "start",
        AssistantMessageEvent::TextStart { .. } => "text_start",
        AssistantMessageEvent::TextDelta { .. } => "text_delta",
        AssistantMessageEvent::TextEnd { .. } => "text_end",
        AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
        AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
        AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
        AssistantMessageEvent::ToolcallStart { .. } => "toolcall_start",
        AssistantMessageEvent::ToolcallDelta { .. } => "toolcall_delta",
        AssistantMessageEvent::ToolcallEnd { .. } => "toolcall_end",
        AssistantMessageEvent::Done { .. } => "done",
        AssistantMessageEvent::Error { .. } => "error",
    }
}
