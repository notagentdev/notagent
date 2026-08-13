//! Parsing of possibly incomplete JSON from streaming tool-call arguments.
//!
//! 1:1 port of `packages/ai/src/utils/json-parse.ts` (124 LOC) including the
//! `partial-json` npm package it delegates to (`node_modules/partial-json/dist/index.js`,
//! 220 LOC, always called with the default `Allow.ALL`). Substitution class 3 of the
//! master plan: "partial-json -> own port of parseStreamingJson incl. repairJson".
//!
//! Indices follow the JavaScript implementation, but count Unicode scalar values
//! instead of UTF-16 code units. Every structural character JSON uses is ASCII, so
//! tokenization is unaffected.

use serde_json::{Map, Value};

use crate::utils::js_number::normalize_json_numbers;

const VALID_JSON_ESCAPES: [char; 9] = ['"', '\\', '/', 'b', 'f', 'n', 'r', 't', 'u'];

fn is_control_character(character: char) -> bool {
    (character as u32) <= 0x1f
}

fn escape_control_character(character: char) -> String {
    match character {
        '\u{8}' => "\\b".to_string(),
        '\u{c}' => "\\f".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        other => format!("\\u{:04x}", other as u32),
    }
}

/// Repairs malformed JSON string literals by escaping raw control characters inside
/// strings and doubling backslashes before invalid escape characters.
pub fn repair_json(json: &str) -> String {
    let characters: Vec<char> = json.chars().collect();
    let mut repaired = String::new();
    let mut in_string = false;
    let mut index = 0;

    while index < characters.len() {
        let character = characters[index];

        if !in_string {
            repaired.push(character);
            if character == '"' {
                in_string = true;
            }
            index += 1;
            continue;
        }

        if character == '"' {
            repaired.push(character);
            in_string = false;
            index += 1;
            continue;
        }

        if character == '\\' {
            let Some(&next_character) = characters.get(index + 1) else {
                repaired.push_str("\\\\");
                index += 1;
                continue;
            };

            if next_character == 'u' {
                let unicode_digits: String = characters.iter().skip(index + 2).take(4).collect();
                if unicode_digits.chars().count() == 4
                    && unicode_digits
                        .chars()
                        .all(|digit| digit.is_ascii_hexdigit())
                {
                    repaired.push_str(&format!("\\u{unicode_digits}"));
                    index += 6;
                    continue;
                }
            }

            if VALID_JSON_ESCAPES.contains(&next_character) {
                repaired.push('\\');
                repaired.push(next_character);
                index += 2;
                continue;
            }

            repaired.push_str("\\\\");
            index += 1;
            continue;
        }

        if is_control_character(character) {
            repaired.push_str(&escape_control_character(character));
        } else {
            repaired.push(character);
        }
        index += 1;
    }

    repaired
}

/// `parseJsonWithRepair` — plain parse, then one repaired retry.
///
/// Numbers are normalized to JavaScript semantics (see [`normalize_json_numbers`]).
pub fn parse_json_with_repair(json: &str) -> Result<Value, serde_json::Error> {
    let mut value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(error) => {
            let repaired = repair_json(json);
            if repaired == json {
                return Err(error);
            }
            serde_json::from_str(&repaired)?
        }
    };
    normalize_json_numbers(&mut value);
    Ok(value)
}

/// Attempts to parse potentially incomplete JSON during streaming. Always returns a
/// value; `{}` when nothing can be salvaged.
pub fn parse_streaming_json(partial_json: Option<&str>) -> Value {
    let Some(partial_json) = partial_json else {
        return Value::Object(Map::new());
    };
    if partial_json.trim().is_empty() {
        return Value::Object(Map::new());
    }

    if let Ok(value) = parse_json_with_repair(partial_json) {
        return value;
    }
    // `result ?? {}`: a parsed `null` becomes `{}` as well.
    if let Ok((mut value, non_finite)) = partial_parse_detailed(partial_json)
        && (value != Value::Null || non_finite)
    {
        normalize_json_numbers(&mut value);
        return value;
    }
    if let Ok((mut value, non_finite)) = partial_parse_detailed(&repair_json(partial_json))
        && (value != Value::Null || non_finite)
    {
        normalize_json_numbers(&mut value);
        return value;
    }
    Value::Object(Map::new())
}

/// Error of the partial parser (`PartialJSON` and `MalformedJSON` in the npm package).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PartialJsonError {
    #[error("{0}")]
    Partial(String),
    #[error("{0}")]
    Malformed(String),
    #[error("{0} is empty")]
    Empty(String),
}

/// `parse(jsonString)` of `partial-json` with `Allow.ALL`.
pub fn partial_parse(json: &str) -> Result<Value, PartialJsonError> {
    partial_parse_detailed(json).map(|(value, _)| value)
}

/// Like [`partial_parse`], but also reports whether the root value came from a
/// non-finite literal (`NaN`, `Infinity`, `-Infinity`).
///
/// JavaScript can hold those as numbers; `serde_json::Value` cannot, so they are mapped
/// to `null` — the same thing `JSON.stringify` writes for them. The flag keeps them
/// apart from a literal `null`, which `parseStreamingJson` replaces with `{}` via `??`.
pub fn partial_parse_detailed(json: &str) -> Result<(Value, bool), PartialJsonError> {
    if json.trim().is_empty() {
        return Err(PartialJsonError::Empty(json.to_string()));
    }
    let mut parser = PartialParser {
        characters: json.trim().chars().collect(),
        index: 0,
        non_finite: false,
    };
    let value = parser.parse_any()?;
    Ok((value, parser.non_finite))
}

struct PartialParser {
    characters: Vec<char>,
    index: usize,
    /// Set when a `NaN`/`Infinity`/`-Infinity` literal was parsed.
    non_finite: bool,
}

impl PartialParser {
    fn length(&self) -> usize {
        self.characters.len()
    }

    fn at(&self, index: usize) -> Option<char> {
        self.characters.get(index).copied()
    }

    fn slice(&self, start: usize, end: usize) -> String {
        let end = end.min(self.length());
        if start >= end {
            return String::new();
        }
        self.characters[start..end].iter().collect()
    }

    fn starts_with_at(&self, index: usize, text: &str) -> bool {
        self.slice(index, index + text.chars().count()) == text
    }

    /// `"null".startsWith(jsonString.substring(index))` — remaining input is a prefix.
    fn remainder_is_prefix_of(&self, text: &str) -> bool {
        text.starts_with(&self.slice(self.index, self.length()))
    }

    fn mark_partial(&self, message: &str) -> PartialJsonError {
        PartialJsonError::Partial(format!("{message} at position {}", self.index))
    }

    fn malformed(&self, message: &str) -> PartialJsonError {
        PartialJsonError::Malformed(format!("{message} at position {}", self.index))
    }

    fn skip_blank(&mut self) {
        while self.index < self.length()
            && matches!(self.at(self.index), Some(' ' | '\n' | '\r' | '\t'))
        {
            self.index += 1;
        }
    }

    fn parse_any(&mut self) -> Result<Value, PartialJsonError> {
        self.skip_blank();
        if self.index >= self.length() {
            return Err(self.mark_partial("Unexpected end of input"));
        }
        match self.at(self.index) {
            Some('"') => return self.parse_str(),
            Some('{') => return self.parse_obj(),
            Some('[') => return self.parse_arr(),
            _ => {}
        }

        // Literals; with Allow.ALL a truncated tail is accepted as a prefix.
        let remaining = self.length() - self.index;
        if self.starts_with_at(self.index, "null")
            || (remaining < 4 && self.remainder_is_prefix_of("null"))
        {
            self.index += 4;
            return Ok(Value::Null);
        }
        if self.starts_with_at(self.index, "true")
            || (remaining < 4 && self.remainder_is_prefix_of("true"))
        {
            self.index += 4;
            return Ok(Value::Bool(true));
        }
        if self.starts_with_at(self.index, "false")
            || (remaining < 5 && self.remainder_is_prefix_of("false"))
        {
            self.index += 5;
            return Ok(Value::Bool(false));
        }
        // Infinity/-Infinity/NaN cannot be represented by serde_json and become `null`,
        // which is what `JSON.stringify` produces for them as well.
        if self.starts_with_at(self.index, "Infinity")
            || (remaining < 8 && self.remainder_is_prefix_of("Infinity"))
        {
            self.index += 8;
            self.non_finite = true;
            return Ok(Value::Null);
        }
        if self.starts_with_at(self.index, "-Infinity")
            || (1 < remaining && remaining < 9 && self.remainder_is_prefix_of("-Infinity"))
        {
            self.index += 9;
            self.non_finite = true;
            return Ok(Value::Null);
        }
        if self.starts_with_at(self.index, "NaN")
            || (remaining < 3 && self.remainder_is_prefix_of("NaN"))
        {
            self.index += 3;
            self.non_finite = true;
            return Ok(Value::Null);
        }
        self.parse_num()
    }

    fn parse_str(&mut self) -> Result<Value, PartialJsonError> {
        let start = self.index;
        let mut escape = false;
        self.index += 1; // skip initial quote
        while self.index < self.length()
            && (self.at(self.index) != Some('"')
                || (escape && self.at(self.index - 1) == Some('\\')))
        {
            escape = if self.at(self.index) == Some('\\') {
                !escape
            } else {
                false
            };
            self.index += 1;
        }

        if self.at(self.index) == Some('"') {
            self.index += 1;
            let end = self.index - usize::from(escape);
            return serde_json::from_str(&self.slice(start, end))
                .map_err(|error| self.malformed(&format!("SyntaxError: {error}")));
        }

        // Allow.STR: close the string literal and retry.
        let end = self.index - usize::from(escape);
        let candidate = format!("{}\"", self.slice(start, end));
        if let Ok(value) = serde_json::from_str::<Value>(&candidate) {
            return Ok(value);
        }
        // Invalid escape sequence: cut at the last backslash.
        let last_backslash = self
            .characters
            .iter()
            .rposition(|character| *character == '\\');
        let cut = last_backslash.unwrap_or(0);
        let fallback = format!("{}\"", self.slice(start, cut));
        serde_json::from_str(&fallback)
            .map_err(|error| self.malformed(&format!("SyntaxError: {error}")))
    }

    fn parse_obj(&mut self) -> Result<Value, PartialJsonError> {
        self.index += 1; // skip initial brace
        self.skip_blank();
        let mut object = Map::new();

        // The outer `try` of the TS implementation: with Allow.OBJ every failure yields
        // the object parsed so far.
        loop {
            if self.at(self.index) == Some('}') {
                break;
            }
            self.skip_blank();
            if self.index >= self.length() {
                return Ok(Value::Object(object));
            }
            let key = match self.parse_str() {
                Ok(Value::String(key)) => key,
                // A non-string key cannot occur in JSON; TS would use the coerced value.
                Ok(other) => other.to_string(),
                Err(_) => return Ok(Value::Object(object)),
            };
            self.skip_blank();
            self.index += 1; // skip colon
            match self.parse_any() {
                Ok(value) => {
                    object.insert(key, value);
                }
                Err(_) => return Ok(Value::Object(object)),
            }
            self.skip_blank();
            if self.at(self.index) == Some(',') {
                self.index += 1; // skip comma
            }
            if self.index >= self.length() {
                return Ok(Value::Object(object));
            }
        }

        self.index += 1; // skip final brace
        Ok(Value::Object(object))
    }

    fn parse_arr(&mut self) -> Result<Value, PartialJsonError> {
        self.index += 1; // skip initial bracket
        let mut array = Vec::new();

        loop {
            if self.at(self.index) == Some(']') {
                break;
            }
            match self.parse_any() {
                Ok(value) => array.push(value),
                // Allow.ARR
                Err(_) => return Ok(Value::Array(array)),
            }
            self.skip_blank();
            if self.at(self.index) == Some(',') {
                self.index += 1; // skip comma
            }
            if self.index >= self.length() {
                return Ok(Value::Array(array));
            }
        }

        self.index += 1; // skip final bracket
        Ok(Value::Array(array))
    }

    fn parse_num(&mut self) -> Result<Value, PartialJsonError> {
        if self.index == 0 {
            let whole: String = self.characters.iter().collect();
            if whole == "-" {
                return Err(self.malformed("Not sure what '-' is"));
            }
            if let Ok(value) = serde_json::from_str::<Value>(&whole) {
                return Ok(value);
            }
            // Allow.NUM: drop a trailing exponent.
            if let Some(exponent) = whole.rfind('e')
                && let Ok(value) = serde_json::from_str::<Value>(&whole[..exponent])
            {
                return Ok(value);
            }
            return Err(self.malformed("SyntaxError: Unexpected token"));
        }

        let start = self.index;
        if self.at(self.index) == Some('-') {
            self.index += 1;
        }
        while let Some(character) = self.at(self.index) {
            if matches!(character, ',' | ']' | '}') {
                break;
            }
            self.index += 1;
        }

        let text = self.slice(start, self.index);
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            return Ok(value);
        }
        if text == "-" {
            return Err(self.mark_partial("Not sure what '-' is"));
        }
        let whole: String = self.characters.iter().collect();
        if let Some(exponent) = whole.rfind('e')
            && exponent > start
            && let Ok(value) = serde_json::from_str::<Value>(&self.slice(start, exponent))
        {
            return Ok(value);
        }
        Err(self.malformed("SyntaxError: Unexpected token"))
    }
}
