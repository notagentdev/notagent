//! Port of `packages/coding-agent/src/migrations.ts`.
//!
//! One-time migrations that run on startup.
//!
//! Deviation class 2 (extension removal): `checkDeprecatedExtensionDirs` and
//! `showDeprecationWarnings` only pointed users at the extension system, which
//! this port drops (plans/facts/extension-boundary.md §6). `commands/` →
//! `prompts/` stays, because prompt templates stay.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::config::{get_agent_dir, get_bin_dir};

/// Migrate legacy oauth.json and settings.json apiKeys to auth.json.
///
/// Returns the provider names that were migrated.
pub fn migrate_auth_to_auth_json() -> Vec<String> {
    migrate_auth_to_auth_json_in(&get_agent_dir())
}

pub(crate) fn migrate_auth_to_auth_json_in(agent_dir: &Path) -> Vec<String> {
    let auth_path = agent_dir.join("auth.json");
    let oauth_path = agent_dir.join("oauth.json");
    let settings_path = agent_dir.join("settings.json");

    // Skip if auth.json already exists.
    if auth_path.exists() {
        return Vec::new();
    }

    let mut migrated: Map<String, Value> = Map::new();
    let mut providers: Vec<String> = Vec::new();

    if oauth_path.exists()
        && let Ok(content) = std::fs::read_to_string(&oauth_path)
        && let Ok(Value::Object(oauth)) = serde_json::from_str::<Value>(&content)
    {
        for (provider, credential) in oauth {
            let mut entry = Map::new();
            entry.insert("type".to_owned(), Value::from("oauth"));
            if let Value::Object(credential) = credential {
                for (key, value) in credential {
                    entry.insert(key, value);
                }
            }
            migrated.insert(provider.clone(), Value::Object(entry));
            providers.push(provider);
        }
        let _ = std::fs::rename(&oauth_path, oauth_path.with_extension("json.migrated"));
    }

    if settings_path.exists()
        && let Ok(content) = std::fs::read_to_string(&settings_path)
        && let Ok(Value::Object(mut settings)) = serde_json::from_str::<Value>(&content)
        && let Some(Value::Object(api_keys)) = settings.get("apiKeys").cloned()
    {
        for (provider, key) in api_keys {
            if !migrated.contains_key(&provider)
                && let Value::String(key) = key
            {
                let mut entry = Map::new();
                entry.insert("type".to_owned(), Value::from("api_key"));
                entry.insert("key".to_owned(), Value::from(key));
                migrated.insert(provider.clone(), Value::Object(entry));
                providers.push(provider);
            }
        }
        settings.remove("apiKeys");
        if let Ok(serialized) = serde_json::to_string_pretty(&Value::Object(settings)) {
            let _ = std::fs::write(&settings_path, serialized);
        }
    }

    if !migrated.is_empty() {
        if let Some(directory) = auth_path.parent() {
            let _ = std::fs::create_dir_all(directory);
        }
        if let Ok(serialized) = serde_json::to_string_pretty(&Value::Object(migrated)) {
            let _ = std::fs::write(&auth_path, serialized);
            set_owner_only(&auth_path);
        }
    }

    providers
}

#[cfg(unix)]
fn set_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) {}

/// Port of the session directory encoding in `session-manager.ts`.
pub(crate) fn encode_cwd_directory(cwd: &str) -> String {
    // TS strips exactly one leading separator (`replace(/^[/\\]/, "")`).
    let trimmed = match cwd.strip_prefix(['/', '\\']) {
        Some(rest) => rest,
        None => cwd,
    };
    let replaced: String = trimmed
        .chars()
        .map(|character| {
            if matches!(character, '/' | '\\' | ':') {
                '-'
            } else {
                character
            }
        })
        .collect();
    format!("--{replaced}--")
}

/// Migrate sessions from `~/.notagent/agent/*.jsonl` into their session directories.
///
/// Bug in v0.30.0: sessions were saved to `~/.notagent/agent/` instead of
/// `~/.notagent/agent/sessions/<encoded-cwd>/`.
pub fn migrate_sessions_from_agent_root() {
    migrate_sessions_from_agent_root_in(&get_agent_dir());
}

pub(crate) fn migrate_sessions_from_agent_root_in(agent_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(agent_dir) else {
        return;
    };
    let files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect();

    for file in files {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Some(first_line) = content.split('\n').next() else {
            continue;
        };
        if first_line.trim().is_empty() {
            continue;
        }
        let Ok(Value::Object(header)) = serde_json::from_str::<Value>(first_line) else {
            continue;
        };
        if header.get("type").and_then(Value::as_str) != Some("session") {
            continue;
        }
        let Some(cwd) = header.get("cwd").and_then(Value::as_str) else {
            continue;
        };

        let correct_dir = agent_dir.join("sessions").join(encode_cwd_directory(cwd));
        if !correct_dir.exists() && std::fs::create_dir_all(&correct_dir).is_err() {
            continue;
        }
        let Some(file_name) = file.file_name() else {
            continue;
        };
        let new_path = correct_dir.join(file_name);
        if new_path.exists() {
            continue;
        }
        let _ = std::fs::rename(&file, &new_path);
    }
}

/// Migrate `commands/` to `prompts/` if needed.
fn migrate_commands_to_prompts(base_dir: &Path, label: &str) -> bool {
    let commands_dir = base_dir.join("commands");
    let prompts_dir = base_dir.join("prompts");
    if commands_dir.exists() && !prompts_dir.exists() {
        match std::fs::rename(&commands_dir, &prompts_dir) {
            Ok(()) => {
                println!("Migrated {label} commands/ → prompts/");
                return true;
            }
            Err(error) => {
                println!("Warning: Could not migrate {label} commands/ to prompts/: {error}");
            }
        }
    }
    false
}

/// Move fd/rg binaries from `tools/` to `bin/` if they exist.
fn migrate_tools_to_bin(agent_dir: &Path, bin_dir: &Path) {
    let tools_dir = agent_dir.join("tools");
    if !tools_dir.exists() {
        return;
    }
    let mut moved_any = false;
    for binary in ["fd", "rg", "fd.exe", "rg.exe"] {
        let old_path = tools_dir.join(binary);
        let new_path = bin_dir.join(binary);
        if !old_path.exists() {
            continue;
        }
        if !bin_dir.exists() {
            let _ = std::fs::create_dir_all(bin_dir);
        }
        if new_path.exists() {
            let _ = std::fs::remove_file(&old_path);
        } else if std::fs::rename(&old_path, &new_path).is_ok() {
            moved_any = true;
        }
    }
    if moved_any {
        println!("Migrated managed binaries tools/ → bin/");
    }
}

pub struct MigrationResult {
    pub migrated_auth_providers: Vec<String>,
}

/// Run all migrations. Called once on startup.
///
/// The keybindings.json migration follows with `core/keybindings.rs` (WS-C task 13).
pub fn run_migrations(cwd: &Path) -> MigrationResult {
    let agent_dir = get_agent_dir();
    let migrated_auth_providers = migrate_auth_to_auth_json_in(&agent_dir);
    migrate_sessions_from_agent_root_in(&agent_dir);
    migrate_tools_to_bin(&agent_dir, &get_bin_dir());
    migrate_commands_to_prompts(&agent_dir, "Global");
    migrate_commands_to_prompts(&cwd.join(crate::config::CONFIG_DIR_NAME), "Project");
    MigrationResult {
        migrated_auth_providers,
    }
}

/// Directory-scoped variants for tests (TS drives them through `getAgentDir()`).
pub mod testing {
    use std::path::Path;

    pub fn migrate_auth_to_auth_json_in(agent_dir: &Path) -> Vec<String> {
        super::migrate_auth_to_auth_json_in(agent_dir)
    }

    pub fn migrate_sessions_from_agent_root_in(agent_dir: &Path) {
        super::migrate_sessions_from_agent_root_in(agent_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_cwd_directories_like_the_session_manager() {
        assert_eq!(
            encode_cwd_directory("/Users/dev/project"),
            "--Users-dev-project--"
        );
        assert_eq!(encode_cwd_directory("C:\\work\\repo"), "--C--work-repo--");
        // Only the first separator is stripped, exactly like the TS regex.
        assert_eq!(encode_cwd_directory("//server/share"), "---server-share--");
    }
}
