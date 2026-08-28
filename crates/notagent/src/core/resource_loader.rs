use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notagent_agent::types::BoxFuture;

use crate::config::CONFIG_DIR_NAME;
use crate::core::diagnostics::ResourceDiagnostic;
use crate::core::footer_data_provider::find_git_paths;
use crate::core::prompt_templates::{
    LoadPromptTemplatesOptions, PromptTemplate, load_prompt_templates,
};
use crate::core::settings_manager::{SettingsManager, SettingsManagerCreateOptions};
use crate::core::skills::{LoadSkillsOptions, LoadSkillsResult, Skill, load_skills};
use crate::core::source_info::{
    PathMetadata, SourceInfo, SourceOrigin, SourceScope, create_source_info,
};
use crate::modes::interactive::theme::theme::{Theme, load_theme_from_path};
use crate::utils::paths::{
    PathInputOptions, canonicalize_path, current_dir, is_local_path, resolve_path,
    resolve_path_default,
};

// ============================================================================
// Installed packages
// ============================================================================

/// One path an installed package contributes, plus whether it is switched on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedResource {
    pub path: String,
    pub metadata: PathMetadata,
    pub enabled: bool,
}

/// What the package manager resolved, by resource type. The `extensions` list
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedResources {
    pub skills: Vec<ResolvedResource>,
    pub prompts: Vec<ResolvedResource>,
    pub themes: Vec<ResolvedResource>,
}

/// The installed-package half of resource discovery.
/// the paths the caller named.
pub trait PackageResources: Send + Sync {
    fn resolve(&self) -> BoxFuture<'_, ResolvedResources>;
}

// ============================================================================
// Context files
// ============================================================================

/// The names a directory's context file may have, in the order they win.
const CONTEXT_FILE_CANDIDATES: [&str; 5] = [
    "AGENTS.override.md",
    "AGENTS.md",
    "AGENTS.MD",
    "CLAUDE.md",
    "CLAUDE.MD",
];

/// One loaded context file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    pub path: String,
    pub content: String,
}

fn resolve_prompt_input(input: Option<&str>) -> Option<String> {
    let input = input?;
    if Path::new(input).exists() {
        return match std::fs::read_to_string(input) {
            Ok(content) => Some(content),
            // A file that exists but cannot be read falls back to the literal
            Err(_) => Some(input.to_string()),
        };
    }
    Some(input.to_string())
}

fn load_context_file_from_dir(dir: &Path) -> Option<ContextFile> {
    for filename in CONTEXT_FILE_CANDIDATES {
        let file_path = dir.join(filename);
        if !file_path.exists() {
            continue;
        }
        match std::fs::metadata(&file_path) {
            Ok(stats) if !stats.is_file() => continue,
            Err(_) => continue,
            Ok(_) => {}
        }
        if let Ok(content) = std::fs::read_to_string(&file_path) {
            return Some(ContextFile {
                path: file_path.to_string_lossy().into_owned(),
                content,
            });
        }
    }
    None
}

/// The main repository's context file that a nested linked worktree's own copy
/// shadows.
/// Both occupy the same logical repository scope, so loading both would apply
/// that context twice. Returns `None` when nothing is shadowed, leaving normal
/// ancestor inheritance alone.
/// The result is canonicalized, because `git worktree add` writes the `.git`
/// file's `gitdir:` target in realpath form while the working directory may
/// still be reached through a symlink (macOS `/tmp` → `/private/tmp`).
fn find_shadowed_context_file(cwd: &str) -> Option<String> {
    let git_paths = find_git_paths(cwd)?;
    let common_git_dir = canonicalize_path(&git_paths.common_git_dir);
    let worktree_root = canonicalize_path(&git_paths.repo_dir);
    let main_repo_root = Path::new(&common_git_dir).parent()?.to_path_buf();
    let main_repo_root_text = main_repo_root.to_string_lossy().into_owned();
    // False for an ordinary repository, where the two are the same directory,
    // and for a sibling worktree, whose main repository is not an ancestor.
    if !worktree_root.starts_with(&format!(
        "{main_repo_root_text}{}",
        std::path::MAIN_SEPARATOR
    )) {
        return None;
    }
    // The parent of the common git directory is the main worktree root only
    // when that directory is itself checked out from the same repository. In a
    // bare layout (`proj/.bare` + `proj/main`) it is just the directory holding
    // `.bare`, which tracks nothing; a submodule's git dir has no `commondir`
    // and lands under `.git/modules`.
    if canonicalize_path(&main_repo_root.join(".git").to_string_lossy()) != common_git_dir {
        return None;
    }
    let worktree_context_file = load_context_file_from_dir(Path::new(&worktree_root))?;
    let basename = Path::new(&worktree_context_file.path)
        .file_name()?
        .to_string_lossy()
        .into_owned();
    Some(main_repo_root.join(basename).to_string_lossy().into_owned())
}

/// The context files a session starts with: the agent directory's, then every
/// ancestor of `cwd` from the root down, so the nearest one is last.
pub fn load_project_context_files(cwd: &str, agent_dir: &str) -> Vec<ContextFile> {
    let resolved_cwd =
        resolve_path_default(cwd, &current_dir()).unwrap_or_else(|_| cwd.to_string());
    let resolved_agent_dir =
        resolve_path_default(agent_dir, &current_dir()).unwrap_or_else(|_| agent_dir.to_string());

    let mut context_files: Vec<ContextFile> = Vec::new();
    let mut seen_paths: Vec<String> = Vec::new();

    if let Some(global) = load_context_file_from_dir(Path::new(&resolved_agent_dir)) {
        seen_paths.push(global.path.clone());
        context_files.push(global);
    }

    let mut ancestor_context_files: Vec<ContextFile> = Vec::new();
    let shadowed = find_shadowed_context_file(&resolved_cwd);
    let mut current = PathBuf::from(&resolved_cwd);

    loop {
        let context_file = load_context_file_from_dir(&current);
        let is_shadowed = match (&shadowed, &context_file) {
            (Some(shadowed), file) => {
                canonicalize_path(file.as_ref().map(|file| file.path.as_str()).unwrap_or(""))
                    == *shadowed
            }
            (None, _) => false,
        };
        if let Some(file) = context_file
            && !is_shadowed
            && !seen_paths.contains(&file.path)
        {
            seen_paths.push(file.path.clone());
            ancestor_context_files.insert(0, file);
        }

        let Some(parent) = current.parent().map(Path::to_path_buf) else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent;
    }

    context_files.extend(ancestor_context_files);
    context_files
}

// ============================================================================
// Loader
// ============================================================================

/// Paths handed to the loader after it was built.
#[derive(Debug, Clone, Default)]
pub struct ResourceExtensionPaths {
    pub skill_paths: Vec<(String, PathMetadata)>,
    pub prompt_paths: Vec<(String, PathMetadata)>,
    pub theme_paths: Vec<(String, PathMetadata)>,
}

/// Asked once per reload, before settings are read for real: is this project
/// trusted?
pub type ResolveProjectTrust = Arc<dyn Fn() -> BoxFuture<'static, bool> + Send + Sync>;

#[derive(Default, Clone)]
pub struct ResourceLoaderReloadOptions {
    pub resolve_project_trust: Option<ResolveProjectTrust>,
}

/// What a session reads off the loader.
pub trait ResourceLoader: Send + Sync {
    fn get_skills(&self) -> (Vec<Skill>, Vec<ResourceDiagnostic>);
    fn get_prompts(&self) -> (Vec<PromptTemplate>, Vec<ResourceDiagnostic>);
    fn get_themes(&self) -> (Vec<Theme>, Vec<ResourceDiagnostic>);
    fn get_agents_files(&self) -> Vec<ContextFile>;
    fn get_system_prompt(&self) -> Option<String>;
    fn get_system_prompt_source(&self) -> Option<String>;
    fn get_append_system_prompt(&self) -> Vec<String>;
    fn get_append_system_prompt_sources(&self) -> Vec<String>;
    fn extend_resources(&self, paths: ResourceExtensionPaths);
    fn reload<'a>(&'a self, options: ResourceLoaderReloadOptions) -> BoxFuture<'a, ()>;
}

type SkillsOverride = Arc<dyn Fn(LoadSkillsResult) -> LoadSkillsResult + Send + Sync>;
type PromptsOverride = Arc<
    dyn Fn(
            (Vec<PromptTemplate>, Vec<ResourceDiagnostic>),
        ) -> (Vec<PromptTemplate>, Vec<ResourceDiagnostic>)
        + Send
        + Sync,
>;
type ThemesOverride = Arc<
    dyn Fn((Vec<Theme>, Vec<ResourceDiagnostic>)) -> (Vec<Theme>, Vec<ResourceDiagnostic>)
        + Send
        + Sync,
>;
type AgentsFilesOverride = Arc<dyn Fn(Vec<ContextFile>) -> Vec<ContextFile> + Send + Sync>;
type SystemPromptOverride = Arc<dyn Fn(Option<String>) -> Option<String> + Send + Sync>;
type AppendSystemPromptOverride = Arc<dyn Fn(Vec<String>) -> Vec<String> + Send + Sync>;

/// Inputs of [`DefaultResourceLoader::new`].
#[derive(Default, Clone)]
pub struct DefaultResourceLoaderOptions {
    pub cwd: String,
    pub agent_dir: String,
    pub settings_manager: Option<Arc<SettingsManager>>,
    pub packages: Option<Arc<dyn PackageResources>>,
    pub additional_skill_paths: Vec<String>,
    pub additional_prompt_template_paths: Vec<String>,
    pub additional_theme_paths: Vec<String>,
    pub no_skills: bool,
    pub no_prompt_templates: bool,
    pub no_themes: bool,
    pub no_context_files: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<Vec<String>>,
    pub skills_override: Option<SkillsOverride>,
    pub prompts_override: Option<PromptsOverride>,
    pub themes_override: Option<ThemesOverride>,
    pub agents_files_override: Option<AgentsFilesOverride>,
    pub system_prompt_override: Option<SystemPromptOverride>,
    pub append_system_prompt_override: Option<AppendSystemPromptOverride>,
}

#[derive(Default)]
struct LoadedState {
    skills: Vec<Skill>,
    skill_diagnostics: Vec<ResourceDiagnostic>,
    prompts: Vec<PromptTemplate>,
    prompt_diagnostics: Vec<ResourceDiagnostic>,
    themes: Vec<Theme>,
    theme_diagnostics: Vec<ResourceDiagnostic>,
    agents_files: Vec<ContextFile>,
    system_prompt: Option<String>,
    system_prompt_source_path: Option<String>,
    append_system_prompt: Vec<String>,
    append_system_prompt_source_paths: Vec<String>,
    last_skill_paths: Vec<String>,
    last_prompt_paths: Vec<String>,
    last_theme_paths: Vec<String>,
    extension_skill_source_infos: Vec<(String, SourceInfo)>,
    extension_prompt_source_infos: Vec<(String, SourceInfo)>,
    extension_theme_source_infos: Vec<(String, SourceInfo)>,
    resource_metadata_by_path: HashMap<String, PathMetadata>,
}

pub struct DefaultResourceLoader {
    cwd: String,
    agent_dir: String,
    settings_manager: Arc<SettingsManager>,
    packages: Option<Arc<dyn PackageResources>>,
    options: DefaultResourceLoaderOptions,
    state: std::sync::Mutex<LoadedState>,
}

impl DefaultResourceLoader {
    pub fn new(options: DefaultResourceLoaderOptions) -> Self {
        let cwd = resolve_path_default(&options.cwd, &current_dir())
            .unwrap_or_else(|_| options.cwd.clone());
        let agent_dir = resolve_path_default(&options.agent_dir, &current_dir())
            .unwrap_or_else(|_| options.agent_dir.clone());
        let settings_manager = options.settings_manager.clone().unwrap_or_else(|| {
            Arc::new(SettingsManager::create(
                Path::new(&cwd),
                Some(Path::new(&agent_dir)),
                SettingsManagerCreateOptions::default(),
            ))
        });
        let packages = options.packages.clone();
        Self {
            cwd,
            agent_dir,
            settings_manager,
            packages,
            options,
            state: std::sync::Mutex::new(LoadedState::default()),
        }
    }

    pub fn settings_manager(&self) -> Arc<SettingsManager> {
        Arc::clone(&self.settings_manager)
    }

    fn resolve_resource_path(&self, path: &str) -> String {
        resolve_path(
            path,
            &self.cwd,
            &PathInputOptions {
                trim: true,
                ..PathInputOptions::default()
            },
        )
        .unwrap_or_else(|_| path.to_string())
    }

    /// Resolves and de-duplicates by canonical path, keeping the first spelling
    /// of each file so precedence is stable across reloads.
    fn merge_paths(&self, primary: &[String], additional: &[String]) -> Vec<String> {
        let mut merged: Vec<String> = Vec::new();
        let mut seen: Vec<String> = Vec::new();

        for path in primary.iter().chain(additional.iter()) {
            let resolved = self.resolve_resource_path(path);
            let canonical = canonicalize_path(&resolved);
            if seen.contains(&canonical) {
                continue;
            }
            seen.push(canonical);
            merged.push(resolved);
        }

        merged
    }

    fn is_under_path(&self, target: &str, root: &str) -> bool {
        let normalized_root =
            resolve_path_default(root, &current_dir()).unwrap_or_else(|_| root.to_string());
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

    fn default_source_info_for_path(&self, file_path: &str) -> SourceInfo {
        if file_path.starts_with('<') && file_path.ends_with('>') {
            let inner = &file_path[1..file_path.len() - 1];
            let source = inner.split(':').next().unwrap_or("");
            return SourceInfo {
                path: file_path.to_string(),
                source: if source.is_empty() {
                    "temporary".to_string()
                } else {
                    source.to_string()
                },
                scope: SourceScope::Temporary,
                origin: SourceOrigin::TopLevel,
                base_dir: None,
            };
        }

        let normalized_path = resolve_path_default(file_path, &current_dir())
            .unwrap_or_else(|_| file_path.to_string());
        let roots = ["skills", "prompts", "themes", "extensions"];
        for root in roots {
            let candidate = Path::new(&self.agent_dir).join(root);
            let candidate = candidate.to_string_lossy();
            if self.is_under_path(&normalized_path, &candidate) {
                return SourceInfo {
                    path: file_path.to_string(),
                    source: "local".to_string(),
                    scope: SourceScope::User,
                    origin: SourceOrigin::TopLevel,
                    base_dir: Some(candidate.into_owned()),
                };
            }
        }
        for root in roots {
            let candidate = Path::new(&self.cwd).join(CONFIG_DIR_NAME).join(root);
            let candidate = candidate.to_string_lossy();
            if self.is_under_path(&normalized_path, &candidate) {
                return SourceInfo {
                    path: file_path.to_string(),
                    source: "local".to_string(),
                    scope: SourceScope::Project,
                    origin: SourceOrigin::TopLevel,
                    base_dir: Some(candidate.into_owned()),
                };
            }
        }

        let base_dir = match std::fs::metadata(&normalized_path) {
            Ok(stats) if stats.is_dir() => normalized_path.clone(),
            _ => Path::new(&normalized_path)
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_else(|| normalized_path.clone()),
        };
        SourceInfo {
            path: file_path.to_string(),
            source: "local".to_string(),
            scope: SourceScope::Temporary,
            origin: SourceOrigin::TopLevel,
            base_dir: Some(base_dir),
        }
    }

    fn find_source_info_for_path(
        &self,
        resource_path: &str,
        extra: Option<&[(String, SourceInfo)]>,
        metadata_by_path: Option<&HashMap<String, PathMetadata>>,
    ) -> Option<SourceInfo> {
        if resource_path.is_empty() {
            return None;
        }
        if resource_path.starts_with('<') {
            return Some(self.default_source_info_for_path(resource_path));
        }

        let normalized = resolve_path_default(resource_path, &current_dir())
            .unwrap_or_else(|_| resource_path.to_string());
        let separator = std::path::MAIN_SEPARATOR;

        if let Some(extra) = extra {
            for (source_path, source_info) in extra {
                let normalized_source = resolve_path_default(source_path, &current_dir())
                    .unwrap_or_else(|_| source_path.clone());
                if normalized == normalized_source
                    || normalized.starts_with(&format!("{normalized_source}{separator}"))
                {
                    let mut cloned = source_info.clone();
                    cloned.path = resource_path.to_string();
                    return Some(cloned);
                }
            }
        }

        if let Some(metadata_by_path) = metadata_by_path {
            if let Some(exact) = metadata_by_path
                .get(&normalized)
                .or_else(|| metadata_by_path.get(resource_path))
            {
                return Some(create_source_info(resource_path, exact));
            }

            for (source_path, metadata) in metadata_by_path {
                let normalized_source = resolve_path_default(source_path, &current_dir())
                    .unwrap_or_else(|_| source_path.clone());
                if normalized == normalized_source
                    || normalized.starts_with(&format!("{normalized_source}{separator}"))
                {
                    return Some(create_source_info(resource_path, metadata));
                }
            }
        }

        None
    }

    fn update_skills_from_paths(&self, state: &mut LoadedState, skill_paths: &[String]) {
        let result = if self.options.no_skills && skill_paths.is_empty() {
            LoadSkillsResult::default()
        } else {
            load_skills(&LoadSkillsOptions {
                cwd: self.cwd.clone(),
                agent_dir: Some(self.agent_dir.clone()),
                skill_paths: skill_paths.to_vec(),
                include_defaults: false,
            })
        };
        let result = match self.options.skills_override.as_ref() {
            Some(over) => over(result),
            None => result,
        };
        state.skills = result
            .skills
            .into_iter()
            .map(|mut skill| {
                if let Some(source_info) = self.find_source_info_for_path(
                    &skill.file_path,
                    Some(&state.extension_skill_source_infos),
                    Some(&state.resource_metadata_by_path),
                ) {
                    skill.source_info = source_info;
                }
                skill
            })
            .collect();
        state.skill_diagnostics = result.diagnostics;
    }

    fn update_prompts_from_paths(&self, state: &mut LoadedState, prompt_paths: &[String]) {
        let result = if self.options.no_prompt_templates && prompt_paths.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let all = load_prompt_templates(&LoadPromptTemplatesOptions {
                cwd: self.cwd.clone(),
                agent_dir: self.agent_dir.clone(),
                prompt_paths: prompt_paths.to_vec(),
                include_defaults: false,
            });
            dedupe_prompts(all)
        };
        let (prompts, diagnostics) = match self.options.prompts_override.as_ref() {
            Some(over) => over(result),
            None => result,
        };
        state.prompts = prompts
            .into_iter()
            .map(|mut prompt| {
                if let Some(source_info) = self.find_source_info_for_path(
                    &prompt.file_path,
                    Some(&state.extension_prompt_source_infos),
                    Some(&state.resource_metadata_by_path),
                ) {
                    prompt.source_info = source_info;
                }
                prompt
            })
            .collect();
        state.prompt_diagnostics = diagnostics;
    }

    fn update_themes_from_paths(&self, state: &mut LoadedState, theme_paths: &[String]) {
        let result = if self.options.no_themes && theme_paths.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let (themes, mut diagnostics) = self.load_themes(theme_paths);
            let (deduped, collisions) = dedupe_themes(themes);
            diagnostics.extend(collisions);
            (deduped, diagnostics)
        };
        let (themes, diagnostics) = match self.options.themes_override.as_ref() {
            Some(over) => over(result),
            None => result,
        };
        state.themes = themes
            .into_iter()
            .map(|mut theme| {
                if let Some(source_path) = theme.source_path.clone()
                    && let Some(source_info) = self.find_source_info_for_path(
                        &source_path,
                        Some(&state.extension_theme_source_infos),
                        Some(&state.resource_metadata_by_path),
                    )
                {
                    theme.source_info = Some(source_info);
                } else if theme.source_info.is_none()
                    && let Some(source_path) = theme.source_path.clone()
                {
                    theme.source_info = Some(self.default_source_info_for_path(&source_path));
                }
                theme
            })
            .collect();
        state.theme_diagnostics = diagnostics;
    }

    fn load_themes(&self, paths: &[String]) -> (Vec<Theme>, Vec<ResourceDiagnostic>) {
        let mut themes: Vec<Theme> = Vec::new();
        let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

        for path in paths {
            let resolved = self.resolve_resource_path(path);
            if !Path::new(&resolved).exists() {
                diagnostics.push(ResourceDiagnostic::warning(
                    "theme path does not exist",
                    &resolved,
                ));
                continue;
            }

            match std::fs::metadata(&resolved) {
                Ok(stats) if stats.is_dir() => {
                    load_themes_from_dir(Path::new(&resolved), &mut themes, &mut diagnostics);
                }
                Ok(stats) if stats.is_file() && resolved.ends_with(".json") => {
                    load_theme_from_file(Path::new(&resolved), &mut themes, &mut diagnostics);
                }
                Ok(_) => diagnostics.push(ResourceDiagnostic::warning(
                    "theme path is not a json file",
                    &resolved,
                )),
                Err(error) => {
                    diagnostics.push(ResourceDiagnostic::warning(error.to_string(), &resolved));
                }
            }
        }

        (themes, diagnostics)
    }

    fn discover_system_prompt_file(&self) -> Option<String> {
        let project_path = Path::new(&self.cwd).join(CONFIG_DIR_NAME).join("SYSTEM.md");
        if self.settings_manager.is_project_trusted() && project_path.exists() {
            return Some(project_path.to_string_lossy().into_owned());
        }
        let global_path = Path::new(&self.agent_dir).join("SYSTEM.md");
        global_path
            .exists()
            .then(|| global_path.to_string_lossy().into_owned())
    }

    fn discover_append_system_prompt_file(&self) -> Option<String> {
        let project_path = Path::new(&self.cwd)
            .join(CONFIG_DIR_NAME)
            .join("APPEND_SYSTEM.md");
        if self.settings_manager.is_project_trusted() && project_path.exists() {
            return Some(project_path.to_string_lossy().into_owned());
        }
        let global_path = Path::new(&self.agent_dir).join("APPEND_SYSTEM.md");
        global_path
            .exists()
            .then(|| global_path.to_string_lossy().into_owned())
    }
}

fn load_themes_from_dir(
    dir: &Path,
    themes: &mut Vec<Theme>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    if !dir.exists() {
        return;
    }
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            diagnostics.push(ResourceDiagnostic::warning(
                error.to_string(),
                dir.to_string_lossy(),
            ));
            return;
        }
    };
    let mut entries: Vec<(String, PathBuf)> = read_dir
        .flatten()
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));

    for (name, path) in entries {
        let is_file = match std::fs::metadata(&path) {
            Ok(stats) => stats.is_file(),
            Err(_) => continue,
        };
        if !is_file || !name.ends_with(".json") {
            continue;
        }
        load_theme_from_file(&path, themes, diagnostics);
    }
}

fn load_theme_from_file(
    file_path: &Path,
    themes: &mut Vec<Theme>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    match load_theme_from_path(file_path, None) {
        Ok(theme) => themes.push(theme),
        Err(error) => diagnostics.push(ResourceDiagnostic::warning(
            error.to_string(),
            file_path.to_string_lossy(),
        )),
    }
}

/// First template of a name wins; the rest are reported.
fn dedupe_prompts(prompts: Vec<PromptTemplate>) -> (Vec<PromptTemplate>, Vec<ResourceDiagnostic>) {
    let mut seen: Vec<PromptTemplate> = Vec::new();
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    for prompt in prompts {
        match seen.iter().find(|existing| existing.name == prompt.name) {
            Some(existing) => diagnostics.push(ResourceDiagnostic::collision(
                format!("name \"/{}\" collision", prompt.name),
                &prompt.file_path,
                crate::core::diagnostics::ResourceCollision {
                    resource_type: crate::core::diagnostics::ResourceKind::Prompt,
                    name: prompt.name.clone(),
                    winner_path: existing.file_path.clone(),
                    loser_path: prompt.file_path.clone(),
                    winner_source: None,
                    loser_source: None,
                },
            )),
            None => seen.push(prompt),
        }
    }

    (seen, diagnostics)
}

fn dedupe_themes(themes: Vec<Theme>) -> (Vec<Theme>, Vec<ResourceDiagnostic>) {
    let mut seen: Vec<Theme> = Vec::new();
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    for theme in themes {
        let name = theme.name.clone().unwrap_or_else(|| "unnamed".to_string());
        let existing = seen.iter().find(|candidate| {
            candidate
                .name
                .clone()
                .unwrap_or_else(|| "unnamed".to_string())
                == name
        });
        match existing {
            Some(existing) => diagnostics.push(ResourceDiagnostic::collision(
                format!("name \"{name}\" collision"),
                theme.source_path.clone().unwrap_or_default(),
                crate::core::diagnostics::ResourceCollision {
                    resource_type: crate::core::diagnostics::ResourceKind::Theme,
                    name: name.clone(),
                    winner_path: existing
                        .source_path
                        .clone()
                        .unwrap_or_else(|| "<builtin>".to_string()),
                    loser_path: theme
                        .source_path
                        .clone()
                        .unwrap_or_else(|| "<builtin>".to_string()),
                    winner_source: None,
                    loser_source: None,
                },
            )),
            None => seen.push(theme),
        }
    }

    (seen, diagnostics)
}

impl ResourceLoader for DefaultResourceLoader {
    fn get_skills(&self) -> (Vec<Skill>, Vec<ResourceDiagnostic>) {
        let state = self.state.lock().expect("poisoned");
        (state.skills.clone(), state.skill_diagnostics.clone())
    }

    fn get_prompts(&self) -> (Vec<PromptTemplate>, Vec<ResourceDiagnostic>) {
        let state = self.state.lock().expect("poisoned");
        (state.prompts.clone(), state.prompt_diagnostics.clone())
    }

    fn get_themes(&self) -> (Vec<Theme>, Vec<ResourceDiagnostic>) {
        let state = self.state.lock().expect("poisoned");
        (state.themes.clone(), state.theme_diagnostics.clone())
    }

    fn get_agents_files(&self) -> Vec<ContextFile> {
        self.state.lock().expect("poisoned").agents_files.clone()
    }

    fn get_system_prompt(&self) -> Option<String> {
        self.state.lock().expect("poisoned").system_prompt.clone()
    }

    fn get_system_prompt_source(&self) -> Option<String> {
        self.state
            .lock()
            .expect("poisoned")
            .system_prompt_source_path
            .clone()
    }

    fn get_append_system_prompt(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("poisoned")
            .append_system_prompt
            .clone()
    }

    fn get_append_system_prompt_sources(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("poisoned")
            .append_system_prompt_source_paths
            .clone()
    }

    fn extend_resources(&self, paths: ResourceExtensionPaths) {
        let mut state = self.state.lock().expect("poisoned");

        let normalize = |entries: Vec<(String, PathMetadata)>| -> Vec<(String, PathMetadata)> {
            entries
                .into_iter()
                .map(|(path, mut metadata)| {
                    if let Some(base_dir) = metadata.base_dir.as_ref() {
                        metadata.base_dir = Some(self.resolve_resource_path(base_dir));
                    }
                    (self.resolve_resource_path(&path), metadata)
                })
                .collect()
        };

        let skill_paths = normalize(paths.skill_paths);
        let prompt_paths = normalize(paths.prompt_paths);
        let theme_paths = normalize(paths.theme_paths);

        for (path, metadata) in &skill_paths {
            state
                .extension_skill_source_infos
                .push((path.clone(), create_source_info(path, metadata)));
        }
        for (path, metadata) in &prompt_paths {
            state
                .extension_prompt_source_infos
                .push((path.clone(), create_source_info(path, metadata)));
        }
        for (path, metadata) in &theme_paths {
            state
                .extension_theme_source_infos
                .push((path.clone(), create_source_info(path, metadata)));
        }

        if !skill_paths.is_empty() {
            let added: Vec<String> = skill_paths.iter().map(|(path, _)| path.clone()).collect();
            let merged = self.merge_paths(&state.last_skill_paths, &added);
            state.last_skill_paths = merged.clone();
            self.update_skills_from_paths(&mut state, &merged);
        }
        if !prompt_paths.is_empty() {
            let added: Vec<String> = prompt_paths.iter().map(|(path, _)| path.clone()).collect();
            let merged = self.merge_paths(&state.last_prompt_paths, &added);
            state.last_prompt_paths = merged.clone();
            self.update_prompts_from_paths(&mut state, &merged);
        }
        if !theme_paths.is_empty() {
            let added: Vec<String> = theme_paths.iter().map(|(path, _)| path.clone()).collect();
            let merged = self.merge_paths(&state.last_theme_paths, &added);
            state.last_theme_paths = merged.clone();
            self.update_themes_from_paths(&mut state, &merged);
        }
    }

    fn reload<'a>(&'a self, options: ResourceLoaderReloadOptions) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(resolve_project_trust) = options.resolve_project_trust.as_ref() {
                // The bootstrap pass reads settings as if the project were
                // untrusted, so the question is answered from user-level
                // configuration only.
                self.settings_manager.set_project_trusted(false);
                self.settings_manager.reload();
                let trusted = resolve_project_trust().await;
                self.settings_manager.set_project_trusted(trusted);
            }

            // reload() keeps the trust decision and re-reads settings for it.
            self.settings_manager.reload();

            let resolved = match self.packages.as_ref() {
                Some(packages) => packages.resolve().await,
                None => ResolvedResources::default(),
            };

            let mut state = self.state.lock().expect("poisoned");
            // Kept on the loader so post-reload passes (extend_resources) can
            // still resolve package metadata.
            state.resource_metadata_by_path = HashMap::new();
            state.extension_skill_source_infos = Vec::new();
            state.extension_prompt_source_infos = Vec::new();
            state.extension_theme_source_infos = Vec::new();

            fn enabled_paths(
                resources: &[ResolvedResource],
                metadata_by_path: &mut HashMap<String, PathMetadata>,
            ) -> Vec<String> {
                for resource in resources {
                    metadata_by_path
                        .entry(resource.path.clone())
                        .or_insert_with(|| resource.metadata.clone());
                }
                resources
                    .iter()
                    .filter(|resource| resource.enabled)
                    .map(|resource| resource.path.clone())
                    .collect()
            }

            let metadata = &mut state.resource_metadata_by_path;
            let enabled_skills: Vec<String> = enabled_paths(&resolved.skills, metadata)
                .into_iter()
                .map(|path| map_skill_path(&path, metadata))
                .collect();
            let enabled_prompts = enabled_paths(&resolved.prompts, metadata);
            let enabled_themes = enabled_paths(&resolved.themes, metadata);

            let skill_paths = if self.options.no_skills {
                self.merge_paths(&[], &self.options.additional_skill_paths)
            } else {
                self.merge_paths(&enabled_skills, &self.options.additional_skill_paths)
            };
            state.last_skill_paths = skill_paths.clone();
            self.update_skills_from_paths(&mut state, &skill_paths);
            for path in &self.options.additional_skill_paths {
                if !is_local_path(path) {
                    continue;
                }
                let resolved = self.resolve_resource_path(path);
                if !Path::new(&resolved).exists()
                    && !state
                        .skill_diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.path.as_deref() == Some(resolved.as_str()))
                {
                    state.skill_diagnostics.push(ResourceDiagnostic::error(
                        "Skill path does not exist",
                        &resolved,
                    ));
                }
            }

            let prompt_paths = if self.options.no_prompt_templates {
                self.merge_paths(&[], &self.options.additional_prompt_template_paths)
            } else {
                self.merge_paths(
                    &enabled_prompts,
                    &self.options.additional_prompt_template_paths,
                )
            };
            state.last_prompt_paths = prompt_paths.clone();
            self.update_prompts_from_paths(&mut state, &prompt_paths);
            for path in &self.options.additional_prompt_template_paths {
                if !is_local_path(path) {
                    continue;
                }
                let resolved = self.resolve_resource_path(path);
                if !Path::new(&resolved).exists()
                    && !state
                        .prompt_diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.path.as_deref() == Some(resolved.as_str()))
                {
                    state.prompt_diagnostics.push(ResourceDiagnostic::error(
                        "Prompt template path does not exist",
                        &resolved,
                    ));
                }
            }

            let theme_paths = if self.options.no_themes {
                self.merge_paths(&[], &self.options.additional_theme_paths)
            } else {
                self.merge_paths(&enabled_themes, &self.options.additional_theme_paths)
            };
            state.last_theme_paths = theme_paths.clone();
            self.update_themes_from_paths(&mut state, &theme_paths);
            for path in &self.options.additional_theme_paths {
                let resolved = self.resolve_resource_path(path);
                if !Path::new(&resolved).exists()
                    && !state
                        .theme_diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.path.as_deref() == Some(resolved.as_str()))
                {
                    state.theme_diagnostics.push(ResourceDiagnostic::error(
                        "Theme path does not exist",
                        &resolved,
                    ));
                }
            }

            let agents_files = if self.options.no_context_files {
                Vec::new()
            } else {
                load_project_context_files(&self.cwd, &self.agent_dir)
            };
            state.agents_files = match self.options.agents_files_override.as_ref() {
                Some(over) => over(agents_files),
                None => agents_files,
            };

            let system_prompt_source = self
                .options
                .system_prompt
                .clone()
                .or_else(|| self.discover_system_prompt_file());
            let base_system_prompt = resolve_prompt_input(system_prompt_source.as_deref());
            state.system_prompt = match self.options.system_prompt_override.as_ref() {
                Some(over) => over(base_system_prompt),
                None => base_system_prompt,
            };
            state.system_prompt_source_path = system_prompt_source
                .as_deref()
                .filter(|source| Path::new(source).exists())
                .map(|source| {
                    resolve_path_default(source, &current_dir())
                        .unwrap_or_else(|_| source.to_string())
                });

            let append_sources = match self.options.append_system_prompt.clone() {
                Some(sources) => sources,
                None => self
                    .discover_append_system_prompt_file()
                    .map(|source| vec![source])
                    .unwrap_or_default(),
            };
            let base_append: Vec<String> = append_sources
                .iter()
                .filter_map(|source| resolve_prompt_input(Some(source)))
                .collect();
            state.append_system_prompt = match self.options.append_system_prompt_override.as_ref() {
                Some(over) => over(base_append),
                None => base_append,
            };
            state.append_system_prompt_source_paths = append_sources
                .iter()
                .filter(|source| Path::new(source.as_str()).exists())
                .map(|source| {
                    resolve_path_default(source, &current_dir()).unwrap_or_else(|_| source.clone())
                })
                .collect();
        })
    }
}

/// A package that installs a skill as a directory is pointed at its `SKILL.md`
/// so the loader treats it as one skill instead of walking it.
fn map_skill_path(path: &str, metadata_by_path: &mut HashMap<String, PathMetadata>) -> String {
    let metadata = metadata_by_path.get(path).cloned();
    let Some(metadata) = metadata else {
        return path.to_string();
    };
    if metadata.source != "auto" && metadata.origin != SourceOrigin::Package {
        return path.to_string();
    }
    match std::fs::metadata(path) {
        Ok(stats) if stats.is_dir() => {}
        _ => return path.to_string(),
    }
    let skill_file = Path::new(path).join("SKILL.md");
    if skill_file.exists() {
        let skill_file = skill_file.to_string_lossy().into_owned();
        metadata_by_path
            .entry(skill_file.clone())
            .or_insert(metadata);
        return skill_file;
    }
    path.to_string()
}
