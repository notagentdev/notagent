//! 1:1 port of `packages/coding-agent/src/utils/changelog.ts` (196 LOC).
//!
//! The `/changelog` command and the "what's new" block after an update read the
//! bundled `CHANGELOG.md` through here. `getChangelogPath` lives in
//! [`crate::config`], as it does in TypeScript.

use std::path::Path;

use regex::{Captures, Regex};

const GITHUB_REPO: &str = "notagentdev/notagent";
const CHANGELOG_LINK_BASE_PATH: &str = "packages/coding-agent";

/// One `## [x.y.z]` section of the changelog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogEntry {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub content: String,
}

impl ChangelogEntry {
    fn version(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// `normalizeTag` — a version string or an entry, always with the `v` prefix.
fn normalize_tag(version: &str) -> String {
    if version.starts_with('v') {
        version.to_owned()
    } else {
        format!("v{version}")
    }
}

/// `/^https:\/\/github\.com\/(?:badlogic|notagentdev)\/notagent(?=\/|$)/`
fn legacy_repo_prefix(target: &str) -> Option<usize> {
    for owner in ["badlogic", "notagentdev"] {
        let prefix = format!("https://github.com/{owner}/notagent");
        if let Some(rest) = target.strip_prefix(&prefix)
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return Some(prefix.len());
        }
    }
    None
}

/// `/^[a-z][a-z0-9+.-]*:/i`
fn has_url_scheme(target: &str) -> bool {
    let mut characters = target.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    for character in characters {
        if character == ':' {
            return true;
        }
        if !(character.is_ascii_alphanumeric() || matches!(character, '+' | '.' | '-')) {
            return false;
        }
    }
    false
}

/// `splitLocalTarget` — `(fragment, path_part, query)`.
fn split_local_target(target: &str) -> (String, String, String) {
    let (before_hash, fragment) = match target.find('#') {
        Some(index) => (&target[..index], target[index..].to_owned()),
        None => (target, String::new()),
    };
    match before_hash.find('?') {
        Some(index) => (
            fragment,
            before_hash[..index].to_owned(),
            before_hash[index..].to_owned(),
        ),
        None => (fragment, before_hash.to_owned(), String::new()),
    }
}

/// `path.posix.normalize` for the subset the changelog links use: it resolves
/// `.` and `..` segments and keeps a trailing slash.
fn posix_normalize(value: &str) -> String {
    let is_absolute = value.starts_with('/');
    let has_trailing_slash = value.ends_with('/') && value.len() > 1;
    let mut segments: Vec<&str> = Vec::new();
    for segment in value.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                match segments.last() {
                    Some(&last) if last != ".." => {
                        segments.pop();
                    }
                    // A leading `..` survives on a relative path and is dropped
                    // on an absolute one, as in Node.
                    _ if !is_absolute => segments.push(".."),
                    _ => {}
                }
            }
            segment => segments.push(segment),
        }
    }
    let mut joined = segments.join("/");
    if joined.is_empty() {
        return if is_absolute {
            "/".to_owned()
        } else {
            ".".to_owned()
        };
    }
    if has_trailing_slash {
        joined.push('/');
    }
    if is_absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

/// `resolveRepositoryPath`
fn resolve_repository_path(target_path: &str) -> Option<String> {
    let normalized_target = target_path.replace('\\', "/");
    let joined = if let Some(stripped) = normalized_target.strip_prefix('/') {
        posix_normalize(stripped.trim_start_matches('/'))
    } else {
        posix_normalize(&format!("{CHANGELOG_LINK_BASE_PATH}/{normalized_target}"))
    };
    if joined == "." || joined.starts_with("../") || joined == ".." {
        return None;
    }
    Some(joined)
}

/// `isDirectoryTarget`
fn is_directory_target(original_path: &str, repository_path: &str) -> bool {
    if original_path.ends_with('/') {
        return true;
    }
    let basename = repository_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(repository_path);
    !basename.contains('.')
}

/// `encodeURI` for the characters a repository path can contain. Node leaves
/// the unreserved and reserved URI characters alone and percent-encodes the
/// rest.
fn encode_uri(value: &str) -> String {
    const KEEP: &str = "A-Za-z0-9;,/?:@&=+$-_.!~*'()#";
    let mut encoded = String::with_capacity(value.len());
    for character in value.chars() {
        let keep = character.is_ascii_alphanumeric()
            || KEEP
                .chars()
                .filter(|candidate| !candidate.is_ascii_alphanumeric() && *candidate != '-')
                .any(|candidate| candidate == character)
            || matches!(character, '-');
        if keep {
            encoded.push(character);
            continue;
        }
        let mut buffer = [0u8; 4];
        for byte in character.encode_utf8(&mut buffer).as_bytes() {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `normalizeChangelogLinkTarget`
fn normalize_changelog_link_target(target: &str, tag: &str) -> String {
    let repo_url = format!("https://github.com/{GITHUB_REPO}");
    let mut canonical_target = match legacy_repo_prefix(target) {
        Some(length) => format!("{repo_url}{}", &target[length..]),
        None => target.to_owned(),
    };

    for route in ["blob", "tree"] {
        for branch in ["main", "master"] {
            let floating_ref_prefix = format!("{repo_url}/{route}/{branch}/");
            if let Some(rest) = canonical_target.strip_prefix(&floating_ref_prefix) {
                canonical_target = format!("{repo_url}/{route}/{tag}/{rest}");
            }
        }
    }

    if canonical_target.starts_with('#')
        || canonical_target.starts_with("//")
        || has_url_scheme(&canonical_target)
    {
        return canonical_target;
    }

    let (fragment, path_part, query) = split_local_target(&canonical_target);
    if path_part.is_empty() {
        return canonical_target;
    }
    let Some(repository_path) = resolve_repository_path(&path_part) else {
        return canonical_target;
    };
    let route = if is_directory_target(&path_part, &repository_path) {
        "tree"
    } else {
        "blob"
    };
    format!(
        "https://github.com/{GITHUB_REPO}/{route}/{tag}/{}{query}{fragment}",
        encode_uri(&repository_path)
    )
}

fn inline_markdown_link_regex() -> &'static Regex {
    static REGEX: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(!?\[[^\]\n]+\]\()([^\s)]+)((?:\s+[^)]*)?\))").expect("valid regex")
    })
}

/// `normalizeChangelogLinks(markdown, version)` for a version string.
pub fn normalize_changelog_links(markdown: &str, version: &str) -> String {
    let tag = normalize_tag(version);
    inline_markdown_link_regex()
        .replace_all(markdown, |captures: &Captures| {
            format!(
                "{}{}{}",
                &captures[1],
                normalize_changelog_link_target(&captures[2], &tag),
                &captures[3]
            )
        })
        .into_owned()
}

/// `normalizeChangelogLinks(markdown, entry)` for a parsed entry.
pub fn normalize_changelog_links_for_entry(markdown: &str, entry: &ChangelogEntry) -> String {
    normalize_changelog_links(markdown, &entry.version())
}

/// `parseChangelog(changelogPath)` — every `## ` line starts a section, and a
/// section without a parsable version drops the lines that follow it.
pub fn parse_changelog(changelog_path: &Path) -> Vec<ChangelogEntry> {
    if !changelog_path.exists() {
        return Vec::new();
    }
    let Ok(content) = std::fs::read_to_string(changelog_path) else {
        // TypeScript logs the warning and returns an empty list.
        eprintln!(
            "Warning: Could not parse changelog: {}",
            changelog_path.display()
        );
        return Vec::new();
    };

    let version_regex = Regex::new(r"##\s+\[?(\d+)\.(\d+)\.(\d+)\]?").expect("valid regex");
    let mut entries: Vec<ChangelogEntry> = Vec::new();
    let mut current_lines: Vec<&str> = Vec::new();
    let mut current_version: Option<(u64, u64, u64)> = None;

    for line in content.split('\n') {
        if line.starts_with("## ") {
            if let Some((major, minor, patch)) = current_version
                && !current_lines.is_empty()
            {
                entries.push(ChangelogEntry {
                    major,
                    minor,
                    patch,
                    content: current_lines.join("\n").trim().to_owned(),
                });
            }
            match version_regex.captures(line) {
                Some(captures) => {
                    current_version = Some((
                        captures[1].parse().unwrap_or(0),
                        captures[2].parse().unwrap_or(0),
                        captures[3].parse().unwrap_or(0),
                    ));
                    current_lines = vec![line];
                }
                None => {
                    current_version = None;
                    current_lines = Vec::new();
                }
            }
        } else if current_version.is_some() {
            current_lines.push(line);
        }
    }

    if let Some((major, minor, patch)) = current_version
        && !current_lines.is_empty()
    {
        entries.push(ChangelogEntry {
            major,
            minor,
            patch,
            content: current_lines.join("\n").trim().to_owned(),
        });
    }

    entries
}

/// `compareVersions`
fn compare_versions(left: (u64, u64, u64), right: (u64, u64, u64)) -> std::cmp::Ordering {
    left.cmp(&right)
}

/// `getNewEntries(entries, lastVersion)` — everything newer than `last_version`.
pub fn get_new_entries(entries: &[ChangelogEntry], last_version: &str) -> Vec<ChangelogEntry> {
    let mut parts = last_version.split('.');
    let last = (
        parts.next().and_then(|part| part.parse().ok()).unwrap_or(0),
        parts.next().and_then(|part| part.parse().ok()).unwrap_or(0),
        parts.next().and_then(|part| part.parse().ok()).unwrap_or(0),
    );
    entries
        .iter()
        .filter(|entry| {
            compare_versions((entry.major, entry.minor, entry.patch), last)
                == std::cmp::Ordering::Greater
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> ChangelogEntry {
        ChangelogEntry {
            major: 0,
            minor: 79,
            patch: 0,
            content: String::new(),
        }
    }

    /// `test/changelog.test.ts`, first case.
    #[test]
    fn rewrites_package_relative_links_to_tag_pinned_github_source_links() {
        let markdown = [
            "[Project Trust](README.md#project-trust)",
            "[Extensions](docs/extensions.md#project_trust)",
            "[Examples](examples/extensions/)",
            "[Root README](../../README.md#supply-chain-hardening)",
        ]
        .join("\n");

        assert_eq!(
            normalize_changelog_links_for_entry(&markdown, &entry()),
            [
                "[Project Trust](https://github.com/notagentdev/notagent/blob/v0.79.0/packages/coding-agent/README.md#project-trust)",
                "[Extensions](https://github.com/notagentdev/notagent/blob/v0.79.0/packages/coding-agent/docs/extensions.md#project_trust)",
                "[Examples](https://github.com/notagentdev/notagent/tree/v0.79.0/packages/coding-agent/examples/extensions/)",
                "[Root README](https://github.com/notagentdev/notagent/blob/v0.79.0/README.md#supply-chain-hardening)",
            ]
            .join("\n")
        );
    }

    /// `test/changelog.test.ts`, second case.
    #[test]
    fn canonicalizes_old_repository_urls_without_changing_external_links() {
        let markdown = [
            "[#5167](https://github.com/notagentdev/notagent/pull/5167)",
            "[#4163](https://github.com/notagentdev/notagent/issues/4163)",
            "[Agent README](https://github.com/notagentdev/notagent/blob/main/packages/agent/README.md)",
            "[External](https://example.com/docs)",
            "[Local anchor](#settings)",
        ]
        .join("\n");

        assert_eq!(
            normalize_changelog_links(&markdown, "0.79.0"),
            [
                "[#5167](https://github.com/notagentdev/notagent/pull/5167)",
                "[#4163](https://github.com/notagentdev/notagent/issues/4163)",
                "[Agent README](https://github.com/notagentdev/notagent/blob/v0.79.0/packages/agent/README.md)",
                "[External](https://example.com/docs)",
                "[Local anchor](#settings)",
            ]
            .join("\n")
        );
    }

    #[test]
    fn parses_sections_and_drops_the_ones_without_a_version() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("CHANGELOG.md");
        std::fs::write(
            &path,
            "# Changelog\n\n## Unreleased\nnot kept\n\n## [1.2.3] - today\nfirst\n\n## 1.3.0\nsecond\n",
        )
        .expect("write");

        let entries = parse_changelog(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            (entries[0].major, entries[0].minor, entries[0].patch),
            (1, 2, 3)
        );
        assert_eq!(entries[0].content, "## [1.2.3] - today\nfirst");
        assert_eq!(entries[1].content, "## 1.3.0\nsecond");

        let newer = get_new_entries(&entries, "1.2.3");
        assert_eq!(newer.len(), 1);
        assert_eq!(newer[0].minor, 3);
    }

    #[test]
    fn a_missing_file_yields_no_entries() {
        assert!(parse_changelog(Path::new("/does/not/exist/CHANGELOG.md")).is_empty());
    }
}
