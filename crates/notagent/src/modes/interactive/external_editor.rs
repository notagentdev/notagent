//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/external-editor.ts` (47 LOC).
//!
//! Hands the prompt to the user's `$EDITOR` (Ctrl+G) and reads it back.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// What [`edit_in_external_editor`] was asked to edit.
pub struct ExternalEditorOptions {
    /// Editor command line, e.g. `code --wait`.
    pub command: String,
    /// Text the editor opens with.
    pub content: String,
}

/// Outcome of an editor session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalEditorResult {
    /// The editor exited with 0; the file's content, without a trailing newline.
    Complete(String),
    /// The editor could not be started or exited non-zero.
    Failed,
}

/// Run `options.command` on a temporary file holding `options.content`.
///
/// Deviation (class 1): TS avoids `spawnSync` because a synchronous Node child
/// keeps libuv's console read alive on Windows, which then races the editor for
/// the input buffer. Rust has no such reader — the terminal is only read while
/// the pump runs, and the caller stops it before handing the console over — so
/// the child is waited for directly.
pub fn edit_in_external_editor(options: &ExternalEditorOptions) -> ExternalEditorResult {
    let Some(directory) = make_temp_dir() else {
        return ExternalEditorResult::Failed;
    };
    let file_path = directory.join("prompt.md");
    let result = run_editor(options, &file_path);
    // Cleanup is best effort.
    let _ = std::fs::remove_dir_all(&directory);
    result
}

fn run_editor(options: &ExternalEditorOptions, file_path: &PathBuf) -> ExternalEditorResult {
    if std::fs::write(file_path, &options.content).is_err() {
        return ExternalEditorResult::Failed;
    }

    let mut parts = options.command.split(' ');
    let Some(editor) = parts.next() else {
        return ExternalEditorResult::Failed;
    };
    let editor_args: Vec<&str> = parts.collect();

    let mut stdout = std::io::stdout();
    let _ = write!(
        stdout,
        "Launching external editor: {}\nnotagent will resume when the editor exits.\n",
        options.command
    );
    let _ = stdout.flush();

    let mut command = if cfg!(windows) {
        // TS passes `shell: true` on Windows so `code --wait` resolves the
        // `.cmd` shim; cmd.exe does that lookup.
        let mut command = Command::new("cmd");
        command.arg("/C").arg(editor);
        command
    } else {
        Command::new(editor)
    };
    command
        .args(&editor_args)
        .arg(file_path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = match command.status() {
        Ok(status) => status,
        Err(_) => return ExternalEditorResult::Failed,
    };
    if !status.success() {
        return ExternalEditorResult::Failed;
    }

    match std::fs::read_to_string(file_path) {
        Ok(content) => ExternalEditorResult::Complete(
            content.strip_suffix('\n').unwrap_or(&content).to_string(),
        ),
        Err(_) => ExternalEditorResult::Failed,
    }
}

/// `mkdtempSync(join(tmpdir(), "notagent-editor-"))`.
fn make_temp_dir() -> Option<PathBuf> {
    let base = std::env::temp_dir();
    for attempt in 0..32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .subsec_nanos();
        let candidate = base.join(format!(
            "notagent-editor-{}-{attempt}-{nanos}",
            std::process::id()
        ));
        if std::fs::create_dir(&candidate).is_ok() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_failure_when_the_editor_cannot_be_started() {
        let result = edit_in_external_editor(&ExternalEditorOptions {
            command: "notagent-editor-that-does-not-exist".to_string(),
            content: "before".to_string(),
        });
        assert_eq!(result, ExternalEditorResult::Failed);
    }

    #[test]
    fn reports_failure_when_the_editor_exits_non_zero() {
        let result = edit_in_external_editor(&ExternalEditorOptions {
            command: "false".to_string(),
            content: "before".to_string(),
        });
        assert_eq!(result, ExternalEditorResult::Failed);
    }

    #[test]
    fn hands_the_content_to_the_editor_and_reads_it_back() {
        // `cp` writes the fixture over the temp file, then exits 0 — the same
        // shape as an editor saving and quitting.
        let fixture =
            std::env::temp_dir().join(format!("notagent-editor-fixture-{}.md", std::process::id()));
        std::fs::write(&fixture, "edited by the editor\n").expect("fixture written");
        let result = edit_in_external_editor(&ExternalEditorOptions {
            command: format!("cp {}", fixture.display()),
            content: "before".to_string(),
        });
        let _ = std::fs::remove_file(&fixture);
        assert_eq!(
            result,
            ExternalEditorResult::Complete("edited by the editor".to_string())
        );
    }
}
