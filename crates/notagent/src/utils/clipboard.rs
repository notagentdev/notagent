use base64::Engine;

/// The largest OSC 52 payload that still goes out, in encoded characters.
const MAX_OSC52_ENCODED_LENGTH: usize = 100_000;

/// The process calls a clipboard tool makes. Injectable so the resolution order
/// can be tested without a display server.
pub trait ClipboardOperations {
    /// Look up an environment variable.
    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    /// Whether this is macOS, Windows or something else. `process.platform`.
    fn platform(&self) -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Darwin
        } else if cfg!(target_os = "windows") {
            Platform::Win32
        } else {
            Platform::Other
        }
    }

    /// Run `program` with `args` and write `input` to its standard input.
    /// Returns whether it exited successfully.
    fn write(&self, program: &str, args: &[&str], input: &str) -> bool {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        if let Some(stdin) = child.stdin.as_mut() {
            // `proc.stdin.on("error", …)` — a tool that exits early leaves a
            // broken pipe, which is not a failure of the copy itself.
            let _ = stdin.write_all(input.as_bytes());
        }
        drop(child.stdin.take());
        child.wait().map(|status| status.success()).unwrap_or(false)
    }

    /// Run `program` with `args` and return its standard output.
    fn capture(&self, program: &str, args: &[&str]) -> Option<String> {
        let output = std::process::Command::new(program)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Whether `program` is on the PATH (`execSync("which …")`).
    fn has_program(&self, program: &str) -> bool {
        self.capture("which", &[program]).is_some()
    }

    /// Write the OSC 52 sequence to standard output.
    fn emit_terminal_sequence(&self, sequence: &str) {
        use std::io::Write;
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(sequence.as_bytes());
        let _ = stdout.flush();
    }
}

/// `process.platform`, as far as the clipboard paths distinguish it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Darwin,
    Win32,
    Other,
}

/// The production implementation — every default method of the trait.
pub struct SystemClipboard;

impl ClipboardOperations for SystemClipboard {}

pub fn is_wayland_session(operations: &dyn ClipboardOperations) -> bool {
    operations
        .env("WAYLAND_DISPLAY")
        .is_some_and(|value| !value.is_empty())
        || operations.env("XDG_SESSION_TYPE").as_deref() == Some("wayland")
}

/// `isRemoteSession(env)`
fn is_remote_session(operations: &dyn ClipboardOperations) -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "MOSH_CONNECTION"]
        .iter()
        .any(|name| operations.env(name).is_some_and(|value| !value.is_empty()))
}

fn has_env(operations: &dyn ClipboardOperations, name: &str) -> bool {
    operations.env(name).is_some_and(|value| !value.is_empty())
}

/// `emitOsc52(text)`
fn emit_osc52(operations: &dyn ClipboardOperations, text: &str) -> bool {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    if encoded.len() > MAX_OSC52_ENCODED_LENGTH {
        return false;
    }
    operations.emit_terminal_sequence(&format!("\x1b]52;c;{encoded}\x07"));
    true
}

/// `copyToX11Clipboard(options)` — xclip first, xsel second.
fn copy_to_x11_clipboard(operations: &dyn ClipboardOperations, text: &str) {
    if operations.write("xclip", &["-selection", "clipboard"], text) {
        return;
    }
    operations.write("xsel", &["--clipboard", "--input"], text);
}

/// `copyToClipboard(text)`
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    copy_to_clipboard_with(&SystemClipboard, text)
}

/// `copyToClipboard(text)` against injected operations.
pub fn copy_to_clipboard_with(
    operations: &dyn ClipboardOperations,
    text: &str,
) -> Result<(), String> {
    let mut copied = false;
    let remote = is_remote_session(operations);

    match operations.platform() {
        Platform::Darwin => {
            copied = operations.write("pbcopy", &[], text);
        }
        Platform::Win32 => {
            copied = operations.write("clip", &[], text);
        }
        Platform::Other => {
            if has_env(operations, "TERMUX_VERSION") {
                copied = operations.write("termux-clipboard-set", &[], text);
            }
            if !copied {
                let has_wayland_display = has_env(operations, "WAYLAND_DISPLAY");
                let has_x11_display = has_env(operations, "DISPLAY");
                if is_wayland_session(operations) && has_wayland_display {
                    // error would arrive asynchronously and escape the `catch`.
                    if operations.has_program("wl-copy") && operations.write("wl-copy", &[], text) {
                        copied = true;
                    } else if has_x11_display {
                        copy_to_x11_clipboard(operations, text);
                        copied = true;
                    }
                } else if has_x11_display {
                    copy_to_x11_clipboard(operations, text);
                    copied = true;
                }
            }
        }
    }

    if remote || !copied {
        copied = emit_osc52(operations, text) || copied;
    }

    if copied {
        Ok(())
    } else {
        Err("Failed to copy to clipboard".to_owned())
    }
}

/// `readClipboardText()`
pub fn read_clipboard_text() -> Option<String> {
    read_clipboard_text_with(&SystemClipboard)
}

/// `readClipboardText()` against injected operations.
pub fn read_clipboard_text_with(operations: &dyn ClipboardOperations) -> Option<String> {
    let text = match operations.platform() {
        Platform::Darwin => operations.capture("pbpaste", &[]),
        Platform::Win32 => {
            operations.capture("powershell", &["-NoProfile", "-Command", "Get-Clipboard"])
        }
        Platform::Other => {
            if is_wayland_session(operations) && has_env(operations, "WAYLAND_DISPLAY") {
                operations
                    .capture("wl-paste", &["--no-newline", "--type", "text"])
                    .or_else(|| operations.capture("xclip", &["-selection", "clipboard", "-o"]))
            } else {
                operations.capture("xclip", &["-selection", "clipboard", "-o"])
            }
        }
    }?;
    // `return text || null` — an empty clipboard reads as nothing.
    if text.is_empty() { None } else { Some(text) }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder {
        platform: Option<Platform>,
        env: Vec<(String, String)>,
        /// Programs whose write succeeds; everything else fails.
        succeeds: Vec<String>,
        missing: Vec<String>,
        calls: RefCell<Vec<String>>,
    }

    impl ClipboardOperations for Recorder {
        fn env(&self, name: &str) -> Option<String> {
            self.env
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }

        fn platform(&self) -> Platform {
            self.platform.unwrap_or(Platform::Other)
        }

        fn write(&self, program: &str, _args: &[&str], _input: &str) -> bool {
            self.calls.borrow_mut().push(program.to_owned());
            self.succeeds.iter().any(|name| name == program)
        }

        fn capture(&self, program: &str, _args: &[&str]) -> Option<String> {
            self.calls.borrow_mut().push(program.to_owned());
            self.succeeds
                .iter()
                .any(|name| name == program)
                .then(|| "clipboard text".to_owned())
        }

        fn has_program(&self, program: &str) -> bool {
            !self.missing.iter().any(|name| name == program)
        }

        fn emit_terminal_sequence(&self, _sequence: &str) {
            self.calls.borrow_mut().push("osc52".to_owned());
        }
    }

    #[test]
    fn macos_writes_through_pbcopy() {
        let recorder = Recorder {
            platform: Some(Platform::Darwin),
            succeeds: vec!["pbcopy".to_owned()],
            ..Recorder::default()
        };
        assert!(copy_to_clipboard_with(&recorder, "hello").is_ok());
        assert_eq!(recorder.calls.borrow().as_slice(), ["pbcopy"]);
    }

    #[test]
    fn a_remote_session_also_emits_osc52() {
        let recorder = Recorder {
            platform: Some(Platform::Darwin),
            env: vec![("SSH_CONNECTION".to_owned(), "1 2 3 4".to_owned())],
            succeeds: vec!["pbcopy".to_owned()],
            ..Recorder::default()
        };
        assert!(copy_to_clipboard_with(&recorder, "hello").is_ok());
        assert_eq!(recorder.calls.borrow().as_slice(), ["pbcopy", "osc52"]);
    }

    #[test]
    fn wayland_falls_back_to_x11_when_wl_copy_is_missing() {
        let recorder = Recorder {
            platform: Some(Platform::Other),
            env: vec![
                ("WAYLAND_DISPLAY".to_owned(), "wayland-0".to_owned()),
                ("DISPLAY".to_owned(), ":0".to_owned()),
            ],
            succeeds: vec!["xclip".to_owned()],
            missing: vec!["wl-copy".to_owned()],
            ..Recorder::default()
        };
        assert!(copy_to_clipboard_with(&recorder, "hello").is_ok());
        assert_eq!(recorder.calls.borrow().as_slice(), ["xclip"]);
    }

    #[test]
    fn x11_falls_from_xclip_to_xsel() {
        let recorder = Recorder {
            platform: Some(Platform::Other),
            env: vec![("DISPLAY".to_owned(), ":0".to_owned())],
            succeeds: vec!["xsel".to_owned()],
            ..Recorder::default()
        };
        assert!(copy_to_clipboard_with(&recorder, "hello").is_ok());
        assert_eq!(recorder.calls.borrow().as_slice(), ["xclip", "xsel"]);
    }

    #[test]
    fn without_a_display_only_osc52_remains() {
        let recorder = Recorder {
            platform: Some(Platform::Other),
            ..Recorder::default()
        };
        assert!(copy_to_clipboard_with(&recorder, "hello").is_ok());
        assert_eq!(recorder.calls.borrow().as_slice(), ["osc52"]);
    }

    #[test]
    fn a_payload_too_large_for_osc52_fails() {
        let recorder = Recorder {
            platform: Some(Platform::Other),
            ..Recorder::default()
        };
        let text = "x".repeat(MAX_OSC52_ENCODED_LENGTH);
        assert_eq!(
            copy_to_clipboard_with(&recorder, &text),
            Err("Failed to copy to clipboard".to_owned())
        );
    }

    #[test]
    fn an_empty_clipboard_reads_as_nothing() {
        struct Empty;
        impl ClipboardOperations for Empty {
            fn platform(&self) -> Platform {
                Platform::Darwin
            }
            fn capture(&self, _program: &str, _args: &[&str]) -> Option<String> {
                Some(String::new())
            }
        }
        assert_eq!(read_clipboard_text_with(&Empty), None);
    }
}
