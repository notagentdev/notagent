use std::path::Path;

/// A session file whose recorded working directory no longer exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCwdIssue {
    pub session_file: Option<String>,
    pub session_cwd: String,
    pub fallback_cwd: String,
}

/// What the check needs from a session manager.
pub trait SessionCwdSource {
    fn get_cwd(&self) -> &str;
    fn get_session_file(&self) -> Option<&str>;
}

impl SessionCwdSource for crate::core::session_manager::SessionManager {
    fn get_cwd(&self) -> &str {
        crate::core::session_manager::SessionManager::get_cwd(self)
    }

    fn get_session_file(&self) -> Option<&str> {
        crate::core::session_manager::SessionManager::get_session_file(self)
    }
}

/// The issue, or `None` when there is none. An in-memory session has no file
/// and no recorded directory to be wrong about.
pub fn get_missing_session_cwd_issue(
    session_manager: &dyn SessionCwdSource,
    fallback_cwd: &str,
) -> Option<SessionCwdIssue> {
    let session_file = session_manager.get_session_file()?;

    let session_cwd = session_manager.get_cwd();
    if session_cwd.is_empty() || Path::new(session_cwd).exists() {
        return None;
    }

    Some(SessionCwdIssue {
        session_file: Some(session_file.to_string()),
        session_cwd: session_cwd.to_string(),
        fallback_cwd: fallback_cwd.to_string(),
    })
}

pub fn format_missing_session_cwd_error(issue: &SessionCwdIssue) -> String {
    let session_file = issue
        .session_file
        .as_ref()
        .map(|file| format!("\nSession file: {file}"))
        .unwrap_or_default();
    format!(
        "Stored session working directory does not exist: {}{session_file}\nCurrent working directory: {}",
        issue.session_cwd, issue.fallback_cwd
    )
}

pub fn format_missing_session_cwd_prompt(issue: &SessionCwdIssue) -> String {
    format!(
        "cwd from session file does not exist\n{}\n\ncontinue in current cwd\n{}",
        issue.session_cwd, issue.fallback_cwd
    )
}

/// The error the runtime raises rather than opening a session it cannot run in.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", format_missing_session_cwd_error(.0))]
pub struct MissingSessionCwdError(pub SessionCwdIssue);

pub fn assert_session_cwd_exists(
    session_manager: &dyn SessionCwdSource,
    fallback_cwd: &str,
) -> Result<(), MissingSessionCwdError> {
    match get_missing_session_cwd_issue(session_manager, fallback_cwd) {
        Some(issue) => Err(MissingSessionCwdError(issue)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Source {
        cwd: String,
        session_file: Option<String>,
    }

    impl SessionCwdSource for Source {
        fn get_cwd(&self) -> &str {
            &self.cwd
        }
        fn get_session_file(&self) -> Option<&str> {
            self.session_file.as_deref()
        }
    }

    #[test]
    fn an_in_memory_session_has_nothing_to_be_wrong_about() {
        let source = Source {
            cwd: "/gone".to_string(),
            session_file: None,
        };
        assert_eq!(get_missing_session_cwd_issue(&source, "/here"), None);
    }

    #[test]
    fn a_directory_that_still_exists_is_no_issue() {
        let temp = tempfile::tempdir().expect("temp dir");
        let source = Source {
            cwd: temp.path().to_string_lossy().into_owned(),
            session_file: Some("/sessions/a.jsonl".to_string()),
        };
        assert_eq!(get_missing_session_cwd_issue(&source, "/here"), None);
    }

    #[test]
    fn a_missing_directory_is_reported_with_both_paths() {
        let source = Source {
            cwd: "/definitely/not/here".to_string(),
            session_file: Some("/sessions/a.jsonl".to_string()),
        };
        let issue = get_missing_session_cwd_issue(&source, "/here").expect("issue");
        assert_eq!(issue.session_cwd, "/definitely/not/here");
        assert_eq!(issue.fallback_cwd, "/here");

        let message = format_missing_session_cwd_error(&issue);
        assert!(message.contains("/definitely/not/here"));
        assert!(message.contains("Session file: /sessions/a.jsonl"));
        assert!(message.contains("Current working directory: /here"));

        let prompt = format_missing_session_cwd_prompt(&issue);
        assert!(prompt.starts_with("cwd from session file does not exist"));
        assert!(prompt.contains("continue in current cwd"));

        assert!(assert_session_cwd_exists(&source, "/here").is_err());
    }
}
