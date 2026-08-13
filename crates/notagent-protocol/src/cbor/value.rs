//! Dynamischer CBOR-Wert.
//!
//! Abweichung Klasse 1 (Sprachidiomatik): TS arbeitet mit `unknown` und den
//! nativen JS-Typen (null, boolean, number, string, Uint8Array, Array, plain
//! object). Rust braucht dafür einen expliziten Wertetyp. `Number` ist wie in
//! JavaScript immer ein f64 — die Unterscheidung Integer/Float trifft der
//! Encoder zur Laufzeit, exakt wie `Number.isInteger` in
//! `packages/protocol/src/cbor/encoder.ts`.

use serde_json::Value as JsonValue;

/// Größter in JavaScript sicher darstellbarer Integer (`Number.MAX_SAFE_INTEGER`).
pub(crate) const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Port von `Number.isSafeInteger`.
pub(crate) fn is_safe_integer(value: f64) -> bool {
    is_integer(value) && value.abs() <= MAX_SAFE_INTEGER
}

/// Port von `Number.isInteger`.
pub(crate) fn is_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0
}

/// Port von `Object.is(value, -0)`.
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
    /// Einfügereihenfolge bleibt erhalten (JS-Objekt-Semantik); doppelte
    /// Schlüssel weist der Decoder ab.
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

    /// Port von `isProtocolValue` aus `codec.ts`: erlaubt sind ausschließlich
    /// JSON-Werte. Byte-Strings (JS: `Uint8Array`) sind keine Protokollwerte.
    /// Zyklen, `undefined` und Nicht-Plain-Objekte sind in `CborValue` nicht
    /// darstellbar und können daher nicht auftreten.
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

    /// Gegenrichtung für den Encode-Pfad.
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

/// JS kennt nur einen Zahlentyp. Ganzzahlige Werte werden als Integer
/// abgebildet, damit `Type.Integer`-Schemas sie akzeptieren.
fn number_to_json(value: f64) -> Option<JsonValue> {
    if is_integer(value) && !is_negative_zero(value) && value.abs() <= MAX_SAFE_INTEGER {
        if value >= 0.0 {
            return Some(JsonValue::from(value as u64));
        }
        return Some(JsonValue::from(value as i64));
    }
    serde_json::Number::from_f64(value).map(JsonValue::Number)
}
