//! Port of `packages/coding-agent/src/utils/clipboard-image.ts` (300 LOC) —
//! reading an image out of the system clipboard for `app.clipboard.pasteImage`.
//!
//! **Deviation (class 3), the same one `utils/clipboard.rs` documents:** the
//! native addon `@mariozechner/clipboard` is gone, so the two paths that went
//! through it are replaced by the platform's own tools — `osascript` on macOS
//! (which is what the addon calls underneath) and the same PowerShell script
//! the TypeScript already uses for WSL on Windows. The Linux paths (wl-paste,
//! xclip, PowerShell under WSL) are ported as they are.
//!
//! **Deviation (class 3):** `photon` becomes `image` for the format
//! conversion, as the master substitution table sets out; the shared helper is
//! `crate::utils::image::convert_image_bytes_to_png`.

use std::path::PathBuf;

use crate::utils::clipboard::{ClipboardOperations, Platform, SystemClipboard, is_wayland_session};

/// `ClipboardImage`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

const SUPPORTED_IMAGE_MIME_TYPES: [&str; 4] =
    ["image/png", "image/jpeg", "image/webp", "image/gif"];

const DEFAULT_LIST_TIMEOUT_MS: u64 = 1000;
const DEFAULT_READ_TIMEOUT_MS: u64 = 3000;
const DEFAULT_POWERSHELL_TIMEOUT_MS: u64 = 5000;

/// `baseMimeType(mimeType)`
fn base_mime_type(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_lowercase()
}

/// `extensionForImageMimeType(mimeType)`
pub fn extension_for_image_mime_type(mime_type: &str) -> Option<&'static str> {
    match base_mime_type(mime_type).as_str() {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        _ => None,
    }
}

/// `selectPreferredImageMimeType(mimeTypes)`
fn select_preferred_image_mime_type(mime_types: &[String]) -> Option<String> {
    let normalized: Vec<(String, String)> = mime_types
        .iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(|value| {
            let base = base_mime_type(&value);
            (value, base)
        })
        .collect();
    for preferred in SUPPORTED_IMAGE_MIME_TYPES {
        if let Some((raw, _)) = normalized.iter().find(|(_, base)| base == preferred) {
            return Some(raw.clone());
        }
    }
    normalized
        .into_iter()
        .find(|(_, base)| base.starts_with("image/"))
        .map(|(raw, _)| raw)
}

fn is_supported_image_mime_type(mime_type: &str) -> bool {
    let base = base_mime_type(mime_type);
    SUPPORTED_IMAGE_MIME_TYPES.contains(&base.as_str())
}

/// The process calls this module makes, behind a seam like
/// [`ClipboardOperations`] so the selection logic is testable.
pub trait ClipboardImageOperations {
    /// `spawnSync(command, args)` — standard output on a zero exit, nothing
    /// otherwise (a missing binary included).
    fn run(&self, command: &str, args: &[&str], timeout_ms: u64) -> Option<Vec<u8>>;
    /// Read a file the command wrote.
    fn read_file(&self, path: &std::path::Path) -> Option<Vec<u8>>;
    /// Remove that file again.
    fn remove_file(&self, path: &std::path::Path);
    /// `/proc/version`, for the WSL check.
    fn proc_version(&self) -> Option<String>;
}

/// The production implementation.
pub struct SystemClipboardImages;

impl ClipboardImageOperations for SystemClipboardImages {
    fn run(&self, command: &str, args: &[&str], timeout_ms: u64) -> Option<Vec<u8>> {
        // `spawnSync`'s timeout has no direct equivalent in `std::process`; the
        // child is spawned and waited for, and the timeout is enforced by a
        // watchdog that kills it (deviation class 1, same observable result).
        use std::process::{Command, Stdio};
        let child = Command::new(command)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let id = child.id();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watchdog_done = std::sync::Arc::clone(&done);
        let watchdog = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
            while std::time::Instant::now() < deadline {
                if watchdog_done.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if !watchdog_done.load(std::sync::atomic::Ordering::Relaxed) {
                // SIGKILL, as Node's `spawnSync` timeout does.
                #[cfg(unix)]
                unsafe {
                    libc::kill(id as libc::pid_t, libc::SIGKILL);
                }
                #[cfg(not(unix))]
                let _ = id;
            }
        });
        let output = child.wait_with_output().ok();
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = watchdog.join();
        let output = output?;
        output.status.success().then_some(output.stdout)
    }

    fn read_file(&self, path: &std::path::Path) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }

    fn remove_file(&self, path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
    }

    fn proc_version(&self) -> Option<String> {
        std::fs::read_to_string("/proc/version").ok()
    }
}

/// `isWSL(env)`
fn is_wsl(env: &dyn ClipboardOperations, images: &dyn ClipboardImageOperations) -> bool {
    if env.env("WSL_DISTRO_NAME").is_some() || env.env("WSLENV").is_some() {
        return true;
    }
    images.proc_version().is_some_and(|release| {
        let release = release.to_lowercase();
        release.contains("microsoft") || release.contains("wsl")
    })
}

fn lines(output: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(output)
        .split('\n')
        .map(|line| line.trim_end_matches('\r').trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect()
}

/// `readClipboardImageViaWlPaste()`
fn read_via_wl_paste(images: &dyn ClipboardImageOperations) -> Option<ClipboardImage> {
    let list = images.run("wl-paste", &["--list-types"], DEFAULT_LIST_TIMEOUT_MS)?;
    let selected = select_preferred_image_mime_type(&lines(&list))?;
    let data = images.run(
        "wl-paste",
        &["--type", &selected, "--no-newline"],
        DEFAULT_READ_TIMEOUT_MS,
    )?;
    if data.is_empty() {
        return None;
    }
    Some(ClipboardImage {
        bytes: data,
        mime_type: base_mime_type(&selected),
    })
}

/// `readClipboardImageViaXclip()`
fn read_via_xclip(images: &dyn ClipboardImageOperations) -> Option<ClipboardImage> {
    let targets = images.run(
        "xclip",
        &["-selection", "clipboard", "-t", "TARGETS", "-o"],
        DEFAULT_LIST_TIMEOUT_MS,
    );
    let candidates = targets.map(|targets| lines(&targets)).unwrap_or_default();
    let preferred = (!candidates.is_empty())
        .then(|| select_preferred_image_mime_type(&candidates))
        .flatten();

    let mut try_types: Vec<String> = Vec::new();
    if let Some(preferred) = preferred {
        try_types.push(preferred);
    }
    try_types.extend(
        SUPPORTED_IMAGE_MIME_TYPES
            .iter()
            .map(|value| (*value).to_owned()),
    );

    for mime_type in try_types {
        if let Some(data) = images.run(
            "xclip",
            &["-selection", "clipboard", "-t", &mime_type, "-o"],
            DEFAULT_READ_TIMEOUT_MS,
        ) && !data.is_empty()
        {
            return Some(ClipboardImage {
                bytes: data,
                mime_type: base_mime_type(&mime_type),
            });
        }
    }
    None
}

/// The PowerShell script both the WSL fallback and the Windows path use.
fn powershell_save_script(windows_path: &str) -> String {
    let quoted = windows_path.replace('\'', "''");
    [
        "Add-Type -AssemblyName System.Windows.Forms".to_owned(),
        "Add-Type -AssemblyName System.Drawing".to_owned(),
        format!("$path = '{quoted}'"),
        "$img = [System.Windows.Forms.Clipboard]::GetImage()".to_owned(),
        "if ($img) { $img.Save($path, [System.Drawing.Imaging.ImageFormat]::Png); Write-Output 'ok' } else { Write-Output 'empty' }".to_owned(),
    ]
    .join("; ")
}

fn temp_png_path(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{prefix}-{}.png", uuid::Uuid::new_v4()))
}

/// `readClipboardImageViaPowerShell()`
fn read_via_powershell_wsl(images: &dyn ClipboardImageOperations) -> Option<ClipboardImage> {
    let temp_file = temp_png_path("notagent-wsl-clip");
    let result = (|| {
        let windows_path = images.run(
            "wslpath",
            &["-w", &temp_file.to_string_lossy()],
            DEFAULT_LIST_TIMEOUT_MS,
        )?;
        let windows_path = String::from_utf8_lossy(&windows_path).trim().to_owned();
        if windows_path.is_empty() {
            return None;
        }
        let output = images.run(
            "powershell.exe",
            &[
                "-NoProfile",
                "-Command",
                &powershell_save_script(&windows_path),
            ],
            DEFAULT_POWERSHELL_TIMEOUT_MS,
        )?;
        if String::from_utf8_lossy(&output).trim() != "ok" {
            return None;
        }
        let bytes = images.read_file(&temp_file)?;
        (!bytes.is_empty()).then_some(ClipboardImage {
            bytes,
            mime_type: "image/png".to_owned(),
        })
    })();
    images.remove_file(&temp_file);
    result
}

/// The Windows counterpart of the native addon: the same PowerShell save,
/// without the `wslpath` translation.
fn read_via_powershell_windows(images: &dyn ClipboardImageOperations) -> Option<ClipboardImage> {
    let temp_file = temp_png_path("notagent-clip");
    let result = (|| {
        let output = images.run(
            "powershell.exe",
            &[
                "-NoProfile",
                "-Command",
                &powershell_save_script(&temp_file.to_string_lossy()),
            ],
            DEFAULT_POWERSHELL_TIMEOUT_MS,
        )?;
        if String::from_utf8_lossy(&output).trim() != "ok" {
            return None;
        }
        let bytes = images.read_file(&temp_file)?;
        (!bytes.is_empty()).then_some(ClipboardImage {
            bytes,
            mime_type: "image/png".to_owned(),
        })
    })();
    images.remove_file(&temp_file);
    result
}

/// The macOS counterpart of the native addon.
///
/// `osascript` writes the clipboard's PNG rendition to a file; the addon calls
/// the same AppKit pasteboard underneath.
fn read_via_osascript(images: &dyn ClipboardImageOperations) -> Option<ClipboardImage> {
    let temp_file = temp_png_path("notagent-clip");
    let script = format!(
        "try\nset theFile to (open for access POSIX file \"{}\" with write permission)\nwrite (the clipboard as «class PNGf») to theFile\nclose access theFile\nreturn \"ok\"\non error\ntry\nclose access POSIX file \"{}\"\nend try\nreturn \"empty\"\nend try",
        temp_file.to_string_lossy(),
        temp_file.to_string_lossy()
    );
    let result = (|| {
        let output = images.run("osascript", &["-e", &script], DEFAULT_READ_TIMEOUT_MS)?;
        if String::from_utf8_lossy(&output).trim() != "ok" {
            return None;
        }
        let bytes = images.read_file(&temp_file)?;
        (!bytes.is_empty()).then_some(ClipboardImage {
            bytes,
            mime_type: "image/png".to_owned(),
        })
    })();
    images.remove_file(&temp_file);
    result
}

/// `readClipboardImage(options)`
pub fn read_clipboard_image() -> Option<ClipboardImage> {
    read_clipboard_image_with(&SystemClipboard, &SystemClipboardImages)
}

/// `readClipboardImage(options)` against injected operations.
pub fn read_clipboard_image_with(
    env: &dyn ClipboardOperations,
    images: &dyn ClipboardImageOperations,
) -> Option<ClipboardImage> {
    if env.env("TERMUX_VERSION").is_some() {
        return None;
    }

    let image = match env.platform() {
        Platform::Other => {
            let wsl = is_wsl(env, images);
            let wayland = is_wayland_session(env);
            let mut image = None;
            if wayland || wsl {
                image = read_via_wl_paste(images).or_else(|| read_via_xclip(images));
            }
            if image.is_none() && wsl {
                image = read_via_powershell_wsl(images);
            }
            if image.is_none() && !wayland {
                image = read_via_xclip(images);
            }
            image
        }
        Platform::Darwin => read_via_osascript(images),
        Platform::Win32 => read_via_powershell_windows(images),
    }?;

    if is_supported_image_mime_type(&image.mime_type) {
        return Some(image);
    }
    // Convert unsupported formats (e.g. BMP from WSLg) to PNG.
    let png_bytes = crate::utils::image::convert_image_bytes_to_png(&image.bytes)?;
    Some(ClipboardImage {
        bytes: png_bytes,
        mime_type: "image/png".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder {
        /// Command name to the output it produces.
        outputs: Vec<(String, Vec<u8>)>,
        files: Vec<(String, Vec<u8>)>,
        proc_version: Option<String>,
        calls: RefCell<Vec<String>>,
    }

    impl ClipboardImageOperations for Recorder {
        fn run(&self, command: &str, args: &[&str], _timeout_ms: u64) -> Option<Vec<u8>> {
            self.calls
                .borrow_mut()
                .push(format!("{command} {}", args.join(" ")));
            self.outputs
                .iter()
                .find(|(name, _)| {
                    name == command || format!("{command} {}", args.join(" ")) == *name
                })
                .map(|(_, output)| output.clone())
        }

        fn read_file(&self, path: &std::path::Path) -> Option<Vec<u8>> {
            let _ = path;
            self.files.first().map(|(_, bytes)| bytes.clone())
        }

        fn remove_file(&self, _path: &std::path::Path) {}

        fn proc_version(&self) -> Option<String> {
            self.proc_version.clone()
        }
    }

    struct Env {
        platform: Platform,
        env: Vec<(String, String)>,
    }

    impl ClipboardOperations for Env {
        fn env(&self, name: &str) -> Option<String> {
            self.env
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
        fn platform(&self) -> Platform {
            self.platform
        }
    }

    #[test]
    fn prefers_png_over_the_other_offered_types() {
        let types = vec![
            "image/bmp".to_owned(),
            "image/jpeg".to_owned(),
            "image/png".to_owned(),
        ];
        assert_eq!(
            select_preferred_image_mime_type(&types).as_deref(),
            Some("image/png")
        );
    }

    #[test]
    fn falls_back_to_any_image_type() {
        let types = vec!["text/plain".to_owned(), "image/bmp".to_owned()];
        assert_eq!(
            select_preferred_image_mime_type(&types).as_deref(),
            Some("image/bmp")
        );
    }

    #[test]
    fn reports_the_extension_of_every_supported_type() {
        assert_eq!(extension_for_image_mime_type("image/png"), Some("png"));
        assert_eq!(
            extension_for_image_mime_type("image/jpeg; charset=binary"),
            Some("jpg")
        );
        assert_eq!(extension_for_image_mime_type("text/plain"), None);
    }

    #[test]
    fn termux_reads_nothing() {
        let env = Env {
            platform: Platform::Other,
            env: vec![("TERMUX_VERSION".to_owned(), "0.118".to_owned())],
        };
        let images = Recorder::default();
        assert_eq!(read_clipboard_image_with(&env, &images), None);
        assert!(images.calls.borrow().is_empty());
    }

    #[test]
    fn wayland_reads_through_wl_paste() {
        let env = Env {
            platform: Platform::Other,
            env: vec![("WAYLAND_DISPLAY".to_owned(), "wayland-0".to_owned())],
        };
        let images = Recorder {
            outputs: vec![
                ("wl-paste --list-types".to_owned(), b"image/png\n".to_vec()),
                (
                    "wl-paste --type image/png --no-newline".to_owned(),
                    b"\x89PNG-bytes".to_vec(),
                ),
            ],
            ..Recorder::default()
        };
        let image = read_clipboard_image_with(&env, &images).expect("an image");
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.bytes, b"\x89PNG-bytes".to_vec());
    }

    #[test]
    fn macos_reads_through_osascript() {
        let env = Env {
            platform: Platform::Darwin,
            env: Vec::new(),
        };
        let images = Recorder {
            outputs: vec![("osascript".to_owned(), b"ok\n".to_vec())],
            files: vec![("clip.png".to_owned(), b"\x89PNG-from-osascript".to_vec())],
            ..Recorder::default()
        };
        let image = read_clipboard_image_with(&env, &images).expect("an image");
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.bytes, b"\x89PNG-from-osascript".to_vec());
        assert!(
            images
                .calls
                .borrow()
                .iter()
                .any(|call| call.contains("osascript")),
            "the macOS path asks osascript: {:?}",
            images.calls.borrow()
        );
    }

    #[test]
    fn an_empty_clipboard_reads_as_nothing() {
        let env = Env {
            platform: Platform::Darwin,
            env: Vec::new(),
        };
        let images = Recorder {
            outputs: vec![("osascript".to_owned(), b"empty\n".to_vec())],
            ..Recorder::default()
        };
        assert_eq!(read_clipboard_image_with(&env, &images), None);
    }
}
