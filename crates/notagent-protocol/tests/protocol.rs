//! Port von `packages/protocol/test/protocol.test.ts`.

use notagent_protocol::{
    CborValue, ClientHello, ClientMessage, ClientMessageDecoder, Command, FrameDecoder,
    FrameDecoderOptions, HelloTag, ListCommand, ListTag, PROTOCOL_VERSION, ProtocolError,
    ProtocolErrorCode, ProtocolVersionTag, RequestEnvelope, RequestTag, ServerHello,
    ServerHelloError, ServerMessage, ServerMessageDecoder, ServerSnapshot, decode_cbor,
    encode_cbor, encode_client_message, encode_frame, encode_server_message,
    is_supported_protocol_version, parse_client_message, parse_server_message,
};
use serde_json::{Value, json};

fn wire(value: Value) -> CborValue {
    CborValue::from_json_value(&value)
}

fn empty_server_snapshot() -> ServerSnapshot {
    ServerSnapshot {
        server_id: "server-1".to_owned(),
        protocol_version: ProtocolVersionTag,
        revision: 0,
        sessions: vec![],
        models: vec![],
    }
}

fn client_hello() -> ClientHello {
    ClientHello {
        kind: HelloTag,
        version: PROTOCOL_VERSION,
    }
}

fn server_hello() -> ServerHello {
    ServerHello {
        kind: HelloTag,
        version: ProtocolVersionTag,
        connection_id: "connection-1".to_owned(),
        snapshot: empty_server_snapshot(),
    }
}

fn to_wire<T: serde::Serialize>(value: &T) -> CborValue {
    CborValue::from_json_value(&serde_json::to_value(value).expect("serializes"))
}

fn item_message(item: Value, kind: &str) -> CborValue {
    wire(json!({
        "type": "event",
        "event": { "type": "session_progress", "sessionId": "session-1", "progress": { "type": kind, "item": item } },
    }))
}

fn finished_item_message(item: Value) -> CborValue {
    item_message(item, "item_finished")
}

// --- protocol validation -----------------------------------------------------

#[test]
fn uses_protocol_version_1() {
    assert_eq!(PROTOCOL_VERSION, 1);
    assert!(is_supported_protocol_version(1));
    assert!(!is_supported_protocol_version(2));
    // `isSupportedProtocolVersion(2.5)` ist mit u64 nicht darstellbar.
}

#[test]
fn accepts_integer_client_hello_versions_for_negotiation() {
    for version in [0, PROTOCOL_VERSION, PROTOCOL_VERSION + 1] {
        let message = wire(json!({ "type": "hello", "version": version }));
        assert_eq!(
            parse_client_message(&message).expect("accepts"),
            ClientMessage::Hello(ClientHello {
                kind: HelloTag,
                version
            })
        );
    }
}

#[test]
fn rejects_invalid_handshakes() {
    for (label, message) in [
        (
            "string version",
            json!({ "type": "hello", "version": PROTOCOL_VERSION.to_string() }),
        ),
        (
            "fractional version",
            json!({ "type": "hello", "version": 1.5 }),
        ),
        (
            "credential field",
            json!({ "type": "hello", "version": PROTOCOL_VERSION, "token": "secret" }),
        ),
        (
            "unknown field",
            json!({ "type": "hello", "version": PROTOCOL_VERSION, "extra": true }),
        ),
    ] {
        assert!(
            parse_client_message(&wire(message)).is_err(),
            "rejects {label}"
        );
    }
}

#[test]
fn does_not_parse_json_strings_as_wire_messages() {
    let client = CborValue::text(serde_json::to_string(&client_hello()).unwrap());
    let server = CborValue::text(serde_json::to_string(&server_hello()).unwrap());
    assert!(parse_client_message(&client).is_err());
    assert!(parse_server_message(&server).is_err());
}

#[test]
fn rejects_image_input_while_the_mvp_remains_text_only() {
    let message = wire(json!({
        "type": "request",
        "id": "request-1",
        "request": {
            "command": "prompt",
            "sessionId": "session-1",
            "text": "inspect",
            "images": [{ "type": "image", "data": "abc", "mimeType": "image/png" }],
        },
    }));
    assert!(parse_client_message(&message).is_err());
}

#[test]
fn parses_a_server_handshake_snapshot() {
    let expected = ServerMessage::Hello(server_hello());
    assert_eq!(
        parse_server_message(&to_wire(&expected)).expect("accepts"),
        expected
    );
}

#[test]
fn represents_listed_sessions_as_durable_metadata() {
    let message = json!({
        "type": "response",
        "id": "request-1",
        "ok": true,
        "result": {
            "command": "list",
            "sessions": [{
                "id": "session-1",
                "createdAt": 1,
                "updatedAt": 2,
                "parentSessionId": "parent-1",
                "sessionName": "Named session",
                "cwd": "/workspace",
            }],
        },
    });
    let parsed = parse_server_message(&wire(message.clone())).expect("accepts");
    assert_eq!(serde_json::to_value(&parsed).unwrap(), message);

    let mut runtime = message;
    runtime["result"]["sessions"] = json!([{ "id": "session-1", "createdAt": 1, "phase": "idle" }]);
    assert!(parse_server_message(&wire(runtime)).is_err());
}

#[test]
fn accepts_the_not_implemented_and_internal_error_codes() {
    for code in [
        ProtocolErrorCode::NotImplemented,
        ProtocolErrorCode::InternalError,
    ] {
        let message = ServerMessage::Response(notagent_protocol::ResponseEnvelope::Error(
            notagent_protocol::ErrorResponseEnvelope {
                kind: notagent_protocol::ResponseTag,
                id: "request-1".to_owned(),
                ok: notagent_protocol::FalseTag,
                error: ProtocolError {
                    code,
                    message: "safe".to_owned(),
                    details: None,
                },
            },
        ));
        assert_eq!(
            parse_server_message(&to_wire(&message)).expect("accepts"),
            message
        );
    }
}

#[test]
fn rejects_invalid_server_messages() {
    for message in [
        json!({
            "type": "hello",
            "version": PROTOCOL_VERSION + 1,
            "connectionId": "connection-1",
            "snapshot": { "serverId": "server-1", "protocolVersion": 1, "revision": 0, "sessions": [], "models": [] },
        }),
        json!({ "type": "hello_error", "error": { "code": "auth", "message": "Authentication failed" } }),
        json!({ "type": "response", "id": "request-1", "ok": true, "result": { "command": "unknown" } }),
        json!({ "type": "event", "event": { "type": "session_removed", "sessionId": 42 } }),
    ] {
        assert!(
            parse_server_message(&wire(message.clone())).is_err(),
            "rejects {message}"
        );
    }
}

#[test]
fn validates_nested_json_tool_details() {
    let message = json!({
        "type": "event",
        "event": {
            "type": "session_progress",
            "sessionId": "session-1",
            "progress": {
                "type": "item_finished",
                "item": {
                    "id": "tool-1",
                    "role": "tool",
                    "toolCallId": "call-1",
                    "toolName": "read",
                    "input": { "path": "/tmp/file" },
                    "content": [{ "type": "text", "text": "done" }],
                    "details": { "lines": [1, 2, 3], "cached": false },
                    "status": "complete",
                    "isError": false,
                    "timestamp": 1,
                },
            },
        },
    });
    let parsed = parse_server_message(&wire(message.clone())).expect("accepts");
    assert_eq!(serde_json::to_value(&parsed).unwrap(), message);
}

fn assistant_item(state: Value) -> Value {
    let mut item = json!({
        "id": "assistant-1",
        "role": "assistant",
        "content": [{ "type": "text", "text": "hello" }],
        "model": { "provider": "test", "id": "model" },
        "timestamp": 1,
    });
    merge(&mut item, state);
    item
}

fn tool_item(state: Value) -> Value {
    let mut item = json!({
        "id": "tool-1",
        "role": "tool",
        "toolCallId": "call-1",
        "toolName": "read",
        "input": {},
        "content": [],
        "timestamp": 1,
    });
    merge(&mut item, state);
    item
}

fn merge(target: &mut Value, source: Value) {
    let object = target.as_object_mut().expect("object");
    for (key, value) in source.as_object().expect("object") {
        object.insert(key.clone(), value.clone());
    }
}

#[test]
fn accepts_consistent_assistant_items() {
    for state in [
        json!({ "status": "streaming" }),
        json!({ "status": "complete", "stopReason": "stop" }),
        json!({ "status": "error", "stopReason": "error" }),
        json!({ "status": "error", "stopReason": "error", "errorMessage": "failed" }),
        json!({ "status": "aborted", "stopReason": "aborted" }),
    ] {
        let kind = if state["status"] == json!("streaming") {
            "item_updated"
        } else {
            "item_finished"
        };
        let message = item_message(assistant_item(state.clone()), kind);
        assert!(parse_server_message(&message).is_ok(), "accepts {state}");
    }
}

#[test]
fn rejects_inconsistent_assistant_items() {
    for state in [
        json!({ "status": "streaming", "stopReason": "stop" }),
        json!({ "status": "complete" }),
        json!({ "status": "complete", "stopReason": "error" }),
        json!({ "status": "error", "stopReason": "error", "errorMessage": "" }),
        json!({ "status": "aborted", "stopReason": "stop" }),
    ] {
        let message = finished_item_message(assistant_item(state.clone()));
        assert!(parse_server_message(&message).is_err(), "rejects {state}");
    }
}

#[test]
fn accepts_consistent_tool_items() {
    for state in [
        json!({ "status": "running", "isError": false }),
        json!({ "status": "complete", "isError": false }),
        json!({ "status": "error", "isError": true }),
    ] {
        let kind = if state["status"] == json!("running") {
            "item_updated"
        } else {
            "item_finished"
        };
        let message = item_message(tool_item(state.clone()), kind);
        assert!(parse_server_message(&message).is_ok(), "accepts {state}");
    }
}

#[test]
fn rejects_nonterminal_items_reported_as_finished() {
    let assistant = json!({
        "id": "assistant-1",
        "role": "assistant",
        "content": [],
        "model": { "provider": "test", "id": "model" },
        "status": "streaming",
        "timestamp": 1,
    });
    let tool = json!({
        "id": "tool-1",
        "role": "tool",
        "toolCallId": "call-1",
        "toolName": "read",
        "input": {},
        "content": [],
        "status": "running",
        "isError": false,
        "timestamp": 1,
    });
    assert!(parse_server_message(&finished_item_message(assistant)).is_err());
    assert!(parse_server_message(&finished_item_message(tool)).is_err());
}

#[test]
fn rejects_inconsistent_tool_items() {
    for state in [
        json!({ "status": "running", "isError": true }),
        json!({ "status": "complete", "isError": true }),
        json!({ "status": "error", "isError": false }),
    ] {
        let message = finished_item_message(tool_item(state.clone()));
        assert!(parse_server_message(&message).is_err(), "rejects {state}");
    }
}

// `rejects cyclic protocol values` entfällt: Zyklen sind in `CborValue` nicht
// darstellbar (Abweichung Klasse 1).

#[test]
fn validation_errors_do_not_retain_rejected_payloads() {
    let error = parse_client_message(&wire(json!({
        "type": "hello",
        "version": PROTOCOL_VERSION.to_string(),
        "extra": "x".repeat(2_000_000),
    })))
    .expect_err("rejects");
    assert!(error.message().len() < 1_000, "{}", error.message().len());
}

// --- validated framed protocol APIs -----------------------------------------

#[test]
fn encodes_complete_client_and_server_frames() {
    let client = ClientMessage::Hello(client_hello());
    let mut decoder = FrameDecoder::new(None).expect("valid options");
    let client_frames = decoder
        .push(&encode_client_message(&client, None).expect("encodes"))
        .expect("pushes");
    assert_eq!(client_frames.len(), 1);
    assert_eq!(
        parse_client_message(&decode_cbor(&client_frames[0], None).expect("decodes"))
            .expect("parses"),
        client
    );

    let server = ServerMessage::Hello(server_hello());
    let mut decoder = FrameDecoder::new(None).expect("valid options");
    let server_frames = decoder
        .push(&encode_server_message(&server, None).expect("encodes"))
        .expect("pushes");
    assert_eq!(server_frames.len(), 1);
    assert_eq!(
        parse_server_message(&decode_cbor(&server_frames[0], None).expect("decodes"))
            .expect("parses"),
        server
    );
}

#[test]
fn enforces_an_outbound_frame_limit_before_returning_encoded_bytes() {
    let options = Some(FrameDecoderOptions::with_max_frame_length(8));
    assert!(encode_client_message(&ClientMessage::Hello(client_hello()), options).is_err());
    assert!(encode_server_message(&ServerMessage::Hello(server_hello()), options).is_err());
}

#[test]
fn validates_messages_before_encoding() {
    // `version: PROTOCOL_VERSION + 0.5` ist in Rust nicht darstellbar; die
    // äquivalente Constraint-Verletzung ist eine leere ID (`minLength: 1`).
    let message = ClientMessage::Request(RequestEnvelope {
        kind: RequestTag,
        id: String::new(),
        request: Command::List(ListCommand { command: ListTag }),
    });
    assert!(encode_client_message(&message, None).is_err());
}

#[test]
fn omits_explicit_undefined_optional_properties_on_the_wire() {
    let message = ClientMessage::Request(RequestEnvelope {
        kind: RequestTag,
        id: "request-1".to_owned(),
        request: Command::Create(notagent_protocol::CreateCommand {
            command: notagent_protocol::CreateTag,
            cwd: None,
            name: None,
            model: None,
            thinking_level: None,
        }),
    });
    let mut decoder = FrameDecoder::new(None).expect("valid options");
    let frames = decoder
        .push(&encode_client_message(&message, None).expect("encodes"))
        .expect("pushes");
    let payload = decode_cbor(&frames[0], None).expect("decodes");
    assert_eq!(
        payload,
        wire(json!({ "type": "request", "id": "request-1", "request": { "command": "create" } }))
    );
}

#[test]
fn incrementally_decodes_fragmented_and_coalesced_client_messages() {
    let hello = ClientMessage::Hello(client_hello());
    let request = ClientMessage::Request(RequestEnvelope {
        kind: RequestTag,
        id: "request-1".to_owned(),
        request: Command::List(ListCommand { command: ListTag }),
    });
    let mut wire = encode_client_message(&hello, None).expect("encodes");
    wire.extend(encode_client_message(&request, None).expect("encodes"));

    for split in 0..=wire.len() {
        let mut decoder = ClientMessageDecoder::new(None).expect("valid options");
        let mut messages = decoder.push(&wire[0..split]).expect("pushes");
        messages.extend(decoder.push(&wire[split..]).expect("pushes"));
        decoder.end().expect("ends");
        assert_eq!(
            messages,
            vec![hello.clone(), request.clone()],
            "split at {split}"
        );
    }
}

#[test]
fn incrementally_decodes_server_messages() {
    let message = ServerMessage::HelloError(ServerHelloError {
        kind: notagent_protocol::HelloErrorTag,
        error: ProtocolError {
            code: ProtocolErrorCode::Version,
            message: "Unsupported protocol version".to_owned(),
            details: None,
        },
    });
    let mut decoder = ServerMessageDecoder::new(None).expect("valid options");
    assert_eq!(
        decoder
            .push(&encode_server_message(&message, None).expect("encodes"))
            .expect("pushes"),
        vec![message]
    );
    decoder.end().expect("ends");
}

#[test]
fn rejects_invalid_framed_client_input() {
    let schema_invalid = encode_cbor(
        &wire(json!({ "type": "hello", "version": PROTOCOL_VERSION, "extra": true })),
        None,
    )
    .expect("encodes");
    for (label, frame) in [
        ("empty CBOR payload", encode_frame(&[]).expect("frames")),
        ("malformed CBOR", encode_frame(&[0xff]).expect("frames")),
        (
            "schema-invalid CBOR",
            encode_frame(&schema_invalid).expect("frames"),
        ),
    ] {
        let mut decoder = ClientMessageDecoder::new(None).expect("valid options");
        assert!(decoder.push(&frame).is_err(), "rejects {label}");
        let error = decoder
            .push(
                &encode_client_message(&ClientMessage::Hello(client_hello()), None)
                    .expect("encodes"),
            )
            .expect_err("stays failed");
        assert!(
            error.message().to_lowercase().contains("failed"),
            "{}",
            error.message()
        );
    }
}

#[test]
fn rejects_cbor_byte_strings_nested_in_json_valued_fields() {
    let payload = encode_cbor(
        &CborValue::map([
            ("type", CborValue::text("response")),
            ("id", CborValue::text("request-1")),
            ("ok", CborValue::Bool(false)),
            (
                "error",
                CborValue::map([
                    ("code", CborValue::text("invalid_request")),
                    ("message", CborValue::text("invalid")),
                    (
                        "details",
                        CborValue::map([("nested", CborValue::Bytes(vec![1, 2, 3]))]),
                    ),
                ]),
            ),
        ]),
        None,
    )
    .expect("encodes");
    let frame = encode_frame(&payload).expect("frames");
    let mut decoder = ServerMessageDecoder::new(None).expect("valid options");
    assert!(decoder.push(&frame).is_err());
}

#[test]
fn rejects_truncated_and_oversized_framing_through_the_validated_decoder() {
    let mut truncated = ServerMessageDecoder::new(None).expect("valid options");
    assert_eq!(truncated.push(&[0, 0, 0, 2, 1]).expect("pushes"), vec![]);
    assert!(truncated.end().is_err());

    let mut oversized =
        ClientMessageDecoder::new(Some(FrameDecoderOptions::with_max_frame_length(3)))
            .expect("valid options");
    assert!(oversized.push(&[0, 0, 0, 4]).is_err());
}
