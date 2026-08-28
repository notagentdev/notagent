use std::collections::HashSet;

/// Subagents accepted by one delegation call.
pub const MAX_DELEGATIONS_PER_CALL: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationRefusal {
    Empty,
    PerCall { requested: usize },
    Duplicate { mode_id: String, task: String },
}

/// Refusal of a delegation request. Carries the numbers so the model can adapt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationError {
    pub refusal: DelegationRefusal,
}

impl DelegationError {
    pub fn new(refusal: DelegationRefusal) -> Self {
        DelegationError { refusal }
    }

    pub fn message(&self) -> String {
        describe_refusal(&self.refusal)
    }
}

impl std::fmt::Display for DelegationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for DelegationError {}

fn describe_refusal(refusal: &DelegationRefusal) -> String {
    match refusal {
        DelegationRefusal::Empty => "Delegation requires at least one non-empty task.".to_owned(),
        DelegationRefusal::PerCall { requested } => format!(
            "Delegation limit exceeded: requested {requested} tasks in one call; maximum is {MAX_DELEGATIONS_PER_CALL}. Consolidate the work into at most {MAX_DELEGATIONS_PER_CALL} independent tasks."
        ),
        DelegationRefusal::Duplicate { mode_id, task } => format!(
            "Duplicate delegation rejected for mode \"{mode_id}\": the same task was requested twice in one call. Ask for it once. Task: {task}"
        ),
    }
}

/// The JS regex `/[!-\/:-@\[-`{-~]+$/` — every ASCII punctuation run at the end.
fn strip_trailing_punctuation(value: &str) -> &str {
    value.trim_end_matches(
        |character: char| matches!(character, '!'..='/' | ':'..='@' | '['..='`' | '{'..='~'),
    )
}

/// Identity of a delegated task, for the purpose of catching a repeat.
/// Whitespace runs collapse, trailing punctuation goes, and case is dropped, so
/// "Inspect the auth module." and "inspect  the auth module" are one task. The
/// mode is part of the identity: the same question asked of a read-only and a
/// worker subagent are different questions.
/// Returns an empty string for a task that normalizes to nothing, which the
/// caller treats as an empty request rather than as a fingerprint.
pub fn delegation_fingerprint(mode_id: &str, task: &str) -> String {
    let collapsed = task
        .split(is_js_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<&str>>()
        .join(" ");
    let normalized = strip_trailing_punctuation(&collapsed).to_lowercase();
    if normalized.is_empty() {
        return String::new();
    }
    format!("{}\n{normalized}", mode_id.trim().to_lowercase())
}

/// `\s` of JavaScript: Unicode whitespace plus the line terminators, minus the
/// next-line character Rust counts as whitespace and JS does not.
fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// Checks one delegation call, reporting the reason it cannot proceed.
pub fn check_delegation_request(mode_id: &str, tasks: &[String]) -> Result<(), DelegationError> {
    let prints: Vec<(String, String)> = tasks
        .iter()
        .map(|task| (task.clone(), delegation_fingerprint(mode_id, task)))
        .collect();
    if prints.is_empty() || prints.iter().any(|(_, print)| print.is_empty()) {
        return Err(DelegationError::new(DelegationRefusal::Empty));
    }
    if prints.len() > MAX_DELEGATIONS_PER_CALL {
        return Err(DelegationError::new(DelegationRefusal::PerCall {
            requested: prints.len(),
        }));
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for (task, print) in &prints {
        if !seen.insert(print.as_str()) {
            return Err(DelegationError::new(DelegationRefusal::Duplicate {
                mode_id: mode_id.to_owned(),
                task: task.clone(),
            }));
        }
    }
    Ok(())
}
