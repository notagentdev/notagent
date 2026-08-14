//! Port of `packages/coding-agent/src/core/diagnostics.ts`.
//!
//! What a resource loader reports about the files it walked past. The loaders
//! never throw on a bad skill or an unreadable theme — one broken file in a
//! directory would otherwise take every sibling with it — so a diagnostic is
//! how the problem still reaches the user.
//!
//! `collision` is the interesting case: two resources claiming the same name is
//! not an error, because one of them does win and the session works. Recording
//! which one lost, and where it lives, is what lets the UI explain why the
//! skill the user just wrote is not the one being loaded.
//!
//! `diagnostics.ts` also carries the startup timing view, which belongs to plan
//! task 12; only the two records the loaders need are here.

use serde::{Deserialize, Serialize};

/// Which kind of resource collided. The extension variant is retained because
/// session files and package manifests written by the TypeScript app still name
/// it; nothing in this port produces one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    Extension,
    Skill,
    Prompt,
    Theme,
}

/// Two resources of one kind claiming one name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceCollision {
    pub resource_type: ResourceKind,
    /// Skill name, command/tool/flag name, prompt name, or theme name.
    pub name: String,
    pub winner_path: String,
    pub loser_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loser_source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Warning,
    Error,
    Collision,
}

/// One thing that went wrong, or one name that was taken twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDiagnostic {
    #[serde(rename = "type")]
    pub level: DiagnosticLevel,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collision: Option<ResourceCollision>,
}

impl ResourceDiagnostic {
    pub fn warning(message: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Warning,
            message: message.into(),
            path: Some(path.into()),
            collision: None,
        }
    }

    pub fn error(message: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Error,
            message: message.into(),
            path: Some(path.into()),
            collision: None,
        }
    }

    pub fn collision(
        message: impl Into<String>,
        path: impl Into<String>,
        collision: ResourceCollision,
    ) -> Self {
        Self {
            level: DiagnosticLevel::Collision,
            message: message.into(),
            path: Some(path.into()),
            collision: Some(collision),
        }
    }
}
