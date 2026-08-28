use std::path::Path;

use serde_json::Value;

/// `PiManifest`
/// Deviation (class 2): the `extensions` field is gone with the extension system
/// (`plans/facts/extension-boundary.md` §6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PiManifest {
    pub skills: Option<Vec<String>>,
    pub prompts: Option<Vec<String>>,
    pub themes: Option<Vec<String>>,
}

impl PiManifest {
    /// `manifest[resourceType]`
    pub fn entries(&self, resource_type: ResourceType) -> Option<&[String]> {
        match resource_type {
            ResourceType::Skills => self.skills.as_deref(),
            ResourceType::Prompts => self.prompts.as_deref(),
            ResourceType::Themes => self.themes.as_deref(),
        }
    }
}

/// `ResourceType` of the package manager, minus `extensions` (class 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceType {
    Skills,
    Prompts,
    Themes,
}

impl ResourceType {
    /// `RESOURCE_TYPES`
    pub const ALL: [ResourceType; 3] = [
        ResourceType::Skills,
        ResourceType::Prompts,
        ResourceType::Themes,
    ];

    /// The directory name and settings key.
    pub fn as_str(self) -> &'static str {
        match self {
            ResourceType::Skills => "skills",
            ResourceType::Prompts => "prompts",
            ResourceType::Themes => "themes",
        }
    }

    /// `FILE_PATTERNS[resourceType]` — the accepted file extension.
    pub fn file_extension(self) -> &'static str {
        match self {
            ResourceType::Skills | ResourceType::Prompts => ".md",
            ResourceType::Themes => ".json",
        }
    }
}

/// `readPiManifest(packageJsonPath)`
pub fn read_pi_manifest(package_json_path: &Path) -> Option<PiManifest> {
    let content = std::fs::read_to_string(package_json_path).ok()?;
    let package: Value = serde_json::from_str(&content).ok()?;
    let package = package.as_object()?;
    let notagent = package.get("notagent")?.as_object()?;

    let field = |name: &str| -> Option<Vec<String>> {
        let entries = notagent.get(name)?.as_array()?;
        entries
            .iter()
            .map(|entry| entry.as_str().map(str::to_owned))
            .collect()
    };
    Some(PiManifest {
        skills: field("skills"),
        prompts: field("prompts"),
        themes: field("themes"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("package.json");
        std::fs::write(&path, content).expect("write");
        path
    }

    #[test]
    fn returns_none_without_a_notagent_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read_pi_manifest(&write(dir.path(), "{}")), None);
        assert_eq!(read_pi_manifest(&write(dir.path(), "not json")), None);
        assert_eq!(
            read_pi_manifest(&write(dir.path(), r#"{"notagent": []}"#)),
            None
        );
    }

    #[test]
    fn keeps_only_string_arrays() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            dir.path(),
            r#"{"notagent": {"skills": ["a.md"], "prompts": [1], "themes": "x"}}"#,
        );
        assert_eq!(
            read_pi_manifest(&path),
            Some(PiManifest {
                skills: Some(vec!["a.md".to_owned()]),
                prompts: None,
                themes: None,
            })
        );
    }
}
