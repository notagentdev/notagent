use serde_json::Value as JsonValue;

/// Largest integer JavaScript can represent safely (`Number.MAX_SAFE_INTEGER`).
pub(crate) const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

pub(crate) fn is_safe_integer(value: f64) -> bool {
    is_integer(value) && value.abs() <= MAX_SAFE_INTEGER
}

pub(crate) fn is_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0
}

pub(crate) fn is_negative_zero(value: f64) -> bool {
    value == 0.0 && value.is_sign_negative()
}

#[derive(Debug, Clone, PartialEq)]
pub enum CborValue {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    Bytes(Vec<u8>),
    Array(Vec<CborValue>),
    /// Insertion order is preserved (JS object semantics); duplicate keys are
    /// rejected by the decoder.
    Map(Vec<(String, CborValue)>),
}

impl CborValue {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn map(entries: impl IntoIterator<Item = (impl Into<String>, CborValue)>) -> Self {
        Self::Map(
            entries
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    pub fn get(&self, key: &str) -> Option<&CborValue> {
        match self {
            Self::Map(entries) => entries
                .iter()
                .find(|(entry, _)| entry == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// Byte strings (JS: `Uint8Array`) are not protocol values. Cycles,
    /// `undefined` and non-plain objects cannot be represented by `CborValue`
    /// and therefore cannot occur.
    pub fn to_json_value(&self) -> Option<JsonValue> {
        match self {
            Self::Null => Some(JsonValue::Null),
            Self::Bool(value) => Some(JsonValue::Bool(*value)),
            Self::Number(value) => number_to_json(*value),
            Self::Text(value) => Some(JsonValue::String(value.clone())),
            Self::Bytes(_) => None,
            Self::Array(items) => {
                let mut result = Vec::with_capacity(items.len());
                for item in items {
                    result.push(item.to_json_value()?);
                }
                Some(JsonValue::Array(result))
            }
            Self::Map(entries) => {
                let mut result = serde_json::Map::new();
                for (key, value) in entries {
                    result.insert(key.clone(), value.to_json_value()?);
                }
                Some(JsonValue::Object(result))
            }
        }
    }

    /// Reverse direction, used by the encode path.
    pub fn from_json_value(value: &JsonValue) -> Self {
        match value {
            JsonValue::Null => Self::Null,
            JsonValue::Bool(value) => Self::Bool(*value),
            JsonValue::Number(value) => Self::Number(value.as_f64().unwrap_or(f64::NAN)),
            JsonValue::String(value) => Self::Text(value.clone()),
            JsonValue::Array(items) => {
                Self::Array(items.iter().map(Self::from_json_value).collect())
            }
            JsonValue::Object(entries) => Self::Map(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_json_value(value)))
                    .collect(),
            ),
        }
    }
}

/// JS has a single number type. Integral values are mapped to integers so that
/// `Type.Integer` schemas accept them.
fn number_to_json(value: f64) -> Option<JsonValue> {
    if is_integer(value) && !is_negative_zero(value) && value.abs() <= MAX_SAFE_INTEGER {
        if value >= 0.0 {
            return Some(JsonValue::from(value as u64));
        }
        return Some(JsonValue::from(value as i64));
    }
    serde_json::Number::from_f64(value).map(JsonValue::Number)
}
