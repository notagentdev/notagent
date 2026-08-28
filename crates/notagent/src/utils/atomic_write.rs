use std::io::Write;
use std::path::Path;

/// Write `contents` to `path` atomically. `mode` is applied on Unix before any
/// bytes are written.
/// The temp file name is fixed (`<name>.tmp`), so concurrent writers must be
/// excluded by the caller — auth.json writes hold the inter-process lock.
pub fn write_secret_file_atomic(path: &Path, contents: &str, mode: u32) -> std::io::Result<()> {
    #[cfg(not(unix))]
    let _ = mode;

    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "secret".to_owned());
    let temp_path = path.with_file_name(format!("{file_name}.tmp"));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }

    let result = (|| {
        let mut file = options.open(&temp_path)?;
        #[cfg(unix)]
        {
            // `mode(...)` only applies to a newly created file; a leftover temp
            // file keeps its old permissions.
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp_path, path)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_contents_and_leaves_no_temp_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("auth.json");
        write_secret_file_atomic(&path, "{\"a\":1}", 0o600).expect("write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{\"a\":1}");
        assert!(!directory.path().join("auth.json.tmp").exists());
    }

    #[test]
    fn replaces_an_existing_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("auth.json");
        std::fs::write(&path, "old").expect("seed");
        write_secret_file_atomic(&path, "new", 0o600).expect("write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "new");
    }

    #[cfg(unix)]
    #[test]
    fn the_final_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("auth.json");
        write_secret_file_atomic(&path, "{}", 0o600).expect("write");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
