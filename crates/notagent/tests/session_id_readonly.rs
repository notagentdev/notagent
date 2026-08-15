//! Port of `packages/coding-agent/test/session-id-readonly.test.ts` and of
//! `test/session-file-invalid.test.ts`.
//!
//! Both spawn the real binary: what is under test is the startup order of
//! `main.ts` — which flags reserve a session on disk and which only read — and
//! that its failures reach the user as sentences rather than stack traces.

use std::path::{Path, PathBuf};
use std::process::Command;

struct CliDirs {
    _root: tempfile::TempDir,
    agent_dir: PathBuf,
    project_dir: PathBuf,
    session_dir: PathBuf,
}

fn create_dirs() -> CliDirs {
    let root = tempfile::Builder::new()
        .prefix("notagent-session-id-readonly-")
        .tempdir()
        .expect("temp dir");
    // On macOS the temp directory is reached through a symlink, but the spawned
    // process sees the physical path; session cwd filtering compares textually.
    let root_path = std::fs::canonicalize(root.path()).expect("canonical temp dir");
    let agent_dir = root_path.join("agent");
    let project_dir = root_path.join("project");
    let session_dir = root_path.join("sessions");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    CliDirs {
        _root: root,
        agent_dir,
        project_dir,
        session_dir,
    }
}

struct CliResult {
    code: Option<i32>,
    stderr: String,
}

fn run_cli(dirs: &CliDirs, args: &[&str]) -> CliResult {
    let output = Command::new(env!("CARGO_BIN_EXE_notagent"))
        .args(args)
        .current_dir(&dirs.project_dir)
        .env("NOTAGENT_CODING_AGENT_DIR", &dirs.agent_dir)
        .env("NOTAGENT_OFFLINE", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run notagent");
    CliResult {
        code: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn has_session_with_id(root: &Path, session_id: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if has_session_with_id(&path, session_id) {
                return true;
            }
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(first_line) = contents.split('\n').next() else {
            continue;
        };
        let Ok(header) = serde_json::from_str::<serde_json::Value>(first_line) else {
            continue;
        };
        if header.get("type").and_then(serde_json::Value::as_str) == Some("session")
            && header.get("id").and_then(serde_json::Value::as_str) == Some(session_id)
        {
            return true;
        }
    }
    false
}

fn write_session(session_dir: &Path, cwd: &Path, id: &str) {
    std::fs::create_dir_all(session_dir).expect("session dir");
    let header = serde_json::json!({
        "type": "session",
        "version": 3,
        "id": id,
        "timestamp": "2026-08-15T00:00:00.000Z",
        "cwd": cwd.to_string_lossy(),
    });
    std::fs::write(
        session_dir.join(format!("{id}.jsonl")),
        format!("{header}\n"),
    )
    .expect("write session");
}

#[test]
fn does_not_reserve_a_session_for_help() {
    let dirs = create_dirs();

    let result = run_cli(&dirs, &["--session-id", "read-only-help", "--help"]);

    assert_eq!(result.code, Some(0), "{}", result.stderr);
    assert!(!has_session_with_id(
        &dirs.agent_dir.join("sessions"),
        "read-only-help"
    ));
}

#[test]
fn allows_no_session_with_session_id() {
    let dirs = create_dirs();

    let result = run_cli(
        &dirs,
        &["--no-session", "--session-id", "ephemeral-id", "--help"],
    );

    assert_eq!(result.code, Some(0), "{}", result.stderr);
    assert!(!has_session_with_id(
        &dirs.agent_dir.join("sessions"),
        "ephemeral-id"
    ));
}

#[test]
fn does_not_reserve_a_session_for_list_models() {
    let dirs = create_dirs();

    let result = run_cli(
        &dirs,
        &["--session-id", "read-only-models", "--list-models"],
    );

    assert_eq!(result.code, Some(0), "{}", result.stderr);
    assert!(!has_session_with_id(
        &dirs.agent_dir.join("sessions"),
        "read-only-models"
    ));
}

#[test]
fn warns_when_a_missing_session_id_creates_a_new_session() {
    let dirs = create_dirs();
    let session_dir = dirs.session_dir.to_string_lossy().into_owned();

    let result = run_cli(
        &dirs,
        &[
            "--session-dir",
            &session_dir,
            "--session-id",
            "missing-session-id",
            "--model",
            "missing-model",
            "-p",
            "hi",
        ],
    );

    assert_eq!(result.code, Some(1));
    assert!(
        result.stderr.contains(
            "Warning: No project session found with id 'missing-session-id'; creating a new session with that id."
        ),
        "{}",
        result.stderr
    );
}

#[test]
fn does_not_warn_when_session_id_opens_an_existing_session() {
    let dirs = create_dirs();
    write_session(&dirs.session_dir, &dirs.project_dir, "existing-session-id");
    let session_dir = dirs.session_dir.to_string_lossy().into_owned();

    let result = run_cli(
        &dirs,
        &[
            "--session-dir",
            &session_dir,
            "--session-id",
            "existing-session-id",
            "--model",
            "missing-model",
            "-p",
            "hi",
        ],
    );

    assert_eq!(result.code, Some(1));
    assert!(
        !result
            .stderr
            .contains("No project session found with id 'existing-session-id'"),
        "{}",
        result.stderr
    );
}

#[test]
fn rejects_an_existing_fork_target_session_id() {
    let dirs = create_dirs();
    write_session(&dirs.session_dir, &dirs.project_dir, "source-id");
    write_session(&dirs.session_dir, &dirs.project_dir, "existing-id");
    let session_dir = dirs.session_dir.to_string_lossy().into_owned();

    let result = run_cli(
        &dirs,
        &[
            "--session-dir",
            &session_dir,
            "--fork",
            "source-id",
            "--session-id",
            "existing-id",
            "-p",
            "hi",
        ],
    );

    assert_eq!(result.code, Some(1));
    assert!(
        result
            .stderr
            .contains("Session already exists with id 'existing-id'"),
        "{}",
        result.stderr
    );
}

#[test]
fn rejects_ids_invalid_under_session_manager_rules_without_stack_traces() {
    for id in ["-bad", "bad id"] {
        let dirs = create_dirs();

        let result = run_cli(&dirs, &["--session-id", id, "-p", "hi"]);

        assert_eq!(result.code, Some(1));
        assert!(
            result.stderr.contains("Session id must be non-empty"),
            "{}",
            result.stderr
        );
        assert!(!result.stderr.contains("SessionManager.create"));
    }
}

#[test]
fn session_prints_a_friendly_error_and_preserves_non_session_file_content() {
    let dirs = create_dirs();
    let session_file = dirs
        .project_dir
        .parent()
        .expect("root")
        .join("not-a-session.log");
    let original_content = "{\"type\":\"event\",\"data\":\"not a session\"}\n";
    std::fs::write(&session_file, original_content).expect("write file");
    let session_path = session_file.to_string_lossy().into_owned();

    let result = run_cli(&dirs, &["--session", &session_path, "-p", "hi"]);

    assert_eq!(result.code, Some(1));
    assert!(
        result.stderr.contains(&format!(
            "Error: Session file is not a valid notagent session: {session_path}"
        )),
        "{}",
        result.stderr
    );
    assert!(!result.stderr.contains("SessionManager.open"));
    assert!(!result.stderr.contains("\n    at "));
    assert_eq!(
        std::fs::read_to_string(&session_file).expect("file"),
        original_content
    );
}
