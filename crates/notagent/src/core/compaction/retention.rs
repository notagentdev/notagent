//! Which of the user's own messages survive a compaction, and how much of each.
//! Everything else in a session — assistant turns, tool results, bash output —
//! is reconstructible from the summary, in the sense that a paraphrase of it is
//! good enough to carry on. A request is not: paraphrasing "do not touch the
//! database" into "the user had reservations about the database" loses the
//! instruction. So the requests are kept verbatim and everything else is
//! summarized.
//! Keeping them is close to free. On a 377k-token session the user's messages
//! were 2,609 tokens — seven tenths of one percent — while tool results alone
//! were 216k. The budget below therefore rarely binds; it exists for the
//! sessions where someone pastes a large file into the prompt.

use notagent_agent::types::AgentMessage;
use notagent_ai::types::{TextContent, TextOrImageContent, UserContent, UserMessage};
use serde::{Deserialize, Serialize};

use crate::core::compaction::compaction::estimate_tokens;

/// Total budget for the user messages carried through a compaction.
pub const RETAINED_USER_TOKENS: u64 = 20_000;

/// The part of that budget reserved for the oldest messages.
/// A pure tail would lose the opening of the session, which is usually where
/// the goal is stated — and it would lose it precisely in the long sessions
/// where a compaction happens at all.
pub const RETAINED_HEAD_TOKENS: u64 = 2_000;

/// One retained message, and how much of it survived.
/// Both token fields absent means the message was kept whole. The counts are
/// stored rather than the truncated text so that the session file keeps a
/// single copy of every message; the text is derived again on load.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetainedEntry {
    pub id: String,
    /// Only the first N tokens survived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_tokens: Option<u64>,
    /// Only the last N tokens survived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix_tokens: Option<u64>,
}

impl RetainedEntry {
    pub fn whole(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            prefix_tokens: None,
            suffix_tokens: None,
        }
    }
}

/// The messages a compaction carries forward, oldest first within each half.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetainedSelection {
    /// The oldest messages, kept so the session's opening survives.
    #[serde(default)]
    pub head: Vec<RetainedEntry>,
    /// The newest messages.
    #[serde(default)]
    pub tail: Vec<RetainedEntry>,
    /// How much was dropped between the two. Zero means nothing was.
    #[serde(default)]
    pub omitted_tokens: u64,
}

impl RetainedSelection {
    pub fn is_empty(&self) -> bool {
        self.head.is_empty() && self.tail.is_empty()
    }

    /// Head and tail in context order.
    pub fn entries(&self) -> impl Iterator<Item = &RetainedEntry> {
        self.head.iter().chain(self.tail.iter())
    }

    pub fn find(&self, id: &str) -> Option<&RetainedEntry> {
        self.entries().find(|entry| entry.id == id)
    }

    /// The id the elision marker goes after, when there is one to place.
    pub fn elision_after(&self) -> Option<&str> {
        (self.omitted_tokens > 0)
            .then(|| self.head.last().map(|entry| entry.id.as_str()))
            .flatten()
    }

    pub fn oldest_id(&self) -> Option<&str> {
        self.head
            .first()
            .or_else(|| self.tail.first())
            .map(|entry| entry.id.as_str())
    }
}

/// Whether a message is something the user actually typed.
/// Injected content — todo reminders, hook output, tool feedback — travels as
/// [`AgentMessage::Custom`] and is re-injected by whatever produced it on the
/// next turn, so carrying it through a compaction would duplicate it.
pub fn is_user_input(message: &AgentMessage) -> bool {
    matches!(message, AgentMessage::User(_))
}

/// The plain text of a user message, when all of its content is text.
/// A message carrying an image has no sensible truncation — half an image is
/// not half a message — so it is kept whole or not at all.
fn truncatable_text(message: &AgentMessage) -> Option<String> {
    let AgentMessage::User(message) = message else {
        return None;
    };
    match &message.content {
        UserContent::Text(text) => Some(text.clone()),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .map(|block| match block {
                TextOrImageContent::Text(text) => Some(text.text.as_str()),
                TextOrImageContent::Image(_) => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(|texts| texts.join("")),
    }
}

/// Keeps the first `tokens` worth of text, cutting on a character boundary.
/// Sized in UTF-16 units to agree with [`estimate_tokens`], which is what the
/// budget is measured in.
fn keep_prefix(text: &str, tokens: u64) -> String {
    let budget = tokens.saturating_mul(4);
    let mut used: u64 = 0;
    let mut end = 0;
    for (index, character) in text.char_indices() {
        let width = character.len_utf16() as u64;
        if used + width > budget {
            break;
        }
        used += width;
        end = index + character.len_utf8();
    }
    text[..end].to_string()
}

/// Keeps the last `tokens` worth of text, cutting on a character boundary.
fn keep_suffix(text: &str, tokens: u64) -> String {
    let budget = tokens.saturating_mul(4);
    let mut used: u64 = 0;
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        let width = character.len_utf16() as u64;
        if used + width > budget {
            break;
        }
        used += width;
        start = index;
    }
    text[start..].to_string()
}

/// Marks a message the compaction cut, so the model does not read a fragment as
/// the whole request.
const TRUNCATION_NOTE: &str = "[… earlier part of this message dropped during compaction]";
const TRUNCATION_NOTE_TAIL: &str = "[… rest of this message dropped during compaction]";

/// Rebuilds a retained message, applying whatever truncation was recorded.
pub fn apply_retention(message: &AgentMessage, retained: &RetainedEntry) -> AgentMessage {
    let (tokens, keep_start) = match (retained.prefix_tokens, retained.suffix_tokens) {
        (Some(tokens), _) => (tokens, true),
        (None, Some(tokens)) => (tokens, false),
        (None, None) => return message.clone(),
    };
    let Some(text) = truncatable_text(message) else {
        return message.clone();
    };
    let AgentMessage::User(original) = message else {
        return message.clone();
    };

    let cut = if keep_start {
        format!("{}\n{TRUNCATION_NOTE_TAIL}", keep_prefix(&text, tokens))
    } else {
        format!("{TRUNCATION_NOTE}\n{}", keep_suffix(&text, tokens))
    };

    AgentMessage::User(UserMessage {
        content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(cut))]),
        timestamp: original.timestamp,
    })
}

/// The note that stands where messages were dropped.
/// Without it the model sees the session's opening followed immediately by
/// recent work and reads the two as adjacent, inventing a continuity that never
/// existed.
pub fn elision_message(omitted_tokens: u64, timestamp: i64) -> AgentMessage {
    let text = format!(
        "[Compaction dropped roughly {omitted_tokens} tokens of user messages here. \
         The messages above are the oldest in this session, the messages below are the \
         most recent, and the summary at the end of the conversation covers what was \
         dropped in between.]"
    );
    AgentMessage::User(UserMessage {
        content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(text))]),
        timestamp,
    })
}

/// Picks the messages to carry forward, oldest and newest, within `budget`.
/// Candidates are given oldest first. The tail is filled backwards from the
/// newest, then the head forwards from the oldest with what the head reservation
/// allows; a message that only partly fits is truncated so the budget is met
/// exactly rather than by dropping it whole.
pub fn select_retained(
    candidates: &[(String, AgentMessage)],
    budget: u64,
    head_budget: u64,
) -> RetainedSelection {
    let sizes: Vec<u64> = candidates
        .iter()
        .map(|(_, message)| estimate_tokens(message))
        .collect();
    let total: u64 = sizes.iter().sum();

    if total <= budget {
        return RetainedSelection {
            head: Vec::new(),
            tail: candidates
                .iter()
                .map(|(id, _)| RetainedEntry::whole(id))
                .collect(),
            omitted_tokens: 0,
        };
    }

    // The head reservation can never take more than half, or a small budget
    // would be spent entirely on the opening and the recent requests — the ones
    // the next turn is actually working on — would all be dropped.
    let head_budget = head_budget.min(budget / 2);
    let tail_budget = budget - head_budget;

    let mut tail: Vec<RetainedEntry> = Vec::new();
    let mut remaining = tail_budget;
    // Where the tail starts; everything before it is a head candidate.
    let mut tail_start = candidates.len();
    for index in (0..candidates.len()).rev() {
        if remaining == 0 {
            break;
        }
        let (id, message) = &candidates[index];
        if sizes[index] <= remaining {
            tail.push(RetainedEntry::whole(id));
            remaining -= sizes[index];
            tail_start = index;
            continue;
        }
        // The newest text of a partly-fitting message is the part worth having:
        // a request that ends "but do not touch the database" must not lose its
        // ending.
        if truncatable_text(message).is_some() {
            tail.push(RetainedEntry {
                id: id.clone(),
                prefix_tokens: None,
                suffix_tokens: Some(remaining),
            });
            tail_start = index;
        }
        break;
    }
    tail.reverse();

    let mut head: Vec<RetainedEntry> = Vec::new();
    let mut remaining = head_budget;
    for index in 0..tail_start {
        if remaining == 0 {
            break;
        }
        let (id, message) = &candidates[index];
        if sizes[index] <= remaining {
            head.push(RetainedEntry::whole(id));
            remaining -= sizes[index];
            continue;
        }
        // Here the opening is the part worth having, so the cut falls the other
        // way round.
        if truncatable_text(message).is_some() {
            head.push(RetainedEntry {
                id: id.clone(),
                prefix_tokens: Some(remaining),
                suffix_tokens: None,
            });
        }
        break;
    }

    let kept: u64 = head
        .iter()
        .chain(tail.iter())
        .map(|entry| match (entry.prefix_tokens, entry.suffix_tokens) {
            (Some(tokens), _) | (None, Some(tokens)) => tokens,
            (None, None) => candidates
                .iter()
                .position(|(id, _)| *id == entry.id)
                .map(|index| sizes[index])
                .unwrap_or(0),
        })
        .sum();

    RetainedSelection {
        head,
        tail,
        omitted_tokens: total.saturating_sub(kept),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notagent_ai::types::UserMessage;

    fn user(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: UserContent::Text(text.to_string()),
            timestamp: 0,
        })
    }

    fn candidates(texts: &[(&str, &str)]) -> Vec<(String, AgentMessage)> {
        texts
            .iter()
            .map(|(id, text)| ((*id).to_string(), user(text)))
            .collect()
    }

    #[test]
    fn keeps_everything_when_the_budget_allows() {
        let candidates = candidates(&[("a", "one"), ("b", "two")]);
        let selection = select_retained(&candidates, 1000, 100);
        assert!(selection.head.is_empty());
        assert_eq!(selection.tail.len(), 2);
        assert_eq!(selection.omitted_tokens, 0);
        assert!(selection.elision_after().is_none());
    }

    #[test]
    fn keeps_both_ends_when_it_does_not() {
        // Each message is 100 tokens: 400 characters.
        let text = "x".repeat(400);
        let candidates = candidates(&[
            ("a", &text),
            ("b", &text),
            ("c", &text),
            ("d", &text),
            ("e", &text),
        ]);
        let selection = select_retained(&candidates, 300, 100);
        assert_eq!(
            selection
                .head
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        assert_eq!(
            selection
                .tail
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec!["d", "e"]
        );
        assert_eq!(selection.omitted_tokens, 200);
        assert_eq!(selection.elision_after(), Some("a"));
    }

    #[test]
    fn truncates_a_message_that_only_partly_fits() {
        let text = "x".repeat(400);
        let candidates = candidates(&[("a", &text), ("b", &text)]);
        let selection = select_retained(&candidates, 150, 0);
        assert_eq!(selection.tail.len(), 2);
        // The oldest of the two is the one that had to give way.
        assert_eq!(selection.tail[0].suffix_tokens, Some(50));
        assert_eq!(selection.tail[1].suffix_tokens, None);
    }

    #[test]
    fn a_truncated_tail_keeps_its_ending() {
        let message = user("first half. do not touch the database");
        let retained = RetainedEntry {
            id: "a".to_string(),
            prefix_tokens: None,
            suffix_tokens: Some(7),
        };
        let applied = apply_retention(&message, &retained);
        let AgentMessage::User(applied) = applied else {
            panic!("expected a user message");
        };
        let text = match &applied.content {
            UserContent::Blocks(blocks) => match &blocks[0] {
                TextOrImageContent::Text(text) => text.text.clone(),
                _ => panic!("expected text"),
            },
            UserContent::Text(text) => text.clone(),
        };
        assert!(text.contains("do not touch the database"), "{text}");
        assert!(text.starts_with(TRUNCATION_NOTE), "{text}");
    }

    #[test]
    fn a_truncated_head_keeps_its_opening() {
        let message = user("read the plan first. then everything else");
        let retained = RetainedEntry {
            id: "a".to_string(),
            prefix_tokens: Some(5),
            suffix_tokens: None,
        };
        let applied = apply_retention(&message, &retained);
        let AgentMessage::User(applied) = applied else {
            panic!("expected a user message");
        };
        let text = match &applied.content {
            UserContent::Blocks(blocks) => match &blocks[0] {
                TextOrImageContent::Text(text) => text.text.clone(),
                _ => panic!("expected text"),
            },
            UserContent::Text(text) => text.clone(),
        };
        assert!(text.starts_with("read the plan"), "{text}");
        assert!(text.ends_with(TRUNCATION_NOTE_TAIL), "{text}");
    }

    #[test]
    fn a_message_kept_whole_is_returned_unchanged() {
        let message = user("unchanged");
        let applied = apply_retention(&message, &RetainedEntry::whole("a"));
        assert_eq!(applied, message);
    }

    #[test]
    fn cuts_on_a_character_boundary() {
        // Four-byte characters: the cut must not split one.
        let text = "😀".repeat(10);
        let kept = keep_prefix(&text, 2);
        assert!(text.starts_with(&kept));
        assert_eq!(kept.chars().count(), 4);
        let kept = keep_suffix(&text, 2);
        assert!(text.ends_with(&kept));
        assert_eq!(kept.chars().count(), 4);
    }

    #[test]
    fn only_real_user_input_counts() {
        assert!(is_user_input(&user("typed")));
        assert!(!is_user_input(&AgentMessage::BranchSummary(
            notagent_agent::create_branch_summary_message("summary", "from", 0)
        )));
    }
}
