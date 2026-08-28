use notagent_ai::types::{Tool, ToolCall};
use notagent_ai::utils::validation::{
    SchemaOrigin, validate_tool_arguments, validate_tool_arguments_with,
};
use serde_json::{Map, Value, json};

fn tool(parameters: Value) -> Tool {
    Tool {
        name: "echo".to_string(),
        description: "Echo tool".to_string(),
        parameters,
        constrained_sampling: None,
    }
}

fn tool_call(arguments: Value) -> ToolCall {
    ToolCall {
        id: "tool-1".to_string(),
        name: "echo".to_string(),
        arguments: arguments.as_object().cloned().unwrap_or_else(Map::new),
        thought_signature: None,
        namespace: None,
        extra: Map::new(),
    }
}

/// `createToolCallWithPlainSchema(schema, value)`
fn plain_schema_call(schema: Value, value: Value) -> (Tool, ToolCall) {
    (
        tool(json!({"type": "object", "properties": {"value": schema}, "required": ["value"]})),
        tool_call(json!({ "value": value })),
    )
}

#[test]
fn converts_string_arguments_for_typebox_schemas() {
    // TypeBox fallback must coerce "42" to 42 just like the generated validator.
    let tool = tool(
        json!({"type": "object", "properties": {"count": {"type": "number"}}, "required": ["count"]}),
    );
    let call = tool_call(json!({"count": "42"}));
    assert_eq!(
        validate_tool_arguments(&tool, &call).unwrap(),
        json!({"count": 42.0})
    );
}

#[test]
fn coerces_serialized_plain_json_schemas_with_ajv_compatible_primitive_rules() {
    let passing_cases: Vec<(Value, Value, Value)> = vec![
        (json!({"type": "number"}), json!("42"), json!(42.0)),
        (json!({"type": "number"}), json!(true), json!(1.0)),
        (json!({"type": "number"}), json!(null), json!(0.0)),
        (json!({"type": "integer"}), json!("42"), json!(42.0)),
        (json!({"type": "boolean"}), json!("true"), json!(true)),
        (json!({"type": "boolean"}), json!("false"), json!(false)),
        (json!({"type": "boolean"}), json!(1), json!(true)),
        (json!({"type": "boolean"}), json!(0), json!(false)),
        (json!({"type": "string"}), json!(null), json!("")),
        (json!({"type": "string"}), json!(true), json!("true")),
        (json!({"type": "null"}), json!(""), json!(null)),
        (json!({"type": "null"}), json!(0), json!(null)),
        (json!({"type": "null"}), json!(false), json!(null)),
        (
            json!({"type": ["number", "string"]}),
            json!("1"),
            json!("1"),
        ),
        (
            json!({"type": ["boolean", "number"]}),
            json!("1"),
            json!(1.0),
        ),
    ];

    for (schema, input, expected) in passing_cases {
        let (tool, call) = plain_schema_call(schema.clone(), input.clone());
        let actual = validate_tool_arguments_with(&tool, &call, SchemaOrigin::PlainJsonSchema)
            .unwrap_or_else(|error| panic!("schema {schema} input {input}: {error}"));
        assert_eq!(
            actual,
            json!({ "value": expected }),
            "schema {schema} input {input}"
        );
    }
}

#[test]
fn rejects_invalid_coercions_for_serialized_plain_json_schemas() {
    let failing_cases: Vec<(Value, Value)> = vec![
        (json!({"type": "boolean"}), json!("1")),
        (json!({"type": "boolean"}), json!("0")),
        (json!({"type": "null"}), json!("null")),
        (json!({"type": "integer"}), json!("42.1")),
    ];

    for (schema, input) in failing_cases {
        let (tool, call) = plain_schema_call(schema.clone(), input.clone());
        let error = validate_tool_arguments_with(&tool, &call, SchemaOrigin::PlainJsonSchema)
            .expect_err(&format!("schema {schema} input {input} must fail"));
        assert!(
            error.to_string().starts_with("Validation failed"),
            "{error}"
        );
    }
}

#[test]
fn treats_null_as_omission_for_optional_non_nullable_properties() {
    let tool = tool(json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "offset": {"type": "number"},
            "nullable": {"anyOf": [{"type": "string"}, {"type": "null"}]},
            "metadata": {"type": "object", "properties": {"enabled": {"type": "boolean"}}}
        },
        "required": ["path", "metadata"]
    }));
    let call = tool_call(json!({
        "path": "file.txt", "offset": null, "nullable": null, "metadata": {"enabled": null}
    }));

    assert_eq!(
        validate_tool_arguments(&tool, &call).unwrap(),
        json!({"path": "file.txt", "nullable": null, "metadata": {}})
    );
}

#[test]
fn preserves_optional_nulls_whose_referenced_schema_is_nullable() {
    let tool = tool(json!({
        "type": "object",
        "properties": {"value": {"$ref": "#/$defs/value"}},
        "$defs": {"value": {"anyOf": [{"type": "number"}, {"type": "null"}]}}
    }));
    let call = tool_call(json!({"value": null}));
    assert_eq!(
        validate_tool_arguments(&tool, &call).unwrap(),
        json!({"value": null})
    );
}

#[test]
fn preserves_a_value_that_already_matches_a_nullable_union_arm() {
    let tool = tool(json!({
        "type": "object",
        "properties": {"value": {"anyOf": [{"type": "number"}, {"type": "null"}]}},
        "required": ["value"]
    }));
    let call = tool_call(json!({"value": null}));
    assert_eq!(
        validate_tool_arguments(&tool, &call).unwrap(),
        json!({"value": null})
    );
}

#[test]
fn preserves_a_value_that_already_matches_a_one_of_nullable_union_arm() {
    let (tool, call) = plain_schema_call(
        json!({"oneOf": [{"type": "number"}, {"type": "null"}]}),
        json!(null),
    );
    assert_eq!(
        validate_tool_arguments_with(&tool, &call, SchemaOrigin::PlainJsonSchema).unwrap(),
        json!({"value": null})
    );
}

#[test]
fn still_coerces_nullable_unions_when_the_value_does_not_match_any_arm() {
    let (tool, call) = plain_schema_call(
        json!({"anyOf": [{"type": "number"}, {"type": "null"}]}),
        json!("42"),
    );
    assert_eq!(
        validate_tool_arguments_with(&tool, &call, SchemaOrigin::PlainJsonSchema).unwrap(),
        json!({"value": 42.0})
    );
}

#[test]
fn accepts_null_for_nullable_array_schemas_with_items() {
    let (tool, call) = plain_schema_call(
        json!({"type": ["array", "null"], "items": {"type": "string"}}),
        json!(null),
    );
    assert_eq!(
        validate_tool_arguments_with(&tool, &call, SchemaOrigin::PlainJsonSchema).unwrap(),
        json!({"value": null})
    );
}

#[test]
fn error_message_matches_the_public_format() {
    let tool = tool(json!({
        "type": "object",
        "properties": {"count": {"type": "number"}, "name": {"type": "string"}},
        "required": ["count", "name"]
    }));
    let call = tool_call(json!({}));
    let error = validate_tool_arguments(&tool, &call).expect_err("must fail");
    assert_eq!(
        error.to_string(),
        "Validation failed for tool \"echo\":\n  - count: must have required properties count, name\n\nReceived arguments:\n{}"
    );
}

#[test]
fn nested_type_errors_use_dotted_paths() {
    let tool = tool(json!({
        "type": "object",
        "properties": {"tags": {"type": "array", "items": {"type": "string"}}},
        "required": ["tags"]
    }));
    // `Value.Convert` turns a number into a string, so use a value it cannot convert.
    let call = tool_call(json!({"tags": [{}]}));
    let error = validate_tool_arguments(&tool, &call).expect_err("must fail");
    assert!(
        error.to_string().contains("  - tags.0: must be string"),
        "{error}"
    );
}
