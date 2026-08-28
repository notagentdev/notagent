use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::config::CONFIG_DIR_NAME;
use crate::utils::lockfile::{LockError, LockOptions, lock_with_retry};
use crate::utils::paths::{canonicalize_path, resolve_path_default};

/// `true`/`false` when a decision was recorded, `None` when none was.
pub type ProjectTrustDecision = Option<bool>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTrustStoreEntry {
    pub path: String,
    pub decision: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTrustUpdate {
    pub path: String,
    pub decision: ProjectTrustDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTrustOption {
    pub label: String,
    pub trusted: bool,
    pub updates: Vec<ProjectTrustUpdate>,
    pub saved_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TrustStoreError(pub String);

const TRUST_REQUIRING_PROJECT_CONFIG_RESOURCES: [&str; 7] = [
    "settings.json",
    "extensions",
    "skills",
    "prompts",
    "themes",
    "SYSTEM.md",
    "APPEND_SYSTEM.md",
];

fn current_dir() -> String {
    std::env::current_dir()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn normalize_cwd(cwd: &str) -> String {
    canonicalize_path(
        &resolve_path_default(cwd, &current_dir()).unwrap_or_else(|_| cwd.to_string()),
    )
}

/// `path.dirname`, which stops at the root rather than becoming empty.
fn dirname(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .filter(|parent| !parent.is_empty())
        .unwrap_or_else(|| path.to_string())
}

fn find_nearest_trust_entry(
    data: &BTreeMap<String, Option<bool>>,
    cwd: &str,
) -> Option<ProjectTrustStoreEntry> {
    let mut current_dir = normalize_cwd(cwd);
    loop {
        if let Some(Some(decision)) = data.get(&current_dir) {
            return Some(ProjectTrustStoreEntry {
                path: current_dir,
                decision: *decision,
            });
        }
        let parent_dir = dirname(&current_dir);
        if parent_dir == current_dir {
            return None;
        }
        current_dir = parent_dir;
    }
}

pub fn get_project_trust_parent_path(cwd: &str) -> Option<String> {
    let trust_path = normalize_cwd(cwd);
    let parent_dir = dirname(&trust_path);
    (parent_dir != trust_path).then_some(parent_dir)
}

pub fn get_project_trust_options(cwd: &str, include_session_only: bool) -> Vec<ProjectTrustOption> {
    let trust_path = normalize_cwd(cwd);
    let mut options = vec![ProjectTrustOption {
        label: "Trust".to_string(),
        trusted: true,
        updates: vec![ProjectTrustUpdate {
            path: trust_path.clone(),
            decision: Some(true),
        }],
        saved_path: Some(trust_path.clone()),
    }];
    if let Some(parent_path) = get_project_trust_parent_path(cwd) {
        options.push(ProjectTrustOption {
            label: format!("Trust parent folder ({parent_path})"),
            trusted: true,
            updates: vec![
                ProjectTrustUpdate {
                    path: parent_path.clone(),
                    decision: Some(true),
                },
                ProjectTrustUpdate {
                    path: trust_path.clone(),
                    decision: None,
                },
            ],
            saved_path: Some(parent_path),
        });
    }
    if include_session_only {
        options.push(ProjectTrustOption {
            label: "Trust (this session only)".to_string(),
            trusted: true,
            updates: Vec::new(),
            saved_path: None,
        });
    }
    options.push(ProjectTrustOption {
        label: "Do not trust".to_string(),
        trusted: false,
        updates: vec![ProjectTrustUpdate {
            path: trust_path.clone(),
            decision: Some(false),
        }],
        saved_path: Some(trust_path),
    });
    if include_session_only {
        options.push(ProjectTrustOption {
            label: "Do not trust (this session only)".to_string(),
            trusted: false,
            updates: Vec::new(),
            saved_path: None,
        });
    }
    options
}

fn read_trust_file(path: &Path) -> Result<BTreeMap<String, Option<bool>>, TrustStoreError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let raw = std::fs::read_to_string(path).map_err(|error| {
        TrustStoreError(format!(
            "Failed to read trust store {}: {error}",
            path.display()
        ))
    })?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|error| {
        TrustStoreError(format!(
            "Failed to read trust store {}: {error}",
            path.display()
        ))
    })?;
    let Value::Object(object) = parsed else {
        return Err(TrustStoreError(format!(
            "Invalid trust store {}: expected an object",
            path.display()
        )));
    };

    let mut data = BTreeMap::new();
    for (key, value) in object {
        let decision = match value {
            Value::Bool(decision) => Some(decision),
            Value::Null => None,
            _ => {
                return Err(TrustStoreError(format!(
                    "Invalid trust store {}: value for {} must be true, false, or null",
                    path.display(),
                    serde_json::to_string(&key).unwrap_or_default()
                )));
            }
        };
        data.insert(key, decision);
    }
    Ok(data)
}

fn write_trust_file(
    path: &Path,
    data: &BTreeMap<String, Option<bool>>,
) -> Result<(), TrustStoreError> {
    // `Object.keys(data).sort()` orders by UTF-16 code unit; `BTreeMap` orders
    // by byte, which agrees for every path that is not outside the BMP.
    let mut sorted = Map::new();
    for (key, value) in data {
        sorted.insert(key.clone(), value.map_or(Value::Null, Value::Bool));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| TrustStoreError(error.to_string()))?;
    }
    let serialized = serde_json::to_string_pretty(&Value::Object(sorted))
        .map_err(|error| TrustStoreError(error.to_string()))?;
    std::fs::write(path, format!("{serialized}\n"))
        .map_err(|error| TrustStoreError(error.to_string()))
}

fn with_trust_file_lock<T>(
    path: &Path,
    run: impl FnOnce() -> Result<T, TrustStoreError>,
) -> Result<T, TrustStoreError> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|error| TrustStoreError(error.to_string()))?;
    }
    let guard = lock_with_retry(path, &LockOptions::default(), 10, Duration::from_millis(20))
        .map_err(|error| match error {
            LockError::Locked => TrustStoreError("Failed to acquire trust store lock".to_string()),
            other => TrustStoreError(other.to_string()),
        })?;
    let result = run();
    guard.release();
    result
}

/// Returns true when cwd has project-local resources that must be gated by
/// project trust: trust-requiring entries under cwd/.notagent, or .agents/skills in
/// cwd or one of its ancestors. Returns false when no such project resources
/// exist. The user/global ~/.agents/skills directory is always treated as a
/// trusted user resource and is ignored here, even when cwd is $HOME.
pub fn has_trust_requiring_project_resources(cwd: &str) -> bool {
    let home = std::env::var("HOME")
        .ok()
        .filter(|home| !home.is_empty())
        .or_else(|| dirs::home_dir().map(|home| home.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let home_dir =
        canonicalize_path(&resolve_path_default(&home, &current_dir()).unwrap_or(home.clone()));
    let user_agents_skills_dir = PathBuf::from(&home_dir).join(".agents").join("skills");
    let mut current = normalize_cwd(cwd);

    let config_dir = PathBuf::from(&current).join(CONFIG_DIR_NAME);
    if TRUST_REQUIRING_PROJECT_CONFIG_RESOURCES
        .iter()
        .any(|entry| config_dir.join(entry).exists())
    {
        return true;
    }

    loop {
        let agents_skills_dir = PathBuf::from(&current).join(".agents").join("skills");
        if agents_skills_dir != user_agents_skills_dir && agents_skills_dir.exists() {
            return true;
        }
        let parent = dirname(&current);
        if parent == current {
            return false;
        }
        current = parent;
    }
}

pub struct ProjectTrustStore {
    trust_path: PathBuf,
}

impl ProjectTrustStore {
    pub fn new(agent_dir: &str) -> Self {
        let resolved = resolve_path_default(agent_dir, &current_dir())
            .unwrap_or_else(|_| agent_dir.to_string());
        Self {
            trust_path: PathBuf::from(resolved).join("trust.json"),
        }
    }

    pub fn get(&self, cwd: &str) -> Result<ProjectTrustDecision, TrustStoreError> {
        Ok(self.get_entry(cwd)?.map(|entry| entry.decision))
    }

    pub fn get_entry(&self, cwd: &str) -> Result<Option<ProjectTrustStoreEntry>, TrustStoreError> {
        with_trust_file_lock(&self.trust_path, || {
            let data = read_trust_file(&self.trust_path)?;
            Ok(find_nearest_trust_entry(&data, cwd))
        })
    }

    pub fn set(&self, cwd: &str, decision: ProjectTrustDecision) -> Result<(), TrustStoreError> {
        self.set_many(&[ProjectTrustUpdate {
            path: cwd.to_string(),
            decision,
        }])
    }

    pub fn set_many(&self, decisions: &[ProjectTrustUpdate]) -> Result<(), TrustStoreError> {
        with_trust_file_lock(&self.trust_path, || {
            let mut data = read_trust_file(&self.trust_path)?;
            for update in decisions {
                let key = normalize_cwd(&update.path);
                match update.decision {
                    None => {
                        data.remove(&key);
                    }
                    Some(decision) => {
                        data.insert(key, Some(decision));
                    }
                }
            }
            write_trust_file(&self.trust_path, &data)
        })
    }
}
