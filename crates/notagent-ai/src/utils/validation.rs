//! Tool-argument validation and coercion.
//!
//! 1:1 port of `packages/ai/src/utils/validation.ts` (350 LOC). The TypeBox pieces the
//! TS module delegates to are ported with it (master-plan substitution: TypeBox schemas
//! become static `serde_json` JSON Schema values):
//!
//! * `Compile(schema).Check/.Errors` -> [`check`] / [`errors`], including the AJV-style
//!   messages TypeBox emits (`must be number`, `must have required properties a, b`, ...),
//!   which reach the model inside tool-error results.
//! * `Value.Convert(schema, value)` -> [`convert`], a port of
//!   `typebox/build/value/convert` including its `Try*` primitives.
//!
//! TypeBox marks its schemas with a runtime symbol that has no JSON representation.
//! `validateToolArguments` uses it to run the extra plain-schema coercion
//! (`coerceWithJsonSchema`) only for schemas that did *not* come from TypeBox. Rust
//! cannot see that marker, so the origin is passed explicitly; [`SchemaOrigin::TypeBox`]
//! is the default because every tool of the app is defined that way in TS.

use serde_json::{Map, Value};

use crate::types::{Tool, ToolCall};

/// A single validation error (`TLocalizedValidationError` in TS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub keyword: String,
    pub instance_path: String,
    pub message: String,
    /// `params.requiredProperties` for the `required` keyword.
    pub required_properties: Vec<String>,
}

/// Where a schema came from — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SchemaOrigin {
    /// Built through TypeBox in TS: only `Value.Convert` runs before checking.
    #[default]
    TypeBox,
    /// A serialized plain JSON schema: `coerceWithJsonSchema` runs as well.
    PlainJsonSchema,
}

/// Error of [`validate_tool_arguments`] (TS throws an `Error`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ToolValidationError(pub String);

// ---------------------------------------------------------------------------
// Schema validation (TypeBox `Compile(...).Check` / `.Errors`)
// ---------------------------------------------------------------------------

fn resolve_ref<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
    let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
        return schema;
    };
    let Some(pointer) = reference.strip_prefix("#/") else {
        return schema;
    };
    let mut current = root;
    for segment in pointer.split('/') {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        match current.get(&segment) {
            Some(next) => current = next,
            None => return schema,
        }
    }
    current
}

fn matches_json_type(value: &Value, type_name: &str) -> bool {
    match type_name {
        "number" => value.is_number(),
        "integer" => value.as_f64().is_some_and(|number| number.fract() == 0.0),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

fn schema_types(schema: &Value) -> Vec<String> {
    match schema.get("type") {
        Some(Value::String(single)) => vec![single.clone()],
        Some(Value::Array(many)) => many
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// `Compile(schema).Check(value)`
pub fn check(schema: &Value, value: &Value) -> bool {
    check_against(schema, value, schema)
}

fn check_against(schema: &Value, value: &Value, root: &Value) -> bool {
    let mut collected = Vec::new();
    collect_errors(schema, value, "", root, &mut collected);
    collected.is_empty()
}

/// `Compile(schema).Errors(value)` in TypeBox's emission order.
pub fn errors(schema: &Value, value: &Value) -> Vec<ValidationError> {
    let mut collected = Vec::new();
    collect_errors(schema, value, "", schema, &mut collected);
    collected
}

fn push_error(out: &mut Vec<ValidationError>, keyword: &str, path: &str, message: String) {
    out.push(ValidationError {
        keyword: keyword.to_string(),
        instance_path: path.to_string(),
        message,
        required_properties: Vec::new(),
    });
}

fn collect_errors(
    schema: &Value,
    value: &Value,
    path: &str,
    root: &Value,
    out: &mut Vec<ValidationError>,
) {
    let schema = resolve_ref(schema, root);
    let Some(object) = schema.as_object() else {
        // `true`/`false` schemas: `false` rejects everything.
        if schema == &Value::Bool(false) {
            push_error(out, "type", path, "must be never".to_string());
        }
        return;
    };

    if let Some(Value::Array(all_of)) = object.get("allOf") {
        for sub_schema in all_of {
            collect_errors(sub_schema, value, path, root, out);
        }
    }

    if let Some(Value::Array(any_of)) = object.get("anyOf") {
        let matched = any_of
            .iter()
            .any(|sub_schema| check_against(sub_schema, value, root));
        if !matched {
            for sub_schema in any_of {
                collect_errors(sub_schema, value, path, root, out);
            }
            push_error(
                out,
                "anyOf",
                path,
                "must match a schema in anyOf".to_string(),
            );
        }
    }

    if let Some(Value::Array(one_of)) = object.get("oneOf") {
        let matches = one_of
            .iter()
            .filter(|sub_schema| check_against(sub_schema, value, root))
            .count();
        if matches != 1 {
            for sub_schema in one_of {
                collect_errors(sub_schema, value, path, root, out);
            }
            push_error(
                out,
                "oneOf",
                path,
                "must match exactly one schema in oneOf".to_string(),
            );
        }
    }

    if let Some(constant) = object.get("const")
        && constant != value
    {
        push_error(out, "const", path, "must be equal to constant".to_string());
        return;
    }

    if let Some(Value::Array(allowed)) = object.get("enum")
        && !allowed.contains(value)
    {
        push_error(
            out,
            "enum",
            path,
            "must be equal to one of the allowed values".to_string(),
        );
        return;
    }

    let types = schema_types(schema);
    if !types.is_empty()
        && !types
            .iter()
            .any(|type_name| matches_json_type(value, type_name))
    {
        push_error(out, "type", path, format!("must be {}", types.join(",")));
        return;
    }

    match value {
        Value::Number(number) => {
            let number = number.as_f64().unwrap_or_default();
            if let Some(minimum) = object.get("minimum").and_then(Value::as_f64)
                && number < minimum
            {
                push_error(
                    out,
                    "minimum",
                    path,
                    format!(
                        "must be >= {}",
                        crate::utils::js_number::to_js_string(minimum)
                    ),
                );
            }
            if let Some(maximum) = object.get("maximum").and_then(Value::as_f64)
                && number > maximum
            {
                push_error(
                    out,
                    "maximum",
                    path,
                    format!(
                        "must be <= {}",
                        crate::utils::js_number::to_js_string(maximum)
                    ),
                );
            }
            if let Some(limit) = object.get("exclusiveMinimum").and_then(Value::as_f64)
                && number <= limit
            {
                push_error(
                    out,
                    "exclusiveMinimum",
                    path,
                    format!("must be > {}", crate::utils::js_number::to_js_string(limit)),
                );
            }
            if let Some(limit) = object.get("exclusiveMaximum").and_then(Value::as_f64)
                && number >= limit
            {
                push_error(
                    out,
                    "exclusiveMaximum",
                    path,
                    format!("must be < {}", crate::utils::js_number::to_js_string(limit)),
                );
            }
            if let Some(multiple) = object.get("multipleOf").and_then(Value::as_f64)
                && multiple != 0.0
                && (number / multiple).fract() != 0.0
            {
                push_error(
                    out,
                    "multipleOf",
                    path,
                    format!(
                        "must be multiple of {}",
                        crate::utils::js_number::to_js_string(multiple)
                    ),
                );
            }
        }
        Value::String(text) => {
            let length = text.encode_utf16().count() as u64;
            if let Some(min_length) = object.get("minLength").and_then(Value::as_u64)
                && length < min_length
            {
                push_error(
                    out,
                    "minLength",
                    path,
                    format!("must not have fewer than {min_length} characters"),
                );
            }
            if let Some(max_length) = object.get("maxLength").and_then(Value::as_u64)
                && length > max_length
            {
                push_error(
                    out,
                    "maxLength",
                    path,
                    format!("must not have more than {max_length} characters"),
                );
            }
            if let Some(pattern) = object.get("pattern").and_then(Value::as_str)
                && let Ok(regex) = regex::Regex::new(pattern)
                && !regex.is_match(text)
            {
                push_error(
                    out,
                    "pattern",
                    path,
                    format!("must match pattern \"{pattern}\""),
                );
            }
        }
        Value::Array(items) => {
            if let Some(min_items) = object.get("minItems").and_then(Value::as_u64)
                && (items.len() as u64) < min_items
            {
                push_error(
                    out,
                    "minItems",
                    path,
                    format!("must not have fewer than {min_items} items"),
                );
            }
            if let Some(max_items) = object.get("maxItems").and_then(Value::as_u64)
                && (items.len() as u64) > max_items
            {
                push_error(
                    out,
                    "maxItems",
                    path,
                    format!("must not have more than {max_items} items"),
                );
            }
            match object.get("items") {
                Some(Value::Array(tuple)) => {
                    for (index, item) in items.iter().enumerate() {
                        if let Some(item_schema) = tuple.get(index) {
                            collect_errors(
                                item_schema,
                                item,
                                &format!("{path}/{index}"),
                                root,
                                out,
                            );
                        }
                    }
                }
                Some(item_schema) => {
                    for (index, item) in items.iter().enumerate() {
                        collect_errors(item_schema, item, &format!("{path}/{index}"), root, out);
                    }
                }
                None => {}
            }
        }
        Value::Object(entries) => {
            let properties = object.get("properties").and_then(Value::as_object);
            let required: Vec<String> = object
                .get("required")
                .and_then(Value::as_array)
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();

            let missing: Vec<String> = required
                .iter()
                .filter(|name| !entries.contains_key(*name))
                .cloned()
                .collect();
            if !missing.is_empty() {
                out.push(ValidationError {
                    keyword: "required".to_string(),
                    instance_path: path.to_string(),
                    message: format!("must have required properties {}", missing.join(", ")),
                    required_properties: missing,
                });
            }

            if let Some(properties) = properties {
                for (name, property_schema) in properties {
                    if let Some(property_value) = entries.get(name) {
                        collect_errors(
                            property_schema,
                            property_value,
                            &format!("{path}/{name}"),
                            root,
                            out,
                        );
                    }
                }
            }

            match object.get("additionalProperties") {
                Some(Value::Bool(false)) => {
                    let known: Vec<&String> = properties
                        .map(|properties| properties.keys().collect())
                        .unwrap_or_default();
                    if entries.keys().any(|name| !known.contains(&name)) {
                        push_error(
                            out,
                            "additionalProperties",
                            path,
                            "must not have additional properties".to_string(),
                        );
                    }
                }
                Some(additional_schema) if additional_schema.is_object() => {
                    let known: Vec<&String> = properties
                        .map(|properties| properties.keys().collect())
                        .unwrap_or_default();
                    for (name, property_value) in entries {
                        if !known.contains(&name) {
                            collect_errors(
                                additional_schema,
                                property_value,
                                &format!("{path}/{name}"),
                                root,
                                out,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// TypeBox `Value.Convert`
// ---------------------------------------------------------------------------

/// `Try.TryNumber(value)`
fn try_number(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        Value::Number(number) => number.as_f64(),
        Value::Null => Some(0.0),
        Value::String(text) => {
            // `+value` in JS: the empty string and whitespace convert to 0.
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Some(0.0);
            }
            if let Ok(number) = trimmed.parse::<f64>()
                && number.is_finite()
            {
                return Some(number);
            }
            match text.to_lowercase().as_str() {
                "false" => Some(0.0),
                "true" => Some(1.0),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `Try.TryBoolean(value)`
fn try_boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Number(number) => match number.as_f64() {
            Some(0.0) => Some(false),
            Some(1.0) => Some(true),
            _ => None,
        },
        Value::Bool(flag) => Some(*flag),
        Value::String(text) => match text.to_lowercase().as_str() {
            "false" => Some(false),
            "true" => Some(true),
            _ => match text.as_str() {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            },
        },
        _ => None,
    }
}

/// `Try.TryString(value)`
fn try_string(value: &Value) -> Option<String> {
    match value {
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => number.as_f64().map(crate::utils::js_number::to_js_string),
        Value::Null => Some("null".to_string()),
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

/// `Try.TryNull(value)`
fn try_null(value: &Value) -> Option<()> {
    match value {
        Value::Bool(false) => Some(()),
        Value::Number(number) if number.as_f64() == Some(0.0) => Some(()),
        Value::Null => Some(()),
        Value::String(text) => {
            let lowercase = text.to_lowercase();
            if lowercase == "undefined" || lowercase == "null" || text.is_empty() || text == "0" {
                Some(())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `Value.Convert(schema, value)`
pub fn convert(schema: &Value, value: &Value) -> Value {
    convert_against(schema, value, schema)
}

fn convert_against(schema: &Value, value: &Value, root: &Value) -> Value {
    let schema = resolve_ref(schema, root);
    let Some(object) = schema.as_object() else {
        return value.clone();
    };

    if let Some(Value::Array(any_of)) = object.get("anyOf") {
        // FromUnion: keep a matching value, otherwise take the first converted arm that checks.
        if any_of.iter().any(|arm| check_against(arm, value, root)) {
            return value.clone();
        }
        for arm in any_of {
            let candidate = convert_against(arm, value, root);
            if check_against(schema, &candidate, root) {
                return candidate;
            }
        }
        return value.clone();
    }

    let types = schema_types(schema);
    // A union via `type: [..]` is left alone when it already matches one member.
    if types.len() > 1
        && types
            .iter()
            .any(|type_name| matches_json_type(value, type_name))
    {
        return value.clone();
    }

    if types.iter().any(|type_name| type_name == "object") && value.is_object() {
        let mut converted = value.as_object().cloned().unwrap_or_default();
        if let Some(properties) = object.get("properties").and_then(Value::as_object) {
            for (name, property_schema) in properties {
                if let Some(property_value) = converted.get(name).cloned() {
                    converted.insert(
                        name.clone(),
                        convert_against(property_schema, &property_value, root),
                    );
                }
            }
        }
        if let Some(additional_schema) = object
            .get("additionalProperties")
            .filter(|schema| schema.is_object())
        {
            let known: Vec<String> = object
                .get("properties")
                .and_then(Value::as_object)
                .map(|properties| properties.keys().cloned().collect())
                .unwrap_or_default();
            let names: Vec<String> = converted.keys().cloned().collect();
            for name in names {
                if !known.contains(&name) {
                    let property_value = converted[&name].clone();
                    converted.insert(
                        name,
                        convert_against(additional_schema, &property_value, root),
                    );
                }
            }
        }
        return Value::Object(converted);
    }

    if types.iter().any(|type_name| type_name == "array") {
        // `TryArray` wraps a non-array value into a single-element array.
        let items: Vec<Value> = match value {
            Value::Array(items) => items.clone(),
            other => vec![other.clone()],
        };
        let item_schema = object.get("items");
        let converted = match item_schema {
            Some(Value::Array(tuple)) => items
                .iter()
                .enumerate()
                .map(|(index, item)| match tuple.get(index) {
                    Some(schema) => convert_against(schema, item, root),
                    None => item.clone(),
                })
                .collect(),
            Some(schema) => items
                .iter()
                .map(|item| convert_against(schema, item, root))
                .collect(),
            None => items,
        };
        return Value::Array(converted);
    }

    for type_name in &types {
        match type_name.as_str() {
            "number" => {
                if let Some(number) = try_number(value) {
                    return number_value(number);
                }
            }
            "integer" => {
                if let Some(number) = try_number(value) {
                    return number_value(number.trunc());
                }
            }
            "boolean" => {
                if let Some(flag) = try_boolean(value) {
                    return Value::Bool(flag);
                }
            }
            "string" => {
                if let Some(text) = try_string(value) {
                    return Value::String(text);
                }
            }
            "null" if try_null(value).is_some() => return Value::Null,
            _ => {}
        }
    }

    value.clone()
}

fn number_value(number: f64) -> Value {
    serde_json::Number::from_f64(number)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// Plain-JSON-schema coercion (`coerceWithJsonSchema`)
// ---------------------------------------------------------------------------

/// `coercePrimitiveByType(value, type)`
fn coerce_primitive_by_type(value: &Value, type_name: &str) -> Value {
    match type_name {
        "number" => {
            if value.is_null() {
                return number_value(0.0);
            }
            if let Some(text) = value.as_str()
                && !text.trim().is_empty()
                && let Ok(parsed) = text.trim().parse::<f64>()
                && parsed.is_finite()
            {
                return number_value(parsed);
            }
            if let Some(flag) = value.as_bool() {
                return number_value(if flag { 1.0 } else { 0.0 });
            }
            value.clone()
        }
        "integer" => {
            if value.is_null() {
                return number_value(0.0);
            }
            if let Some(text) = value.as_str()
                && !text.trim().is_empty()
                && let Ok(parsed) = text.trim().parse::<f64>()
                && parsed.fract() == 0.0
            {
                return number_value(parsed);
            }
            if let Some(flag) = value.as_bool() {
                return number_value(if flag { 1.0 } else { 0.0 });
            }
            value.clone()
        }
        "boolean" => {
            if value.is_null() {
                return Value::Bool(false);
            }
            if let Some(text) = value.as_str() {
                if text == "true" {
                    return Value::Bool(true);
                }
                if text == "false" {
                    return Value::Bool(false);
                }
            }
            if let Some(number) = value.as_f64() {
                if number == 1.0 {
                    return Value::Bool(true);
                }
                if number == 0.0 {
                    return Value::Bool(false);
                }
            }
            value.clone()
        }
        "string" => {
            if value.is_null() {
                return Value::String(String::new());
            }
            match value {
                Value::Number(number) => Value::String(
                    number
                        .as_f64()
                        .map(crate::utils::js_number::to_js_string)
                        .unwrap_or_default(),
                ),
                Value::Bool(flag) => Value::String(flag.to_string()),
                other => other.clone(),
            }
        }
        "null" => {
            let is_empty_string = value.as_str() == Some("");
            let is_zero = value.as_f64() == Some(0.0);
            let is_false = value.as_bool() == Some(false);
            if is_empty_string || is_zero || is_false {
                Value::Null
            } else {
                value.clone()
            }
        }
        _ => value.clone(),
    }
}

/// `coerceWithJsonSchema(value, schema)`
pub fn coerce_with_json_schema(value: &Value, schema: &Value, root: &Value) -> Value {
    let mut next_value = value.clone();

    if let Some(Value::Array(all_of)) = schema.get("allOf") {
        for nested in all_of {
            next_value = coerce_with_json_schema(&next_value, nested, root);
        }
    }
    if let Some(Value::Array(any_of)) = schema.get("anyOf") {
        next_value = coerce_with_union_schema(&next_value, any_of, root);
    }
    if let Some(Value::Array(one_of)) = schema.get("oneOf") {
        next_value = coerce_with_union_schema(&next_value, one_of, root);
    }

    let types = schema_types(schema);
    let matches_union_member = types.len() > 1
        && types
            .iter()
            .any(|type_name| matches_json_type(&next_value, type_name));
    if !types.is_empty() && !matches_union_member {
        for type_name in &types {
            let candidate = coerce_primitive_by_type(&next_value, type_name);
            if candidate != next_value {
                next_value = candidate;
                break;
            }
        }
    }

    if types.iter().any(|type_name| type_name == "object")
        && let Value::Object(entries) = &mut next_value
    {
        apply_schema_object_coercion(entries, schema, root);
    }

    if types.iter().any(|type_name| type_name == "array")
        && let Value::Array(items) = &mut next_value
    {
        apply_schema_array_coercion(items, schema, root);
    }

    next_value
}

/// `applySchemaObjectCoercion(value, schema)`
fn apply_schema_object_coercion(entries: &mut Map<String, Value>, schema: &Value, root: &Value) {
    let properties = schema.get("properties").and_then(Value::as_object).cloned();
    let defined_keys: Vec<String> = properties
        .as_ref()
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default();

    if let Some(properties) = &properties {
        for (key, property_schema) in properties {
            if let Some(current) = entries.get(key).cloned() {
                entries.insert(
                    key.clone(),
                    coerce_with_json_schema(&current, property_schema, root),
                );
            }
        }
    }

    if let Some(additional_schema) = schema
        .get("additionalProperties")
        .filter(|schema| schema.is_object())
    {
        let names: Vec<String> = entries.keys().cloned().collect();
        for name in names {
            if defined_keys.contains(&name) {
                continue;
            }
            let current = entries[&name].clone();
            entries.insert(
                name,
                coerce_with_json_schema(&current, additional_schema, root),
            );
        }
    }
}

/// `applySchemaArrayCoercion(value, schema)`
fn apply_schema_array_coercion(items: &mut [Value], schema: &Value, root: &Value) {
    match schema.get("items") {
        Some(Value::Array(tuple)) => {
            for (index, item) in items.iter_mut().enumerate() {
                if let Some(item_schema) = tuple.get(index) {
                    *item = coerce_with_json_schema(item, item_schema, root);
                }
            }
        }
        Some(item_schema) if item_schema.is_object() => {
            for item in items.iter_mut() {
                *item = coerce_with_json_schema(item, item_schema, root);
            }
        }
        _ => {}
    }
}

/// `coerceWithUnionSchema(value, schemas)`
fn coerce_with_union_schema(value: &Value, schemas: &[Value], root: &Value) -> Value {
    for schema in schemas {
        if check_against(schema, value, root) {
            return value.clone();
        }
    }
    for schema in schemas {
        let coerced = coerce_with_json_schema(value, schema, root);
        if check_against(schema, &coerced, root) {
            return coerced;
        }
    }
    value.clone()
}

// ---------------------------------------------------------------------------
// normalizeOptionalNulls
// ---------------------------------------------------------------------------

/// `normalizeOptionalNulls(value, schema)` — drops `null` for optional, non-nullable
/// properties so providers that send explicit nulls behave like an omission.
pub fn normalize_optional_nulls(value: &mut Value, schema: &Value, root: &Value) {
    let schema = resolve_ref(schema, root);

    if let Value::Array(items) = value {
        match schema.get("items") {
            Some(Value::Array(tuple)) => {
                for (index, item) in items.iter_mut().enumerate() {
                    if let Some(item_schema) = tuple.get(index) {
                        normalize_optional_nulls(item, item_schema, root);
                    }
                }
            }
            Some(item_schema) => {
                for item in items.iter_mut() {
                    normalize_optional_nulls(item, item_schema, root);
                }
            }
            None => {}
        }
        return;
    }

    let Value::Object(entries) = value else {
        return;
    };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    for (key, property_schema) in properties {
        if !entries.contains_key(key) {
            continue;
        }
        let is_null = entries[key].is_null();
        // TS skips `$ref` properties here: their target may well be nullable.
        let is_reference = property_schema
            .get("$ref")
            .and_then(Value::as_str)
            .is_some();
        if is_null
            && !required.contains(&key.as_str())
            && !is_reference
            && !check_against(property_schema, &Value::Null, root)
        {
            entries.remove(key);
        } else if let Some(nested) = entries.get_mut(key) {
            normalize_optional_nulls(nested, property_schema, root);
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// `formatValidationPath(error)`
fn format_validation_path(error: &ValidationError) -> String {
    if error.keyword == "required"
        && let Some(required_property) = error.required_properties.first()
    {
        let base_path = error
            .instance_path
            .trim_start_matches('/')
            .replace('/', ".");
        return if base_path.is_empty() {
            required_property.clone()
        } else {
            format!("{base_path}.{required_property}")
        };
    }
    let path = error
        .instance_path
        .trim_start_matches('/')
        .replace('/', ".");
    if path.is_empty() {
        "root".to_string()
    } else {
        path
    }
}

/// `validateToolCall(tools, toolCall)` — finds the tool by name and validates.
pub fn validate_tool_call(
    tools: &[Tool],
    tool_call: &ToolCall,
) -> Result<Value, ToolValidationError> {
    let Some(tool) = tools.iter().find(|tool| tool.name == tool_call.name) else {
        return Err(ToolValidationError(format!(
            "Tool \"{}\" not found",
            tool_call.name
        )));
    };
    validate_tool_arguments(tool, tool_call)
}

/// `validateToolArguments(tool, toolCall)` for TypeBox-defined tools.
pub fn validate_tool_arguments(
    tool: &Tool,
    tool_call: &ToolCall,
) -> Result<Value, ToolValidationError> {
    validate_tool_arguments_with(tool, tool_call, SchemaOrigin::TypeBox)
}

/// `validateToolArguments(tool, toolCall)` with an explicit schema origin.
pub fn validate_tool_arguments_with(
    tool: &Tool,
    tool_call: &ToolCall,
    origin: SchemaOrigin,
) -> Result<Value, ToolValidationError> {
    let schema = &tool.parameters;
    let mut args = Value::Object(tool_call.arguments.clone());

    normalize_optional_nulls(&mut args, schema, schema);

    // `Value.Convert` dispatches on TypeBox's runtime type guards, so it is a no-op for
    // serialized plain schemas; those go through `coerceWithJsonSchema` instead. The two
    // paths are mutually exclusive in TS as well (`validation.ts:307-325`).
    match origin {
        SchemaOrigin::TypeBox => args = convert(schema, &args),
        SchemaOrigin::PlainJsonSchema => {
            let coerced = coerce_with_json_schema(&args, schema, schema);
            if coerced != args {
                match (&args, &coerced) {
                    // TS mutates the object in place, so the identity check never fires.
                    (Value::Object(_), Value::Object(_)) => args = coerced,
                    _ => {
                        return Ok(if check(schema, &coerced) {
                            coerced
                        } else {
                            args
                        });
                    }
                }
            }
        }
    }

    if check(schema, &args) {
        return Ok(args);
    }

    let messages = errors(schema, &args)
        .iter()
        .map(|error| format!("  - {}: {}", format_validation_path(error), error.message))
        .collect::<Vec<_>>()
        .join("\n");
    let messages = if messages.is_empty() {
        "Unknown validation error".to_string()
    } else {
        messages
    };
    let received = serde_json::to_string_pretty(&Value::Object(tool_call.arguments.clone()))
        .unwrap_or_else(|_| "{}".to_string());

    Err(ToolValidationError(format!(
        "Validation failed for tool \"{}\":\n{messages}\n\nReceived arguments:\n{received}",
        tool_call.name
    )))
}
