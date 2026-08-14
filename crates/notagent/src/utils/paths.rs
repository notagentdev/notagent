//! Port of `packages/coding-agent/src/utils/paths.ts`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// Unicode space variants that users paste from documents and chat clients.
static UNICODE_SPACES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("[\u{00A0}\u{2000}-\u{200A}\u{202F}\u{205F}\u{3000}]").expect("unicode space regex")
});

static WINDOWS_SHELL_DRIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^/(?:mnt/|cygdrive/)?([a-z])(?:/(.*))?$").expect("windows drive regex")
});

/// Failures of `fileURLToPath`, which `normalizePath` propagates.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("Invalid URL")]
    InvalidUrl,
    #[error("File URL host must be \"localhost\" or empty")]
    InvalidFileUrlHost,
    #[error("File URL path must not include encoded / characters")]
    EncodedSlash,
    #[error("URI malformed")]
    MalformedUri,
}

#[derive(Debug, Clone, Default)]
pub struct PathInputOptions {
    /// Trim leading/trailing whitespace before normalization.
    pub trim: bool,
    /// Expand leading `~` to a home directory. Defaults to true.
    pub expand_tilde: Option<bool>,
    /// Home directory used for `~` expansion. Defaults to the user's home.
    pub home_dir: Option<String>,
    /// Strip a leading `@`, used for CLI @file paths.
    pub strip_at_prefix: bool,
    /// Normalize unicode space variants to regular spaces.
    pub normalize_unicode_spaces: bool,
}

/// Resolve a path to its canonical (real) form, following symlinks.
///
/// Falls back to the raw path if resolution fails (e.g. the target does not
/// exist yet), so that callers never crash on missing filesystem entries.
pub fn canonicalize_path(path: &str) -> String {
    match std::fs::canonicalize(path) {
        Ok(real) => strip_verbatim_prefix(real),
        Err(_) => path.to_owned(),
    }
}

/// `\\?\` is how Windows spells a canonical path; Node's `realpathSync` does not.
fn strip_verbatim_prefix(path: PathBuf) -> String {
    let text = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return rest.to_owned();
        }
    }
    text
}

/// An opaque token that changes whenever the file at `path` changes.
pub fn get_file_revision(path: &str) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mtime_ns = metadata.mtime() as i128 * 1_000_000_000 + i128::from(metadata.mtime_nsec());
        let ctime_ns = metadata.ctime() as i128 * 1_000_000_000 + i128::from(metadata.ctime_nsec());
        Some(format!(
            "{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.size(),
            mtime_ns,
            ctime_ns
        ))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Some(format!(
            "{}:{}:{}:{}:{}",
            metadata.volume_serial_number().unwrap_or(0),
            metadata.file_index().unwrap_or(0),
            metadata.file_size(),
            metadata.last_write_time(),
            metadata.creation_time()
        ))
    }
}

/// Returns true if the value is NOT a package source (npm:, git:, etc.) or a
/// remote URL protocol. Bare names, relative paths and file: URLs are local.
pub fn is_local_path(value: &str) -> bool {
    let trimmed = value.trim();
    !(trimmed.starts_with("npm:")
        || trimmed.starts_with("git:")
        || trimmed.starts_with("github:")
        || trimmed.starts_with("http:")
        || trimmed.starts_with("https:")
        || trimmed.starts_with("ssh:"))
}

/// Convert Git Bash, MSYS, Cygwin and WSL drive paths to a form native Windows
/// APIs accept.
pub fn normalize_windows_shell_path(file_path: &str) -> String {
    if !file_path.starts_with('/') || file_path.starts_with("//") || file_path.contains('\\') {
        return file_path.to_owned();
    }
    let Some(captures) = WINDOWS_SHELL_DRIVE.captures(file_path) else {
        return file_path.to_owned();
    };
    let drive = captures[1].to_uppercase();
    let suffix = captures
        .get(2)
        .map(|suffix| suffix.as_str().replace('/', "\\"))
        .unwrap_or_default();
    format!("{drive}:\\{suffix}")
}

pub fn normalize_path(input: &str, options: &PathInputOptions) -> Result<String, PathError> {
    let mut normalized = if options.trim {
        input.trim().to_owned()
    } else {
        input.to_owned()
    };
    if options.normalize_unicode_spaces {
        normalized = UNICODE_SPACES.replace_all(&normalized, " ").into_owned();
    }
    if options.strip_at_prefix
        && let Some(rest) = normalized.strip_prefix('@')
    {
        normalized = rest.to_owned();
    }
    if cfg!(windows) {
        normalized = normalize_windows_shell_path(&normalized);
    }

    if options.expand_tilde.unwrap_or(true) {
        let home = options.home_dir.clone().unwrap_or_else(default_home_dir);
        if normalized == "~" {
            return Ok(home);
        }
        if normalized.starts_with("~/") || (cfg!(windows) && normalized.starts_with("~\\")) {
            return Ok(join_path(&home, &normalized[2..]));
        }
    }

    if normalized.starts_with("file://") {
        return file_url_to_path(&normalized);
    }

    Ok(normalized)
}

/// `normalizePath(input)` with default options.
pub fn normalize_path_default(input: &str) -> Result<String, PathError> {
    normalize_path(input, &PathInputOptions::default())
}

pub fn resolve_path(
    input: &str,
    base_dir: &str,
    options: &PathInputOptions,
) -> Result<String, PathError> {
    let normalized = normalize_path(input, options)?;
    let normalized_base_dir = normalize_path_default(base_dir)?;
    Ok(if is_absolute_path(&normalized) {
        node_resolve(&[&normalized])
    } else {
        node_resolve(&[&normalized_base_dir, &normalized])
    })
}

/// `resolvePath(input, cwd)` with default options.
pub fn resolve_path_default(input: &str, base_dir: &str) -> Result<String, PathError> {
    resolve_path(input, base_dir, &PathInputOptions::default())
}

/// The path of `file_path` relative to `cwd`, or `None` when it escapes `cwd`.
pub fn get_cwd_relative_path(file_path: &str, cwd: &str) -> Result<Option<String>, PathError> {
    let resolved_cwd = resolve_path_default(cwd, &current_dir())?;
    let resolved_path = resolve_path_default(file_path, &resolved_cwd)?;
    let relative_path = node_relative(&resolved_cwd, &resolved_path);
    let separator = main_separator();
    let is_inside_cwd = relative_path.is_empty()
        || (relative_path != ".."
            && !relative_path.starts_with(&format!("..{separator}"))
            && !is_absolute_path(&relative_path));
    Ok(is_inside_cwd.then(|| {
        if relative_path.is_empty() {
            ".".to_owned()
        } else {
            relative_path
        }
    }))
}

pub fn format_path_relative_to_cwd_or_absolute(
    file_path: &str,
    cwd: &str,
) -> Result<String, PathError> {
    let absolute_path = resolve_path_default(file_path, cwd)?;
    let displayed = get_cwd_relative_path(&absolute_path, cwd)?.unwrap_or(absolute_path);
    Ok(displayed
        .split(main_separator())
        .collect::<Vec<_>>()
        .join("/"))
}

/// Ask cloud sync clients to leave a directory alone.
pub fn mark_path_ignored_by_cloud_sync(path: &str) {
    let attributes: &[&str] = if cfg!(target_os = "macos") {
        &["com.dropbox.ignored", "com.apple.fileprovider.ignore#P"]
    } else if cfg!(target_os = "linux") {
        &["user.com.dropbox.ignored"]
    } else {
        &[]
    };

    for attribute in attributes {
        let mut command = if cfg!(target_os = "macos") {
            let mut command = std::process::Command::new("xattr");
            command.args(["-w", attribute, "1", path]);
            command
        } else {
            let mut command = std::process::Command::new("setfattr");
            command.args(["-n", attribute, "-v", "1", path]);
            command
        };
        let _ = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

// =============================================================================
// Node path semantics
// =============================================================================

fn default_home_dir() -> String {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .to_string_lossy()
        .into_owned()
}

fn current_dir() -> String {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn main_separator() -> char {
    if cfg!(windows) { '\\' } else { '/' }
}

fn is_separator(character: char) -> bool {
    character == '/' || (cfg!(windows) && character == '\\')
}

fn join_path(base: &str, rest: &str) -> String {
    Path::new(base).join(rest).to_string_lossy().into_owned()
}

pub(crate) fn is_absolute_path(path: &str) -> bool {
    if cfg!(windows) {
        let bytes = path.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && is_separator(path.as_bytes()[2] as char)
        {
            return true;
        }
        return path.starts_with('/') || path.starts_with('\\');
    }
    path.starts_with('/')
}

/// `path.resolve(...)`: right-to-left until absolute, then dot-segment removal.
pub(crate) fn node_resolve(segments: &[&str]) -> String {
    let separator = main_separator();
    let mut resolved = String::new();
    let mut absolute = false;
    for segment in segments.iter().rev() {
        if segment.is_empty() {
            continue;
        }
        resolved = if resolved.is_empty() {
            (*segment).to_owned()
        } else {
            format!("{segment}{separator}{resolved}")
        };
        if is_absolute_path(segment) {
            absolute = true;
            break;
        }
    }
    if !absolute {
        let cwd = current_dir();
        resolved = if resolved.is_empty() {
            cwd
        } else {
            format!("{cwd}{separator}{resolved}")
        };
    }

    let (root, rest) = split_root(&resolved);
    let normalized = normalize_dot_segments(rest, true);
    let joined = format!("{root}{normalized}");
    if joined.is_empty() {
        separator.to_string()
    } else {
        trim_trailing_separator(&joined)
    }
}

fn trim_trailing_separator(path: &str) -> String {
    let (root, _) = split_root(path);
    if path.len() <= root.len() {
        return path.to_owned();
    }
    let trimmed = path.trim_end_matches(is_separator);
    if trimmed.len() < root.len() {
        root
    } else {
        trimmed.to_owned()
    }
}

/// Split off the root (`/`, `C:\`, `\\`) that dot-segment removal must keep.
fn split_root(path: &str) -> (String, &str) {
    if cfg!(windows) {
        let bytes = path.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && is_separator(bytes[2] as char)
        {
            return (format!("{}:\\", &path[..1]), &path[3..]);
        }
    }
    match path
        .char_indices()
        .find(|(_, character)| !is_separator(*character))
    {
        Some((index, _)) if index > 0 => (main_separator().to_string(), &path[index..]),
        Some(_) => (String::new(), path),
        None if !path.is_empty() => (main_separator().to_string(), ""),
        None => (String::new(), path),
    }
}

fn normalize_dot_segments(path: &str, rooted: bool) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(is_separator) {
        match part {
            "" | "." => {}
            ".." => {
                match parts.last() {
                    Some(&last) if last != ".." => {
                        parts.pop();
                    }
                    // A leading `..` is dropped at the root and kept otherwise.
                    _ if !rooted => parts.push(".."),
                    _ => {}
                }
            }
            other => parts.push(other),
        }
    }
    parts.join(&main_separator().to_string())
}

/// `path.relative(from, to)` for already-resolved absolute paths.
pub(crate) fn node_relative(from: &str, to: &str) -> String {
    let case_sensitive = !cfg!(windows);
    let equal = |left: &str, right: &str| {
        if case_sensitive {
            left == right
        } else {
            left.eq_ignore_ascii_case(right)
        }
    };
    let from_parts: Vec<&str> = from
        .split(is_separator)
        .filter(|part| !part.is_empty())
        .collect();
    let to_parts: Vec<&str> = to
        .split(is_separator)
        .filter(|part| !part.is_empty())
        .collect();
    let mut common = 0;
    while common < from_parts.len()
        && common < to_parts.len()
        && equal(from_parts[common], to_parts[common])
    {
        common += 1;
    }
    let mut parts: Vec<&str> = vec![".."; from_parts.len() - common];
    parts.extend_from_slice(&to_parts[common..]);
    parts.join(&main_separator().to_string())
}

/// `url.fileURLToPath(url)`.
fn file_url_to_path(url: &str) -> Result<String, PathError> {
    let Some(rest) = url.strip_prefix("file://") else {
        return Err(PathError::InvalidUrl);
    };
    // Everything up to the next separator is the host; WHATWG drops "localhost".
    let (host, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
        return Err(PathError::InvalidFileUrlHost);
    }
    // A query or fragment is not part of the path.
    let path = path.split(['?', '#']).next().unwrap_or("");
    let path = if path.is_empty() { "/" } else { path };

    let bytes = path.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && bytes.get(index + 1) == Some(&b'2')
            && matches!(bytes.get(index + 2), Some(b'f') | Some(b'F'))
        {
            return Err(PathError::EncodedSlash);
        }
    }

    let normalized = format!("/{}", normalize_url_dot_segments(&path[1..]));
    let decoded = decode_uri_component(&normalized)?;
    if cfg!(windows) {
        // `file:///C:/x` has a leading slash before the drive letter.
        let without_slash = &decoded[1..];
        if without_slash.len() >= 2
            && without_slash.as_bytes()[0].is_ascii_alphabetic()
            && without_slash.as_bytes()[1] == b':'
        {
            return Ok(without_slash.replace('/', "\\"));
        }
        return Ok(decoded.replace('/', "\\"));
    }
    Ok(decoded)
}

/// The URL parser removes dot segments before the path is decoded.
fn normalize_url_dot_segments(path: &str) -> String {
    let is_dot = |part: &str| part == "." || part.eq_ignore_ascii_case("%2e");
    let is_double_dot = |part: &str| {
        let lowercase = part.to_ascii_lowercase();
        matches!(lowercase.as_str(), ".." | ".%2e" | "%2e." | "%2e%2e")
    };
    let mut parts: Vec<&str> = Vec::new();
    let ends_with_separator = path.ends_with('/');
    for part in path.split('/') {
        if is_dot(part) {
            continue;
        }
        if is_double_dot(part) {
            parts.pop();
            continue;
        }
        parts.push(part);
    }
    let mut joined = parts.join("/");
    if ends_with_separator && !joined.ends_with('/') {
        joined.push('/');
    }
    joined
}

fn decode_uri_component(value: &str) -> Result<String, PathError> {
    let bytes = value.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).and_then(|byte| hex_value(*byte));
            let low = bytes.get(index + 2).and_then(|byte| hex_value(*byte));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(PathError::MalformedUri);
            };
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| PathError::MalformedUri)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// `url.pathToFileURL(path).href` — the inverse of [`file_url_to_path`].
pub fn path_to_file_url(path: &str) -> String {
    let path = if is_absolute_path(path) {
        path.to_owned()
    } else {
        node_resolve(&[path])
    };
    let path = if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path
    };
    let mut encoded = String::from("file://");
    if !path.starts_with('/') {
        encoded.push('/');
    }
    for byte in path.as_bytes() {
        match byte {
            b'%' => encoded.push_str("%25"),
            b'#' => encoded.push_str("%23"),
            b'?' => encoded.push_str("%3F"),
            b'\\' => encoded.push_str("%5C"),
            b'\n' => encoded.push_str("%0A"),
            b'\r' => encoded.push_str("%0D"),
            b'\t' => encoded.push_str("%09"),
            b' ' => encoded.push_str("%20"),
            b'"' => encoded.push_str("%22"),
            b'<' => encoded.push_str("%3C"),
            b'>' => encoded.push_str("%3E"),
            b'`' => encoded.push_str("%60"),
            b'{' => encoded.push_str("%7B"),
            b'}' => encoded.push_str("%7D"),
            byte if *byte < 0x20 || *byte >= 0x7f => encoded.push_str(&format!("%{byte:02X}")),
            byte => encoded.push(*byte as char),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let directory = tempfile::Builder::new()
                .prefix("notagent-paths-")
                .tempdir()
                .expect("temp dir");
            // `keep` mirrors the TS suite, which removes the directory itself.
            let path = directory.path().to_path_buf();
            let _ = directory.keep();
            Self { path }
        }

        fn join(&self, name: &str) -> String {
            self.path.join(name).to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn real_path(path: &str) -> String {
        strip_verbatim_prefix(std::fs::canonicalize(path).expect("real path"))
    }

    #[test]
    fn canonicalize_returns_the_real_path_for_a_regular_file() {
        let directory = TempDir::new();
        let file = directory.join("file.txt");
        std::fs::write(&file, "hello").expect("write");
        assert_eq!(canonicalize_path(&file), real_path(&file));
    }

    #[cfg(unix)]
    #[test]
    fn canonicalize_resolves_symlinks_to_their_targets() {
        let directory = TempDir::new();
        let target = directory.join("target.txt");
        let link = directory.join("link.txt");
        std::fs::write(&target, "hello").expect("write");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert_eq!(canonicalize_path(&link), real_path(&target));
    }

    #[cfg(unix)]
    #[test]
    fn canonicalize_resolves_directory_symlinks() {
        let directory = TempDir::new();
        let target_dir = directory.join("target-dir");
        let link_dir = directory.join("link-dir");
        std::fs::create_dir(&target_dir).expect("create dir");
        std::os::unix::fs::symlink(&target_dir, &link_dir).expect("symlink");
        assert_eq!(canonicalize_path(&link_dir), real_path(&target_dir));
    }

    #[test]
    fn canonicalize_falls_back_to_the_raw_path_when_the_target_does_not_exist() {
        let directory = TempDir::new();
        let nonexistent = directory.join("no-such-file");
        assert_eq!(canonicalize_path(&nonexistent), nonexistent);
    }

    #[cfg(unix)]
    #[test]
    fn canonicalize_falls_back_to_the_raw_path_for_a_dangling_symlink() {
        let directory = TempDir::new();
        let target = directory.join("target.txt");
        let link = directory.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert_eq!(canonicalize_path(&link), link);
    }

    #[test]
    fn keeps_cwd_relative_names_that_start_with_dots() {
        let cwd = join_path(
            &std::env::temp_dir().to_string_lossy(),
            "notagent-paths-cwd",
        );
        let file = join_path(&cwd, "..config/AGENTS.md");
        assert_eq!(
            get_cwd_relative_path(&file, &cwd).expect("relative"),
            Some(join_path("..config", "AGENTS.md"))
        );
    }

    #[test]
    fn rejects_parent_directory_traversals() {
        let cwd = join_path(
            &std::env::temp_dir().to_string_lossy(),
            "notagent-paths-cwd",
        );
        let file = join_path(&cwd, "../AGENTS.md");
        assert_eq!(get_cwd_relative_path(&file, &cwd).expect("relative"), None);
    }

    #[test]
    fn expands_only_home_tilde_shortcuts() {
        let cwd = join_path(
            &std::env::temp_dir().to_string_lossy(),
            "notagent-paths-cwd",
        );
        let home = default_home_dir();
        assert_eq!(normalize_path_default("~").expect("home"), home);
        assert_eq!(
            normalize_path_default("~/file.txt").expect("home file"),
            join_path(&home, "file.txt")
        );
        assert_eq!(
            resolve_path_default("~draft.md", &cwd).expect("resolved"),
            node_resolve(&[&cwd, "~draft.md"])
        );
        assert_eq!(
            normalize_path_default("~draft.md").expect("literal"),
            "~draft.md"
        );
    }

    #[test]
    fn resolves_relative_paths_against_the_base_directory() {
        let cwd = join_path(
            &std::env::temp_dir().to_string_lossy(),
            "notagent-paths-cwd",
        );
        let expected = node_resolve(&[&cwd, "subdir/file.txt"]);
        assert_eq!(
            resolve_path_default("subdir/file.txt", &cwd).expect("resolved"),
            expected
        );
        assert_eq!(
            resolve_path_default("subdir/file.txt", &path_to_file_url(&cwd)).expect("resolved"),
            expected
        );
    }

    #[test]
    fn accepts_file_urls() {
        let directory = TempDir::new();
        let file_path = directory.join("file with spaces.txt");
        let base = directory.join("base");
        assert_eq!(
            resolve_path_default(&path_to_file_url(&file_path), &base).expect("resolved"),
            node_resolve(&[&file_path])
        );
    }

    #[test]
    fn rejects_invalid_file_urls() {
        assert_eq!(
            resolve_path_default("file:///%E0%A4%A", &current_dir()).expect_err("throws"),
            PathError::MalformedUri
        );
        assert_eq!(
            normalize_path_default("file:///a%2Fb").expect_err("throws"),
            PathError::EncodedSlash
        );
        assert_eq!(
            normalize_path_default("file://example.com/share").expect_err("throws"),
            PathError::InvalidFileUrlHost
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserves_posix_absolute_paths_with_literal_percent_sequences() {
        let directory = TempDir::new();
        let base = directory.join("base");
        for name in ["report%2026.md", "foo%2Fbar", "malformed%A.md"] {
            let file_path = directory.join(name);
            assert_eq!(
                resolve_path_default(&file_path, &base).expect("resolved"),
                node_resolve(&[&file_path])
            );
        }
    }

    #[test]
    fn converts_git_bash_msys_cygwin_and_wsl_drive_paths() {
        assert_eq!(
            normalize_windows_shell_path("/c/Users/example/project"),
            "C:\\Users\\example\\project"
        );
        assert_eq!(normalize_windows_shell_path("/cygdrive/d/work"), "D:\\work");
        assert_eq!(normalize_windows_shell_path("/mnt/e/source"), "E:\\source");
        assert_eq!(normalize_windows_shell_path("/c"), "C:\\");
    }

    #[test]
    fn leaves_other_path_forms_unchanged() {
        for path in [
            "C:/Users/example",
            "C:\\Users\\example",
            "//server/share/file",
            "/c/Users\\example",
            "relative/file",
            "/tmp/file",
        ] {
            assert_eq!(normalize_windows_shell_path(path), path);
        }
    }

    #[test]
    fn classifies_local_paths() {
        assert!(is_local_path("my-package"));
        assert!(is_local_path("./foo"));
        assert!(is_local_path("file:///tmp/foo"));
        assert!(!is_local_path("npm:package"));
        assert!(!is_local_path("git://repo"));
        assert!(!is_local_path("https://example.com"));
    }

    #[test]
    fn file_revisions_change_with_the_file() {
        let directory = TempDir::new();
        let file = directory.join("file.txt");
        assert_eq!(get_file_revision(&file), None);
        std::fs::write(&file, "one").expect("write");
        let first = get_file_revision(&file).expect("revision");
        assert_eq!(get_file_revision(&file).as_deref(), Some(first.as_str()));
        std::fs::write(&file, "another size").expect("write");
        assert_ne!(get_file_revision(&file).expect("revision"), first);
    }

    #[test]
    fn formats_paths_relative_to_cwd_or_absolute() {
        let cwd = join_path(
            &std::env::temp_dir().to_string_lossy(),
            "notagent-paths-cwd",
        );
        assert_eq!(
            format_path_relative_to_cwd_or_absolute(&join_path(&cwd, "src/main.rs"), &cwd)
                .expect("formatted"),
            "src/main.rs"
        );
        let outside = node_resolve(&[&cwd, "../outside.md"]);
        assert_eq!(
            format_path_relative_to_cwd_or_absolute(&outside, &cwd).expect("formatted"),
            outside
                .split(main_separator())
                .collect::<Vec<_>>()
                .join("/")
        );
    }

    #[test]
    fn resolve_removes_dot_segments_like_node() {
        assert_eq!(node_resolve(&["/a/b", "../c/./d"]), "/a/c/d");
        assert_eq!(node_resolve(&["/a/b/"]), "/a/b");
        assert_eq!(node_resolve(&["/"]), "/");
        assert_eq!(node_resolve(&["/a/../.."]), "/");
    }

    #[test]
    fn normalize_path_applies_the_optional_transforms() {
        let options = PathInputOptions {
            trim: true,
            strip_at_prefix: true,
            normalize_unicode_spaces: true,
            home_dir: Some("/home/tester".to_owned()),
            expand_tilde: Some(true),
        };
        assert_eq!(
            normalize_path("  @~/a\u{00A0}b.txt  ", &options).expect("normalized"),
            "/home/tester/a b.txt"
        );
        let no_tilde = PathInputOptions {
            expand_tilde: Some(false),
            ..PathInputOptions::default()
        };
        assert_eq!(
            normalize_path("~/a.txt", &no_tilde).expect("normalized"),
            "~/a.txt"
        );
    }
}
