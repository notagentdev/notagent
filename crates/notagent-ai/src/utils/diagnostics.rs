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
