use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::config::{CONFIG_DIR_NAME, get_agent_dir};
use crate::core::diagnostics::{ResourceCollision, ResourceDiagnostic, ResourceKind};
use crate::core::source_info::{
    SourceInfo, SourceScope, SyntheticSourceInfoOptions, create_synthetic_source_info,
};
use crate::utils::frontmatter::parse_frontmatter;
use crate::utils::paths::{PathInputOptions, canonicalize_path, current_dir, resolve_path};

/// Max name length per the Agent Skills spec.
const MAX_NAME_LENGTH: usize = 64;

/// Max description length per the Agent Skills spec.
const MAX_DESCRIPTION_LENGTH: usize = 1024;

const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

fn to_posix_path(path: &Path) -> String {
    path.components()
        .map(|component| match component {
            Component::Normal(part) => part.to_string_lossy().into_owned(),
            Component::RootDir => String::new(),
            Component::CurDir => ".".to_string(),
            Component::ParentDir => "..".to_string(),
            Component::Prefix(prefix) => prefix.as_os_str().to_string_lossy().into_owned(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The accumulating matcher.
/// The npm `ignore` package takes patterns one batch at a time and keeps them
/// in order; the `ignore` crate compiles a fixed set. Keeping the ordered
/// pattern list and recompiling on every addition preserves last-match-wins
/// across nested ignore files, which is what the walk below relies on.
#[derive(Default)]
pub struct IgnoreMatcher {
    patterns: Vec<String>,
    compiled: Option<Gitignore>,
}

impl IgnoreMatcher {
    pub fn new() -> Self {
        Self::default()
    }

    fn add(&mut self, patterns: Vec<String>) {
        if patterns.is_empty() {
            return;
        }
        self.patterns.extend(patterns);
        let mut builder = GitignoreBuilder::new("");
        for pattern in &self.patterns {
            let _ = builder.add_line(None, pattern);
        }
        self.compiled = builder.build().ok();
    }

    fn ignores(&self, relative_path: &str) -> bool {
        let Some(compiled) = self.compiled.as_ref() else {
            return false;
        };
        let is_dir = relative_path.ends_with('/');
        let trimmed = relative_path.trim_end_matches('/');
        compiled.matched(Path::new(trimmed), is_dir).is_ignore()
    }
}

/// Rewrites one ignore-file line so it applies from the walk root rather than
/// from the directory the file sits in.
fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && !trimmed.starts_with("\\#") {
        return None;
    }

    let mut pattern = line.to_string();
    let mut negated = false;

    if let Some(rest) = pattern.strip_prefix('!') {
        negated = true;
        pattern = rest.to_string();
    } else if let Some(rest) = pattern.strip_prefix("\\!") {
        pattern = rest.to_string();
    }

    if let Some(rest) = pattern.strip_prefix('/') {
        pattern = rest.to_string();
    }

    let prefixed = if prefix.is_empty() {
        pattern
    } else {
        format!("{prefix}{pattern}")
    };
    Some(if negated {
        format!("!{prefixed}")
    } else {
        prefixed
    })
}

fn add_ignore_rules(matcher: &mut IgnoreMatcher, dir: &Path, root_dir: &Path) {
    let relative_dir = dir.strip_prefix(root_dir).unwrap_or(Path::new(""));
    let relative = to_posix_path(relative_dir);
    let prefix = if relative.is_empty() {
        String::new()
    } else {
        format!("{relative}/")
    };

    for filename in IGNORE_FILE_NAMES {
        let ignore_path = dir.join(filename);
        if !ignore_path.exists() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&ignore_path) else {
            continue;
        };
        let patterns: Vec<String> = content
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .filter_map(|line| prefix_ignore_pattern(line, &prefix))
            .collect();
        matcher.add(patterns);
    }
}

/// One loaded skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub base_dir: String,
    pub source_info: SourceInfo,
    pub disable_model_invocation: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadSkillsResult {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

/// Validates a skill name per the Agent Skills spec.
fn validate_name(name: &str) -> Vec<String> {
    let mut errors = Vec::new();

    // `String.length` counts UTF-16 code units.
    let length = name.encode_utf16().count();
    if length > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {MAX_NAME_LENGTH} characters ({length})"
        ));
    }

    let valid = !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        });
    if !valid {
        errors.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
                .to_string(),
        );
    }

    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".to_string());
    }

    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".to_string());
    }

    errors
}

/// Validates a description per the Agent Skills spec.
fn validate_description(description: Option<&str>) -> Vec<String> {
    let mut errors = Vec::new();
    match description {
        None => errors.push("description is required".to_string()),
        Some(description) if description.trim().is_empty() => {
            errors.push("description is required".to_string());
        }
        Some(description) => {
            let length = description.encode_utf16().count();
            if length > MAX_DESCRIPTION_LENGTH {
                errors.push(format!(
                    "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({length})"
                ));
            }
        }
    }
    errors
}

fn create_skill_source_info(file_path: &str, base_dir: &str, source: &str) -> SourceInfo {
    let options = match source {
        "user" => SyntheticSourceInfoOptions {
            source: "local".to_string(),
            scope: Some(SourceScope::User),
            origin: None,
            base_dir: Some(base_dir.to_string()),
        },
        "project" => SyntheticSourceInfoOptions {
            source: "local".to_string(),
            scope: Some(SourceScope::Project),
            origin: None,
            base_dir: Some(base_dir.to_string()),
        },
        "path" => SyntheticSourceInfoOptions {
            source: "local".to_string(),
            scope: None,
            origin: None,
            base_dir: Some(base_dir.to_string()),
        },
        other => SyntheticSourceInfoOptions {
            source: other.to_string(),
            scope: None,
            origin: None,
            base_dir: Some(base_dir.to_string()),
        },
    };
    create_synthetic_source_info(file_path, options)
}

/// Loads skills from a directory.
/// Discovery rules:
/// - a directory containing `SKILL.md` is a skill root; the walk does not descend further
/// - otherwise direct `.md` children of the root are loaded
/// - subdirectories are recursed into looking for `SKILL.md`
pub fn load_skills_from_dir(dir: &str, source: &str) -> LoadSkillsResult {
    let mut matcher = IgnoreMatcher::new();
    load_skills_from_dir_internal(Path::new(dir), source, true, &mut matcher, Path::new(dir))
}

fn load_skills_from_dir_internal(
    dir: &Path,
    source: &str,
    include_root_files: bool,
    matcher: &mut IgnoreMatcher,
    root: &Path,
) -> LoadSkillsResult {
    let mut skills: Vec<Skill> = Vec::new();
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    if !dir.exists() {
        return LoadSkillsResult {
            skills,
            diagnostics,
        };
    }

    add_ignore_rules(matcher, dir, root);

    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return LoadSkillsResult {
            skills,
            diagnostics,
        };
    };
    let mut entries: Vec<(String, std::fs::DirEntry)> = Vec::new();
    for entry in read_dir.flatten() {
        entries.push((entry.file_name().to_string_lossy().into_owned(), entry));
    }
    // `readdirSync` returns directory order; Rust's `read_dir` does too. The
    // walk below is order-independent apart from the `SKILL.md` short circuit,
    // which scans the whole listing first.
    entries.sort_by(|left, right| left.0.cmp(&right.0));

    for (name, entry) in &entries {
        if name != "SKILL.md" {
            continue;
        }

        let full_path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let mut is_file = file_type.is_file();
        if file_type.is_symlink() {
            match std::fs::metadata(&full_path) {
                Ok(stats) => is_file = stats.is_file(),
                Err(_) => continue,
            }
        }

        let rel_path = to_posix_path(full_path.strip_prefix(root).unwrap_or(&full_path));
        if !is_file || matcher.ignores(&rel_path) {
            continue;
        }

        let (skill, file_diagnostics) = load_skill_from_file(&full_path, source);
        if let Some(skill) = skill {
            skills.push(skill);
        }
        diagnostics.extend(file_diagnostics);
        return LoadSkillsResult {
            skills,
            diagnostics,
        };
    }

    for (name, entry) in &entries {
        if name.starts_with('.') {
            continue;
        }

        // Dependencies are not a place to look for skills.
        if name == "node_modules" {
            continue;
        }

        let full_path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let mut is_directory = file_type.is_dir();
        let mut is_file = file_type.is_file();
        if file_type.is_symlink() {
            match std::fs::metadata(&full_path) {
                Ok(stats) => {
                    is_directory = stats.is_dir();
                    is_file = stats.is_file();
                }
                // Broken symlink, skip it.
                Err(_) => continue,
            }
        }

        let rel_path = to_posix_path(full_path.strip_prefix(root).unwrap_or(&full_path));
        let ignore_path = if is_directory {
            format!("{rel_path}/")
        } else {
            rel_path
        };
        if matcher.ignores(&ignore_path) {
            continue;
        }

        if is_directory {
            let sub = load_skills_from_dir_internal(&full_path, source, false, matcher, root);
            skills.extend(sub.skills);
            diagnostics.extend(sub.diagnostics);
            continue;
        }

        if !is_file || !include_root_files || !name.ends_with(".md") {
            continue;
        }

        let (skill, file_diagnostics) = load_skill_from_file(&full_path, source);
        if let Some(skill) = skill {
            skills.push(skill);
        }
        diagnostics.extend(file_diagnostics);
    }

    LoadSkillsResult {
        skills,
        diagnostics,
    }
}

fn load_skill_from_file(
    file_path: &Path,
    source: &str,
) -> (Option<Skill>, Vec<ResourceDiagnostic>) {
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();
    let path_text = file_path.to_string_lossy().into_owned();

    let raw_content = match std::fs::read_to_string(file_path) {
        Ok(content) => content,
        Err(error) => {
            diagnostics.push(ResourceDiagnostic::warning(error.to_string(), &path_text));
            return (None, diagnostics);
        }
    };

    let frontmatter = match parse_frontmatter(&raw_content) {
        Ok(parsed) => parsed,
        Err(error) => {
            diagnostics.push(ResourceDiagnostic::warning(error.to_string(), &path_text));
            return (None, diagnostics);
        }
    };

    let skill_dir = file_path
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent_dir_name = file_path
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let description = frontmatter
        .get_str("description")
        .map(|text| text.to_string());

    for error in validate_description(description.as_deref()) {
        diagnostics.push(ResourceDiagnostic::warning(error, &path_text));
    }

    // Frontmatter name wins; the parent directory name is the fallback.
    let name = match frontmatter.get_str("name") {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => parent_dir_name,
    };

    for error in validate_name(&name) {
        diagnostics.push(ResourceDiagnostic::warning(error, &path_text));
    }

    // Warnings still load the skill; a missing description does not.
    let description = match description {
        Some(description) if !description.trim().is_empty() => description,
        _ => return (None, diagnostics),
    };

    let disable_model_invocation = frontmatter
        .get("disable-model-invocation")
        .and_then(|value| value.as_bool())
        == Some(true);

    (
        Some(Skill {
            name,
            description,
            file_path: path_text.clone(),
            base_dir: skill_dir.clone(),
            source_info: create_skill_source_info(&path_text, &skill_dir, source),
            disable_model_invocation,
        }),
        diagnostics,
    )
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Formats skills for inclusion in a system prompt, in the XML shape the Agent
/// Skills standard describes (<https://agentskills.io/integrate-skills>).
/// Skills marked `disable-model-invocation` are left out: they exist to be
/// invoked explicitly with `/skill:name`, and listing them would invite the
/// model to load them anyway.
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<&Skill> = skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
        .collect();

    if visible.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "\n\nThe following skills provide specialized instructions for specific tasks.".to_string(),
        "Use the skill tool with the skill's name to load its instructions when the task matches its description. Do not load skill instructions with the read tool."
            .to_string(),
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands."
            .to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];

    for skill in visible {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.file_path)
        ));
        lines.push("  </skill>".to_string());
    }

    lines.push("</available_skills>".to_string());

    lines.join("\n")
}

/// Inputs of [`load_skills`].
#[derive(Debug, Clone, Default)]
pub struct LoadSkillsOptions {
    /// Working directory for project-local skills.
    pub cwd: String,
    /// Agent config directory for global skills.
    pub agent_dir: Option<String>,
    /// Explicit skill paths (files or directories).
    pub skill_paths: Vec<String>,
    /// Whether the two default skill directories are included.
    pub include_defaults: bool,
}

fn is_under_path(target: &str, root: &str) -> bool {
    let normalized_root = resolve_path(root, &current_dir(), &PathInputOptions::default())
        .unwrap_or_else(|_| root.to_string());
    if target == normalized_root {
        return true;
    }
    let separator = std::path::MAIN_SEPARATOR;
    let prefix = if normalized_root.ends_with(separator) {
        normalized_root
    } else {
        format!("{normalized_root}{separator}")
    };
    target.starts_with(&prefix)
}

/// Loads skills from every configured location, plus the diagnostics collected
/// on the way.
/// Precedence is first-wins by name: the user directory is walked before the
/// project directory, and explicit paths come last. A file reached twice
/// through a symlink is skipped silently rather than reported as a collision —
/// it is the same skill, not two.
pub fn load_skills(options: &LoadSkillsOptions) -> LoadSkillsResult {
    let resolved_cwd = resolve_path(&options.cwd, &current_dir(), &PathInputOptions::default())
        .unwrap_or_else(|_| options.cwd.clone());
    let agent_dir = options
        .agent_dir
        .clone()
        .unwrap_or_else(|| get_agent_dir().to_string_lossy().into_owned());
    let resolved_agent_dir = resolve_path(&agent_dir, &current_dir(), &PathInputOptions::default())
        .unwrap_or_else(|_| agent_dir.clone());

    let mut skill_map: Vec<(String, Skill)> = Vec::new();
    let mut skill_names: HashMap<String, usize> = HashMap::new();
    let mut real_path_set: HashSet<String> = HashSet::new();
    let mut all_diagnostics: Vec<ResourceDiagnostic> = Vec::new();
    let mut collision_diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    let add_skills = |result: LoadSkillsResult,
                      skill_map: &mut Vec<(String, Skill)>,
                      skill_names: &mut HashMap<String, usize>,
                      real_path_set: &mut HashSet<String>,
                      all_diagnostics: &mut Vec<ResourceDiagnostic>,
                      collision_diagnostics: &mut Vec<ResourceDiagnostic>| {
        all_diagnostics.extend(result.diagnostics);
        for skill in result.skills {
            // Symlinks are resolved so the same file found twice is one skill.
            let real_path = canonicalize_path(&skill.file_path);

            if real_path_set.contains(&real_path) {
                continue;
            }

            match skill_names.get(&skill.name) {
                Some(&index) => {
                    let existing: &Skill = &skill_map[index].1;
                    collision_diagnostics.push(ResourceDiagnostic::collision(
                        format!("name \"{}\" collision", skill.name),
                        &skill.file_path,
                        ResourceCollision {
                            resource_type: ResourceKind::Skill,
                            name: skill.name.clone(),
                            winner_path: existing.file_path.clone(),
                            loser_path: skill.file_path.clone(),
                            winner_source: None,
                            loser_source: None,
                        },
                    ));
                }
                None => {
                    skill_names.insert(skill.name.clone(), skill_map.len());
                    skill_map.push((skill.name.clone(), skill));
                    real_path_set.insert(real_path);
                }
            }
        }
    };

    let user_skills_dir = Path::new(&resolved_agent_dir)
        .join("skills")
        .to_string_lossy()
        .into_owned();
    let project_skills_dir = Path::new(&resolved_cwd)
        .join(CONFIG_DIR_NAME)
        .join("skills")
        .to_string_lossy()
        .into_owned();

    if options.include_defaults {
        let mut matcher = IgnoreMatcher::new();
        let user = load_skills_from_dir_internal(
            Path::new(&user_skills_dir),
            "user",
            true,
            &mut matcher,
            Path::new(&user_skills_dir),
        );
        add_skills(
            user,
            &mut skill_map,
            &mut skill_names,
            &mut real_path_set,
            &mut all_diagnostics,
            &mut collision_diagnostics,
        );

        let mut matcher = IgnoreMatcher::new();
        let project = load_skills_from_dir_internal(
            Path::new(&project_skills_dir),
            "project",
            true,
            &mut matcher,
            Path::new(&project_skills_dir),
        );
        add_skills(
            project,
            &mut skill_map,
            &mut skill_names,
            &mut real_path_set,
            &mut all_diagnostics,
            &mut collision_diagnostics,
        );
    }

    let include_defaults = options.include_defaults;
    let get_source = |resolved_path: &str| -> &'static str {
        if !include_defaults {
            if is_under_path(resolved_path, &user_skills_dir) {
                return "user";
            }
            if is_under_path(resolved_path, &project_skills_dir) {
                return "project";
            }
        }
        "path"
    };

    for raw_path in &options.skill_paths {
        let resolved_path = match resolve_path(
            raw_path,
            &resolved_cwd,
            &PathInputOptions {
                trim: true,
                ..PathInputOptions::default()
            },
        ) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !Path::new(&resolved_path).exists() {
            all_diagnostics.push(ResourceDiagnostic::warning(
                "skill path does not exist",
                &resolved_path,
            ));
            continue;
        }

        let stats = match std::fs::metadata(&resolved_path) {
            Ok(stats) => stats,
            Err(error) => {
                all_diagnostics.push(ResourceDiagnostic::warning(
                    error.to_string(),
                    &resolved_path,
                ));
                continue;
            }
        };
        let source = get_source(&resolved_path);
        if stats.is_dir() {
            let mut matcher = IgnoreMatcher::new();
            let result = load_skills_from_dir_internal(
                Path::new(&resolved_path),
                source,
                true,
                &mut matcher,
                Path::new(&resolved_path),
            );
            add_skills(
                result,
                &mut skill_map,
                &mut skill_names,
                &mut real_path_set,
                &mut all_diagnostics,
                &mut collision_diagnostics,
            );
        } else if stats.is_file() && resolved_path.ends_with(".md") {
            let (skill, diagnostics) = load_skill_from_file(Path::new(&resolved_path), source);
            match skill {
                Some(skill) => add_skills(
                    LoadSkillsResult {
                        skills: vec![skill],
                        diagnostics,
                    },
                    &mut skill_map,
                    &mut skill_names,
                    &mut real_path_set,
                    &mut all_diagnostics,
                    &mut collision_diagnostics,
                ),
                None => all_diagnostics.extend(diagnostics),
            }
        } else {
            all_diagnostics.push(ResourceDiagnostic::warning(
                "skill path is not a markdown file",
                &resolved_path,
            ));
        }
    }

    all_diagnostics.extend(collision_diagnostics);
    LoadSkillsResult {
        skills: skill_map.into_iter().map(|(_, skill)| skill).collect(),
        diagnostics: all_diagnostics,
    }
}

/// The skill directories the defaults cover, for callers that need to name
/// them without loading anything.
pub fn default_skill_dirs(cwd: &str, agent_dir: &str) -> (PathBuf, PathBuf) {
    (
        Path::new(agent_dir).join("skills"),
        Path::new(cwd).join(CONFIG_DIR_NAME).join("skills"),
    )
}

/// The skills shipped with the binary, written into the user skills directory
/// on startup when absent. Once on disk they are ordinary user skills: the
/// loader, the tools, and the user's editor all see plain files, and the
/// regular name precedence applies.
pub const BUILTIN_SKILLS: &[(&str, &str)] = &[
    (
        "create-plan",
        include_str!("skills/builtin/create-plan/SKILL.md"),
    ),
    (
        "execute-plan",
        include_str!("skills/builtin/execute-plan/SKILL.md"),
    ),
    ("explore", include_str!("skills/builtin/explore/SKILL.md")),
    ("debug", include_str!("skills/builtin/debug/SKILL.md")),
];

/// Writes each shipped skill to `<agent_dir>/skills/<name>/SKILL.md` unless
/// that file already exists. An existing file is never touched, whatever its
/// content: the user's edit is the override mechanism, and deleting the file
/// restores the shipped version on the next start. Write failures become
/// warnings rather than errors — a read-only home directory must not stop the
/// session, only cost it the shipped skills.
pub fn materialize_builtin_skills(agent_dir: &str) -> Vec<ResourceDiagnostic> {
    let mut diagnostics = Vec::new();
    for (name, content) in BUILTIN_SKILLS {
        let dir = Path::new(agent_dir).join("skills").join(name);
        let file = dir.join("SKILL.md");
        if file.exists() {
            continue;
        }
        let path_text = file.to_string_lossy().into_owned();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            diagnostics.push(ResourceDiagnostic::warning(
                format!("cannot create built-in skill directory: {error}"),
                &path_text,
            ));
            continue;
        }
        if let Err(error) = std::fs::write(&file, content) {
            diagnostics.push(ResourceDiagnostic::warning(
                format!("cannot write built-in skill: {error}"),
                &path_text,
            ));
        }
    }
    diagnostics
}
