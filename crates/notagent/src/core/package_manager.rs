//! Port of `packages/coding-agent/src/core/package-manager.ts` (2 677 LOC).
//!
//! Packages are npm specs, git repositories or local directories. This module
//! installs them, keeps them current, and turns the settings entries plus the
//! auto-discovered directories into the resource lists the resource loader
//! consumes.
//!
//! Deviation (class 2): the resource type `extensions` is gone with the
//! extension system (`plans/facts/extension-boundary.md` §6) — with it go
//! `resolveExtensionSources` (its only caller was the CLI `--extension` path of
//! the resource loader) and the extension-entry discovery
//! (`resolveExtensionEntries` / `collectAutoExtensionEntries`). The remaining
//! three types keep their behaviour exactly.

pub mod command_runner;
pub mod discovery;
pub mod patterns;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use futures::future::BoxFuture;
use globset::GlobBuilder;
use regex::Regex;
use semver::{Version, VersionReq};
use sha2::{Digest, Sha256};

use self::command_runner::{CommandOptions, CommandRunner, ProcessCommandRunner};
use self::discovery::{
    SkillDiscoveryMode, collect_ancestor_agents_skill_dirs, collect_auto_prompt_entries,
    collect_auto_theme_entries, collect_resource_files, collect_skill_entries,
};
use self::patterns::{
    apply_autoload_disabled_patterns, apply_patterns, contains_path, has_glob_pattern,
    is_enabled_by_overrides, is_override_pattern, split_patterns, to_posix_path,
};
use crate::config::CONFIG_DIR_NAME;
use crate::core::notagent_manifest::{PiManifest, ResourceType, read_pi_manifest};
use crate::core::resource_loader::{PackageResources, ResolvedResource, ResolvedResources};
use crate::core::settings_manager::{PackageSource, Settings, SettingsManager};
use crate::core::source_info::{PathMetadata, SourceOrigin, SourceScope};
use crate::utils::git::{GitSource, parse_git_url};
use crate::utils::paths::{
    PathInputOptions, canonicalize_path, is_local_path, mark_path_ignored_by_cloud_sync,
    node_relative, resolve_path,
};

const NETWORK_TIMEOUT_MS: u64 = 10_000;
const UPDATE_CHECK_CONCURRENCY: usize = 4;
const GIT_UPDATE_CONCURRENCY: usize = 4;

/// Everything the package manager can fail with; the message is the one the
/// TypeScript puts into its `Error`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PackageManagerError(pub String);

impl PackageManagerError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

type Result<T> = std::result::Result<T, PackageManagerError>;

/// `isOfflineModeEnabled()`
fn is_offline_mode_enabled() -> bool {
    let Ok(value) = std::env::var("NOTAGENT_OFFLINE") else {
        return false;
    };
    if value.is_empty() {
        return false;
    }
    value == "1" || value.to_lowercase() == "true" || value.to_lowercase() == "yes"
}

/// `isExactNpmVersion(version)`
///
/// Deviation (class 3, master substitution node-semver → the `semver` crate):
/// `valid()` becomes `Version::parse`. The crate rejects the `v1.2.3` spelling
/// node-semver tolerates; no caller of this port produces one.
fn is_exact_npm_version(version: Option<&str>) -> bool {
    Version::parse(version.unwrap_or("")).is_ok()
}

/// `getNpmVersionRange(version)`
///
/// Deviation (class 1): the TypeScript stores the *normalized* range string;
/// this port keeps the source spelling and parses it where it is used, which
/// is equivalent — the value is only ever fed back into `satisfies` and
/// `maxSatisfying`.
fn get_npm_version_range(version: Option<&str>) -> Option<String> {
    let version = version?;
    VersionReq::parse(version).ok().map(|_| version.to_owned())
}

/// `satisfies(version, range)`
fn satisfies(version: &str, range: &str) -> bool {
    match (Version::parse(version), VersionReq::parse(range)) {
        (Ok(version), Ok(range)) => range.matches(&version),
        _ => false,
    }
}

/// `maxSatisfying(versions, range)`
fn max_satisfying(versions: &[String], range: &str) -> Option<String> {
    let request = VersionReq::parse(range).ok()?;
    versions
        .iter()
        .filter_map(|value| Version::parse(value).ok().map(|parsed| (parsed, value)))
        .filter(|(parsed, _)| request.matches(parsed))
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, value)| value.clone())
}

/// `[...versions].sort(rcompare)[0]`
fn highest_version(versions: &[String]) -> Option<String> {
    versions
        .iter()
        .filter_map(|value| Version::parse(value).ok().map(|parsed| (parsed, value)))
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, value)| value.clone())
}

/// `MissingSourceAction`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingSourceAction {
    Install,
    Skip,
    Error,
}

/// The `onMissing` callback of `resolve`.
pub type MissingSourceHandler =
    Arc<dyn Fn(String) -> BoxFuture<'static, MissingSourceAction> + Send + Sync>;

/// `ProgressEvent["type"]`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressKind {
    Start,
    Progress,
    Complete,
    Error,
}

/// `ProgressEvent["action"]`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressAction {
    Install,
    Remove,
    Update,
    Clone,
    Pull,
}

/// `ProgressEvent`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEvent {
    pub kind: ProgressKind,
    pub action: ProgressAction,
    pub source: String,
    pub message: Option<String>,
}

/// `ProgressCallback`
pub type ProgressCallback = Arc<dyn Fn(&ProgressEvent) + Send + Sync>;

/// `PackageUpdate["type"]`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    Npm,
    Git,
}

/// `PackageUpdate`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageUpdate {
    pub source: String,
    pub display_name: String,
    pub kind: PackageKind,
    pub scope: SourceScope,
}

/// `ConfiguredPackage`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredPackage {
    pub source: String,
    pub scope: SourceScope,
    pub filtered: bool,
    pub installed_path: Option<String>,
}

/// `NpmSource`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmSource {
    pub spec: String,
    pub name: String,
    pub version: Option<String>,
    pub range: Option<String>,
    pub pinned: bool,
}

/// `ParsedSource`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedSource {
    Npm(NpmSource),
    Git(GitSource),
    Local(String),
}

impl ParsedSource {
    /// `parsed.pinned` — a local path is never pinned.
    fn pinned(&self) -> bool {
        match self {
            ParsedSource::Npm(source) => source.pinned,
            ParsedSource::Git(source) => source.pinned,
            ParsedSource::Local(_) => false,
        }
    }
}

/// `PackageFilter` — the settings entry that narrows a package's resources.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PackageFilter {
    autoload: Option<bool>,
    skills: Option<Vec<String>>,
    prompts: Option<Vec<String>>,
    themes: Option<Vec<String>>,
}

impl PackageFilter {
    fn entries(&self, resource_type: ResourceType) -> Option<&Vec<String>> {
        match resource_type {
            ResourceType::Skills => self.skills.as_ref(),
            ResourceType::Prompts => self.prompts.as_ref(),
            ResourceType::Themes => self.themes.as_ref(),
        }
    }
}

fn package_filter(package: &PackageSource) -> Option<PackageFilter> {
    match package {
        PackageSource::Source(_) => None,
        PackageSource::Filtered(filter) => Some(PackageFilter {
            autoload: filter.autoload,
            skills: filter.skills.clone(),
            prompts: filter.prompts.clone(),
            themes: filter.themes.clone(),
        }),
    }
}

/// `resourcePrecedenceRank(m)`
fn resource_precedence_rank(metadata: &PathMetadata) -> u8 {
    if metadata.origin == SourceOrigin::Package {
        return 4;
    }
    let scope_base = if metadata.scope == SourceScope::Project {
        0
    } else {
        2
    };
    scope_base + u8::from(metadata.source != "local")
}

/// `getHomeDir()`
fn get_home_dir() -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => home,
        _ => dirs::home_dir()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// `getExtensionTempFolder(agentDir)`
pub fn get_extension_temp_folder(agent_dir: &str) -> PathBuf {
    let temp_folder = Path::new(agent_dir).join("tmp").join("extensions");
    let _ = std::fs::create_dir_all(&temp_folder);
    set_owner_only_permissions(&temp_folder);
    temp_folder
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) {}

/// `globSync(entry, { cwd: root, absolute: true, dot: false, nodir: false })`
///
/// Deviation (class 3, master substitution glob → globset): the walk is local;
/// `dot: false` becomes "skip entries whose name starts with a dot", and the
/// matches are sorted so the resolved order does not depend on the filesystem.
fn glob_sync(pattern: &str, root: &Path) -> Vec<String> {
    // `glob` normalizes a leading `./` away; globset would match it literally.
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    let Ok(glob) = GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(true)
        .empty_alternates(true)
        .build()
    else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();
    let mut matches = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = dir.join(&name);
            let relative = to_posix_path(&node_relative(
                &root.to_string_lossy(),
                &path.to_string_lossy(),
            ));
            if matcher.is_match(&relative) {
                matches.push(path.to_string_lossy().into_owned());
            }
            if entry.file_type().is_ok_and(|kind| kind.is_dir())
                || (entry.file_type().is_ok_and(|kind| kind.is_symlink()) && path.is_dir())
            {
                stack.push(path);
            }
        }
    }
    matches.sort();
    matches
}

/// One entry of the `ResourceAccumulator` maps of the TypeScript.
#[derive(Debug, Clone)]
struct AccumulatedResource {
    metadata: PathMetadata,
    enabled: bool,
}

/// `ResourceAccumulator`
///
/// Deviation (class 1): the `Map`s are `Vec`s of pairs, because the insertion
/// order carries meaning (first-wins collision resolution downstream) and the
/// port must not depend on hash order.
#[derive(Debug, Default)]
struct ResourceAccumulator {
    skills: Vec<(String, AccumulatedResource)>,
    prompts: Vec<(String, AccumulatedResource)>,
    themes: Vec<(String, AccumulatedResource)>,
}

impl ResourceAccumulator {
    /// `getTargetMap(accumulator, resourceType)`
    fn target(&mut self, resource_type: ResourceType) -> &mut Vec<(String, AccumulatedResource)> {
        match resource_type {
            ResourceType::Skills => &mut self.skills,
            ResourceType::Prompts => &mut self.prompts,
            ResourceType::Themes => &mut self.themes,
        }
    }
}

/// `addResource(map, path, metadata, enabled)`
fn add_resource(
    target: &mut Vec<(String, AccumulatedResource)>,
    path: &str,
    metadata: &PathMetadata,
    enabled: bool,
) {
    if path.is_empty() {
        return;
    }
    if target.iter().any(|(existing, _)| existing == path) {
        return;
    }
    target.push((
        path.to_owned(),
        AccumulatedResource {
            metadata: metadata.clone(),
            enabled,
        },
    ));
}

/// `{ pkg, scope }`
#[derive(Debug, Clone)]
struct ScopedPackage {
    package: PackageSource,
    scope: SourceScope,
}

/// `ConfiguredUpdateSource`
#[derive(Debug, Clone)]
struct ConfiguredUpdateSource {
    source: String,
    scope: SourceScope,
}

/// `{ ref, head, fetchArgs }` of `getLocalGitUpdateTarget`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitUpdateTarget {
    pub git_ref: String,
    pub head: String,
    pub fetch_args: Vec<String>,
}

/// `PackageManagerOptions`
pub struct PackageManagerOptions {
    pub cwd: String,
    pub agent_dir: String,
    pub settings_manager: Arc<SettingsManager>,
    /// The spawn seam; defaults to [`ProcessCommandRunner`].
    pub command_runner: Option<Arc<dyn CommandRunner>>,
}

/// `DefaultPackageManager`
pub struct DefaultPackageManager {
    cwd: String,
    agent_dir: String,
    settings_manager: Arc<SettingsManager>,
    runner: Arc<dyn CommandRunner>,
    global_npm_root: Mutex<Option<(String, String)>>,
    progress_callback: Mutex<Option<ProgressCallback>>,
}

impl DefaultPackageManager {
    pub fn new(options: PackageManagerOptions) -> Self {
        let home_dir = get_home_dir();
        let path_options = PathInputOptions {
            trim: true,
            home_dir: Some(home_dir),
            ..PathInputOptions::default()
        };
        let resolve = |input: &str| {
            resolve_path(input, &crate::utils::paths::current_dir(), &path_options)
                .unwrap_or_else(|_| input.to_owned())
        };
        Self {
            cwd: resolve(&options.cwd),
            agent_dir: resolve(&options.agent_dir),
            settings_manager: options.settings_manager,
            runner: options
                .command_runner
                .unwrap_or_else(|| Arc::new(ProcessCommandRunner)),
            global_npm_root: Mutex::new(None),
            progress_callback: Mutex::new(None),
        }
    }

    /// `setProgressCallback(callback)`
    pub fn set_progress_callback(&self, callback: Option<ProgressCallback>) {
        *self.progress_callback.lock().expect("poisoned") = callback;
    }

    /// `addSourceToSettings(source, options)`
    pub fn add_source_to_settings(&self, source: &str, local: bool) -> Result<bool> {
        let scope = if local {
            SourceScope::Project
        } else {
            SourceScope::User
        };
        let current_settings = if local {
            self.settings_manager.get_project_settings()
        } else {
            self.settings_manager.get_global_settings()
        };
        let current_packages = current_settings.packages.clone().unwrap_or_default();
        let normalized_source = self.normalize_package_source_for_settings(source, scope)?;
        let match_index = current_packages.iter().position(|existing| {
            self.package_sources_match(existing, source, scope)
                .unwrap_or(false)
        });

        if let Some(index) = match_index {
            let existing = current_packages[index].clone();
            if package_source_string(&existing) == normalized_source {
                return Ok(false);
            }
            let mut next_packages = current_packages.clone();
            next_packages[index] = match existing {
                PackageSource::Source(_) => PackageSource::Source(normalized_source),
                PackageSource::Filtered(mut filter) => {
                    filter.source = normalized_source;
                    PackageSource::Filtered(filter)
                }
            };
            self.write_packages(&next_packages, scope)?;
            return Ok(true);
        }

        let mut next_packages = current_packages;
        next_packages.push(PackageSource::Source(normalized_source));
        self.write_packages(&next_packages, scope)?;
        Ok(true)
    }

    /// `removeSourceFromSettings(source, options)`
    pub fn remove_source_from_settings(&self, source: &str, local: bool) -> Result<bool> {
        let scope = if local {
            SourceScope::Project
        } else {
            SourceScope::User
        };
        let current_settings = if local {
            self.settings_manager.get_project_settings()
        } else {
            self.settings_manager.get_global_settings()
        };
        let current_packages = current_settings.packages.clone().unwrap_or_default();
        let mut next_packages = Vec::new();
        for existing in &current_packages {
            if !self.package_sources_match(existing, source, scope)? {
                next_packages.push(existing.clone());
            }
        }
        if next_packages.len() == current_packages.len() {
            return Ok(false);
        }
        self.write_packages(&next_packages, scope)?;
        Ok(true)
    }

    fn write_packages(&self, packages: &[PackageSource], scope: SourceScope) -> Result<()> {
        if scope == SourceScope::Project {
            self.settings_manager
                .set_project_packages(packages)
                .map_err(|error| PackageManagerError::new(error.message.clone()))
        } else {
            self.settings_manager.set_packages(packages);
            Ok(())
        }
    }

    /// `getInstalledPath(source, scope)`
    pub fn get_installed_path(&self, source: &str, scope: SourceScope) -> Result<Option<String>> {
        let parsed = self.parse_source(source);
        let path = match &parsed {
            ParsedSource::Npm(npm) => self.get_npm_install_path(npm, scope)?,
            ParsedSource::Git(git) => self.get_git_install_path(git, scope)?,
            ParsedSource::Local(local) => {
                let base_dir = self.get_base_dir_for_scope(scope)?;
                self.resolve_path_from_base(local, &base_dir)
            }
        };
        Ok(Path::new(&path).exists().then_some(path))
    }

    fn emit_progress(&self, event: &ProgressEvent) {
        let callback = self.progress_callback.lock().expect("poisoned").clone();
        if let Some(callback) = callback {
            callback(event);
        }
    }

    /// `withProgress(action, source, message, operation)`
    async fn with_progress<F>(
        &self,
        action: ProgressAction,
        source: &str,
        message: &str,
        operation: F,
    ) -> Result<()>
    where
        F: Future<Output = Result<()>>,
    {
        self.emit_progress(&ProgressEvent {
            kind: ProgressKind::Start,
            action,
            source: source.to_owned(),
            message: Some(message.to_owned()),
        });
        match operation.await {
            Ok(()) => {
                self.emit_progress(&ProgressEvent {
                    kind: ProgressKind::Complete,
                    action,
                    source: source.to_owned(),
                    message: None,
                });
                Ok(())
            }
            Err(error) => {
                self.emit_progress(&ProgressEvent {
                    kind: ProgressKind::Error,
                    action,
                    source: source.to_owned(),
                    message: Some(error.0.clone()),
                });
                Err(error)
            }
        }
    }

    /// `resolve(onMissing)`
    pub async fn resolve(
        &self,
        on_missing: Option<MissingSourceHandler>,
    ) -> Result<ResolvedResources> {
        let mut accumulator = ResourceAccumulator::default();
        let global_settings = self.settings_manager.get_global_settings();
        let project_settings = self.settings_manager.get_project_settings();

        // Collect all packages with scope (project first so cwd resources win collisions)
        let mut all_packages: Vec<ScopedPackage> = Vec::new();
        for package in project_settings.packages.clone().unwrap_or_default() {
            all_packages.push(ScopedPackage {
                package,
                scope: SourceScope::Project,
            });
        }
        for package in global_settings.packages.clone().unwrap_or_default() {
            all_packages.push(ScopedPackage {
                package,
                scope: SourceScope::User,
            });
        }

        // Dedupe: project scope wins over global for same package identity
        let package_sources = self.dedupe_packages(&all_packages)?;
        self.resolve_package_sources(&package_sources, &mut accumulator, on_missing)
            .await?;

        let global_base_dir = self.agent_dir.clone();
        let project_base_dir = join(&self.cwd, &[CONFIG_DIR_NAME]);

        for resource_type in ResourceType::ALL {
            let global_entries = settings_entries(&global_settings, resource_type);
            let project_entries = settings_entries(&project_settings, resource_type);
            self.resolve_local_entries(
                &project_entries,
                resource_type,
                accumulator.target(resource_type),
                &PathMetadata {
                    source: "local".to_owned(),
                    scope: SourceScope::Project,
                    origin: SourceOrigin::TopLevel,
                    base_dir: None,
                },
                &project_base_dir,
            );
            self.resolve_local_entries(
                &global_entries,
                resource_type,
                accumulator.target(resource_type),
                &PathMetadata {
                    source: "local".to_owned(),
                    scope: SourceScope::User,
                    origin: SourceOrigin::TopLevel,
                    base_dir: None,
                },
                &global_base_dir,
            );
        }

        self.add_auto_discovered_resources(
            &mut accumulator,
            &global_settings,
            &project_settings,
            &global_base_dir,
            &project_base_dir,
        );

        Ok(to_resolved_paths(accumulator))
    }

    /// `listConfiguredPackages()`
    pub fn list_configured_packages(&self) -> Result<Vec<ConfiguredPackage>> {
        let global_settings = self.settings_manager.get_global_settings();
        let project_settings = self.settings_manager.get_project_settings();
        let mut configured_packages = Vec::new();

        for package in global_settings.packages.clone().unwrap_or_default() {
            let source = package_source_string(&package);
            configured_packages.push(ConfiguredPackage {
                installed_path: self.get_installed_path(&source, SourceScope::User)?,
                source,
                scope: SourceScope::User,
                filtered: matches!(package, PackageSource::Filtered(_)),
            });
        }

        for package in project_settings.packages.clone().unwrap_or_default() {
            let source = package_source_string(&package);
            configured_packages.push(ConfiguredPackage {
                installed_path: self.get_installed_path(&source, SourceScope::Project)?,
                source,
                scope: SourceScope::Project,
                filtered: matches!(package, PackageSource::Filtered(_)),
            });
        }

        Ok(configured_packages)
    }

    /// `install(source, options)`
    pub async fn install(&self, source: &str, local: bool) -> Result<()> {
        let parsed = self.parse_source(source);
        let scope = if local {
            SourceScope::Project
        } else {
            SourceScope::User
        };
        self.assert_project_trusted_for_scope(scope)?;
        self.with_progress(
            ProgressAction::Install,
            source,
            &format!("Installing {source}..."),
            async {
                match &parsed {
                    ParsedSource::Npm(npm) => self.install_npm(npm, scope, false).await,
                    ParsedSource::Git(git) => self.install_git(git, scope).await,
                    ParsedSource::Local(path) => {
                        let resolved = self.resolve_path(path);
                        if !Path::new(&resolved).exists() {
                            return Err(PackageManagerError::new(format!(
                                "Path does not exist: {resolved}"
                            )));
                        }
                        Ok(())
                    }
                }
            },
        )
        .await
    }

    /// `installAndPersist(source, options)`
    pub async fn install_and_persist(&self, source: &str, local: bool) -> Result<()> {
        self.install(source, local).await?;
        self.add_source_to_settings(source, local)?;
        Ok(())
    }

    /// `remove(source, options)`
    pub async fn remove(&self, source: &str, local: bool) -> Result<()> {
        let parsed = self.parse_source(source);
        let scope = if local {
            SourceScope::Project
        } else {
            SourceScope::User
        };
        self.assert_project_trusted_for_scope(scope)?;
        self.with_progress(
            ProgressAction::Remove,
            source,
            &format!("Removing {source}..."),
            async {
                match &parsed {
                    ParsedSource::Npm(npm) => self.uninstall_npm(npm, scope).await,
                    ParsedSource::Git(git) => self.remove_git(git, scope),
                    ParsedSource::Local(_) => Ok(()),
                }
            },
        )
        .await
    }

    /// `removeAndPersist(source, options)`
    pub async fn remove_and_persist(&self, source: &str, local: bool) -> Result<bool> {
        self.remove(source, local).await?;
        self.remove_source_from_settings(source, local)
    }

    /// `update(source)`
    pub async fn update(&self, source: Option<&str>) -> Result<()> {
        let global_settings = self.settings_manager.get_global_settings();
        let project_settings = self.settings_manager.get_project_settings();
        let identity = match source {
            Some(source) => Some(self.get_package_identity(source, None)?),
            None => None,
        };
        let mut matched = false;
        let mut update_sources: Vec<ConfiguredUpdateSource> = Vec::new();

        for package in global_settings.packages.clone().unwrap_or_default() {
            let source_str = package_source_string(&package);
            if let Some(identity) = identity.as_ref()
                && self.get_package_identity(&source_str, Some(SourceScope::User))? != *identity
            {
                continue;
            }
            matched = true;
            update_sources.push(ConfiguredUpdateSource {
                source: source_str,
                scope: SourceScope::User,
            });
        }
        for package in project_settings.packages.clone().unwrap_or_default() {
            let source_str = package_source_string(&package);
            if let Some(identity) = identity.as_ref()
                && self.get_package_identity(&source_str, Some(SourceScope::Project))? != *identity
            {
                continue;
            }
            matched = true;
            update_sources.push(ConfiguredUpdateSource {
                source: source_str,
                scope: SourceScope::Project,
            });
        }

        if let Some(source) = source
            && !matched
        {
            let mut configured = global_settings.packages.clone().unwrap_or_default();
            configured.extend(project_settings.packages.clone().unwrap_or_default());
            return Err(PackageManagerError::new(
                self.build_no_matching_package_message(source, &configured),
            ));
        }

        self.update_configured_sources(&update_sources).await
    }

    /// `updateConfiguredSources(sources)`
    async fn update_configured_sources(&self, sources: &[ConfiguredUpdateSource]) -> Result<()> {
        if is_offline_mode_enabled() || sources.is_empty() {
            return Ok(());
        }

        let mut npm_candidates: Vec<(ConfiguredUpdateSource, NpmSource)> = Vec::new();
        let mut git_candidates: Vec<(ConfiguredUpdateSource, GitSource)> = Vec::new();

        for entry in sources {
            // Pinned npm versions are fixed. Pinned git refs are configured checkout targets,
            // so include them to reconcile an existing clone when the configured ref changes.
            match self.parse_source(&entry.source) {
                ParsedSource::Npm(npm) => {
                    if !npm.pinned {
                        npm_candidates.push((entry.clone(), npm));
                    }
                }
                ParsedSource::Git(git) => git_candidates.push((entry.clone(), git)),
                ParsedSource::Local(_) => {}
            }
        }

        let npm_check_results = run_with_concurrency(
            npm_candidates
                .iter()
                .map(|(entry, npm)| async move {
                    (
                        entry.clone(),
                        npm.clone(),
                        self.should_update_npm_source(npm, entry.scope).await,
                    )
                })
                .collect(),
            UPDATE_CHECK_CONCURRENCY,
        )
        .await;

        let mut user_npm_updates: Vec<(ConfiguredUpdateSource, NpmSource)> = Vec::new();
        let mut project_npm_updates: Vec<(ConfiguredUpdateSource, NpmSource)> = Vec::new();
        for (entry, npm, should_update) in npm_check_results {
            if !should_update? {
                continue;
            }
            if entry.scope == SourceScope::User {
                user_npm_updates.push((entry, npm));
            } else {
                project_npm_updates.push((entry, npm));
            }
        }

        let user_task = async {
            if user_npm_updates.is_empty() {
                return Ok(());
            }
            self.update_npm_batch(&user_npm_updates, SourceScope::User)
                .await
        };
        let project_task = async {
            if project_npm_updates.is_empty() {
                return Ok(());
            }
            self.update_npm_batch(&project_npm_updates, SourceScope::Project)
                .await
        };
        let git_task = async {
            if git_candidates.is_empty() {
                return Ok(());
            }
            let results = run_with_concurrency(
                git_candidates
                    .iter()
                    .map(|(entry, git)| async move {
                        self.with_progress(
                            ProgressAction::Update,
                            &entry.source,
                            &format!("Updating {}...", entry.source),
                            self.update_git(git, entry.scope),
                        )
                        .await
                    })
                    .collect(),
                GIT_UPDATE_CONCURRENCY,
            )
            .await;
            for result in results {
                result?;
            }
            Ok(())
        };

        let (user_result, project_result, git_result) =
            futures::future::join3(user_task, project_task, git_task).await;
        user_result?;
        project_result?;
        git_result
    }

    /// `shouldUpdateNpmSource(source, scope)`
    async fn should_update_npm_source(
        &self,
        source: &NpmSource,
        scope: SourceScope,
    ) -> Result<bool> {
        let installed_path = self.get_managed_npm_install_path(source, scope)?;
        let installed_version = if Path::new(&installed_path).exists() {
            get_installed_npm_version(&installed_path)
        } else {
            None
        };
        let Some(installed_version) = installed_version else {
            return Ok(true);
        };

        let spec = if source.version.is_some() {
            source.spec.clone()
        } else {
            source.name.clone()
        };
        match self
            .get_latest_npm_version(&spec, source.range.as_deref())
            .await
        {
            // Preserve existing update behavior when version lookup fails.
            Err(_) => Ok(true),
            Ok(target_version) => Ok(target_version != installed_version),
        }
    }

    /// `updateNpmBatch(sources, scope)`
    async fn update_npm_batch(
        &self,
        sources: &[(ConfiguredUpdateSource, NpmSource)],
        scope: SourceScope,
    ) -> Result<()> {
        if sources.is_empty() {
            return Ok(());
        }

        let scope_label = if scope == SourceScope::User {
            "user"
        } else {
            "project"
        };
        let source_label = if sources.len() == 1 {
            sources[0].0.source.clone()
        } else {
            format!("{scope_label} npm packages")
        };
        let message = if sources.len() == 1 {
            format!("Updating {}...", sources[0].0.source)
        } else {
            format!("Updating {scope_label} npm packages...")
        };
        let specs: Vec<String> = sources
            .iter()
            .map(|(_, npm)| {
                if npm.version.is_some() {
                    npm.spec.clone()
                } else {
                    format!("{}@latest", npm.name)
                }
            })
            .collect();

        self.with_progress(
            ProgressAction::Update,
            &source_label,
            &message,
            self.install_npm_batch(&specs, scope),
        )
        .await
    }

    /// `installNpmBatch(specs, scope)`
    async fn install_npm_batch(&self, specs: &[String], scope: SourceScope) -> Result<()> {
        let install_root = self.get_npm_install_root(scope, false)?;
        self.ensure_npm_project(&install_root);
        let args = self.get_npm_install_args(specs, &install_root)?;
        self.run_npm_command(&args, None).await
    }

    /// `checkForAvailableUpdates()`
    pub async fn check_for_available_updates(&self) -> Result<Vec<PackageUpdate>> {
        if is_offline_mode_enabled() {
            return Ok(Vec::new());
        }

        let global_settings = self.settings_manager.get_global_settings();
        let project_settings = self.settings_manager.get_project_settings();
        let mut all_packages: Vec<ScopedPackage> = Vec::new();
        for package in project_settings.packages.clone().unwrap_or_default() {
            all_packages.push(ScopedPackage {
                package,
                scope: SourceScope::Project,
            });
        }
        for package in global_settings.packages.clone().unwrap_or_default() {
            all_packages.push(ScopedPackage {
                package,
                scope: SourceScope::User,
            });
        }

        let package_sources = self.dedupe_packages(&all_packages)?;
        let checks: Vec<_> = package_sources
            .iter()
            .filter(|entry| entry.scope != SourceScope::Temporary)
            .map(|entry| async move {
                let source = package_source_string(&entry.package);
                let parsed = self.parse_source(&source);
                if matches!(parsed, ParsedSource::Local(_)) || parsed.pinned() {
                    return Ok(None);
                }

                match parsed {
                    ParsedSource::Npm(npm) => {
                        let installed_path = self.get_npm_install_path(&npm, entry.scope)?;
                        if !Path::new(&installed_path).exists() {
                            return Ok(None);
                        }
                        if !self.npm_has_available_update(&npm, &installed_path).await {
                            return Ok(None);
                        }
                        Ok(Some(PackageUpdate {
                            source,
                            display_name: npm.name.clone(),
                            kind: PackageKind::Npm,
                            scope: entry.scope,
                        }))
                    }
                    ParsedSource::Git(git) => {
                        let installed_path = self.get_git_install_path(&git, entry.scope)?;
                        if !Path::new(&installed_path).exists() {
                            return Ok(None);
                        }
                        if !self.git_has_available_update(&installed_path).await {
                            return Ok(None);
                        }
                        Ok(Some(PackageUpdate {
                            source,
                            display_name: format!("{}/{}", git.host, git.path),
                            kind: PackageKind::Git,
                            scope: entry.scope,
                        }))
                    }
                    ParsedSource::Local(_) => Ok(None),
                }
            })
            .collect();

        let results = run_with_concurrency(checks, UPDATE_CHECK_CONCURRENCY).await;
        let mut updates = Vec::new();
        for result in results {
            if let Some(update) = result? {
                updates.push(update);
            }
        }
        Ok(updates)
    }

    /// `resolvePackageSources(sources, accumulator, onMissing)`
    async fn resolve_package_sources(
        &self,
        sources: &[ScopedPackage],
        accumulator: &mut ResourceAccumulator,
        on_missing: Option<MissingSourceHandler>,
    ) -> Result<()> {
        for entry in sources {
            let source_str = package_source_string(&entry.package);
            let filter = package_filter(&entry.package);
            let delta_base = self.find_autoload_delta_base(&entry.package, entry.scope, sources)?;
            let (resolved_source, resolved_scope) = match delta_base {
                Some((source, scope)) => (source, scope),
                None => (source_str.clone(), entry.scope),
            };
            let parsed = self.parse_source(&resolved_source);
            let mut metadata = PathMetadata {
                source: source_str.clone(),
                scope: entry.scope,
                origin: SourceOrigin::Package,
                base_dir: None,
            };

            if let ParsedSource::Local(path) = &parsed {
                let base_dir = self.get_base_dir_for_scope(resolved_scope)?;
                self.resolve_local_package_source(
                    path,
                    accumulator,
                    filter.as_ref(),
                    &mut metadata,
                    &base_dir,
                );
                continue;
            }

            match &parsed {
                ParsedSource::Npm(npm) => {
                    let mut installed_path = self.get_npm_install_path(npm, resolved_scope)?;
                    let needs_install = !Path::new(&installed_path).exists()
                        || !installed_npm_matches_configured_version(npm, &installed_path);
                    if needs_install {
                        if !self
                            .install_missing(&parsed, &resolved_source, resolved_scope, &on_missing)
                            .await?
                        {
                            continue;
                        }
                        installed_path = self.get_npm_install_path(npm, resolved_scope)?;
                    }
                    metadata.base_dir = Some(installed_path.clone());
                    self.collect_package_resources(
                        Path::new(&installed_path),
                        accumulator,
                        filter.as_ref(),
                        &metadata,
                    );
                }
                ParsedSource::Git(git) => {
                    let installed_path = self.get_git_install_path(git, resolved_scope)?;
                    if !Path::new(&installed_path).exists() {
                        if !self
                            .install_missing(&parsed, &resolved_source, resolved_scope, &on_missing)
                            .await?
                        {
                            continue;
                        }
                    } else if resolved_scope == SourceScope::Temporary
                        && !git.pinned
                        && !is_offline_mode_enabled()
                    {
                        self.refresh_temporary_git_source(git, &resolved_source)
                            .await;
                    }
                    metadata.base_dir = Some(installed_path.clone());
                    self.collect_package_resources(
                        Path::new(&installed_path),
                        accumulator,
                        filter.as_ref(),
                        &metadata,
                    );
                }
                ParsedSource::Local(_) => unreachable!("handled above"),
            }
        }
        Ok(())
    }

    /// The `installMissing` closure of `resolvePackageSources`.
    async fn install_missing(
        &self,
        parsed: &ParsedSource,
        resolved_source: &str,
        resolved_scope: SourceScope,
        on_missing: &Option<MissingSourceHandler>,
    ) -> Result<bool> {
        if is_offline_mode_enabled() {
            return Ok(false);
        }
        let Some(on_missing) = on_missing else {
            self.install_parsed_source(parsed, resolved_scope).await?;
            return Ok(true);
        };
        match on_missing(resolved_source.to_owned()).await {
            MissingSourceAction::Skip => Ok(false),
            MissingSourceAction::Error => Err(PackageManagerError::new(format!(
                "Missing source: {resolved_source}"
            ))),
            MissingSourceAction::Install => {
                self.install_parsed_source(parsed, resolved_scope).await?;
                Ok(true)
            }
        }
    }

    /// `findAutoloadDeltaBase(pkg, scope, sources)`
    fn find_autoload_delta_base(
        &self,
        package: &PackageSource,
        scope: SourceScope,
        sources: &[ScopedPackage],
    ) -> Result<Option<(String, SourceScope)>> {
        if scope != SourceScope::Project {
            return Ok(None);
        }
        let PackageSource::Filtered(filter) = package else {
            return Ok(None);
        };
        if filter.autoload != Some(false) {
            return Ok(None);
        }
        let identity = self.get_package_identity(&filter.source, Some(scope))?;
        for entry in sources {
            if entry.scope != SourceScope::User {
                continue;
            }
            let entry_identity = self.get_package_identity(
                &package_source_string(&entry.package),
                Some(SourceScope::User),
            )?;
            if entry_identity == identity {
                return Ok(Some((
                    package_source_string(&entry.package),
                    SourceScope::User,
                )));
            }
        }
        Ok(None)
    }

    /// `resolveLocalExtensionSource(source, accumulator, filter, metadata, baseDir)`
    ///
    /// Deviation (class 2): the two branches that registered the source itself
    /// as an extension (a file source, and a directory without resources) are
    /// gone with the extension system; a local package now contributes exactly
    /// its skills, prompts and themes.
    fn resolve_local_package_source(
        &self,
        source_path: &str,
        accumulator: &mut ResourceAccumulator,
        filter: Option<&PackageFilter>,
        metadata: &mut PathMetadata,
        base_dir: &str,
    ) {
        let resolved = self.resolve_path_from_base(source_path, base_dir);
        let path = Path::new(&resolved);
        if !path.exists() {
            return;
        }
        let Ok(stats) = std::fs::metadata(path) else {
            return;
        };
        if stats.is_dir() {
            metadata.base_dir = Some(resolved.clone());
            self.collect_package_resources(path, accumulator, filter, metadata);
        }
    }

    /// `installParsedSource(parsed, scope)`
    async fn install_parsed_source(&self, parsed: &ParsedSource, scope: SourceScope) -> Result<()> {
        match parsed {
            ParsedSource::Npm(npm) => {
                self.install_npm(npm, scope, scope == SourceScope::Temporary)
                    .await
            }
            ParsedSource::Git(git) => self.install_git(git, scope).await,
            ParsedSource::Local(_) => Ok(()),
        }
    }

    /// `getSourceMatchKeyForInput(source)`
    fn get_source_match_key_for_input(&self, source: &str) -> String {
        match self.parse_source(source) {
            ParsedSource::Npm(npm) => format!("npm:{}", npm.name),
            ParsedSource::Git(git) => format!("git:{}/{}", git.host, git.path),
            ParsedSource::Local(path) => format!("local:{}", self.resolve_path(&path)),
        }
    }

    /// `getSourceMatchKeyForSettings(source, scope)`
    fn get_source_match_key_for_settings(
        &self,
        source: &str,
        scope: SourceScope,
    ) -> Result<String> {
        Ok(match self.parse_source(source) {
            ParsedSource::Npm(npm) => format!("npm:{}", npm.name),
            ParsedSource::Git(git) => format!("git:{}/{}", git.host, git.path),
            ParsedSource::Local(path) => {
                let base_dir = self.get_base_dir_for_scope(scope)?;
                format!("local:{}", self.resolve_path_from_base(&path, &base_dir))
            }
        })
    }

    /// `buildNoMatchingPackageMessage(source, configuredPackages)`
    fn build_no_matching_package_message(
        &self,
        source: &str,
        configured_packages: &[PackageSource],
    ) -> String {
        match self.find_suggested_configured_source(source, configured_packages) {
            None => format!("No matching package found for {source}"),
            Some(suggestion) => {
                format!("No matching package found for {source}. Did you mean {suggestion}?")
            }
        }
    }

    /// `findSuggestedConfiguredSource(source, configuredPackages)`
    fn find_suggested_configured_source(
        &self,
        source: &str,
        configured_packages: &[PackageSource],
    ) -> Option<String> {
        let trimmed_source = source.trim();

        for package in configured_packages {
            let source_str = package_source_string(package);
            match self.parse_source(&source_str) {
                ParsedSource::Npm(npm) => {
                    if trimmed_source == npm.name || trimmed_source == npm.spec {
                        return Some(source_str);
                    }
                }
                ParsedSource::Git(git) => {
                    let shorthand = format!("{}/{}", git.host, git.path);
                    let shorthand_with_ref = git
                        .ref_name
                        .as_ref()
                        .map(|git_ref| format!("{shorthand}@{git_ref}"));
                    if trimmed_source == shorthand
                        || shorthand_with_ref.is_some_and(|with_ref| trimmed_source == with_ref)
                    {
                        return Some(source_str);
                    }
                }
                ParsedSource::Local(_) => {}
            }
        }

        None
    }

    /// `packageSourcesMatch(existing, inputSource, scope)`
    fn package_sources_match(
        &self,
        existing: &PackageSource,
        input_source: &str,
        scope: SourceScope,
    ) -> Result<bool> {
        let left =
            self.get_source_match_key_for_settings(&package_source_string(existing), scope)?;
        let right = self.get_source_match_key_for_input(input_source);
        Ok(left == right)
    }

    /// `normalizePackageSourceForSettings(source, scope)`
    fn normalize_package_source_for_settings(
        &self,
        source: &str,
        scope: SourceScope,
    ) -> Result<String> {
        let ParsedSource::Local(path) = self.parse_source(source) else {
            return Ok(source.to_owned());
        };
        let base_dir = self.get_base_dir_for_scope(scope)?;
        let resolved = self.resolve_path(&path);
        let relative = node_relative(&base_dir, &resolved);
        Ok(if relative.is_empty() {
            ".".to_owned()
        } else {
            relative
        })
    }

    /// `parseSource(source)`
    pub fn parse_source(&self, source: &str) -> ParsedSource {
        if let Some(rest) = source.strip_prefix("npm:") {
            let spec = rest.trim().to_owned();
            let (name, version) = parse_npm_spec(&spec);
            return ParsedSource::Npm(NpmSource {
                range: get_npm_version_range(version.as_deref()),
                pinned: is_exact_npm_version(version.as_deref()),
                spec,
                name,
                version,
            });
        }

        if is_local_path(source) {
            return ParsedSource::Local(source.to_owned());
        }

        // Try parsing as git URL
        if let Some(git) = parse_git_url(source) {
            return ParsedSource::Git(git);
        }

        ParsedSource::Local(source.to_owned())
    }

    /// `npmHasAvailableUpdate(source, installedPath)`
    async fn npm_has_available_update(&self, source: &NpmSource, installed_path: &str) -> bool {
        if is_offline_mode_enabled() {
            return false;
        }

        let Some(installed_version) = get_installed_npm_version(installed_path) else {
            return false;
        };

        let spec = if source.version.is_some() {
            source.spec.clone()
        } else {
            source.name.clone()
        };
        match self
            .get_latest_npm_version(&spec, source.range.as_deref())
            .await
        {
            Ok(target_version) => target_version != installed_version,
            Err(_) => false,
        }
    }

    /// `getLatestNpmVersion(packageSpec, range)`
    pub async fn get_latest_npm_version(
        &self,
        package_spec: &str,
        range: Option<&str>,
    ) -> Result<String> {
        let npm_command = self.get_npm_command()?;
        let mut args = npm_command.args.clone();
        args.extend([
            "view".to_owned(),
            package_spec.to_owned(),
            "version".to_owned(),
            "--json".to_owned(),
        ]);
        let stdout = self
            .run_command_capture(
                &npm_command.command,
                &args,
                CommandOptions {
                    cwd: Some(self.cwd.clone()),
                    timeout_ms: Some(NETWORK_TIMEOUT_MS),
                    ..CommandOptions::default()
                },
            )
            .await?;
        let raw = stdout.trim();
        if raw.is_empty() {
            return Err(PackageManagerError::new("Empty response from npm view"));
        }
        let parsed: serde_json::Value = serde_json::from_str(raw)
            .map_err(|error| PackageManagerError::new(error.to_string()))?;
        if let Some(version) = parsed.as_str() {
            return Ok(version.to_owned());
        }
        if let Some(entries) = parsed.as_array() {
            let versions: Vec<String> = entries
                .iter()
                .filter_map(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect();
            let latest = match range {
                Some(range) => max_satisfying(&versions, range),
                None => highest_version(&versions),
            };
            if let Some(latest) = latest {
                return Ok(latest);
            }
        }
        Err(PackageManagerError::new(
            "Unexpected response from npm view",
        ))
    }

    /// `gitHasAvailableUpdate(installedPath)`
    async fn git_has_available_update(&self, installed_path: &str) -> bool {
        if is_offline_mode_enabled() {
            return false;
        }

        let local_head = self
            .run_command_capture(
                "git",
                &["rev-parse".to_owned(), "HEAD".to_owned()],
                CommandOptions {
                    cwd: Some(installed_path.to_owned()),
                    timeout_ms: Some(NETWORK_TIMEOUT_MS),
                    ..CommandOptions::default()
                },
            )
            .await;
        let Ok(local_head) = local_head else {
            return false;
        };
        match self.get_remote_git_head(installed_path).await {
            Ok(remote_head) => local_head.trim() != remote_head.trim(),
            Err(_) => false,
        }
    }

    /// `getRemoteGitHead(installedPath)`
    async fn get_remote_git_head(&self, installed_path: &str) -> Result<String> {
        if let Some(upstream_ref) = self.get_git_upstream_ref(installed_path).await {
            let remote_head = self
                .run_git_remote_command(
                    installed_path,
                    &["ls-remote".to_owned(), "origin".to_owned(), upstream_ref],
                )
                .await?;
            if let Some(hash) = first_object_hash(&remote_head, None) {
                return Ok(hash);
            }
        }

        let remote_head = self
            .run_git_remote_command(
                installed_path,
                &[
                    "ls-remote".to_owned(),
                    "origin".to_owned(),
                    "HEAD".to_owned(),
                ],
            )
            .await?;
        first_object_hash(&remote_head, Some("HEAD"))
            .ok_or_else(|| PackageManagerError::new("Failed to determine remote HEAD"))
    }

    /// `getLocalGitUpdateTarget(installedPath)`
    pub async fn get_local_git_update_target(&self, installed_path: &str) -> GitUpdateTarget {
        let upstream = self
            .run_command_capture(
                "git",
                &[
                    "rev-parse".to_owned(),
                    "--abbrev-ref".to_owned(),
                    "@{upstream}".to_owned(),
                ],
                CommandOptions {
                    cwd: Some(installed_path.to_owned()),
                    timeout_ms: Some(NETWORK_TIMEOUT_MS),
                    ..CommandOptions::default()
                },
            )
            .await;

        let upstream_branch = upstream.ok().and_then(|upstream| {
            let trimmed = upstream.trim().to_owned();
            let branch = trimmed.strip_prefix("origin/")?.to_owned();
            (!branch.is_empty()).then_some(branch)
        });

        if let Some(branch) = upstream_branch {
            let head = self
                .run_command_capture(
                    "git",
                    &["rev-parse".to_owned(), "@{upstream}".to_owned()],
                    CommandOptions {
                        cwd: Some(installed_path.to_owned()),
                        timeout_ms: Some(NETWORK_TIMEOUT_MS),
                        ..CommandOptions::default()
                    },
                )
                .await;
            if let Ok(head) = head {
                return GitUpdateTarget {
                    git_ref: "@{upstream}".to_owned(),
                    head,
                    fetch_args: vec![
                        "fetch".to_owned(),
                        "--prune".to_owned(),
                        "--no-tags".to_owned(),
                        "origin".to_owned(),
                        format!("+refs/heads/{branch}:refs/remotes/origin/{branch}"),
                    ],
                };
            }
        }

        let _ = self
            .run_command(
                "git",
                &[
                    "remote".to_owned(),
                    "set-head".to_owned(),
                    "origin".to_owned(),
                    "-a".to_owned(),
                ],
                CommandOptions::with_cwd(installed_path),
            )
            .await;
        let head = self
            .run_command_capture(
                "git",
                &["rev-parse".to_owned(), "origin/HEAD".to_owned()],
                CommandOptions {
                    cwd: Some(installed_path.to_owned()),
                    timeout_ms: Some(NETWORK_TIMEOUT_MS),
                    ..CommandOptions::default()
                },
            )
            .await
            .unwrap_or_default();
        let origin_head_ref = self
            .run_command_capture(
                "git",
                &[
                    "symbolic-ref".to_owned(),
                    "refs/remotes/origin/HEAD".to_owned(),
                ],
                CommandOptions {
                    cwd: Some(installed_path.to_owned()),
                    timeout_ms: Some(NETWORK_TIMEOUT_MS),
                    ..CommandOptions::default()
                },
            )
            .await
            .unwrap_or_default();
        let branch = origin_head_ref
            .trim()
            .strip_prefix("refs/remotes/origin/")
            .unwrap_or(origin_head_ref.trim())
            .to_owned();
        if !branch.is_empty() {
            return GitUpdateTarget {
                git_ref: "origin/HEAD".to_owned(),
                head,
                fetch_args: vec![
                    "fetch".to_owned(),
                    "--prune".to_owned(),
                    "--no-tags".to_owned(),
                    "origin".to_owned(),
                    format!("+refs/heads/{branch}:refs/remotes/origin/{branch}"),
                ],
            };
        }
        GitUpdateTarget {
            git_ref: "origin/HEAD".to_owned(),
            head,
            fetch_args: vec![
                "fetch".to_owned(),
                "--prune".to_owned(),
                "--no-tags".to_owned(),
                "origin".to_owned(),
                "+HEAD:refs/remotes/origin/HEAD".to_owned(),
            ],
        }
    }

    /// `getGitUpstreamRef(installedPath)`
    async fn get_git_upstream_ref(&self, installed_path: &str) -> Option<String> {
        let upstream = self
            .run_command_capture(
                "git",
                &[
                    "rev-parse".to_owned(),
                    "--abbrev-ref".to_owned(),
                    "@{upstream}".to_owned(),
                ],
                CommandOptions {
                    cwd: Some(installed_path.to_owned()),
                    timeout_ms: Some(NETWORK_TIMEOUT_MS),
                    ..CommandOptions::default()
                },
            )
            .await
            .ok()?;
        let trimmed = upstream.trim();
        let branch = trimmed.strip_prefix("origin/")?;
        (!branch.is_empty()).then(|| format!("refs/heads/{branch}"))
    }

    /// `runGitRemoteCommand(installedPath, args)`
    async fn run_git_remote_command(
        &self,
        installed_path: &str,
        args: &[String],
    ) -> Result<String> {
        self.run_command_capture(
            "git",
            args,
            CommandOptions {
                cwd: Some(installed_path.to_owned()),
                timeout_ms: Some(NETWORK_TIMEOUT_MS),
                env: BTreeMap::from([("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned())]),
            },
        )
        .await
    }

    /// `getPackageIdentity(source, scope)`
    pub fn get_package_identity(&self, source: &str, scope: Option<SourceScope>) -> Result<String> {
        Ok(match self.parse_source(source) {
            ParsedSource::Npm(npm) => format!("npm:{}", npm.name),
            // Use host/path for identity to normalize SSH and HTTPS
            ParsedSource::Git(git) => format!("git:{}/{}", git.host, git.path),
            ParsedSource::Local(path) => match scope {
                Some(scope) => {
                    let base_dir = self.get_base_dir_for_scope(scope)?;
                    format!("local:{}", self.resolve_path_from_base(&path, &base_dir))
                }
                None => format!("local:{}", self.resolve_path(&path)),
            },
        })
    }

    /// `dedupePackages(packages)`
    fn dedupe_packages(&self, packages: &[ScopedPackage]) -> Result<Vec<ScopedPackage>> {
        let mut result: Vec<ScopedPackage> = Vec::new();
        let mut seen: Vec<(String, usize)> = Vec::new();
        for entry in packages {
            let identity = self
                .get_package_identity(&package_source_string(&entry.package), Some(entry.scope))?;
            let index = seen
                .iter()
                .find(|(key, _)| *key == identity)
                .map(|(_, index)| *index);
            let Some(index) = index else {
                seen.push((identity, result.len()));
                result.push(entry.clone());
                continue;
            };
            let existing = &result[index];
            if existing.scope == SourceScope::Project && entry.scope == SourceScope::User {
                if matches!(&existing.package, PackageSource::Filtered(filter) if filter.autoload == Some(false))
                {
                    result.push(entry.clone());
                }
            } else if entry.scope == SourceScope::Project {
                result[index] = entry.clone();
            }
        }
        Ok(result)
    }

    /// `assertProjectTrustedForScope(scope)`
    fn assert_project_trusted_for_scope(&self, scope: SourceScope) -> Result<()> {
        if scope == SourceScope::Project && !self.settings_manager.is_project_trusted() {
            return Err(PackageManagerError::new(
                "Project is not trusted; refusing to access project package storage",
            ));
        }
        Ok(())
    }

    /// `getNpmCommand()`
    fn get_npm_command(&self) -> Result<NpmCommand> {
        let configured_command = self.settings_manager.get_npm_command();
        let Some(configured_command) = configured_command.filter(|command| !command.is_empty())
        else {
            return Ok(NpmCommand {
                command: "npm".to_owned(),
                args: Vec::new(),
            });
        };
        let (command, args) = configured_command.split_first().expect("non-empty");
        if command.is_empty() {
            return Err(PackageManagerError::new(
                "Invalid npmCommand: first array entry must be a non-empty command",
            ));
        }
        Ok(NpmCommand {
            command: command.clone(),
            args: args.to_vec(),
        })
    }

    /// `getPackageManagerName()`
    fn get_package_manager_name(&self) -> Result<String> {
        let npm_command = self.get_npm_command()?;
        let mut command_parts = vec![npm_command.command.clone()];
        command_parts.extend(npm_command.args.clone());
        let separator_index = command_parts.iter().rposition(|part| part == "--");
        let package_manager_command = match separator_index {
            Some(index) => command_parts.get(index + 1).cloned(),
            None => Some(npm_command.command.clone()),
        };
        Ok(package_manager_command
            .map(|command| {
                let base = Path::new(&command)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or(command);
                let lowercase = base.to_lowercase();
                if lowercase.ends_with(".cmd") || lowercase.ends_with(".exe") {
                    base[..base.len() - 4].to_owned()
                } else {
                    base
                }
            })
            .unwrap_or_default())
    }

    /// `runNpmCommand(args, options)`
    async fn run_npm_command(&self, args: &[String], cwd: Option<&str>) -> Result<()> {
        let npm_command = self.get_npm_command()?;
        let mut full_args = npm_command.args.clone();
        full_args.extend_from_slice(args);
        let options = match cwd {
            Some(cwd) => CommandOptions::with_cwd(cwd),
            None => CommandOptions::default(),
        };
        self.run_command(&npm_command.command, &full_args, options)
            .await
    }

    /// `getGitDependencyInstallArgs()`
    fn get_git_dependency_install_args(&self) -> Vec<String> {
        let configured_command = self.settings_manager.get_npm_command();
        if configured_command.is_some_and(|command| !command.is_empty()) {
            return vec!["install".to_owned()];
        }
        vec!["install".to_owned(), "--omit=dev".to_owned()]
    }

    /// `runNpmCommandSync(args)`
    fn run_npm_command_sync(&self, args: &[String]) -> Result<String> {
        let npm_command = self.get_npm_command()?;
        let mut full_args = npm_command.args.clone();
        full_args.extend_from_slice(args);
        self.run_command_sync(&npm_command.command, &full_args)
    }

    /// `getNpmInstallArgs(specs, installRoot)`
    fn get_npm_install_args(&self, specs: &[String], install_root: &str) -> Result<Vec<String>> {
        let package_manager_name = self.get_package_manager_name()?;
        // Extension packages run inside notagent and resolve notagent APIs through loader aliases.
        // Disable peer dependency resolution for managed installs (npm's --legacy-peer-deps, and
        // equivalent bun/pnpm settings) so package managers do not install or solve host-provided
        // @notagent/* peers. Stale auto-installed notagent peers can otherwise block updates.
        let mut args = vec!["install".to_owned()];
        args.extend_from_slice(specs);
        if package_manager_name == "bun" {
            args.extend([
                "--cwd".to_owned(),
                install_root.to_owned(),
                "--omit=peer".to_owned(),
            ]);
            return Ok(args);
        }
        if package_manager_name == "pnpm" {
            args.extend([
                "--prefix".to_owned(),
                install_root.to_owned(),
                "--config.auto-install-peers=false".to_owned(),
                "--config.strict-peer-dependencies=false".to_owned(),
                "--config.strict-dep-builds=false".to_owned(),
            ]);
            return Ok(args);
        }
        args.extend([
            "--prefix".to_owned(),
            install_root.to_owned(),
            "--legacy-peer-deps".to_owned(),
        ]);
        Ok(args)
    }

    /// `installNpm(source, scope, temporary)`
    async fn install_npm(
        &self,
        source: &NpmSource,
        scope: SourceScope,
        temporary: bool,
    ) -> Result<()> {
        let install_root = self.get_npm_install_root(scope, temporary)?;
        self.ensure_npm_project(&install_root);
        let args = self.get_npm_install_args(std::slice::from_ref(&source.spec), &install_root)?;
        self.run_npm_command(&args, None).await
    }

    /// `uninstallNpm(source, scope)`
    async fn uninstall_npm(&self, source: &NpmSource, scope: SourceScope) -> Result<()> {
        let install_root = self.get_npm_install_root(scope, false)?;
        if !Path::new(&install_root).exists() {
            return Ok(());
        }
        let package_manager_name = self.get_package_manager_name()?;
        if package_manager_name == "bun" {
            return self
                .run_npm_command(
                    &[
                        "uninstall".to_owned(),
                        source.name.clone(),
                        "--cwd".to_owned(),
                        install_root,
                    ],
                    None,
                )
                .await;
        }
        let mut args = vec![
            "uninstall".to_owned(),
            source.name.clone(),
            "--prefix".to_owned(),
            install_root,
        ];
        if package_manager_name != "pnpm" {
            args.push("--legacy-peer-deps".to_owned());
        }
        self.run_npm_command(&args, None).await
    }

    /// `installGit(source, scope)`
    fn install_git<'a>(
        &'a self,
        source: &'a GitSource,
        scope: SourceScope,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let target_dir = self.get_git_install_path(source, scope)?;
            if Path::new(&target_dir).exists() {
                if let Some(git_ref) = source.ref_name.as_ref() {
                    return self
                        .ensure_git_ref(
                            &target_dir,
                            &["fetch".to_owned(), "origin".to_owned(), git_ref.clone()],
                            "FETCH_HEAD",
                        )
                        .await;
                }
                let target = self.get_local_git_update_target(&target_dir).await;
                return self
                    .ensure_git_ref(&target_dir, &target.fetch_args, &target.git_ref)
                    .await;
            }
            let git_root = self.get_git_install_root(scope)?;
            if let Some(git_root) = git_root.as_ref() {
                ensure_git_ignore(git_root);
            }
            if let Some(parent) = Path::new(&target_dir).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::remove_file(git_update_marker_path(&target_dir));

            let clone_result = async {
                self.run_command(
                    "git",
                    &["clone".to_owned(), source.repo.clone(), target_dir.clone()],
                    CommandOptions::default(),
                )
                .await?;
                if let Some(git_ref) = source.ref_name.as_ref() {
                    self.run_command(
                        "git",
                        &["checkout".to_owned(), git_ref.clone()],
                        CommandOptions::with_cwd(&target_dir),
                    )
                    .await?;
                }
                if Path::new(&target_dir).join("package.json").exists() {
                    self.run_npm_command(
                        &self.get_git_dependency_install_args(),
                        Some(&target_dir),
                    )
                    .await?;
                }
                Ok(())
            }
            .await;

            if let Err(error) = clone_result {
                let _ = std::fs::remove_dir_all(&target_dir);
                self.prune_empty_git_parents(&target_dir, git_root.as_deref());
                return Err(error);
            }
            Ok(())
        })
    }

    /// `updateGit(source, scope)`
    fn update_git<'a>(
        &'a self,
        source: &'a GitSource,
        scope: SourceScope,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let target_dir = self.get_git_install_path(source, scope)?;
            if !Path::new(&target_dir).exists() {
                return self.install_git(source, scope).await;
            }

            if let Some(git_ref) = source.ref_name.as_ref() {
                return self
                    .ensure_git_ref(
                        &target_dir,
                        &["fetch".to_owned(), "origin".to_owned(), git_ref.clone()],
                        "FETCH_HEAD",
                    )
                    .await;
            }

            let target = self.get_local_git_update_target(&target_dir).await;
            self.ensure_git_ref(&target_dir, &target.fetch_args, &target.git_ref)
                .await
        })
    }

    /// `repairMissingGitDependencies(targetDir)`
    async fn repair_missing_git_dependencies(&self, target_dir: &str) -> Result<()> {
        if !has_missing_git_dependencies(target_dir) {
            return Ok(());
        }
        self.run_npm_command(&self.get_git_dependency_install_args(), Some(target_dir))
            .await
    }

    /// `cleanAndInstallGitDependencies(targetDir, markerPath)`
    async fn clean_and_install_git_dependencies(
        &self,
        target_dir: &str,
        marker_path: &str,
    ) -> Result<()> {
        // Clean untracked files (packages should be pristine). If this fails after
        // deleting dependencies, repair them so the existing package still loads.
        if let Err(error) = self
            .run_command(
                "git",
                &["clean".to_owned(), "-fdx".to_owned()],
                CommandOptions::with_cwd(target_dir),
            )
            .await
        {
            let _ = self.repair_missing_git_dependencies(target_dir).await;
            return Err(error);
        }

        if Path::new(target_dir).join("package.json").exists() {
            self.run_npm_command(&self.get_git_dependency_install_args(), Some(target_dir))
                .await?;
        }
        let _ = std::fs::remove_file(marker_path);
        Ok(())
    }

    /// `ensureGitRef(targetDir, fetchArgs, ref)`
    async fn ensure_git_ref(
        &self,
        target_dir: &str,
        fetch_args: &[String],
        git_ref: &str,
    ) -> Result<()> {
        // Fetch only the ref we will reset to, avoiding unrelated branch/tag noise.
        self.run_command("git", fetch_args, CommandOptions::with_cwd(target_dir))
            .await?;

        let local_head = self
            .run_command_capture(
                "git",
                &["rev-parse".to_owned(), "HEAD".to_owned()],
                CommandOptions {
                    cwd: Some(target_dir.to_owned()),
                    timeout_ms: Some(NETWORK_TIMEOUT_MS),
                    ..CommandOptions::default()
                },
            )
            .await?;
        let commit_ref = format!("{git_ref}^{{commit}}");
        let target_head = self
            .run_command_capture(
                "git",
                &["rev-parse".to_owned(), commit_ref.clone()],
                CommandOptions {
                    cwd: Some(target_dir.to_owned()),
                    timeout_ms: Some(NETWORK_TIMEOUT_MS),
                    ..CommandOptions::default()
                },
            )
            .await?;
        let marker_path = git_update_marker_path(target_dir);
        if local_head.trim() == target_head.trim() {
            if Path::new(&marker_path).exists() {
                return self
                    .clean_and_install_git_dependencies(target_dir, &marker_path)
                    .await;
            }
            return self.repair_missing_git_dependencies(target_dir).await;
        }

        let _ = std::fs::write(&marker_path, "");
        self.run_command(
            "git",
            &["reset".to_owned(), "--hard".to_owned(), commit_ref],
            CommandOptions::with_cwd(target_dir),
        )
        .await?;
        self.clean_and_install_git_dependencies(target_dir, &marker_path)
            .await
    }

    /// `refreshTemporaryGitSource(source, sourceStr)`
    async fn refresh_temporary_git_source(&self, source: &GitSource, source_str: &str) {
        if is_offline_mode_enabled() {
            return;
        }
        // Keep cached temporary checkout if refresh fails.
        let _ = self
            .with_progress(
                ProgressAction::Pull,
                source_str,
                &format!("Refreshing {source_str}..."),
                self.update_git(source, SourceScope::Temporary),
            )
            .await;
    }

    /// `removeGit(source, scope)`
    fn remove_git(&self, source: &GitSource, scope: SourceScope) -> Result<()> {
        let target_dir = self.get_git_install_path(source, scope)?;
        let _ = std::fs::remove_dir_all(&target_dir);
        let _ = std::fs::remove_file(git_update_marker_path(&target_dir));
        let install_root = self.get_git_install_root(scope)?;
        self.prune_empty_git_parents(&target_dir, install_root.as_deref());
        Ok(())
    }

    /// `pruneEmptyGitParents(targetDir, installRoot)`
    fn prune_empty_git_parents(&self, target_dir: &str, install_root: Option<&str>) {
        let Some(install_root) = install_root else {
            return;
        };
        let resolved_root = self.resolve_path(install_root);
        let mut current = dirname_string(target_dir);
        while current.starts_with(&resolved_root) && current != resolved_root {
            if !Path::new(&current).exists() {
                current = dirname_string(&current);
                continue;
            }
            let entries = std::fs::read_dir(&current)
                .map(|entries| entries.flatten().count())
                .unwrap_or(0);
            if entries > 0 {
                break;
            }
            if std::fs::remove_dir_all(&current).is_err() {
                break;
            }
            current = dirname_string(&current);
        }
    }

    /// `ensureNpmProject(installRoot)`
    fn ensure_npm_project(&self, install_root: &str) {
        if !Path::new(install_root).exists() {
            let _ = std::fs::create_dir_all(install_root);
        }
        mark_path_ignored_by_cloud_sync(install_root);
        ensure_git_ignore(install_root);
        let package_json_path = Path::new(install_root).join("package.json");
        if !package_json_path.exists() {
            let package_json =
                serde_json::json!({ "name": "notagent-extensions", "private": true });
            let _ = std::fs::write(
                &package_json_path,
                serde_json::to_string_pretty(&package_json).unwrap_or_default(),
            );
        }
    }

    /// `getNpmInstallRoot(scope, temporary)`
    fn get_npm_install_root(&self, scope: SourceScope, temporary: bool) -> Result<String> {
        if temporary {
            return self.get_temporary_dir("npm", None);
        }
        if scope == SourceScope::Project {
            self.assert_project_trusted_for_scope(scope)?;
            return Ok(join(&self.cwd, &[CONFIG_DIR_NAME, "npm"]));
        }
        Ok(join(&self.agent_dir, &["npm"]))
    }

    /// `getGlobalNpmRoot()`
    fn get_global_npm_root(&self) -> Result<String> {
        let npm_command = self.get_npm_command()?;
        let mut key_parts = vec![npm_command.command.clone()];
        key_parts.extend(npm_command.args.clone());
        let command_key = key_parts.join("\0");
        if let Some((cached_root, cached_key)) =
            self.global_npm_root.lock().expect("poisoned").clone()
            && cached_key == command_key
        {
            return Ok(cached_root);
        }
        let root = if self.get_package_manager_name()? == "bun" {
            let bin_dir = self
                .run_npm_command_sync(&["pm".to_owned(), "bin".to_owned(), "-g".to_owned()])?
                .trim()
                .to_owned();
            join(
                &dirname_string(&bin_dir),
                &["install", "global", "node_modules"],
            )
        } else {
            self.run_npm_command_sync(&["root".to_owned(), "-g".to_owned()])?
                .trim()
                .to_owned()
        };
        *self.global_npm_root.lock().expect("poisoned") = Some((root.clone(), command_key));
        Ok(root)
    }

    /// `getPnpmGlobalPackagePath(packageName)`
    fn get_pnpm_global_package_path(&self, package_name: &str) -> Result<Option<String>> {
        if self.get_package_manager_name()? != "pnpm" {
            return Ok(None);
        }

        let output = self.run_npm_command_sync(&[
            "list".to_owned(),
            "-g".to_owned(),
            "--depth".to_owned(),
            "0".to_owned(),
            "--json".to_owned(),
        ])?;
        let entries: serde_json::Value = serde_json::from_str(&output)
            .map_err(|error| PackageManagerError::new(error.to_string()))?;
        let Some(entries) = entries.as_array() else {
            return Err(PackageManagerError::new(
                "Unexpected response from package manager list",
            ));
        };
        for entry in entries {
            if let Some(path) = entry
                .get("dependencies")
                .and_then(|dependencies| dependencies.get(package_name))
                .and_then(|dependency| dependency.get("path"))
                .and_then(serde_json::Value::as_str)
            {
                return Ok(Some(path.to_owned()));
            }
        }
        Ok(None)
    }

    /// `getManagedNpmInstallPath(source, scope)`
    fn get_managed_npm_install_path(
        &self,
        source: &NpmSource,
        scope: SourceScope,
    ) -> Result<String> {
        if scope == SourceScope::Temporary {
            let temp_dir = self.get_temporary_dir("npm", None)?;
            return Ok(join(&temp_dir, &["node_modules", &source.name]));
        }
        if scope == SourceScope::Project {
            self.assert_project_trusted_for_scope(scope)?;
            return Ok(join(
                &self.cwd,
                &[CONFIG_DIR_NAME, "npm", "node_modules", &source.name],
            ));
        }
        Ok(join(
            &self.agent_dir,
            &["npm", "node_modules", &source.name],
        ))
    }

    /// `getLegacyGlobalNpmInstallPath(source)`
    fn get_legacy_global_npm_install_path(&self, source: &NpmSource) -> Option<String> {
        match self.get_pnpm_global_package_path(&source.name) {
            Ok(Some(path)) => Some(path),
            Ok(None) => self
                .get_global_npm_root()
                .ok()
                .map(|root| join(&root, &[&source.name])),
            Err(_) => None,
        }
    }

    /// `getNpmInstallPath(source, scope)`
    pub fn get_npm_install_path(&self, source: &NpmSource, scope: SourceScope) -> Result<String> {
        let managed_path = self.get_managed_npm_install_path(source, scope)?;
        if scope != SourceScope::User || Path::new(&managed_path).exists() {
            return Ok(managed_path);
        }
        let legacy_path = self.get_legacy_global_npm_install_path(source);
        Ok(match legacy_path {
            Some(legacy_path) if Path::new(&legacy_path).exists() => legacy_path,
            _ => managed_path,
        })
    }

    /// `getGitInstallPath(source, scope)`
    pub fn get_git_install_path(&self, source: &GitSource, scope: SourceScope) -> Result<String> {
        if scope == SourceScope::Temporary {
            return self.get_temporary_dir(&format!("git-{}", source.host), Some(&source.path));
        }
        let install_root = self.get_git_install_root(scope)?;
        let Some(install_root) = install_root else {
            return Err(PackageManagerError::new("Missing git install root"));
        };
        self.resolve_managed_path(&install_root, &[&source.host, &source.path])
    }

    /// `getGitInstallRoot(scope)`
    fn get_git_install_root(&self, scope: SourceScope) -> Result<Option<String>> {
        if scope == SourceScope::Temporary {
            return Ok(None);
        }
        if scope == SourceScope::Project {
            self.assert_project_trusted_for_scope(scope)?;
            return Ok(Some(join(&self.cwd, &[CONFIG_DIR_NAME, "git"])));
        }
        Ok(Some(join(&self.agent_dir, &["git"])))
    }

    /// `getTemporaryDir(prefix, suffix)`
    fn get_temporary_dir(&self, prefix: &str, suffix: Option<&str>) -> Result<String> {
        let temp_folder = get_extension_temp_folder(&self.agent_dir);
        let root = self.resolve_managed_path(&temp_folder.to_string_lossy(), &[prefix])?;
        let digest = Sha256::digest(format!("{prefix}-{}", suffix.unwrap_or("")).as_bytes());
        let hash: String = digest
            .iter()
            .flat_map(|byte| [byte >> 4, byte & 0x0f])
            .take(8)
            .map(|nibble| char::from_digit(u32::from(nibble), 16).unwrap_or('0'))
            .collect();
        self.resolve_managed_path(&root, &[&hash, suffix.unwrap_or("")])
    }

    /// `resolveManagedPath(root, ...parts)`
    fn resolve_managed_path(&self, root: &str, parts: &[&str]) -> Result<String> {
        let resolved_root = self.resolve_path_absolute(root);
        let mut segments: Vec<&str> = vec![resolved_root.as_str()];
        segments.extend_from_slice(parts);
        let resolved_path = crate::utils::paths::node_resolve(&segments);
        if resolved_path != resolved_root
            && !resolved_path.starts_with(&format!(
                "{resolved_root}{}",
                crate::utils::paths::main_separator()
            ))
        {
            return Err(PackageManagerError::new(format!(
                "Refusing to use path outside package install root: {resolved_path}"
            )));
        }
        Ok(resolved_path)
    }

    /// `getBaseDirForScope(scope)`
    fn get_base_dir_for_scope(&self, scope: SourceScope) -> Result<String> {
        if scope == SourceScope::Project {
            self.assert_project_trusted_for_scope(scope)?;
            return Ok(join(&self.cwd, &[CONFIG_DIR_NAME]));
        }
        if scope == SourceScope::User {
            return Ok(self.agent_dir.clone());
        }
        Ok(self.cwd.clone())
    }

    /// `resolvePath(input)`
    fn resolve_path(&self, input: &str) -> String {
        self.resolve_path_from_base(input, &self.cwd)
    }

    /// `resolvePathFromBase(input, baseDir)`
    fn resolve_path_from_base(&self, input: &str, base_dir: &str) -> String {
        resolve_path(
            input,
            base_dir,
            &PathInputOptions {
                trim: true,
                home_dir: Some(get_home_dir()),
                ..PathInputOptions::default()
            },
        )
        .unwrap_or_else(|_| input.to_owned())
    }

    /// `resolve(root)` of `resolveManagedPath`.
    fn resolve_path_absolute(&self, input: &str) -> String {
        crate::utils::paths::node_resolve(&[input])
    }

    /// `collectPackageResources(packageRoot, accumulator, filter, metadata)`
    fn collect_package_resources(
        &self,
        package_root: &Path,
        accumulator: &mut ResourceAccumulator,
        filter: Option<&PackageFilter>,
        metadata: &PathMetadata,
    ) -> bool {
        if let Some(filter) = filter {
            for resource_type in ResourceType::ALL {
                let patterns = filter.entries(resource_type).cloned();
                let target = accumulator.target(resource_type);
                if filter.autoload == Some(false) {
                    apply_package_delta_filter(
                        package_root,
                        &patterns.unwrap_or_default(),
                        resource_type,
                        target,
                        metadata,
                    );
                } else if let Some(patterns) = patterns {
                    apply_package_filter(package_root, &patterns, resource_type, target, metadata);
                } else {
                    collect_default_resources(package_root, resource_type, target, metadata);
                }
            }
            return true;
        }

        let manifest = read_pi_manifest(&package_root.join("package.json"));
        if let Some(manifest) = manifest {
            for resource_type in ResourceType::ALL {
                let entries = manifest.entries(resource_type).map(<[String]>::to_vec);
                add_manifest_entries(
                    entries.as_deref(),
                    package_root,
                    resource_type,
                    accumulator.target(resource_type),
                    metadata,
                );
            }
            return true;
        }

        let mut has_any_dir = false;
        for resource_type in ResourceType::ALL {
            let dir = package_root.join(resource_type.as_str());
            if dir.exists() {
                // Collect all files from the directory (all enabled by default)
                let files = collect_resource_files(&dir, resource_type);
                let target = accumulator.target(resource_type);
                for file in files {
                    add_resource(target, &file, metadata, true);
                }
                has_any_dir = true;
            }
        }
        has_any_dir
    }

    /// `resolveLocalEntries(entries, resourceType, target, metadata, baseDir)`
    fn resolve_local_entries(
        &self,
        entries: &[String],
        resource_type: ResourceType,
        target: &mut Vec<(String, AccumulatedResource)>,
        metadata: &PathMetadata,
        base_dir: &str,
    ) {
        if entries.is_empty() {
            return;
        }

        // Collect all files from plain entries (non-pattern entries)
        let (plain, patterns) = split_patterns(entries);
        let resolved_plain: Vec<String> = plain
            .iter()
            .map(|entry| self.resolve_path_from_base(entry, base_dir))
            .collect();
        let all_files = collect_files_from_paths(&resolved_plain, resource_type);

        // Determine which files are enabled based on patterns
        let enabled_paths = apply_patterns(&all_files, &patterns, base_dir);

        // Add all files with their enabled state
        for file in &all_files {
            add_resource(target, file, metadata, contains_path(&enabled_paths, file));
        }
    }

    /// `addAutoDiscoveredResources(...)`
    fn add_auto_discovered_resources(
        &self,
        accumulator: &mut ResourceAccumulator,
        global_settings: &Settings,
        project_settings: &Settings,
        global_base_dir: &str,
        project_base_dir: &str,
    ) {
        let user_metadata = PathMetadata {
            source: "auto".to_owned(),
            scope: SourceScope::User,
            origin: SourceOrigin::TopLevel,
            base_dir: Some(global_base_dir.to_owned()),
        };
        let project_metadata = PathMetadata {
            source: "auto".to_owned(),
            scope: SourceScope::Project,
            origin: SourceOrigin::TopLevel,
            base_dir: Some(project_base_dir.to_owned()),
        };

        let user_agents_skills_dir = Path::new(&get_home_dir()).join(".agents").join("skills");
        let project_trusted = self.settings_manager.is_project_trusted();
        let project_agents_skill_dirs: Vec<PathBuf> = if project_trusted {
            collect_ancestor_agents_skill_dirs(Path::new(&self.cwd))
                .into_iter()
                .filter(|dir| {
                    crate::utils::paths::node_resolve(&[&dir.to_string_lossy()])
                        != crate::utils::paths::node_resolve(&[
                            &user_agents_skills_dir.to_string_lossy()
                        ])
                })
                .collect()
        } else {
            Vec::new()
        };

        let add_resources = |accumulator: &mut ResourceAccumulator,
                             resource_type: ResourceType,
                             paths: Vec<String>,
                             metadata: &PathMetadata,
                             overrides: &[String],
                             base_dir: &str| {
            let target = accumulator.target(resource_type);
            for path in paths {
                let enabled = is_enabled_by_overrides(&path, overrides, base_dir);
                add_resource(target, &path, metadata, enabled);
            }
        };

        let user_overrides = |resource_type| settings_entries(global_settings, resource_type);
        let project_overrides = |resource_type| settings_entries(project_settings, resource_type);

        if project_trusted {
            // Project skills from .notagent/
            add_resources(
                accumulator,
                ResourceType::Skills,
                collect_skill_entries(
                    &Path::new(project_base_dir).join("skills"),
                    SkillDiscoveryMode::Notagent,
                ),
                &project_metadata,
                &project_overrides(ResourceType::Skills),
                project_base_dir,
            );
        }

        // Project skills from .agents/ (each with its own baseDir)
        for agents_skills_dir in &project_agents_skill_dirs {
            let agents_base_dir = dirname_string(&agents_skills_dir.to_string_lossy());
            let agents_metadata = PathMetadata {
                base_dir: Some(agents_base_dir.clone()),
                ..project_metadata.clone()
            };
            add_resources(
                accumulator,
                ResourceType::Skills,
                collect_skill_entries(agents_skills_dir, SkillDiscoveryMode::Agents),
                &agents_metadata,
                &project_overrides(ResourceType::Skills),
                &agents_base_dir,
            );
        }

        if project_trusted {
            add_resources(
                accumulator,
                ResourceType::Prompts,
                collect_auto_prompt_entries(&Path::new(project_base_dir).join("prompts")),
                &project_metadata,
                &project_overrides(ResourceType::Prompts),
                project_base_dir,
            );
            add_resources(
                accumulator,
                ResourceType::Themes,
                collect_auto_theme_entries(&Path::new(project_base_dir).join("themes")),
                &project_metadata,
                &project_overrides(ResourceType::Themes),
                project_base_dir,
            );
        }

        // User skills from ~/.notagent/agent/
        add_resources(
            accumulator,
            ResourceType::Skills,
            collect_skill_entries(
                &Path::new(global_base_dir).join("skills"),
                SkillDiscoveryMode::Notagent,
            ),
            &user_metadata,
            &user_overrides(ResourceType::Skills),
            global_base_dir,
        );

        // User skills from ~/.agents/ (with its own baseDir)
        let user_agents_base_dir = dirname_string(&user_agents_skills_dir.to_string_lossy());
        let user_agents_metadata = PathMetadata {
            base_dir: Some(user_agents_base_dir.clone()),
            ..user_metadata.clone()
        };
        add_resources(
            accumulator,
            ResourceType::Skills,
            collect_skill_entries(&user_agents_skills_dir, SkillDiscoveryMode::Agents),
            &user_agents_metadata,
            &user_overrides(ResourceType::Skills),
            &user_agents_base_dir,
        );

        add_resources(
            accumulator,
            ResourceType::Prompts,
            collect_auto_prompt_entries(&Path::new(global_base_dir).join("prompts")),
            &user_metadata,
            &user_overrides(ResourceType::Prompts),
            global_base_dir,
        );
        add_resources(
            accumulator,
            ResourceType::Themes,
            collect_auto_theme_entries(&Path::new(global_base_dir).join("themes")),
            &user_metadata,
            &user_overrides(ResourceType::Themes),
            global_base_dir,
        );
    }

    /// `runCommand(command, args, options)`
    async fn run_command(
        &self,
        command: &str,
        args: &[String],
        options: CommandOptions,
    ) -> Result<()> {
        self.runner
            .run(command.to_owned(), args.to_vec(), options)
            .await
            .map_err(PackageManagerError)
    }

    /// `runCommandCapture(command, args, options)`
    async fn run_command_capture(
        &self,
        command: &str,
        args: &[String],
        options: CommandOptions,
    ) -> Result<String> {
        self.runner
            .run_capture(command.to_owned(), args.to_vec(), options)
            .await
            .map_err(PackageManagerError)
    }

    /// `runCommandSync(command, args)`
    fn run_command_sync(&self, command: &str, args: &[String]) -> Result<String> {
        self.runner
            .run_sync(command, args)
            .map_err(PackageManagerError)
    }
}

impl PackageResources for DefaultPackageManager {
    /// Deviation (class 1): the trait C defined for the resource loader
    /// (interface request C-11) cannot report an error, while the TypeScript
    /// `resolve()` propagates a failed install to the loader. A failed resolve
    /// therefore yields an empty set here.
    fn resolve(&self) -> BoxFuture<'_, ResolvedResources> {
        Box::pin(async move { self.resolve(None).await.unwrap_or_default() })
    }
}

/// `{ command, args }` of `getNpmCommand`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NpmCommand {
    command: String,
    args: Vec<String>,
}

/// `getPackageSourceString(pkg)`
fn package_source_string(package: &PackageSource) -> String {
    match package {
        PackageSource::Source(source) => source.clone(),
        PackageSource::Filtered(filter) => filter.source.clone(),
    }
}

/// `settings[resourceType] ?? []`
fn settings_entries(settings: &Settings, resource_type: ResourceType) -> Vec<String> {
    match resource_type {
        ResourceType::Skills => settings.skills.clone(),
        ResourceType::Prompts => settings.prompts.clone(),
        ResourceType::Themes => settings.themes.clone(),
    }
    .unwrap_or_default()
}

/// `parseNpmSpec(spec)`
fn parse_npm_spec(spec: &str) -> (String, Option<String>) {
    let pattern = Regex::new(r"^(@?[^@]+(?:/[^@]+)?)(?:@(.+))?$").expect("valid regex");
    let Some(captures) = pattern.captures(spec) else {
        return (spec.to_owned(), None);
    };
    let name = captures
        .get(1)
        .map_or_else(|| spec.to_owned(), |group| group.as_str().to_owned());
    let version = captures.get(2).map(|group| group.as_str().to_owned());
    (name, version)
}

/// `getInstalledNpmVersion(installedPath)`
fn get_installed_npm_version(installed_path: &str) -> Option<String> {
    let package_json_path = Path::new(installed_path).join("package.json");
    if !package_json_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&package_json_path).ok()?;
    let package: serde_json::Value = serde_json::from_str(&content).ok()?;
    package
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// `installedNpmMatchesConfiguredVersion(source, installedPath)`
fn installed_npm_matches_configured_version(source: &NpmSource, installed_path: &str) -> bool {
    let Some(installed_version) = get_installed_npm_version(installed_path) else {
        return false;
    };
    match source.range.as_ref() {
        Some(range) => satisfies(&installed_version, range),
        None => true,
    }
}

/// `hasMissingGitDependencies(targetDir)`
fn has_missing_git_dependencies(target_dir: &str) -> bool {
    let package_json_path = Path::new(target_dir).join("package.json");
    if !package_json_path.exists() {
        return false;
    }

    let Ok(content) = std::fs::read_to_string(&package_json_path) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    let Some(dependencies) = manifest
        .get("dependencies")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };

    let node_modules_dir = crate::utils::paths::node_resolve(&[target_dir, "node_modules"]);
    let prefix = format!(
        "{node_modules_dir}{}",
        crate::utils::paths::main_separator()
    );
    dependencies.keys().any(|name| {
        let dependency_path = crate::utils::paths::node_resolve(&[&node_modules_dir, name]);
        if !dependency_path.starts_with(&prefix) {
            return false;
        }
        !Path::new(&dependency_path).exists()
    })
}

/// `getGitUpdateMarkerPath(targetDir)`
fn git_update_marker_path(target_dir: &str) -> String {
    let parent = dirname_string(target_dir);
    let base = Path::new(target_dir)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    join(&parent, &[&format!(".{base}.notagent-update-incomplete")])
}

/// `ensureGitIgnore(dir)`
fn ensure_git_ignore(dir: &str) {
    if !Path::new(dir).exists() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ignore_path = Path::new(dir).join(".gitignore");
    if !ignore_path.exists() {
        let _ = std::fs::write(&ignore_path, "*\n!.gitignore\n");
    }
}

/// `collectDefaultResources(packageRoot, resourceType, target, metadata)`
fn collect_default_resources(
    package_root: &Path,
    resource_type: ResourceType,
    target: &mut Vec<(String, AccumulatedResource)>,
    metadata: &PathMetadata,
) {
    let manifest = read_pi_manifest(&package_root.join("package.json"));
    let entries = manifest
        .as_ref()
        .and_then(|manifest| manifest.entries(resource_type))
        .map(<[String]>::to_vec);
    if let Some(entries) = entries {
        add_manifest_entries(
            Some(&entries),
            package_root,
            resource_type,
            target,
            metadata,
        );
        return;
    }
    let dir = package_root.join(resource_type.as_str());
    if dir.exists() {
        // Collect all files from the directory (all enabled by default)
        for file in collect_resource_files(&dir, resource_type) {
            add_resource(target, &file, metadata, true);
        }
    }
}

/// `applyPackageFilter(packageRoot, userPatterns, resourceType, target, metadata)`
fn apply_package_filter(
    package_root: &Path,
    user_patterns: &[String],
    resource_type: ResourceType,
    target: &mut Vec<(String, AccumulatedResource)>,
    metadata: &PathMetadata,
) {
    let all_files = collect_manifest_files(package_root, resource_type);
    let package_root_str = package_root.to_string_lossy().into_owned();

    if user_patterns.is_empty() {
        // Empty array explicitly disables all resources of this type
        for file in &all_files {
            add_resource(target, file, metadata, false);
        }
        return;
    }

    // Apply user patterns
    let enabled_by_user = apply_patterns(&all_files, user_patterns, &package_root_str);

    for file in &all_files {
        let enabled = contains_path(&enabled_by_user, file);
        add_resource(target, file, metadata, enabled);
    }
}

/// `applyPackageDeltaFilter(packageRoot, userPatterns, resourceType, target, metadata)`
fn apply_package_delta_filter(
    package_root: &Path,
    user_patterns: &[String],
    resource_type: ResourceType,
    target: &mut Vec<(String, AccumulatedResource)>,
    metadata: &PathMetadata,
) {
    if user_patterns.is_empty() {
        return;
    }

    let all_files = collect_manifest_files(package_root, resource_type);
    let package_root_str = package_root.to_string_lossy().into_owned();
    let enabled_by_user =
        apply_autoload_disabled_patterns(&all_files, user_patterns, &package_root_str);
    for (file_path, enabled) in enabled_by_user {
        add_resource(target, &file_path, metadata, enabled);
    }
}

/// `collectManifestFiles(packageRoot, resourceType)`
///
/// Returns the TypeScript's `allFiles`; the second half of its return value
/// (`enabledByManifest`) is the same list in both branches.
fn collect_manifest_files(package_root: &Path, resource_type: ResourceType) -> Vec<String> {
    let manifest = read_pi_manifest(&package_root.join("package.json"));
    let entries = manifest
        .as_ref()
        .and_then(|manifest: &PiManifest| manifest.entries(resource_type))
        .map(<[String]>::to_vec)
        .filter(|entries| !entries.is_empty());
    if let Some(entries) = entries {
        let all_files = collect_files_from_manifest_entries(&entries, package_root, resource_type);
        let manifest_patterns: Vec<String> = entries
            .iter()
            .filter(|entry| is_override_pattern(entry))
            .cloned()
            .collect();
        return if manifest_patterns.is_empty() {
            all_files
        } else {
            apply_patterns(
                &all_files,
                &manifest_patterns,
                &package_root.to_string_lossy(),
            )
        };
    }

    let convention_dir = package_root.join(resource_type.as_str());
    if !convention_dir.exists() {
        return Vec::new();
    }
    collect_resource_files(&convention_dir, resource_type)
}

/// `addManifestEntries(entries, root, resourceType, target, metadata)`
fn add_manifest_entries(
    entries: Option<&[String]>,
    root: &Path,
    resource_type: ResourceType,
    target: &mut Vec<(String, AccumulatedResource)>,
    metadata: &PathMetadata,
) {
    let Some(entries) = entries else {
        return;
    };

    let all_files = collect_files_from_manifest_entries(entries, root, resource_type);
    let patterns: Vec<String> = entries
        .iter()
        .filter(|entry| is_override_pattern(entry))
        .cloned()
        .collect();
    let enabled_paths = apply_patterns(&all_files, &patterns, &root.to_string_lossy());

    for file in &all_files {
        if contains_path(&enabled_paths, file) {
            add_resource(target, file, metadata, true);
        }
    }
}

/// `collectFilesFromManifestEntries(entries, root, resourceType)`
fn collect_files_from_manifest_entries(
    entries: &[String],
    root: &Path,
    resource_type: ResourceType,
) -> Vec<String> {
    let root_str = root.to_string_lossy().into_owned();
    let mut resolved: Vec<String> = Vec::new();
    for entry in entries.iter().filter(|entry| !is_override_pattern(entry)) {
        if has_glob_pattern(entry) {
            resolved.extend(
                glob_sync(entry, root)
                    .iter()
                    .map(|matched| crate::utils::paths::node_resolve(&[matched])),
            );
        } else {
            resolved.push(crate::utils::paths::node_resolve(&[&root_str, entry]));
        }
    }
    collect_files_from_paths(&resolved, resource_type)
}

/// `collectFilesFromPaths(paths, resourceType)`
fn collect_files_from_paths(paths: &[String], resource_type: ResourceType) -> Vec<String> {
    let mut files = Vec::new();
    for path in paths {
        let candidate = Path::new(path);
        if !candidate.exists() {
            continue;
        }
        let Ok(stats) = std::fs::metadata(candidate) else {
            continue;
        };
        if stats.is_file() {
            files.push(path.clone());
        } else if stats.is_dir() {
            files.extend(collect_resource_files(candidate, resource_type));
        }
    }
    files
}

/// `toResolvedPaths(accumulator)`
fn to_resolved_paths(accumulator: ResourceAccumulator) -> ResolvedResources {
    ResolvedResources {
        skills: map_to_resolved(accumulator.skills),
        prompts: map_to_resolved(accumulator.prompts),
        themes: map_to_resolved(accumulator.themes),
    }
}

fn map_to_resolved(entries: Vec<(String, AccumulatedResource)>) -> Vec<ResolvedResource> {
    let mut resolved: Vec<ResolvedResource> = entries
        .into_iter()
        .map(|(path, entry)| ResolvedResource {
            path,
            enabled: entry.enabled,
            metadata: entry.metadata,
        })
        .collect();
    resolved.sort_by_key(|resource| resource_precedence_rank(&resource.metadata));

    let mut seen: Vec<String> = Vec::new();
    resolved.retain(|entry| {
        let canonical_path = canonicalize_path(&entry.path);
        if seen.contains(&canonical_path) {
            return false;
        }
        seen.push(canonical_path);
        true
    });
    resolved
}

/// `runWithConcurrency(tasks, limit)`
async fn run_with_concurrency<F, T>(tasks: Vec<F>, limit: usize) -> Vec<T>
where
    F: Future<Output = T>,
{
    if tasks.is_empty() {
        return Vec::new();
    }
    let worker_count = limit.clamp(1, tasks.len());
    futures::stream::iter(tasks)
        .buffered(worker_count)
        .collect()
        .await
}

/// `join(base, ...parts)` of `node:path`.
fn join(base: &str, parts: &[&str]) -> String {
    let mut path = PathBuf::from(base);
    for part in parts {
        path.push(part);
    }
    path.to_string_lossy().into_owned()
}

/// `dirname(path)` of `node:path`.
fn dirname_string(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The `^([0-9a-f]{40})\s+` / `^([0-9a-f]{40})\s+HEAD$` matches of `getRemoteGitHead`.
fn first_object_hash(output: &str, suffix: Option<&str>) -> Option<String> {
    for line in output.lines() {
        let mut parts = line.splitn(2, char::is_whitespace);
        let hash = parts.next()?;
        if hash.len() != 40
            || !hash
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
        {
            continue;
        }
        match suffix {
            None => {
                if line.len() > hash.len() {
                    return Some(hash.to_owned());
                }
            }
            Some(suffix) => {
                if parts.next().map(str::trim) == Some(suffix) {
                    return Some(hash.to_owned());
                }
            }
        }
    }
    None
}
