//! Port of `packages/coding-agent/src/core/tools/path-utils.ts`.

use std::path::Path;

use unicode_normalization::UnicodeNormalization;

use crate::utils::paths::{PathInputOptions, normalize_path, resolve_path};

const NARROW_NO_BREAK_SPACE: char = '\u{202F}';

/// macOS screenshots use a narrow no-break space before AM/PM.
fn try_macos_screenshot_path(file_path: &str) -> String {
    let bytes = file_path.as_bytes();
    let mut result = String::with_capacity(file_path.len());
    let mut index = 0;
    while index < file_path.len() {
        let rest = &file_path[index..];
        let matched = rest.starts_with(' ') && rest.len() >= 4 && rest.as_bytes()[3] == b'.' && {
            let marker = &rest[1..3].to_ascii_uppercase();
            marker == "AM" || marker == "PM"
        };
        if matched {
            result.push(NARROW_NO_BREAK_SPACE);
            result.push_str(&rest[1..4]);
            index += 4;
            continue;
        }
        let character = rest.chars().next().expect("non-empty");
        result.push(character);
        index += character.len_utf8();
        let _ = bytes;
    }
    result
}

/// macOS stores filenames in NFD (decomposed) form.
fn try_nfd_variant(file_path: &str) -> String {
    file_path.nfd().collect()
}

/// macOS uses U+2019 in screenshot names like "Capture d'écran".
fn try_curly_quote_variant(file_path: &str) -> String {
    file_path.replace('\'', "\u{2019}")
}

fn file_exists(file_path: &str) -> bool {
    Path::new(file_path).symlink_metadata().is_ok() || Path::new(file_path).exists()
}

pub fn path_exists(file_path: &str) -> bool {
    file_exists(file_path)
}

fn path_input_options() -> PathInputOptions {
    PathInputOptions {
        normalize_unicode_spaces: true,
        strip_at_prefix: true,
        ..PathInputOptions::default()
    }
}

pub fn expand_path(file_path: &str) -> String {
    normalize_path(file_path, &path_input_options()).unwrap_or_else(|_| file_path.to_owned())
}

/// Resolve a path relative to `cwd`, expanding `~` and keeping absolute paths.
pub fn resolve_to_cwd(file_path: &str, cwd: &str) -> String {
    resolve_path(file_path, cwd, &path_input_options()).unwrap_or_else(|_| file_path.to_owned())
}

/// Resolve a path for reading, trying the macOS filename variants in order.
pub fn resolve_read_path(file_path: &str, cwd: &str) -> String {
    let resolved = resolve_to_cwd(file_path, cwd);
    if file_exists(&resolved) {
        return resolved;
    }

    let am_pm_variant = try_macos_screenshot_path(&resolved);
    if am_pm_variant != resolved && file_exists(&am_pm_variant) {
        return am_pm_variant;
    }

    let nfd_variant = try_nfd_variant(&resolved);
    if nfd_variant != resolved && file_exists(&nfd_variant) {
        return nfd_variant;
    }

    let curly_variant = try_curly_quote_variant(&resolved);
    if curly_variant != resolved && file_exists(&curly_variant) {
        return curly_variant;
    }

    // French macOS screenshots combine both, e.g. "Capture d'écran".
    let nfd_curly_variant = try_curly_quote_variant(&nfd_variant);
    if nfd_curly_variant != resolved && file_exists(&nfd_curly_variant) {
        return nfd_curly_variant;
    }

    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let directory = tempfile::Builder::new()
                .prefix("notagent-path-utils-")
                .tempdir()
                .expect("temp dir");
            let path = directory.path().to_path_buf();
            let _ = directory.keep();
            Self { path }
        }

        fn cwd(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }

        fn write(&self, name: &str) -> String {
            let path = self.path.join(name);
            std::fs::write(&path, "x").expect("writes");
            path.to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn expands_tildes_and_unicode_spaces_and_strips_at_prefixes() {
        assert!(!expand_path("~").contains('~'));
        assert!(!expand_path("~/Documents/file.txt").contains("~/"));
        // A tilde that starts a filename stays literal.
        assert_eq!(expand_path("~draft.md"), "~draft.md");
        assert_eq!(expand_path("@~draft.md"), "~draft.md");
        assert_eq!(expand_path("@src/main.rs"), "src/main.rs");
        assert_eq!(expand_path("file\u{00A0}name.txt"), "file name.txt");
    }

    #[test]
    fn resolves_paths_against_cwd() {
        let directory = TempDir::new();
        let resolved = resolve_to_cwd("sub/file.txt", &directory.cwd());
        assert_eq!(resolved, format!("{}/sub/file.txt", directory.cwd()));
        assert_eq!(
            resolve_to_cwd("/absolute/file.txt", &directory.cwd()),
            "/absolute/file.txt"
        );
        assert_eq!(
            resolve_to_cwd("~draft.md", &directory.cwd()),
            format!("{}/~draft.md", directory.cwd())
        );
        assert_eq!(
            resolve_to_cwd("@~draft.md", &directory.cwd()),
            format!("{}/~draft.md", directory.cwd())
        );
    }

    #[test]
    fn falls_back_to_the_macos_screenshot_name() {
        let directory = TempDir::new();
        let actual = directory.write("Screenshot 2025-01-01 at 10.12.13\u{202F}AM.png");
        let typed = "Screenshot 2025-01-01 at 10.12.13 AM.png";
        assert_eq!(resolve_read_path(typed, &directory.cwd()), actual);
    }

    #[test]
    fn falls_back_to_the_curly_quote_name() {
        let directory = TempDir::new();
        let actual = directory.write("Capture d\u{2019}ecran.png");
        assert_eq!(
            resolve_read_path("Capture d'ecran.png", &directory.cwd()),
            actual
        );
    }

    #[test]
    fn builds_the_macos_filename_variants() {
        // APFS compares filenames normalization-insensitively, so the NFD fallback
        // cannot be observed through the filesystem on macOS; the variant itself is.
        assert_eq!(try_nfd_variant("écran.png"), "e\u{301}cran.png");
        assert_eq!(try_curly_quote_variant("d'ecran"), "d\u{2019}ecran");
        assert_eq!(
            try_curly_quote_variant(&try_nfd_variant("Capture d'écran.png")),
            "Capture d\u{2019}e\u{301}cran.png"
        );
        assert_eq!(
            try_macos_screenshot_path("Screenshot at 10.12.13 AM.png"),
            "Screenshot at 10.12.13\u{202F}AM.png"
        );
        assert_eq!(
            try_macos_screenshot_path("Screenshot at 10.12.13 pm.png"),
            "Screenshot at 10.12.13\u{202F}pm.png"
        );
        // Only the AM/PM marker directly before a dot is replaced.
        assert_eq!(try_macos_screenshot_path("a AM b.png"), "a AM b.png");
    }

    #[test]
    fn keeps_the_resolved_path_when_no_variant_exists() {
        let directory = TempDir::new();
        let expected = format!("{}/missing.png", directory.cwd());
        assert_eq!(resolve_read_path("missing.png", &directory.cwd()), expected);
        assert!(!path_exists(&expected));
    }
}
