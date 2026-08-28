//! `/init`: the brief that produces the project's `AGENTS.md`, and the
//! reminder that carries the result back.
//! The work runs in a subagent rather than in the main conversation, because
//! exploring a repository costs far more context than the file it produces. The
//! main agent would spend its window on directory listings and file reads it
//! never needs again; a child spends its own and hands back one file.
//! That split creates the problem this module's second half solves: the child's
//! conversation is not the parent's, so once the child is gone the main agent
//! has no idea what was written. The reminder puts the finished file into the
//! main conversation, which is also what the next turn would have needed anyway
//! — the context files are read when the session builds its system prompt, and
//! one written afterwards would otherwise stay invisible.

use std::path::Path;

/// What the child is asked to do. Kept as its own file so it reads as the prose
/// it is.
pub const INIT_PROMPT: &str = include_str!("session_init/init.md");

/// The file the brief names. Only this one is read back: the loader would also
/// accept `CLAUDE.md` and an override variant, but `/init` wrote exactly one
/// file and the reminder has to speak about that one.
pub const INIT_FILE_NAME: &str = "AGENTS.md";

/// The custom-message type the reminder is recorded under.
pub const INIT_REMINDER_TYPE: &str = "init";

/// Reads back what the child wrote, or `None` when it wrote nothing usable.
/// Read from disk rather than taken from the child's answer: a child that
/// reports success without writing is exactly the case worth catching.
pub fn read_written_guide(cwd: &str) -> Option<String> {
    let content = std::fs::read_to_string(Path::new(cwd).join(INIT_FILE_NAME)).ok()?;
    (!content.trim().is_empty()).then_some(content)
}

/// The reminder appended to the main conversation once the child is done.
pub fn init_completion_reminder(written: Option<&str>) -> String {
    let mut lines = vec![format!(
        "The user ran /init. A subagent explored the project and wrote its findings to {INIT_FILE_NAME}."
    )];
    match written {
        Some(content) => {
            lines.push(String::new());
            lines.push(format!("Current {INIT_FILE_NAME}:"));
            lines.push(content.trim_end().to_string());
        }
        // Worth saying rather than staying silent: the run reported success, so
        // an empty result means the child did not do what it was asked.
        None => lines.push(format!(
            "No {INIT_FILE_NAME} content was found afterwards, so the file was not written."
        )),
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brief_asks_for_the_file_it_is_named_after() {
        assert!(INIT_PROMPT.contains(INIT_FILE_NAME));
        assert!(!INIT_PROMPT.trim().is_empty());
    }

    #[test]
    fn the_reminder_carries_the_written_file() {
        let reminder = init_completion_reminder(Some("# Guide\n\nBuild with cargo.\n"));
        assert!(reminder.contains("The user ran /init"));
        assert!(reminder.contains("Build with cargo."));
    }

    #[test]
    fn the_reminder_says_so_when_nothing_was_written() {
        let reminder = init_completion_reminder(None);
        assert!(reminder.contains("was not written"));
    }

    #[test]
    fn a_missing_or_empty_file_reads_back_as_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cwd = dir.path().to_string_lossy().into_owned();
        assert_eq!(read_written_guide(&cwd), None);

        std::fs::write(dir.path().join(INIT_FILE_NAME), "   \n").expect("write");
        assert_eq!(read_written_guide(&cwd), None);

        std::fs::write(dir.path().join(INIT_FILE_NAME), "# Guide").expect("write");
        assert_eq!(read_written_guide(&cwd).as_deref(), Some("# Guide"));
    }
}
