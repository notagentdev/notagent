//! Structured questions the model puts to the user in the middle of a turn.
//! The tool (`core/tools/ask_user_question.rs`) validates and forwards; whoever
//! can show a dialog answers through a [`QuestionPresenter`]. Only the
//! interactive mode supplies one, bound in `main_app` the same way the approval
//! presenter is. Print mode, the server and every subagent have nobody to ask,
//! and there the tool fails with an instruction to decide instead — a question
//! that can never be answered must not stall a turn.

use std::collections::HashSet;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

pub const MIN_QUESTIONS: usize = 1;
pub const MAX_QUESTIONS: usize = 4;
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserQuestion {
    /// Echoed in the answer, so the model maps answers without relying on the
    /// question text surviving unchanged.
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub header: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub id: String,
    /// Labels of the chosen options, in the order they were offered.
    pub selected: Vec<String>,
    /// Free text typed instead of choosing an option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// Shows the questions and resolves with the answers, or `None` when the user
/// dismissed the dialog. A dismissal is not an answer, and the tool reports it
/// as one the model may not fill in by itself.
pub type QuestionPresenter =
    Arc<dyn Fn(Vec<UserQuestion>) -> BoxFuture<'static, Option<Vec<QuestionAnswer>>> + Send + Sync>;

/// Called once the dialog is actually on screen. A question the UI drops — the
/// session is being replaced, or the asking side already gave up — never
/// calls it, so nothing downstream reports a wait that is not happening.
pub type QuestionShown = Box<dyn FnOnce() + Send>;

/// What the interactive mode supplies: shows the questions, calls `shown` when
/// they are on screen, and resolves like a [`QuestionPresenter`]. Dropping the
/// returned future takes the dialog down again, so a cancelled question never
/// leaves a dead dialog in the editor's place.
pub type QuestionDisplay = Arc<
    dyn Fn(Vec<UserQuestion>, QuestionShown) -> BoxFuture<'static, Option<Vec<QuestionAnswer>>>
        + Send
        + Sync,
>;

/// Where a session finds its presenter at the moment of a call. A source
/// rather than a presenter, because the session and its tools exist before the
/// interactive mode that can show a dialog.
pub type QuestionPresenterSource = Arc<dyn Fn() -> Option<QuestionPresenter> + Send + Sync>;

/// Checks what a schema cannot express. Every message names the fix, because
/// the model reads it as a tool error and calls again.
pub fn validate_questions(questions: &[UserQuestion]) -> Result<(), String> {
    if !(MIN_QUESTIONS..=MAX_QUESTIONS).contains(&questions.len()) {
        return Err(format!(
            "Ask between {MIN_QUESTIONS} and {MAX_QUESTIONS} questions; got {}.",
            questions.len()
        ));
    }
    let mut ids = HashSet::new();
    let mut texts = HashSet::new();
    for question in questions {
        if question.id.trim().is_empty() {
            return Err("Every question needs a non-empty id.".to_owned());
        }
        if question.question.trim().is_empty() {
            return Err(format!("Question {:?} has no question text.", question.id));
        }
        if !ids.insert(question.id.as_str()) {
            return Err(format!(
                "Duplicate question id {:?}. Give every question its own id and call the tool again.",
                question.id
            ));
        }
        if !texts.insert(question.question.as_str()) {
            return Err(format!(
                "Duplicate question text {:?}. Rephrase the duplicates and call the tool again.",
                question.question
            ));
        }
        if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&question.options.len()) {
            return Err(format!(
                "Question {:?} needs between {MIN_OPTIONS} and {MAX_OPTIONS} options; got {}. Do not add an \"Other\" option, the dialog offers free text itself.",
                question.id,
                question.options.len()
            ));
        }
        let mut labels = HashSet::new();
        for option in &question.options {
            if option.label.trim().is_empty() {
                return Err(format!(
                    "Question {:?} has an option without a label.",
                    question.id
                ));
            }
            if !labels.insert(option.label.as_str()) {
                return Err(format!(
                    "Duplicate option label {:?} in question {:?}. Rephrase the duplicates and call the tool again.",
                    option.label, question.id
                ));
            }
        }
    }
    Ok(())
}

/// The answers as the model reads them: one line per question, keyed by id.
pub fn format_answers(questions: &[UserQuestion], answers: &[QuestionAnswer]) -> String {
    let mut lines = vec!["The user answered:".to_owned()];
    for question in questions {
        let answer = answers.iter().find(|answer| answer.id == question.id);
        let text = match answer {
            Some(QuestionAnswer {
                custom: Some(custom),
                ..
            }) => format!("(own answer) {custom}"),
            Some(answer) if !answer.selected.is_empty() => answer.selected.join(", "),
            _ => "(no answer)".to_owned(),
        };
        lines.push(format!("- {} ({}): {text}", question.id, question.question));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(id: &str, text: &str, labels: &[&str]) -> UserQuestion {
        UserQuestion {
            id: id.to_owned(),
            header: String::new(),
            question: text.to_owned(),
            options: labels
                .iter()
                .map(|label| QuestionOption {
                    label: (*label).to_owned(),
                    description: String::new(),
                })
                .collect(),
            multi_select: false,
        }
    }

    #[test]
    fn accepts_a_well_formed_question() {
        assert_eq!(
            validate_questions(&[question("db", "Which database?", &["Postgres", "SQLite"])]),
            Ok(())
        );
    }

    #[test]
    fn refuses_duplicate_labels_and_says_how_to_recover() {
        let error = validate_questions(&[question("db", "Which?", &["A", "A"])])
            .expect_err("duplicate labels must be refused");
        assert!(error.contains("call the tool again"), "saw: {error}");
    }

    #[test]
    fn refuses_duplicate_ids_across_questions() {
        let error = validate_questions(&[
            question("x", "First?", &["A", "B"]),
            question("x", "Second?", &["A", "B"]),
        ])
        .expect_err("duplicate ids must be refused");
        assert!(error.contains("Duplicate question id"), "saw: {error}");
    }

    #[test]
    fn refuses_a_single_option_because_that_is_not_a_choice() {
        assert!(validate_questions(&[question("x", "Only?", &["A"])]).is_err());
    }

    #[test]
    fn refuses_more_questions_than_a_dialog_should_carry() {
        let many: Vec<UserQuestion> = (0..5)
            .map(|index| question(&format!("q{index}"), &format!("Q{index}?"), &["A", "B"]))
            .collect();
        assert!(validate_questions(&many).is_err());
    }

    #[test]
    fn a_typed_answer_wins_over_selected_options_in_the_summary() {
        let questions = [question("db", "Which?", &["A", "B"])];
        let answers = [QuestionAnswer {
            id: "db".to_owned(),
            selected: vec![],
            custom: Some("MariaDB".to_owned()),
        }];
        let text = format_answers(&questions, &answers);
        assert!(
            text.contains("db (Which?): (own answer) MariaDB"),
            "saw: {text}"
        );
    }
}
