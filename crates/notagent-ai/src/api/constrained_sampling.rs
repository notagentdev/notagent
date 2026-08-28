use serde_json::{Map, Value, json};

use crate::types::{
    ConstrainedSampling, ConstrainedSamplingConfig, GrammarFormat, StrictMode, Tool,
};

/// Schema keywords the strict subset does not support.
const UNSUPPORTED_STRICT_SCHEMA_KEYS: [&str; 16] = [
    "$ref",
    "$defs",
    "definitions",
    "allOf",
    "oneOf",
    "patternProperties",
    "dependentSchemas",
    "dependencies",
    "unevaluatedProperties",
    "propertyNames",
    "contains",
    "prefixItems",
    "not",
    "if",
    "then",
    "else",
];

/// `UnsupportedStrictJsonSchemaError`
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UnsupportedStrictJsonSchema(pub String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConstrainedSamplingError(pub String);

fn is_structured_schema(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    let types: Vec<&str> = match object.get("type") {
        Some(Value::String(single)) => vec![single.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    types.contains(&"object")
        || types.contains(&"array")
        || object.contains_key("properties")
        || object.contains_key("items")
}

fn schema_allows_null(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    match object.get("type") {
        Some(Value::String(single)) if single == "null" => return true,
        Some(Value::Array(many)) if many.iter().any(|entry| entry.as_str() == Some("null")) => {
            return true;
        }
        _ => {}
    }
    if object.get("const") == Some(&Value::Null) {
        return true;
    }
    if let Some(Value::Array(values)) = object.get("enum")
        && values.contains(&Value::Null)
    {
        return true;
    }
    matches!(object.get("anyOf"), Some(Value::Array(variants)) if variants.iter().any(schema_allows_null))
}

/// `makeJsonSchemaNodeStrict(schema)` — mutates the node in place.
fn make_json_schema_node_strict(schema: &mut Value) -> Result<(), UnsupportedStrictJsonSchema> {
    let Some(object) = schema.as_object_mut() else {
        return Err(UnsupportedStrictJsonSchema(
            "boolean schemas are unsupported".to_string(),
        ));
    };
    for key in UNSUPPORTED_STRICT_SCHEMA_KEYS {
        if object.contains_key(key) {
            return Err(UnsupportedStrictJsonSchema(format!(
                "{key} schemas are unsupported"
            )));
        }
    }

    if let Some(any_of) = object.get_mut("anyOf") {
        let Some(variants) = any_of
            .as_array_mut()
            .filter(|variants| !variants.is_empty())
        else {
            return Err(UnsupportedStrictJsonSchema(
                "anyOf must contain at least one schema".to_string(),
            ));
        };
        for variant in variants.iter_mut() {
            if is_structured_schema(variant) {
                return Err(UnsupportedStrictJsonSchema(
                    "object and array unions are unsupported".to_string(),
                ));
            }
            make_json_schema_node_strict(variant)?;
        }
    }

    if let Some(items) = object.get_mut("items") {
        if items.is_array() {
            return Err(UnsupportedStrictJsonSchema(
                "tuple schemas are unsupported".to_string(),
            ));
        }
        make_json_schema_node_strict(items)?;
    }

    let is_object_schema = object.get("type") == Some(&json!("object"));
    if object.contains_key("properties") && !is_object_schema {
        return Err(UnsupportedStrictJsonSchema(
            "properties require type object".to_string(),
        ));
    }
    if !is_object_schema {
        return Ok(());
    }
    match object.get("additionalProperties") {
        None | Some(Value::Bool(false)) => {}
        Some(_) => {
            return Err(UnsupportedStrictJsonSchema(
                "schema-valued or true additionalProperties is unsupported".to_string(),
            ));
        }
    }
    if object
        .get("properties")
        .is_some_and(|properties| !properties.is_object())
    {
        return Err(UnsupportedStrictJsonSchema(
            "object properties must be a schema map".to_string(),
        ));
    }
    let required: Vec<String> = match object.get("required") {
        None => Vec::new(),
        Some(Value::Array(entries)) if entries.iter().all(Value::is_string) => entries
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(_) => {
            return Err(UnsupportedStrictJsonSchema(
                "object required must be a string array".to_string(),
            ));
        }
    };

    let property_names: Vec<String> = object
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default();
    if required.iter().any(|key| !property_names.contains(key)) {
        return Err(UnsupportedStrictJsonSchema(
            "required contains an unknown property".to_string(),
        ));
    }

    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        for (key, property) in properties.iter_mut() {
            make_json_schema_node_strict(property)?;
            // Optional properties become nullable unions so `required` can list them all.
            if !required.contains(key) && !schema_allows_null(property) {
                *property = json!({"anyOf": [property.clone(), {"type": "null"}]});
            }
        }
    }
    object.insert(
        "required".to_string(),
        Value::Array(property_names.into_iter().map(Value::String).collect()),
    );
    object.insert("additionalProperties".to_string(), Value::Bool(false));
    Ok(())
}

/// `makeStrictJsonSchema(schema)`
pub fn make_strict_json_schema(schema: &Value) -> Result<Value, UnsupportedStrictJsonSchema> {
    let mut cloned = schema.clone();
    if !cloned.is_object() {
        return Err(UnsupportedStrictJsonSchema(
            "root schema must have type object".to_string(),
        ));
    }
    make_json_schema_node_strict(&mut cloned)?;
    if cloned.get("type") != Some(&json!("object")) {
        return Err(UnsupportedStrictJsonSchema(
            "root schema must have type object".to_string(),
        ));
    }
    Ok(cloned)
}

/// `getJsonSchemaToolParameters(tool, strict)`
pub fn get_json_schema_tool_parameters(tool: &Tool, strict: Option<bool>) -> Value {
    match strict {
        Some(true) => {
            make_strict_json_schema(&tool.parameters).unwrap_or_else(|_| tool.parameters.clone())
        }
        _ => tool.parameters.clone(),
    }
}

/// `resolveJsonSchemaStrictSampling(tool, supportsStrictMode)`
pub fn resolve_json_schema_strict_sampling(
    tool: &Tool,
    supports_strict_mode: bool,
) -> Result<Option<bool>, ConstrainedSamplingError> {
    let Some(ConstrainedSampling::Config(ConstrainedSamplingConfig::JsonSchema { strict })) =
        &tool.constrained_sampling
    else {
        return Ok(None);
    };

    if supports_strict_mode {
        return match make_strict_json_schema(&tool.parameters) {
            Ok(_) => Ok(Some(true)),
            Err(error) => {
                if *strict != StrictMode::Require {
                    return Ok(None);
                }
                Err(ConstrainedSamplingError(format!(
                    "Tool \"{}\" requires JSON-schema constrained sampling, but {error}.",
                    tool.name
                )))
            }
        };
    }
    if *strict == StrictMode::Require {
        return Err(ConstrainedSamplingError(format!(
            "Tool \"{}\" requires JSON-schema constrained sampling, but strict tools are unsupported.",
            tool.name
        )));
    }
    Ok(None)
}

/// `GrammarConstrainedSampling`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrammarConstrainedSampling {
    pub format: GrammarSyntax,
    pub definition: String,
    pub input_property: String,
}

/// `format: "lark" | "regex"`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrammarSyntax {
    Lark,
    Regex,
}

/// `inferGrammarInputProperty(tool)`
fn infer_grammar_input_property(tool: &Tool) -> Result<String, String> {
    let Some(schema) = tool.parameters.as_object() else {
        return Err("grammar constrained sampling requires an object parameter schema".to_string());
    };
    if schema.get("type") != Some(&json!("object")) {
        return Err("grammar constrained sampling requires an object parameter schema".to_string());
    }
    let required = schema.get("required").and_then(Value::as_array);
    let input_property = match required {
        Some(entries) if entries.len() == 1 => {
            match entries[0].as_str() {
                Some(name) => name.to_string(),
                None => {
                    return Err("grammar constrained sampling requires exactly one required string property".to_string());
                }
            }
        }
        _ => {
            return Err(
                "grammar constrained sampling requires exactly one required string property"
                    .to_string(),
            );
        }
    };
    let property = schema
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get(&input_property));
    let Some(property) = property else {
        return Err(format!(
            "grammar constrained sampling requires a properties entry for {input_property}"
        ));
    };
    if property.get("type") != Some(&json!("string")) {
        return Err(format!(
            "grammar constrained sampling property {input_property} must have type string"
        ));
    }
    Ok(input_property)
}

/// `resolveGrammarConstrainedSampling(tool, supportsOpenAIGrammarTools)`
pub fn resolve_grammar_constrained_sampling(
    tool: &Tool,
    supports_openai_grammar_tools: bool,
) -> Result<Option<GrammarConstrainedSampling>, ConstrainedSamplingError> {
    let Some(ConstrainedSampling::Config(ConstrainedSamplingConfig::Grammar { variants })) =
        &tool.constrained_sampling
    else {
        return Ok(None);
    };
    if !supports_openai_grammar_tools {
        return Ok(None);
    }

    let lark = variants
        .get(&GrammarFormat::OpenaiLark)
        .filter(|definition| !definition.trim().is_empty());
    let regex = variants
        .get(&GrammarFormat::OpenaiRegex)
        .filter(|definition| !definition.trim().is_empty());
    let (format, definition) = match (lark, regex) {
        (Some(lark), _) => (GrammarSyntax::Lark, lark.clone()),
        (None, Some(regex)) => (GrammarSyntax::Regex, regex.clone()),
        (None, None) => {
            return Err(ConstrainedSamplingError(format!(
                "Tool \"{}\" cannot use grammar constrained sampling: no supported grammar variant was provided.",
                tool.name
            )));
        }
    };

    match infer_grammar_input_property(tool) {
        Ok(input_property) => Ok(Some(GrammarConstrainedSampling {
            format,
            definition,
            input_property,
        })),
        Err(message) => Err(ConstrainedSamplingError(format!(
            "Tool \"{}\" cannot use grammar constrained sampling: {message}.",
            tool.name
        ))),
    }
}

/// `createGrammarToolInputProperties(tools, supportsOpenAIGrammarTools)`
pub fn create_grammar_tool_input_properties(
    tools: Option<&[Tool]>,
    supports_openai_grammar_tools: bool,
) -> Result<std::collections::BTreeMap<String, String>, ConstrainedSamplingError> {
    let mut properties = std::collections::BTreeMap::new();
    for tool in tools.unwrap_or_default() {
        if let Some(grammar) =
            resolve_grammar_constrained_sampling(tool, supports_openai_grammar_tools)?
        {
            properties.insert(tool.name.clone(), grammar.input_property);
        }
    }
    Ok(properties)
}

/// `getGrammarToolInput(toolName, arguments, inputProperty)`
pub fn get_grammar_tool_input(
    tool_name: &str,
    arguments: &Map<String, Value>,
    input_property: &str,
) -> Result<String, ConstrainedSamplingError> {
    match arguments.get(input_property).and_then(Value::as_str) {
        Some(input) => Ok(input.to_string()),
        None => Err(ConstrainedSamplingError(format!(
            "Grammar tool call \"{tool_name}\" requires argument \"{input_property}\" to be a string."
        ))),
    }
}

/// `GrammarToolInputJsonBuffer`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrammarToolInputJsonBuffer {
    pub input: String,
    pub started: bool,
    pub closed: bool,
}

/// `appendGrammarToolInputJsonDelta(buffer, inputProperty, nextInput, close)`
pub fn append_grammar_tool_input_json_delta(
    buffer: &mut GrammarToolInputJsonBuffer,
    input_property: &str,
    next_input: &str,
    close: bool,
) -> Result<Option<String>, ConstrainedSamplingError> {
    if buffer.closed {
        if close && next_input == buffer.input {
            return Ok(None);
        }
        return Err(ConstrainedSamplingError(format!(
            "grammar tool input for property \"{input_property}\" changed after it was closed"
        )));
    }
    if !next_input.starts_with(&buffer.input) {
        return Err(ConstrainedSamplingError(format!(
            "grammar tool input for property \"{input_property}\" changed non-monotonically"
        )));
    }

    let input_delta = &next_input[buffer.input.len()..];
    if !close && input_delta.is_empty() {
        return Ok(None);
    }

    let mut delta = String::new();
    if !buffer.started {
        delta.push('{');
        delta.push_str(&Value::String(input_property.to_string()).to_string());
        delta.push_str(":\"");
        buffer.started = true;
    }
    // `JSON.stringify(delta).slice(1, -1)` — the escaped body without the quotes.
    let escaped = Value::String(input_delta.to_string()).to_string();
    delta.push_str(&escaped[1..escaped.len() - 1]);
    buffer.input = next_input.to_string();

    if close {
        delta.push_str("\"}");
        buffer.closed = true;
    }
    Ok(Some(delta))
}
