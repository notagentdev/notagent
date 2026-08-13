//! JS-kompatible Zahlenformatierung für `serde_json`.
//!
//! Abweichung Klasse 1 (Sprachidiomatik, verhaltenserhaltend): `JSON.stringify` in
//! JavaScript formatiert Zahlen nach ECMAScript `Number::toString` — `0` statt `0.0`,
//! `0.000003` statt `3e-6`, `1e+21` statt `1000000000000000000000`. `serde_json`
//! benutzt dagegen ryu. Ohne Anpassung wären persistierte Session-Dateien und
//! Wire-Bodies nicht byte-identisch zum TS-Original.
//!
//! [`to_js_string`] implementiert den ECMAScript-Algorithmus (ECMA-262, `Number::toString`),
//! [`serialize`]/[`serialize_option`] geben ganzzahlige Werte als JSON-Integer aus, sodass
//! `0.0` als `0` erscheint.

use serde::{Deserialize, Deserializer, Serializer};
use serde_json::{Number, Value};

/// Größte ganze Zahl, die als f64 exakt darstellbar ist (`Number.MAX_SAFE_INTEGER + 1`).
const MAX_EXACT_INTEGER: f64 = 9_007_199_254_740_992.0;

/// Formatiert `value` exakt wie `String(value)` in JavaScript.
pub fn to_js_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value == 0.0 {
        // Deckt +0 und -0 ab: JS liefert für beide "0".
        return "0".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }

    let sign = if value < 0.0 { "-" } else { "" };
    let magnitude = value.abs();

    // `{:e}` liefert die kürzeste Round-Trip-Darstellung als `d[.ddd]e<exp>`,
    // also genau die Ziffernfolge s und den Exponenten aus dem ECMAScript-Algorithmus.
    let scientific = format!("{magnitude:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("{:e} liefert immer einen Exponenten");
    let exponent: i32 = exponent
        .parse()
        .expect("{:e} liefert einen gültigen Exponenten");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32; // Anzahl der Ziffern
    let n = exponent + 1; // Position des Dezimalpunkts: value = 0.digits * 10^n

    let formatted = if k <= n && n <= 21 {
        // Ziffern, aufgefüllt mit (n - k) Nullen.
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        // Dezimalpunkt nach n Ziffern.
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        // "0." + (-n) Nullen + Ziffern.
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        // Exponentialschreibweise mit explizitem Vorzeichen wie in JS ("1e+21", "1e-7").
        let exponent_part = if n > 0 {
            format!("e+{}", n - 1)
        } else {
            format!("e-{}", 1 - n)
        };
        if k == 1 {
            format!("{digits}{exponent_part}")
        } else {
            format!("{}.{}{exponent_part}", &digits[..1], &digits[1..])
        }
    };

    format!("{sign}{formatted}")
}

/// Normalizes every number of a JSON value the way JavaScript would hold it.
///
/// JavaScript knows exactly one number type (f64), so `JSON.parse` turns `1e3` into
/// `1000` and loses precision above 2^53 (`12345678901234567890` becomes
/// `12345678901234567000`). serde_json instead keeps `u64`/`i64` precision and the
/// float-ness of the literal. Tool-call arguments are parsed from provider streams and
/// re-serialized into the next request, so they must round-trip like the TS original.
pub fn normalize_json_numbers(value: &mut Value) {
    match value {
        Value::Number(number) => {
            let Some(as_f64) = number.as_f64() else {
                return;
            };
            *number = f64_to_js_number(as_f64);
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_json_numbers),
        Value::Object(entries) => entries
            .iter_mut()
            .for_each(|(_, item)| normalize_json_numbers(item)),
        _ => {}
    }
}

/// Maps an f64 to the JSON number JavaScript would print for it.
///
/// Going through [`to_js_string`] keeps the JS behaviour for magnitudes beyond 2^53,
/// where the shortest round-tripping decimal is padded with zeros
/// (`12345678901234567890` -> `12345678901234567000`).
fn f64_to_js_number(value: f64) -> Number {
    serde_json::from_str::<Number>(&to_js_string(value))
        .unwrap_or_else(|_| Number::from_f64(value).unwrap_or_else(|| Number::from(0)))
}

/// Serialisiert ganzzahlige Werte als JSON-Integer (`0` statt `0.0`), alles andere als f64.
pub fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < MAX_EXACT_INTEGER {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    f64::deserialize(deserializer)
}

/// Wie [`serialize`], aber für `Option<f64>`-Felder.
pub mod option {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => super::serialize(value, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<f64>, D::Error> {
        Option::<f64>::deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Referenzwerte aus `node -e "console.log(JSON.stringify(v))"` im TS-Repo.
    #[test]
    fn matches_javascript_number_to_string() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.0, "-1"),
            (0.5, "0.5"),
            (3.0, "3"),
            (15.0, "15"),
            (0.25, "0.25"),
            (0.000003, "0.000003"),
            (0.0000015, "0.0000015"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (100.0, "100"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (0.1, "0.1"),
            (0.3, "0.3"),
            (1.0 / 3.0, "0.3333333333333333"),
            (2.5e-9, "2.5e-9"),
            (123456789012345680000.0, "123456789012345680000"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
        ];
        for (value, expected) in cases {
            assert_eq!(&to_js_string(*value), expected, "to_js_string({value})");
        }
    }

    #[test]
    fn serializes_integral_values_as_integers() {
        #[derive(serde::Serialize)]
        struct Holder {
            #[serde(with = "super")]
            value: f64,
        }
        assert_eq!(
            serde_json::to_string(&Holder { value: 0.0 }).unwrap(),
            r#"{"value":0}"#
        );
        assert_eq!(
            serde_json::to_string(&Holder { value: 12.0 }).unwrap(),
            r#"{"value":12}"#
        );
        assert_eq!(
            serde_json::to_string(&Holder { value: 0.5 }).unwrap(),
            r#"{"value":0.5}"#
        );
    }
}
