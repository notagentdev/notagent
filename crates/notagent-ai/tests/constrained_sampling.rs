//! Port of the provider-independent cases of
//! `packages/ai/test/constrained-sampling.test.ts` (305 LOC). The cases that drive
//! `convertResponsesTools` belong to the OpenAI Responses port (task 9).

use notagent_ai::api::constrained_sampling::*;
use notagent_ai::types::{
    ConstrainedSampling, ConstrainedSamplingConfig, GrammarFormat, StrictMode, Tool,
};
use serde_json::{Value, json};

fn tool(parameters: Value, constrained_sampling: Option<ConstrainedSampling>) -> Tool {
    Tool {
        name: "sample_tool".to_string(),
        description: "Sample tool".to_string(),
        parameters,
        constrained_sampling,
    }
}

/// `Type.Object({ path, offset?, metadata: Object({ enabled? }), nullable?: string|null })`
fn typebox_like_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "offset": {"type": "number"},
            "metadata": {
                "type": "object",
                "properties": {"enabled": {"type": "boolean"}},
                "required": []
            },
            "nullable": {"anyOf": [{"type": "string"}, {"type": "null"}]}
        },
        "required": ["path", "metadata"]
    })
}

#[test]
fn derives_strict_provider_schemas_without_changing_the_tool_definition() {
    let parameters = typebox_like_parameters();
    let strict = make_strict_json_schema(&parameters).expect("convertible");

    // The tool definition is untouched.
    assert!(parameters.get("additionalProperties").is_none());
    assert_eq!(parameters["required"], json!(["path", "metadata"]));

    assert_eq!(strict["additionalProperties"], json!(false));
    assert_eq!(
        strict["required"],
        json!(["path", "offset", "metadata", "nullable"])
    );
    assert_eq!(
        strict["properties"]["offset"],
        json!({"anyOf": [{"type": "number"}, {"type": "null"}]})
    );
    assert_eq!(
        strict["properties"]["metadata"]["additionalProperties"],
        json!(false)
    );
    assert_eq!(
        strict["properties"]["metadata"]["required"],
        json!(["enabled"])
    );
    assert_eq!(
        strict["properties"]["metadata"]["properties"]["enabled"],
        json!({"anyOf": [{"type": "boolean"}, {"type": "null"}]})
    );
    // A property that already allows null is not wrapped again.
    assert_eq!(
        strict["properties"]["nullable"],
        json!({"anyOf": [{"type": "string"}, {"type": "null"}]})
    );
}

#[test]
fn rejects_schemas_that_cannot_be_safely_converted() {
    let cases: Vec<(Value, &str)> = vec![
        (
            json!({"type": "object", "properties": {"metadata": {"type": "object", "additionalProperties": {"type": "string"}}}, "required": ["metadata"]}),
            "additionalProperties is unsupported",
        ),
        (
            json!({"allOf": [{"type": "object"}, {"type": "object"}], "type": "object"}),
            "allOf schemas are unsupported",
        ),
        (
            json!({"type": "object", "properties": {"value": {"anyOf": [{"type": "object", "properties": {"nested": {"type": "string"}}}, {"type": "null"}]}}, "required": ["value"]}),
            "object and array unions are unsupported",
        ),
        (
            json!({"type": "object", "properties": {"child": {"$ref": "https://example.com/child.json"}}, "required": ["child"]}),
            "$ref schemas are unsupported",
        ),
    ];

    for (parameters, expected_error) in cases {
        let error = make_strict_json_schema(&parameters).expect_err("must not convert");
        assert!(
            error.to_string().contains(expected_error),
            "{error} does not contain {expected_error}"
        );

        // strict: "prefer" falls back to no constrained sampling.
        let preferring = tool(
            parameters.clone(),
            Some(ConstrainedSampling::Config(
                ConstrainedSamplingConfig::JsonSchema {
                    strict: StrictMode::Prefer,
                },
            )),
        );
        assert_eq!(
            resolve_json_schema_strict_sampling(&preferring, true).unwrap(),
            None
        );

        // strict: "require" surfaces the reason.
        let requiring = tool(
            parameters,
            Some(ConstrainedSampling::Config(
                ConstrainedSamplingConfig::JsonSchema {
                    strict: StrictMode::Require,
                },
            )),
        );
        let error = resolve_json_schema_strict_sampling(&requiring, true).expect_err("must fail");
        assert!(
            error
                .to_string()
                .contains("requires JSON-schema constrained sampling"),
            "{error}"
        );
        assert!(error.to_string().contains(expected_error), "{error}");
    }
}

#[test]
fn strict_sampling_resolution_follows_provider_support() {
    let convertible = tool(
        typebox_like_parameters(),
        Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::JsonSchema {
                strict: StrictMode::Prefer,
            },
        )),
    );
    assert_eq!(
        resolve_json_schema_strict_sampling(&convertible, true).unwrap(),
        Some(true)
    );
    // Provider without strict support: "prefer" simply falls back.
    assert_eq!(
        resolve_json_schema_strict_sampling(&convertible, false).unwrap(),
        None
    );

    let requiring = tool(
        typebox_like_parameters(),
        Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::JsonSchema {
                strict: StrictMode::Require,
            },
        )),
    );
    let error = resolve_json_schema_strict_sampling(&requiring, false).expect_err("must fail");
    assert!(
        error.to_string().contains("strict tools are unsupported"),
        "{error}"
    );

    // No config and the explicit `false` both mean: no constrained sampling.
    assert_eq!(
        resolve_json_schema_strict_sampling(&tool(typebox_like_parameters(), None), true).unwrap(),
        None
    );
    assert_eq!(
        resolve_json_schema_strict_sampling(
            &tool(
                typebox_like_parameters(),
                Some(ConstrainedSampling::Disabled(false))
            ),
            true
        )
        .unwrap(),
        None
    );
}

fn grammar_tool(variants: Vec<(GrammarFormat, &str)>, parameters: Value) -> Tool {
    tool(
        parameters,
        Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::Grammar {
                variants: variants
                    .into_iter()
                    .map(|(format, definition)| (format, definition.to_string()))
                    .collect(),
            },
        )),
    )
}

fn grammar_parameters() -> Value {
    json!({"type": "object", "properties": {"payload": {"type": "string"}}, "required": ["payload"]})
}

#[test]
fn resolves_grammar_variants_and_the_input_property() {
    let lark = grammar_tool(
        vec![(GrammarFormat::OpenaiLark, "start: /[a-z]+/")],
        grammar_parameters(),
    );
    let resolved = resolve_grammar_constrained_sampling(&lark, true)
        .unwrap()
        .expect("grammar");
    assert_eq!(resolved.format, GrammarSyntax::Lark);
    assert_eq!(resolved.definition, "start: /[a-z]+/");
    assert_eq!(resolved.input_property, "payload");

    // Lark wins when both variants exist.
    let both = grammar_tool(
        vec![
            (GrammarFormat::OpenaiLark, "start: /a/"),
            (GrammarFormat::OpenaiRegex, "[a-z]+"),
        ],
        grammar_parameters(),
    );
    assert_eq!(
        resolve_grammar_constrained_sampling(&both, true)
            .unwrap()
            .unwrap()
            .format,
        GrammarSyntax::Lark
    );

    let regex_only = grammar_tool(
        vec![(GrammarFormat::OpenaiRegex, "[a-z]+")],
        grammar_parameters(),
    );
    assert_eq!(
        resolve_grammar_constrained_sampling(&regex_only, true)
            .unwrap()
            .unwrap()
            .format,
        GrammarSyntax::Regex
    );

    // Providers without grammar support fall back silently.
    assert_eq!(
        resolve_grammar_constrained_sampling(&lark, false).unwrap(),
        None
    );
}

#[test]
fn rejects_grammar_tools_without_a_usable_variant_or_input_property() {
    let empty = grammar_tool(vec![], grammar_parameters());
    let error = resolve_grammar_constrained_sampling(&empty, true).expect_err("must fail");
    assert!(
        error
            .to_string()
            .contains("no supported grammar variant was provided"),
        "{error}"
    );

    let blank = grammar_tool(
        vec![(GrammarFormat::OpenaiLark, "   ")],
        grammar_parameters(),
    );
    assert!(resolve_grammar_constrained_sampling(&blank, true).is_err());

    // Two required properties: the input property is ambiguous.
    let ambiguous = grammar_tool(
        vec![(GrammarFormat::OpenaiLark, "start: /a/")],
        json!({"type": "object", "properties": {"a": {"type": "string"}, "b": {"type": "string"}}, "required": ["a", "b"]}),
    );
    let error = resolve_grammar_constrained_sampling(&ambiguous, true).expect_err("must fail");
    assert!(
        error
            .to_string()
            .contains("exactly one required string property"),
        "{error}"
    );

    // The required property must be a string.
    let non_string = grammar_tool(
        vec![(GrammarFormat::OpenaiLark, "start: /a/")],
        json!({"type": "object", "properties": {"payload": {"type": "number"}}, "required": ["payload"]}),
    );
    let error = resolve_grammar_constrained_sampling(&non_string, true).expect_err("must fail");
    assert!(
        error.to_string().contains("must have type string"),
        "{error}"
    );
}

#[test]
fn grammar_tool_input_properties_are_collected_per_tool() {
    let tools = vec![
        grammar_tool(
            vec![(GrammarFormat::OpenaiLark, "start: /a/")],
            grammar_parameters(),
        ),
        tool(grammar_parameters(), None),
    ];
    let properties = create_grammar_tool_input_properties(Some(&tools), true).unwrap();
    assert_eq!(properties.len(), 1);
    assert_eq!(properties.get("sample_tool"), Some(&"payload".to_string()));
}

#[test]
fn keeps_grammar_input_json_deltas_append_only() {
    let mut buffer = GrammarToolInputJsonBuffer::default();
    assert_eq!(
        append_grammar_tool_input_json_delta(&mut buffer, "payload", "ab", false).unwrap(),
        Some("{\"payload\":\"ab".to_string())
    );
    assert_eq!(
        append_grammar_tool_input_json_delta(&mut buffer, "payload", "abc", false).unwrap(),
        Some("c".to_string())
    );
    // No new input and no close: nothing to emit.
    assert_eq!(
        append_grammar_tool_input_json_delta(&mut buffer, "payload", "abc", false).unwrap(),
        None
    );
    assert_eq!(
        append_grammar_tool_input_json_delta(&mut buffer, "payload", "abc", true).unwrap(),
        Some("\"}".to_string())
    );
    // Closing again with the same input is a no-op.
    assert_eq!(
        append_grammar_tool_input_json_delta(&mut buffer, "payload", "abc", true).unwrap(),
        None
    );
    // Any change after closing is an error.
    assert!(append_grammar_tool_input_json_delta(&mut buffer, "payload", "abcd", false).is_err());

    // Non-monotonic input is rejected.
    let mut buffer = GrammarToolInputJsonBuffer::default();
    append_grammar_tool_input_json_delta(&mut buffer, "payload", "abc", false).unwrap();
    assert!(append_grammar_tool_input_json_delta(&mut buffer, "payload", "xyz", false).is_err());
}

#[test]
fn grammar_input_json_escapes_control_characters() {
    let mut buffer = GrammarToolInputJsonBuffer::default();
    let delta = append_grammar_tool_input_json_delta(&mut buffer, "payload", "a\"b\nc", true)
        .unwrap()
        .unwrap();
    assert_eq!(delta, "{\"payload\":\"a\\\"b\\nc\"}");
    // The result is valid JSON.
    let parsed: Value = serde_json::from_str(&delta).expect("valid JSON");
    assert_eq!(parsed["payload"], json!("a\"b\nc"));
}

#[test]
fn grammar_tool_input_requires_a_string_argument() {
    let arguments = json!({"payload": "abc"}).as_object().unwrap().clone();
    assert_eq!(
        get_grammar_tool_input("sample_tool", &arguments, "payload").unwrap(),
        "abc"
    );

    let wrong = json!({"payload": 5}).as_object().unwrap().clone();
    let error = get_grammar_tool_input("sample_tool", &wrong, "payload").expect_err("must fail");
    assert!(
        error
            .to_string()
            .contains("requires argument \"payload\" to be a string"),
        "{error}"
    );
}
