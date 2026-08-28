use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use super::patterns::to_posix_path;
use crate::core::notagent_manifest::ResourceType;
use crate::utils::paths::node_relative;

/// `IGNORE_FILE_NAMES`
const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

/// `SkillDiscoveryMode`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillDiscoveryMode {
    /// `<dir>/*.md` counts as a skill, next to every nested `SKILL.md`.
    Notagent,
    /// Only nested `SKILL.md` files count.
    Agents,
}

/// Deviation (class 3, master substitution for the npm package `ignore`): the
/// rules go into a `GitignoreBuilder`, rebuilt when a directory adds new ones.
/// sibling directory contributed stay visible afterwards.
pub struct IgnoreMatcher {
    root: PathBuf,
    lines: Vec<String>,
    compiled: Option<Gitignore>,
}

impl IgnoreMatcher {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            lines: Vec::new(),
            compiled: None,
        }
    }

    fn add(&mut self, patterns: Vec<String>) {
        if patterns.is_empty() {
            return;
        }
        self.lines.extend(patterns);
        self.compiled = None;
    }

    /// `ig.ignores(path)`
    fn ignores(&mut self, relative_path: &str, is_dir: bool) -> bool {
        if self.lines.is_empty() {
            return false;
        }
        if self.compiled.is_none() {
            let mut builder = GitignoreBuilder::new(&self.root);
            for line in &self.lines {
                let _ = builder.add_line(None, line);
            }
            self.compiled = builder.build().ok();
        }
        self.compiled
            .as_ref()
            .is_some_and(|gitignore| gitignore.matched(relative_path, is_dir).is_ignore())
    }
}

/// `prefixIgnorePattern(line, prefix)`
fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && !trimmed.starts_with("\\#") {
        return None;
    }

    let mut pattern = line;
    let mut negated = false;

    if let Some(rest) = pattern.strip_prefix('!') {
        negated = true;
        pattern = rest;
    } else if pattern.starts_with("\\!") {
        pattern = &pattern[1..];
    }

    if let Some(rest) = pattern.strip_prefix('/') {
        pattern = rest;
    }

    let prefixed = format!("{prefix}{pattern}");
    Some(if negated {
        format!("!{prefixed}")
    } else {
        prefixed
    })
}

/// `addIgnoreRules(ig, dir, rootDir)`
fn add_ignore_rules(matcher: &mut IgnoreMatcher, dir: &Path, root_dir: &Path) {
    let relative_dir = node_relative(&root_dir.to_string_lossy(), &dir.to_string_lossy());
    let prefix = if relative_dir.is_empty() {
        String::new()
    } else {
        format!("{}/", to_posix_path(&relative_dir))
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

/// What a directory entry turned out to be once symlinks are followed.
struct EntryKind {
    is_dir: bool,
    is_file: bool,
}

/// `entry.isDirectory()` / `entry.isFile()`, resolved through a symlink like
fn entry_kind(entry: &std::fs::DirEntry, full_path: &Path) -> Option<EntryKind> {
    let file_type = entry.file_type().ok()?;
    if file_type.is_symlink() {
        let stats = std::fs::metadata(full_path).ok()?;
        return Some(EntryKind {
            is_dir: stats.is_dir(),
            is_file: stats.is_file(),
        });
    }
    Some(EntryKind {
        is_dir: file_type.is_dir(),
        is_file: file_type.is_file(),
    })
}

/// The directory entries in `readdirSync` order, or an empty list on error.
fn read_dir_entries(dir: &Path) -> Vec<std::fs::DirEntry> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

fn file_name(entry: &std::fs::DirEntry) -> String {
    entry.file_name().to_string_lossy().into_owned()
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn relative_posix(root: &Path, path: &Path) -> String {
    to_posix_path(&node_relative(&path_string(root), &path_string(path)))
}

/// `collectFiles(dir, filePattern, skipNodeModules, ignoreMatcher, rootDir)`
fn collect_files_inner(
    dir: &Path,
    file_extension: &str,
    matcher: &mut IgnoreMatcher,
    root: &Path,
    files: &mut Vec<String>,
) {
    if !dir.exists() {
        return;
    }
    add_ignore_rules(matcher, dir, root);

    for entry in read_dir_entries(dir) {
        let name = file_name(&entry);
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }

        let full_path = dir.join(&name);
        let Some(kind) = entry_kind(&entry, &full_path) else {
            continue;
        };

        let relative_path = relative_posix(root, &full_path);
        if matcher.ignores(&relative_path, kind.is_dir) {
            continue;
        }

        if kind.is_dir {
            collect_files_inner(&full_path, file_extension, matcher, root, files);
        } else if kind.is_file && name.ends_with(file_extension) {
            files.push(path_string(&full_path));
        }
    }
}

/// `collectFiles(dir, FILE_PATTERNS[resourceType])`
pub fn collect_files(dir: &Path, file_extension: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut matcher = IgnoreMatcher::new(dir);
    collect_files_inner(dir, file_extension, &mut matcher, dir, &mut files);
    files
}

/// `collectSkillEntries(dir, mode, ignoreMatcher, rootDir)`
fn collect_skill_entries_inner(
    dir: &Path,
    mode: SkillDiscoveryMode,
    matcher: &mut IgnoreMatcher,
    root: &Path,
    entries: &mut Vec<String>,
) {
    if !dir.exists() {
        return;
    }
    add_ignore_rules(matcher, dir, root);

    let dir_entries = read_dir_entries(dir);

    for entry in &dir_entries {
        let name = file_name(entry);
        if name != "SKILL.md" {
            continue;
        }

        let full_path = dir.join(&name);
        let Some(kind) = entry_kind(entry, &full_path) else {
            continue;
        };

        let relative_path = relative_posix(root, &full_path);
        if kind.is_file && !matcher.ignores(&relative_path, false) {
            entries.push(path_string(&full_path));
            return;
        }
    }

    for entry in &dir_entries {
        let name = file_name(entry);
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }

        let full_path = dir.join(&name);
        let Some(kind) = entry_kind(entry, &full_path) else {
            continue;
        };

        let relative_path = relative_posix(root, &full_path);
        if mode == SkillDiscoveryMode::Notagent
            && dir == root
            && kind.is_file
            && name.ends_with(".md")
            && !matcher.ignores(&relative_path, false)
        {
            entries.push(path_string(&full_path));
            continue;
        }

        if !kind.is_dir {
            continue;
        }
        if matcher.ignores(&relative_path, true) {
            continue;
        }

        collect_skill_entries_inner(&full_path, mode, matcher, root, entries);
    }
}

/// `collectSkillEntries(dir, mode)` / `collectAutoSkillEntries(dir, mode)`
pub fn collect_skill_entries(dir: &Path, mode: SkillDiscoveryMode) -> Vec<String> {
    let mut entries = Vec::new();
    let mut matcher = IgnoreMatcher::new(dir);
    collect_skill_entries_inner(dir, mode, &mut matcher, dir, &mut entries);
    entries
}

/// The flat directory scan behind `collectAutoPromptEntries` and
/// `collectAutoThemeEntries` — the two differ only in the file extension.
fn collect_auto_entries(dir: &Path, file_extension: &str) -> Vec<String> {
    let mut entries = Vec::new();
    if !dir.exists() {
        return entries;
    }

    let mut matcher = IgnoreMatcher::new(dir);
    add_ignore_rules(&mut matcher, dir, dir);

    for entry in read_dir_entries(dir) {
        let name = file_name(&entry);
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }

        let full_path = dir.join(&name);
        let Some(kind) = entry_kind(&entry, &full_path) else {
            continue;
        };

        let relative_path = relative_posix(dir, &full_path);
        if matcher.ignores(&relative_path, false) {
            continue;
        }

        if kind.is_file && name.ends_with(file_extension) {
            entries.push(path_string(&full_path));
        }
    }

    entries
}

/// `collectAutoPromptEntries(dir)`
pub fn collect_auto_prompt_entries(dir: &Path) -> Vec<String> {
    collect_auto_entries(dir, ".md")
}

/// `collectAutoThemeEntries(dir)`
pub fn collect_auto_theme_entries(dir: &Path) -> Vec<String> {
    collect_auto_entries(dir, ".json")
}

/// `collectResourceFiles(dir, resourceType)`
pub fn collect_resource_files(dir: &Path, resource_type: ResourceType) -> Vec<String> {
    if resource_type == ResourceType::Skills {
        return collect_skill_entries(dir, SkillDiscoveryMode::Notagent);
    }
    collect_files(dir, resource_type.file_extension())
}

/// `findGitRepoRoot(startDir)`
fn find_git_repo_root(start_dir: &Path) -> Option<PathBuf> {
    let mut dir = start_dir.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        let parent = dir.parent()?.to_path_buf();
        if parent == dir {
            return None;
        }
        dir = parent;
    }
}

/// `collectAncestorAgentsSkillDirs(startDir)`
pub fn collect_ancestor_agents_skill_dirs(start_dir: &Path) -> Vec<PathBuf> {
    let mut skill_dirs = Vec::new();
    let git_repo_root = find_git_repo_root(start_dir);

    let mut dir = start_dir.to_path_buf();
    loop {
        skill_dirs.push(dir.join(".agents").join("skills"));
        if git_repo_root.as_ref().is_some_and(|root| *root == dir) {
            break;
        }
        let Some(parent) = dir.parent().map(Path::to_path_buf) else {
            break;
        };
        if parent == dir {
            break;
        }
        dir = parent;
    }

    skill_dirs
}
