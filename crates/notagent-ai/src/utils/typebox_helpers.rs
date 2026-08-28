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
    if let Some(description) = description.filter(|value| !value.is_empty()) {
        schema.insert("description".to_string(), json!(description));
    }
    if let Some(default) = default.filter(|value| !value.is_empty()) {
        schema.insert("default".to_string(), json!(default));
    }
    Value::Object(schema)
}
