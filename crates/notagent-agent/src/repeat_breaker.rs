//! Breaks runs of the same tool call issued over and over.
//! A model that has lost track of a result, or keeps hoping a failing command
//! will pass, can reissue one byte-identical call until its budget is gone. The
//! call itself is never refused here: each repetition past a threshold gets a
//! reminder appended to its result, escalating from "say what this call is for"
//! to "answer now", and after the last threshold the model is given one more
//! response in which tool calls are refused, so the run ends with a text answer
//! stating the blocker rather than mid-loop.
//! The streak counts across steps but not across user input: a steering or
//! follow-up message is new information, and a call repeated after it is not the
//! same loop.

use serde_json::{Map, Value};

use crate::types::{AgentToolCall, AgentToolResult};
use notagent_ai::types::{TextContent, TextOrImageContent};

/// Streak length at which each reminder starts.
const REFLECT_FROM: u32 = 3;
const CHOOSE_FROM: u32 = 5;
const CONCLUDE_FROM: u32 = 8;
/// Streak length at which the run is steered to its end.
const STOP_AT: u32 = 12;

const REFLECT_TEXT: &str = "The same tool call has been repeated several times in a row. Before making your next call, write one sentence stating what new information you expect it to produce. Then act on that sentence: if it names something this result does not already give you, choose the action that best provides it; otherwise, continue with the evidence you already have.";

const CONCLUDE_TEXT: &str = "Write your final response now, without any further tool calls. Cover: the current blocker, each approach you have tried and what it established, and the specific information or decision you need from the user to unblock progress. Text only.";

/// The result a tool call gets in the response after a forced stop.
pub(crate) fn handoff_refusal_text() -> String {
    format!(
        "This tool call was not executed: the same tool call was issued {STOP_AT} times in a row, and this response accepts text only. Reply in text: the current blocker, what you tried, and what you need next."
    )
}

fn choose_text(streak: u32) -> String {
    format!(
        "The same tool call has now been issued {streak} times in a row. Choose exactly one of the following and state your choice before acting:\n(1) Falsification check: run the cheapest test that could conclusively disprove your current approach, if such a test exists.\n(2) Missing input: tell the user precisely what information or decision you need to proceed, and ask for it.\n(3) Conclude: deliver your best result based on the evidence already gathered, listing anything that remains uncertain."
    )
}

/// What one call's result gets appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepeatNudge {
    Reflect,
    Choose(u32),
    Conclude,
    /// Conclude, and the next response is text-only.
    Stop,
}

impl RepeatNudge {
    fn for_streak(streak: u32) -> Option<Self> {
        match streak {
            s if s >= STOP_AT => Some(Self::Stop),
            s if s >= CONCLUDE_FROM => Some(Self::Conclude),
            s if s >= CHOOSE_FROM => Some(Self::Choose(s)),
            s if s >= REFLECT_FROM => Some(Self::Reflect),
            _ => None,
        }
    }

    fn text(self) -> String {
        let body = match self {
            Self::Reflect => REFLECT_TEXT.to_string(),
            Self::Choose(streak) => choose_text(streak),
            Self::Conclude | Self::Stop => CONCLUDE_TEXT.to_string(),
        };
        format!("<system_reminder>\n{body}\n</system_reminder>")
    }

    /// Appends the reminder to the text the model reads. It joins the last text
    /// block rather than adding one, because providers disagree on whether
    /// separate blocks of one result are separated at all.
    pub(crate) fn apply(self, result: &mut AgentToolResult) {
        let text = self.text();
        match result.content.last_mut() {
            Some(TextOrImageContent::Text(last)) => {
                if !last.text.is_empty() && !last.text.ends_with('\n') {
                    last.text.push('\n');
                }
                last.text.push('\n');
                last.text.push_str(&text);
            }
            _ => result
                .content
                .push(TextOrImageContent::Text(TextContent::new(text))),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct RepeatBreaker {
    last_key: Option<String>,
    streak: u32,
    handoff_pending: bool,
}

impl RepeatBreaker {
    /// Forgets the streak; called when user input arrives.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Records a batch in source order and returns the nudge for each call.
    /// Parallel calls in one batch count in the order the model wrote them, so
    /// the outcome does not depend on which call finishes first. Adjacent copies
    /// within one batch count once: the model saw no result between them, so
    /// they are not a result being ignored, and a reminder asking what the next
    /// call adds would have nothing to refer to.
    pub(crate) fn observe(&mut self, tool_calls: &[AgentToolCall]) -> Vec<Option<RepeatNudge>> {
        let mut previous_in_batch: Option<String> = None;
        tool_calls
            .iter()
            .map(|tool_call| {
                let key = call_key(tool_call);
                if previous_in_batch.as_deref() == Some(key.as_str()) {
                    return None;
                }
                previous_in_batch = Some(key.clone());
                if self.last_key.as_deref() == Some(key.as_str()) {
                    self.streak += 1;
                } else {
                    self.last_key = Some(key);
                    self.streak = 1;
                }
                let nudge = RepeatNudge::for_streak(self.streak);
                if nudge == Some(RepeatNudge::Stop) {
                    self.handoff_pending = true;
                }
                nudge
            })
            .collect()
    }

    /// Whether the response now arriving is the text-only one after a forced
    /// stop. Taking it clears the streak, so the refusal happens once.
    pub(crate) fn take_handoff(&mut self) -> bool {
        let pending = self.handoff_pending;
        if pending {
            self.reset();
        }
        pending
    }
}

/// Tool name plus arguments with object keys sorted, so two calls that differ
/// only in key order count as the same call.
fn call_key(tool_call: &AgentToolCall) -> String {
    let arguments = canonical(&Value::Object(tool_call.arguments.clone()));
    format!("{}\u{0}{arguments}", tool_call.name)
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            let mut sorted = Map::with_capacity(fields.len());
            for key in keys {
                if let Some(field) = fields.get(key) {
                    sorted.insert(key.clone(), canonical(field));
                }
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, arguments: Value) -> AgentToolCall {
        AgentToolCall {
            id: "id".to_string(),
            name: name.to_string(),
            arguments: arguments.as_object().cloned().unwrap_or_default(),
            ..Default::default()
        }
    }

    #[test]
    fn reminders_escalate_with_the_length_of_the_streak() {
        let mut breaker = RepeatBreaker::default();
        let same = call("bash", json!({"command": "cargo test"}));
        let nudges: Vec<Option<RepeatNudge>> = (0..STOP_AT)
            .flat_map(|_| breaker.observe(std::slice::from_ref(&same)))
            .collect();
        assert_eq!(nudges[0], None, "a first call is never nudged: {nudges:?}");
        assert_eq!(
            nudges[1], None,
            "a single retry is not yet a loop: {nudges:?}"
        );
        assert_eq!(nudges[2], Some(RepeatNudge::Reflect));
        assert_eq!(nudges[4], Some(RepeatNudge::Choose(5)));
        assert_eq!(nudges[7], Some(RepeatNudge::Conclude));
        assert_eq!(nudges[11], Some(RepeatNudge::Stop));
        assert!(breaker.take_handoff(), "the twelfth call arms the handoff");
        assert!(
            !breaker.take_handoff(),
            "the handoff is taken once and clears the streak"
        );
    }

    #[test]
    fn a_different_call_in_between_starts_the_streak_over() {
        let mut breaker = RepeatBreaker::default();
        let a = call("read", json!({"path": "a.rs"}));
        let b = call("read", json!({"path": "b.rs"}));
        let mut nudges = Vec::new();
        for next in [&a, &a, &b, &a, &a] {
            nudges.extend(breaker.observe(std::slice::from_ref(next)));
        }
        assert!(
            nudges.iter().all(Option::is_none),
            "no run of three identical calls exists here: {nudges:?}"
        );
    }

    #[test]
    fn calls_that_differ_only_in_key_order_are_the_same_call() {
        let mut breaker = RepeatBreaker::default();
        let mut nudges = Vec::new();
        for arguments in [
            json!({"pattern": "x", "path": "src"}),
            json!({"path": "src", "pattern": "x"}),
            json!({"path": "src", "pattern": "x"}),
        ] {
            nudges.extend(breaker.observe(&[call("grep", arguments)]));
        }
        assert_eq!(nudges[2], Some(RepeatNudge::Reflect), "{nudges:?}");
    }

    #[test]
    fn copies_within_one_batch_count_once() {
        let mut breaker = RepeatBreaker::default();
        let a = call("read", json!({"path": "a.rs"}));
        let first = breaker.observe(&[a.clone(), a.clone(), a.clone()]);
        assert!(
            first.iter().all(Option::is_none),
            "three copies side by side are one step, not a streak of three: {first:?}"
        );
        let second = breaker.observe(std::slice::from_ref(&a));
        assert_eq!(second, [None], "the batch counted as one, so this is two");
        let third = breaker.observe(std::slice::from_ref(&a));
        assert_eq!(third, [Some(RepeatNudge::Reflect)]);
    }

    #[test]
    fn the_same_arguments_to_another_tool_are_a_different_call() {
        let mut breaker = RepeatBreaker::default();
        let mut nudges = Vec::new();
        for (name, path) in [("read", "a"), ("ls", "a"), ("read", "a")] {
            nudges.extend(breaker.observe(&[call(name, json!({"path": path}))]));
        }
        assert!(nudges.iter().all(Option::is_none), "{nudges:?}");
    }

    #[test]
    fn a_reminder_joins_the_last_text_block_of_the_result() {
        let mut result = AgentToolResult::text("exit code 1");
        RepeatNudge::Reflect.apply(&mut result);
        assert_eq!(result.content.len(), 1, "no second block is added");
        let TextOrImageContent::Text(text) = &result.content[0] else {
            panic!("the result stays text");
        };
        assert!(
            text.text.starts_with("exit code 1\n\n<system_reminder>"),
            "the original output comes first, separated by a blank line: {:?}",
            text.text
        );
    }

    #[test]
    fn a_result_without_text_gets_the_reminder_as_its_own_block() {
        let mut result = AgentToolResult {
            content: vec![AgentToolResult::image("data", "image/png")],
            ..Default::default()
        };
        RepeatNudge::Conclude.apply(&mut result);
        assert_eq!(result.content.len(), 2);
        assert!(matches!(result.content[1], TextOrImageContent::Text(_)));
    }
}
