//! Node.js `path` and `os.homedir()` semantics.
//!
//! The autocomplete provider builds display paths with `path.join`,
//! `path.dirname` and `path.basename` and relies on their normalization (`.`
//! and `..` resolution, collapsed separators). `std::path` does not normalize,
//! so the POSIX algorithms of Node are reproduced here.

/// `path.normalize` for POSIX paths.
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let is_absolute = path.starts_with('/');
    let trailing_separator = path.ends_with('/');
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                match segments.last() {
                    Some(&last) if last != ".." => {
                        segments.pop();
                    }
                    _ if !is_absolute => segments.push(".."),
                    _ => {}
                };
            }
            segment => segments.push(segment),
        }
    }
    let mut result = segments.join("/");
    if result.is_empty() {
        if is_absolute {
            return "/".to_string();
        }
        return if trailing_separator { "./" } else { "." }.to_string();
    }
    if trailing_separator {
        result.push('/');
    }
    if is_absolute {
        return format!("/{result}");
    }
    result
}

/// `path.join` for POSIX paths.
pub fn join(parts: &[&str]) -> String {
    let mut joined = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if joined.is_empty() {
            joined.push_str(part);
        } else {
            joined.push('/');
            joined.push_str(part);
        }
    }
    if joined.is_empty() {
        return ".".to_string();
    }
    normalize(&joined)
}

/// `path.dirname` for POSIX paths.
pub fn dirname(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let bytes = path.as_bytes();
    let has_root = bytes[0] == b'/';
    let mut end: Option<usize> = None;
    let mut matched_slash = true;
    for index in (1..bytes.len()).rev() {
        if bytes[index] == b'/' {
            if !matched_slash {
                end = Some(index);
                break;
            }
        } else {
            matched_slash = false;
        }
    }
    match end {
        None if has_root => "/".to_string(),
        None => ".".to_string(),
        // Node returns "//" for a doubled root separator.
        Some(1) if has_root => "//".to_string(),
        Some(end) => path[..end].to_string(),
    }
}

/// `path.basename` for POSIX paths.
pub fn basename(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b'/' {
        end -= 1;
    }
    let mut start = 0;
    for index in (0..end).rev() {
        if bytes[index] == b'/' {
            start = index + 1;
            break;
        }
    }
    path[start..end].to_string()
}

/// `os.homedir()`.
pub fn homedir() -> String {
    #[cfg(windows)]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            return profile.to_string_lossy().into_owned();
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return home.to_string_lossy().into_owned();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference values taken from Node's `path.posix`.
    #[test]
    fn matches_node_semantics() {
        let cases = [
            ("", ".", "", "."),
            ("update.sh", ".", "update.sh", "update.sh"),
            ("./up", ".", "up", "up"),
            ("/a/b", "/a", "b", "/a/b"),
            ("a/b/", "a", "b", "a/b/"),
            ("/a", "/", "a", "/a"),
            ("/", "/", "", "/"),
            ("//a", "//", "a", "/a"),
            ("a", ".", "a", "a"),
            ("./", ".", ".", "./"),
            ("../x/", "..", "x", "../x/"),
            ("/a//b//c", "/a//b/", "c", "/a/b/c"),
        ];
        for (path, expected_dirname, expected_basename, expected_normalize) in cases {
            assert_eq!(dirname(path), expected_dirname, "dirname({path:?})");
            assert_eq!(basename(path), expected_basename, "basename({path:?})");
            assert_eq!(normalize(path), expected_normalize, "normalize({path:?})");
        }

        assert_eq!(join(&[".", "x"]), "x");
        assert_eq!(join(&["a/b", "../c"]), "a/c");
        assert_eq!(join(&["", "x"]), "x");
        assert_eq!(join(&["/tmp", "./"]), "/tmp/");
        assert_eq!(join(&["/tmp", ""]), "/tmp");
        assert_eq!(join(&["a", ""]), "a");
    }
}
