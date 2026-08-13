//! Schema helpers.
//!
//! 1:1 port of `packages/ai/src/utils/typebox-helpers.ts` (24 LOC). TypeBox schemas are
//! `serde_json::Value` here (master-plan substitution class 3), so `StringEnum` builds
//! the same JSON Schema object without the TypeBox `TUnsafe` wrapper.

use serde_json::{Map, Value, json};

/// `StringEnum(values, options?)` — a string enum schema for providers that reject
/// `anyOf`/`const` patterns.
pub fn string_enum(
    values: impl IntoIterator<Item = impl Into<String>>,
    description: Option<&str>,
    default: Option<&str>,
) -> Value {
    let mut schema = Map::new();
    schema.insert("type".to_string(), json!("string"));
    schema.insert(
        "enum".to_string(),
        Value::Array(
            values
                .into_iter()
                .map(|value| Value::String(value.into()))
                .collect(),
        ),
    );
    // TS spreads the fields conditionally, so an empty string is left out as well.
    if let Some(description) = description.filter(|value| !value.is_empty()) {
        schema.insert("description".to_string(), json!(description));
    }
    if let Some(default) = default.filter(|value| !value.is_empty()) {
        schema.insert("default".to_string(), json!(default));
    }
    Value::Object(schema)
}
