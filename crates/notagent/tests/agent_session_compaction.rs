mod suite;

use std::sync::Arc;

use notagent::core::agent_session::{
    AgentSessionEvent, CompactionReason, NavigateTreeOptions, PromptOptions,
};
use notagent_agent::types::AgentMessage;
use notagent_ai::providers::faux::{FauxResponseStep, faux_assistant_message, faux_text};
use notagent_ai::types::StopReason;
use serde_json::json;
use suite::{HarnessOptions, create_harness};

fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

/// Compaction only has something to do once there is a history to summarize.
async fn with_history(harness: &suite::Harness, turns: usize) {
    let responses: Vec<FauxResponseStep> = (0..turns)
        .map(|index| reply(&format!("answer {index} {}", "padding ".repeat(40))))
        .collect();
    harness.set_responses(responses);
    for index in 0..turns {
        harness
            .session
            .prompt(
                &format!("question {index} {}", "padding ".repeat(40)),
                PromptOptions::default(),
            )
            .await
            .expect("prompt");
    }
}

#[tokio::test]
async fn refuses_to_compact_a_session_with_nothing_in_it() {
    let harness = create_harness(HarnessOptions::default());

    let error = harness
        .session
        .compact(None)
        .await
        .expect_err("should refuse");
    assert!(error.contains("Nothing to compact") || error.contains("Already compacted"));
}

#[tokio::test]
async fn compacts_manually_and_reports_the_boundary() {
    let harness = create_harness(HarnessOptions {
        settings: Some(json!({ "compaction": { "enabled": true, "retainedUserTokens": 10 } })),
        ..HarnessOptions::default()
    });
    with_history(&harness, 3).await;

    harness.set_responses(vec![reply("## Goal\na summary"), reply("turn prefix")]);
    let result = harness.session.compact(None).await.expect("compaction");

    assert!(result.summary.contains("a summary"));
    assert!(!result.first_kept_entry_id.is_empty());
    assert!(result.tokens_before > 0);
    assert!(result.estimated_tokens_after.is_some());

    // The context ends with the note: it is the newest thing said about the
    // older history, and everything before it is the user's own words.
    let messages = harness.session.messages();
    assert!(matches!(
        messages.last(),
        Some(AgentMessage::CompactionSummary(_))
    ));
    assert!(
        messages
            .iter()
            .take(messages.len() - 1)
            .all(|message| matches!(message, AgentMessage::User(_))),
        "something other than the user's messages survived"
    );

    // Start and end were announced, with the manual reason.
    let reasons: Vec<CompactionReason> = harness
        .events()
        .into_iter()
        .filter_map(|event| match event {
            AgentSessionEvent::CompactionStart { reason } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(reasons, vec![CompactionReason::Manual]);
    let ended = harness
        .events()
        .into_iter()
        .filter(|event| matches!(event, AgentSessionEvent::CompactionEnd { .. }))
        .count();
    assert_eq!(ended, 1);
}

#[tokio::test]
async fn refuses_a_second_compaction_right_after_one() {
    let harness = create_harness(HarnessOptions {
        settings: Some(json!({ "compaction": { "enabled": true, "retainedUserTokens": 10 } })),
        ..HarnessOptions::default()
    });
    with_history(&harness, 3).await;
    harness.set_responses(vec![reply("## Goal\na summary"), reply("turn prefix")]);
    harness.session.compact(None).await.expect("compaction");

    harness.set_responses(vec![
        reply("## Goal\nanother summary"),
        reply("turn prefix"),
    ]);
    let error = harness
        .session
        .compact(None)
        .await
        .expect_err("should refuse");
    assert_eq!(error, "Already compacted");
}

#[tokio::test]
async fn a_compaction_is_the_only_thing_running_while_it_runs() {
    let harness = create_harness(HarnessOptions {
        settings: Some(json!({ "compaction": { "enabled": true, "retainedUserTokens": 10 } })),
        ..HarnessOptions::default()
    });
    with_history(&harness, 3).await;
    assert!(!harness.session.is_compacting());

    harness.set_responses(vec![reply("## Goal\na summary"), reply("turn prefix")]);
    harness.session.compact(None).await.expect("compaction");
    assert!(!harness.session.is_compacting());
}

#[tokio::test]
async fn reports_the_context_usage_against_the_window() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("hello")]);
    harness
        .session
        .prompt("hi", PromptOptions::default())
        .await
        .expect("prompt");

    let usage = harness.session.get_context_usage().expect("usage");
    assert_eq!(usage.context_window, harness.model().context_window);
    assert!(usage.tokens.is_some());
    assert!(usage.percent.is_some_and(|percent| percent >= 0.0));
}

#[tokio::test]
async fn the_context_count_is_unknown_until_the_first_answer_after_a_compaction() {
    let harness = create_harness(HarnessOptions {
        settings: Some(json!({ "compaction": { "enabled": true, "retainedUserTokens": 10 } })),
        ..HarnessOptions::default()
    });
    with_history(&harness, 3).await;
    harness.set_responses(vec![reply("## Goal\na summary"), reply("turn prefix")]);
    harness.session.compact(None).await.expect("compaction");

    let usage = harness.session.get_context_usage().expect("usage");
    assert_eq!(usage.tokens, None);
    assert_eq!(usage.percent, None);
}

#[tokio::test]
async fn counts_the_tokens_and_the_cost_over_the_whole_session() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("one"), reply("two")]);
    harness
        .session
        .prompt("first", PromptOptions::default())
        .await
        .expect("prompt");
    harness
        .session
        .prompt("second", PromptOptions::default())
        .await
        .expect("prompt");

    let stats = harness.session.get_session_stats();
    assert_eq!(stats.user_messages, 2);
    assert_eq!(stats.assistant_messages, 2);
    assert_eq!(stats.total_messages, 4);
    assert!(stats.tokens.total > 0);
    assert_eq!(
        stats.tokens.total,
        stats.tokens.input
            + stats.tokens.output
            + stats.tokens.cache_read
            + stats.tokens.cache_write
    );
}

#[tokio::test]
async fn navigating_the_tree_puts_the_selected_message_back_in_the_editor() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("one"), reply("two")]);
    harness
        .session
        .prompt("first question", PromptOptions::default())
        .await
        .expect("prompt");
    harness
        .session
        .prompt("second question", PromptOptions::default())
        .await
        .expect("prompt");

    let first_user_entry = harness
        .session
        .get_user_messages_for_forking()
        .into_iter()
        .find(|(_, text)| text == "first question")
        .map(|(id, _)| id)
        .expect("first user entry");

    let result = harness
        .session
        .navigate_tree(&first_user_entry, NavigateTreeOptions::default())
        .await
        .expect("navigate");

    assert!(!result.cancelled);
    assert_eq!(result.editor_text.as_deref(), Some("first question"));
    // The branch was left behind: nothing after the fork point is in context.
    let texts: Vec<String> = harness.user_texts();
    assert!(!texts.contains(&"second question".to_string()));
}

#[tokio::test]
async fn navigating_to_where_it_already_is_does_nothing() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("one")]);
    harness
        .session
        .prompt("only question", PromptOptions::default())
        .await
        .expect("prompt");

    let leaf = harness
        .session
        .with_session_manager(|manager| manager.get_leaf_id().map(str::to_string))
        .expect("leaf");
    let before = harness.session.messages().len();

    let result = harness
        .session
        .navigate_tree(&leaf, NavigateTreeOptions::default())
        .await
        .expect("navigate");

    assert!(!result.cancelled);
    assert_eq!(result.editor_text, None);
    assert_eq!(harness.session.messages().len(), before);
}

#[tokio::test]
async fn refuses_to_navigate_to_an_entry_that_does_not_exist() {
    let harness = create_harness(HarnessOptions::default());

    let error = harness
        .session
        .navigate_tree("no-such-entry", NavigateTreeOptions::default())
        .await
        .expect_err("should refuse");
    assert!(error.contains("not found"));
}

#[tokio::test]
async fn exports_the_current_branch_as_a_straight_line_of_jsonl() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("answered")]);
    harness
        .session
        .prompt("asked", PromptOptions::default())
        .await
        .expect("prompt");

    let temp = tempfile::tempdir().expect("temp dir");
    let target = temp.path().join("export.jsonl");
    let written = harness
        .session
        .export_to_jsonl(Some(&target.to_string_lossy()))
        .expect("export");

    let content = std::fs::read_to_string(&written).expect("read export");
    let lines: Vec<&str> = content.lines().collect();
    assert!(lines.len() >= 3);

    let header: serde_json::Value = serde_json::from_str(lines[0]).expect("header");
    assert_eq!(header.get("type").and_then(|v| v.as_str()), Some("session"));
    assert_eq!(
        header.get("id").and_then(|v| v.as_str()),
        Some(harness.session.session_id().as_str())
    );

    // Every entry is chained to the previous line, whatever the tree looked like.
    let mut previous: Option<String> = None;
    for line in &lines[1..] {
        let entry: serde_json::Value = serde_json::from_str(line).expect("entry");
        let parent = entry
            .get("parentId")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        assert_eq!(parent, previous);
        previous = entry.get("id").and_then(|v| v.as_str()).map(str::to_string);
    }
}

#[tokio::test]
async fn a_prompt_is_refused_while_a_manual_compaction_is_running() {
    let harness = create_harness(HarnessOptions {
        settings: Some(json!({ "compaction": { "enabled": true, "retainedUserTokens": 10 } })),
        // A slow provider keeps the guard observable.
        tokens_per_second: Some(20.0),
        ..HarnessOptions::default()
    });
    with_history(&harness, 3).await;

    let session = Arc::clone(&harness.session);
    harness.set_responses(vec![
        reply(&"summary ".repeat(60)),
        reply(&"turn prefix ".repeat(60)),
    ]);
    let compaction = tokio::spawn(async move { session.compact(None).await });

    let mut refused = false;
    for _ in 0..200 {
        if compaction.is_finished() {
            break;
        }
        if let Err(error) = harness
            .session
            .prompt("during", PromptOptions::default())
            .await
            && error.contains("compaction is in progress")
        {
            refused = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    let _ = compaction.await.expect("join");
    assert!(refused, "the prompt guard never fired");
}

/// The threshold decides against the catalog's current window, not the
/// session's snapshot. A dynamic provider's server can shrink its window
/// between selection and use; compacting against the stale figure would wait
/// until the server has long been rejecting the requests.
#[tokio::test]
async fn the_threshold_follows_the_catalogs_current_window() {
    let harness = create_harness(HarnessOptions {
        settings: Some(json!({ "compaction": { "enabled": true, "retainedUserTokens": 10 } })),
        // The catalog now says the server enforces a far smaller window than
        // the 128k the session model was selected with: threshold at 5 000
        // tokens once the 16 384-token reserve is set aside.
        runtime_window_override: Some(16_384 + 5_000),
        ..HarnessOptions::default()
    });
    with_history(&harness, 2).await;

    harness.set_responses(vec![reply("a short answer"), reply("## Goal\na summary")]);
    harness
        .session
        .prompt(
            &format!("consider this: {}", "x".repeat(30_000)),
            PromptOptions::default(),
        )
        .await
        .expect("prompt");

    let reasons: Vec<CompactionReason> = harness
        .events()
        .into_iter()
        .filter_map(|event| match event {
            AgentSessionEvent::CompactionStart { reason } => Some(reason),
            _ => None,
        })
        .collect();
    // The summarization turn itself can cross the small threshold again, so
    // the pinned fact is the trigger, not the count.
    assert!(
        !reasons.is_empty()
            && reasons
                .iter()
                .all(|reason| *reason == CompactionReason::Threshold),
        "{reasons:?}"
    );
}

/// The reported context usage follows the catalog's current window too: the
/// footer reads this figure, and a stale one hides that the server will
/// reject requests far before the shown window fills.
#[tokio::test]
async fn the_context_usage_reports_the_catalogs_current_window() {
    let harness = create_harness(HarnessOptions {
        runtime_window_override: Some(50_000),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![reply("hello")]);
    harness
        .session
        .prompt("hi", PromptOptions::default())
        .await
        .expect("prompt");

    let usage = harness.session.get_context_usage().expect("usage");
    assert_eq!(usage.context_window, 50_000);
    // The session snapshot still says otherwise — the catalog outranks it.
    assert_ne!(harness.model().context_window, 50_000);
}
