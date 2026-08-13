//! Redigierte Provider-/Laufzeit-Diagnosen an `AssistantMessage`.
//!
//! 1:1-Port von `packages/ai/src/utils/diagnostics.ts` (45 LOC).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `DiagnosticErrorInfo { name?, message, stack?, code? }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DiagnosticErrorInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    /// TS: `string | number`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Value>,
}

/// `AssistantMessageDiagnostic { type, timestamp, error?, details? }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessageDiagnostic {
    #[serde(rename = "type")]
    pub r#type: String,
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DiagnosticErrorInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Map<String, Value>>,
}

/// `formatThrownValue(value)` — `Error.message || Error.name`, strings pass through.
pub fn format_thrown_value(value: &dyn std::fmt::Display) -> String {
    value.to_string()
}

/// `extractDiagnosticError(error)` for a Rust error value.
///
/// TS distinguishes `Error` instances from arbitrary thrown values; the Rust
/// counterpart of a thrown non-error is a plain value, which callers pass through
/// [`thrown_value_diagnostic`].
pub fn extract_diagnostic_error(error: &dyn std::error::Error) -> DiagnosticErrorInfo {
    DiagnosticErrorInfo {
        name: Some("Error".to_string()),
        message: error.to_string(),
        stack: None,
        code: None,
    }
}

/// `extractDiagnosticError(value)` for a non-error value (TS: `{ name: "ThrownValue" }`).
pub fn thrown_value_diagnostic(value: &dyn std::fmt::Display) -> DiagnosticErrorInfo {
    DiagnosticErrorInfo {
        name: Some("ThrownValue".to_string()),
        message: format_thrown_value(value),
        stack: None,
        code: None,
    }
}

/// `createAssistantMessageDiagnostic(type, error, details?)`
pub fn create_assistant_message_diagnostic(
    diagnostic_type: impl Into<String>,
    error: DiagnosticErrorInfo,
    details: Option<Map<String, Value>>,
    timestamp: i64,
) -> AssistantMessageDiagnostic {
    AssistantMessageDiagnostic {
        r#type: diagnostic_type.into(),
        timestamp,
        error: Some(error),
        details,
    }
}

/// `appendAssistantMessageDiagnostic(message, diagnostic)`
pub fn append_assistant_message_diagnostic(
    diagnostics: &mut Option<Vec<AssistantMessageDiagnostic>>,
    diagnostic: AssistantMessageDiagnostic,
) {
    diagnostics.get_or_insert_with(Vec::new).push(diagnostic);
}
