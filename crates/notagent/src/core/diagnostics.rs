use serde::{Deserialize, Serialize};

/// Which kind of resource collided. The extension variant is retained because
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
