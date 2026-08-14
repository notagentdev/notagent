//! Pattern matching of `packages/coding-agent/src/core/package-manager.ts`.
//!
//! The four pattern kinds a settings entry or a manifest entry can carry:
//! a plain include, `!exclude`, `+force-include` and `-force-exclude`. The
//! first two match through minimatch, the last two only match exactly.

use std::path::Path;

use globset::GlobBuilder;

use crate::utils::paths::node_relative;

/// `toPosixPath(p)`
pub fn to_posix_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn dirname(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `isPattern(s)`
pub fn is_pattern(entry: &str) -> bool {
    entry.starts_with('!')
        || entry.starts_with('+')
        || entry.starts_with('-')
        || entry.contains('*')
        || entry.contains('?')
}

/// `isOverridePattern(s)`
pub fn is_override_pattern(entry: &str) -> bool {
    entry.starts_with('!') || entry.starts_with('+') || entry.starts_with('-')
}

/// `hasGlobPattern(s)`
pub fn has_glob_pattern(entry: &str) -> bool {
    entry.contains('*') || entry.contains('?')
}

/// `splitPatterns(entries)`
pub fn split_patterns(entries: &[String]) -> (Vec<String>, Vec<String>) {
    let mut plain = Vec::new();
    let mut patterns = Vec::new();
    for entry in entries {
        if is_pattern(entry) {
            patterns.push(entry.clone());
        } else {
            plain.push(entry.clone());
        }
    }
    (plain, patterns)
}

/// `minimatch(value, pattern)`
///
/// Deviation (class 3, master substitution minimatch → globset): `literal_separator`
/// keeps minimatch's rule that `*` does not cross `/`, `**` does.
fn minimatch(value: &str, pattern: &str) -> bool {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(true)
        .empty_alternates(true)
        .build()
        .ok()
        .is_some_and(|glob| glob.compile_matcher().is_match(value))
}

struct SkillParent {
    relative: String,
    name: String,
    absolute: String,
}

/// The three extra spellings a `SKILL.md` path is matched under: its directory
/// relative to the base, that directory's name, and its absolute path.
fn skill_parent(file_path: &str, base_dir: &str) -> Option<SkillParent> {
    if basename(file_path) != "SKILL.md" {
        return None;
    }
    let parent = dirname(file_path);
    Some(SkillParent {
        relative: to_posix_path(&node_relative(base_dir, &parent)),
        name: basename(&parent),
        absolute: to_posix_path(&parent),
    })
}

/// `matchesAnyPattern(filePath, patterns, baseDir)`
pub fn matches_any_pattern(file_path: &str, patterns: &[String], base_dir: &str) -> bool {
    let relative = to_posix_path(&node_relative(base_dir, file_path));
    let name = basename(file_path);
    let absolute = to_posix_path(file_path);
    let parent = skill_parent(file_path, base_dir);

    patterns.iter().any(|pattern| {
        let normalized = to_posix_path(pattern);
        if minimatch(&relative, &normalized)
            || minimatch(&name, &normalized)
            || minimatch(&absolute, &normalized)
        {
            return true;
        }
        match parent.as_ref() {
            None => false,
            Some(parent) => {
                minimatch(&parent.relative, &normalized)
                    || minimatch(&parent.name, &normalized)
                    || minimatch(&parent.absolute, &normalized)
            }
        }
    })
}

/// `normalizeExactPattern(pattern)`
fn normalize_exact_pattern(pattern: &str) -> String {
    let normalized = pattern
        .strip_prefix("./")
        .or_else(|| pattern.strip_prefix(".\\"))
        .unwrap_or(pattern);
    to_posix_path(normalized)
}

/// `matchesAnyExactPattern(filePath, patterns, baseDir)`
pub fn matches_any_exact_pattern(file_path: &str, patterns: &[String], base_dir: &str) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let relative = to_posix_path(&node_relative(base_dir, file_path));
    let absolute = to_posix_path(file_path);
    let parent = skill_parent(file_path, base_dir);

    patterns.iter().any(|pattern| {
        let normalized = normalize_exact_pattern(pattern);
        if normalized == relative || normalized == absolute {
            return true;
        }
        match parent.as_ref() {
            None => false,
            Some(parent) => normalized == parent.relative || normalized == parent.absolute,
        }
    })
}

/// `getOverridePatterns(entries)`
pub fn get_override_patterns(entries: &[String]) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| is_override_pattern(entry))
        .cloned()
        .collect()
}

fn strip_prefix_of(patterns: &[String], prefix: char) -> Vec<String> {
    patterns
        .iter()
        .filter(|pattern| pattern.starts_with(prefix))
        .map(|pattern| pattern[1..].to_owned())
        .collect()
}

/// `isEnabledByOverrides(filePath, patterns, baseDir)`
pub fn is_enabled_by_overrides(file_path: &str, patterns: &[String], base_dir: &str) -> bool {
    let overrides = get_override_patterns(patterns);
    let excludes = strip_prefix_of(&overrides, '!');
    let force_includes = strip_prefix_of(&overrides, '+');
    let force_excludes = strip_prefix_of(&overrides, '-');

    let mut enabled = true;
    if !excludes.is_empty() && matches_any_pattern(file_path, &excludes, base_dir) {
        enabled = false;
    }
    if !force_includes.is_empty() && matches_any_exact_pattern(file_path, &force_includes, base_dir)
    {
        enabled = true;
    }
    if !force_excludes.is_empty() && matches_any_exact_pattern(file_path, &force_excludes, base_dir)
    {
        enabled = false;
    }
    enabled
}

/// `applyPatterns(allPaths, patterns, baseDir)`
///
/// Deviation (class 1): the TypeScript returns a `Set` whose insertion order one
/// caller reads back (`collectManifestFiles` turns it into `allFiles`), so the
/// port keeps a `Vec` — membership tests go through [`contains_path`].
pub fn apply_patterns(all_paths: &[String], patterns: &[String], base_dir: &str) -> Vec<String> {
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    let mut force_includes = Vec::new();
    let mut force_excludes = Vec::new();

    for pattern in patterns {
        if let Some(rest) = pattern.strip_prefix('+') {
            force_includes.push(rest.to_owned());
        } else if let Some(rest) = pattern.strip_prefix('-') {
            force_excludes.push(rest.to_owned());
        } else if let Some(rest) = pattern.strip_prefix('!') {
            excludes.push(rest.to_owned());
        } else {
            includes.push(pattern.clone());
        }
    }

    // Step 1: apply includes (or all if no includes)
    let mut result: Vec<String> = if includes.is_empty() {
        all_paths.to_vec()
    } else {
        all_paths
            .iter()
            .filter(|path| matches_any_pattern(path, &includes, base_dir))
            .cloned()
            .collect()
    };

    // Step 2: apply excludes
    if !excludes.is_empty() {
        result.retain(|path| !matches_any_pattern(path, &excludes, base_dir));
    }

    // Step 3: force-include (add back from allPaths, overriding exclusions)
    if !force_includes.is_empty() {
        for path in all_paths {
            if !result.contains(path) && matches_any_exact_pattern(path, &force_includes, base_dir)
            {
                result.push(path.clone());
            }
        }
    }

    // Step 4: force-exclude (remove even if included or force-included)
    if !force_excludes.is_empty() {
        result.retain(|path| !matches_any_exact_pattern(path, &force_excludes, base_dir));
    }

    result
}

/// `enabledPaths.has(f)` over the list [`apply_patterns`] returns.
pub fn contains_path(paths: &[String], path: &str) -> bool {
    paths.iter().any(|candidate| candidate == path)
}

/// `applyAutoloadDisabledPatterns(allPaths, patterns, baseDir)`
///
/// Deviation (class 1): the TypeScript `Map` is a `Vec` of pairs — the caller
/// iterates it in insertion order, and re-setting a key keeps its first slot.
pub fn apply_autoload_disabled_patterns(
    all_paths: &[String],
    patterns: &[String],
    base_dir: &str,
) -> Vec<(String, bool)> {
    let mut result: Vec<(String, bool)> = Vec::new();
    for pattern in patterns {
        let prefixed =
            pattern.starts_with('+') || pattern.starts_with('-') || pattern.starts_with('!');
        let target = if prefixed {
            pattern[1..].to_owned()
        } else {
            pattern.clone()
        };
        let enabled = !pattern.starts_with('-') && !pattern.starts_with('!');
        let exact = pattern.starts_with('+') || pattern.starts_with('-');
        let targets = [target];
        for file_path in all_paths {
            let matched = if exact {
                matches_any_exact_pattern(file_path, &targets, base_dir)
            } else {
                matches_any_pattern(file_path, &targets, base_dir)
            };
            if !matched {
                continue;
            }
            match result.iter_mut().find(|(path, _)| path == file_path) {
                Some(entry) => entry.1 = enabled,
                None => result.push((file_path.clone(), enabled)),
            }
        }
    }
    result
}
