//! Port of `packages/coding-agent/src/core/source-info.ts`.
//!
//! Where a loaded resource — a skill, a prompt template, a theme — came from.
//! Every resource that reaches the user carries one of these so the UI can say
//! "this skill is installed from package X" instead of just showing a path.
//!
//! `PathMetadata` is declared here rather than in `core/package-manager.rs`
//! (task 14): in TypeScript the two files import from each other, which Rust
//! modules of one crate do not need — and the type is the source-info record
//! minus its `path`. `package_manager` re-exports it when it lands.

use serde::{Deserialize, Serialize};

/// Whose configuration directory a resource lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceScope {
    User,
    Project,
    Temporary,
}

/// Whether a resource is an installed package or a hand-placed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceOrigin {
    Package,
    TopLevel,
}

/// What the package manager knows about a path it resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathMetadata {
    pub source: String,
    pub scope: SourceScope,
    pub origin: SourceOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<String>,
}

/// `PathMetadata` plus the resolved path of the resource itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub path: String,
    pub source: String,
    pub scope: SourceScope,
    pub origin: SourceOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<String>,
}

pub fn create_source_info(path: impl Into<String>, metadata: &PathMetadata) -> SourceInfo {
    SourceInfo {
        path: path.into(),
        source: metadata.source.clone(),
        scope: metadata.scope,
        origin: metadata.origin,
        base_dir: metadata.base_dir.clone(),
    }
}

/// The optional half of `create_synthetic_source_info`'s argument object.
///
/// `source` is the only field every caller sets; the defaults below are the
/// `?? "temporary"` / `?? "top-level"` fallbacks from the TypeScript.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyntheticSourceInfoOptions {
    pub source: String,
    pub scope: Option<SourceScope>,
    pub origin: Option<SourceOrigin>,
    pub base_dir: Option<String>,
}

impl SyntheticSourceInfoOptions {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            ..Self::default()
        }
    }
}

pub fn create_synthetic_source_info(
    path: impl Into<String>,
    options: SyntheticSourceInfoOptions,
) -> SourceInfo {
    SourceInfo {
        path: path.into(),
        source: options.source,
        scope: options.scope.unwrap_or(SourceScope::Temporary),
        origin: options.origin.unwrap_or(SourceOrigin::TopLevel),
        base_dir: options.base_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_the_package_manager_metadata_next_to_the_path() {
        let metadata = PathMetadata {
            source: "notagentdev/skills".to_string(),
            scope: SourceScope::Project,
            origin: SourceOrigin::Package,
            base_dir: Some("/repo/.notagent/packages/skills".to_string()),
        };
        assert_eq!(
            create_source_info("/repo/.notagent/packages/skills/review/SKILL.md", &metadata),
            SourceInfo {
                path: "/repo/.notagent/packages/skills/review/SKILL.md".to_string(),
                source: "notagentdev/skills".to_string(),
                scope: SourceScope::Project,
                origin: SourceOrigin::Package,
                base_dir: Some("/repo/.notagent/packages/skills".to_string()),
            }
        );
    }

    #[test]
    fn falls_back_to_a_temporary_top_level_source() {
        let info =
            create_synthetic_source_info("<sdk:review>", SyntheticSourceInfoOptions::new("sdk"));
        assert_eq!(info.scope, SourceScope::Temporary);
        assert_eq!(info.origin, SourceOrigin::TopLevel);
        assert_eq!(info.base_dir, None);
    }

    #[test]
    fn keeps_explicit_scope_and_origin() {
        let info = create_synthetic_source_info(
            "/home/u/.notagent/agent/skills/review/SKILL.md",
            SyntheticSourceInfoOptions {
                source: "user".to_string(),
                scope: Some(SourceScope::User),
                origin: Some(SourceOrigin::TopLevel),
                base_dir: Some("/home/u/.notagent/agent/skills".to_string()),
            },
        );
        assert_eq!(info.scope, SourceScope::User);
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            serde_json::json!({
                "path": "/home/u/.notagent/agent/skills/review/SKILL.md",
                "source": "user",
                "scope": "user",
                "origin": "top-level",
                "baseDir": "/home/u/.notagent/agent/skills",
            })
        );
    }

    #[test]
    fn omits_the_base_dir_when_absent_like_an_undefined_field() {
        let info =
            create_synthetic_source_info("<sdk:review>", SyntheticSourceInfoOptions::new("sdk"));
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            serde_json::json!({
                "path": "<sdk:review>",
                "source": "sdk",
                "scope": "temporary",
                "origin": "top-level",
            })
        );
    }
}
