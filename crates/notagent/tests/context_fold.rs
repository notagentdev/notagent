use notagent::core::compaction::{
    DEFAULT_COMPACTION_SETTINGS, estimate_context_tokens, estimate_tokens, prepare_compaction,
    usage_correction_after_rebuild,
};
use notagent::core::context_fold::{
    FOLD_ENTRY_TYPE, FOLD_MESSAGE_TYPE, FOLD_REMINDER_TYPE, UNFOLD_ENTRY_TYPE, UnfoldRecord,
    active_folds, foldable_steps, plan_fold, search_folds,
};
use notagent::core::session_manager::{SessionEntry, SessionManager};
use notagent_agent::types::AgentMessage;
use serde_json::{Value, json};

fn usage() -> Value {
    json!({
        "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
    })
}

fn user(text: &str) -> AgentMessage {
    serde_json::from_value(json!({ "role": "user", "content": text, "timestamp": 1 }))
        .expect("user message")
}

fn tool_call(call_id: &str, text: &str) -> AgentMessage {
    serde_json::from_value(json!({
        "role": "assistant",
        "content": [
            { "type": "text", "text": text },
            { "type": "toolCall", "id": call_id, "name": "grep", "arguments": { "pattern": "x" } },
        ],
        "api": "anthropic-messages",
        "provider": "anthropic",
        "model": "test",
        "usage": usage(),
        "stopReason": "toolUse",
        "timestamp": 2,
    }))
    .expect("assistant message")
}

fn tool_result(call_id: &str, text: &str) -> AgentMessage {
    serde_json::from_value(json!({
        "role": "toolResult",
        "toolCallId": call_id,
        "toolName": "grep",
        "content": [{ "type": "text", "text": text }],
        "isError": false,
        "timestamp": 3,
    }))
    .expect("tool result")
}

/// A session: one request and `steps` grep steps, each an assistant call and
/// its result. Returns the ids of the assistant entries.
fn session_with_steps(manager: &mut SessionManager, steps: usize) -> Vec<String> {
    manager.append_message(&user("find the bug")).expect("user");
    (0..steps)
        .map(|index| {
            let call = format!("call-{index}");
            let id = manager
                .append_message(&tool_call(&call, &format!("searching part {index}")))
                .expect("assistant");
            manager
                .append_message(&tool_result(
                    &call,
                    &format!("result {index} needle-{index}"),
                ))
                .expect("result");
            id
        })
        .collect()
}

fn in_memory() -> SessionManager {
    SessionManager::in_memory(Some("/tmp/notagent-context-fold-tests"), None).expect("session")
}

fn fold(
    manager: &mut SessionManager,
    from: &str,
    to: &str,
    summary: &str,
) -> Result<String, String> {
    let entries = manager.build_context_entries();
    let refs: Vec<&SessionEntry> = entries.iter().collect();
    let planned = plan_fold(&refs, from, to, summary)?;
    manager
        .append_custom_entry(
            FOLD_ENTRY_TYPE,
            Some(serde_json::to_value(planned.record).expect("record")),
        )
        .map_err(|error| error.to_string())
}

fn texts(manager: &SessionManager) -> Vec<String> {
    manager
        .build_session_context()
        .messages
        .iter()
        .map(|message| serde_json::to_string(message).expect("json"))
        .collect()
}

fn fold_messages(manager: &SessionManager) -> usize {
    manager
        .build_session_context()
        .messages
        .iter()
        .filter(|message| matches!(message, AgentMessage::Custom(custom) if custom.custom_type == FOLD_MESSAGE_TYPE))
        .count()
}

#[test]
fn a_fold_replaces_its_steps_with_the_summary_and_keeps_tool_pairs_whole() {
    let mut manager = in_memory();
    let steps = session_with_steps(&mut manager, 4);
    let before = manager.build_session_context().messages.len();

    fold(
        &mut manager,
        &steps[0],
        &steps[1],
        "parts 0 and 1 hold nothing",
    )
    .expect("fold");

    let context = texts(&manager);
    assert_eq!(fold_messages(&manager), 1);
    assert!(
        !context
            .iter()
            .any(|text| text.contains("needle-0") || text.contains("needle-1")),
        "the folded results are out of the context"
    );
    assert!(
        context
            .iter()
            .any(|text| text.contains("parts 0 and 1 hold nothing"))
    );
    assert!(
        context.iter().any(|text| text.contains("needle-2")),
        "later steps stay"
    );
    // user + fold + two unfolded steps (call and result each).
    assert_eq!(
        manager.build_session_context().messages.len(),
        before - 4 + 1
    );
    // Every remaining tool result still follows its own call.
    let messages = manager.build_session_context().messages;
    for (index, message) in messages.iter().enumerate() {
        if let AgentMessage::ToolResult(result) = message {
            let AgentMessage::Assistant(call) = &messages[index - 1] else {
                panic!("a tool result lost its call: {result:?}");
            };
            assert!(
                serde_json::to_string(call)
                    .expect("json")
                    .contains(&result.tool_call_id)
            );
        }
    }
}

#[test]
fn the_newest_step_cannot_be_folded() {
    let mut manager = in_memory();
    let steps = session_with_steps(&mut manager, 3);
    let error = fold(&mut manager, &steps[0], &steps[2], "everything").expect_err("in flight");
    assert!(error.contains("newest step"), "{error}");
    let entries = manager.build_context_entries();
    let refs: Vec<&SessionEntry> = entries.iter().collect();
    assert_eq!(
        foldable_steps(&refs).len(),
        2,
        "the step making the call is never offered"
    );
}

#[test]
fn the_users_messages_inside_a_fold_are_kept_verbatim() {
    let mut manager = in_memory();
    let first = session_with_steps(&mut manager, 2);
    manager
        .append_message(&user("do not touch the database"))
        .expect("user");
    let call = "call-late";
    let later = manager
        .append_message(&tool_call(call, "more work"))
        .expect("call");
    manager
        .append_message(&tool_result(call, "late result"))
        .expect("result");
    manager
        .append_message(&tool_call("call-now", "now"))
        .expect("now");

    fold(&mut manager, &first[0], &later, "explored the codebase").expect("fold");

    let context = texts(&manager);
    let kept = context
        .iter()
        .position(|text| text.contains("do not touch the database"))
        .expect("the request survives");
    let summary = context
        .iter()
        .position(|text| text.contains("explored the codebase"))
        .expect("summary");
    assert!(
        kept < summary,
        "requests come ahead of the summary that replaced their surroundings"
    );
}

#[test]
fn unfolding_brings_the_original_steps_back() {
    let mut manager = in_memory();
    let steps = session_with_steps(&mut manager, 4);
    let original = texts(&manager);
    let fold_id = fold(&mut manager, &steps[0], &steps[2], "summary").expect("fold");
    manager
        .append_custom_entry(
            UNFOLD_ENTRY_TYPE,
            Some(serde_json::to_value(UnfoldRecord { fold_id }).expect("record")),
        )
        .expect("unfold");
    assert_eq!(
        texts(&manager),
        original,
        "an unfolded range reads as if it was never folded"
    );
}

#[test]
fn a_fold_may_extend_the_one_right_before_it() {
    let mut manager = in_memory();
    let steps = session_with_steps(&mut manager, 6);
    let inner = fold(&mut manager, &steps[0], &steps[1], "inner").expect("inner");

    let error =
        fold(&mut manager, &steps[1], &steps[3], "names a folded step").expect_err("folded");
    assert!(error.contains(&inner), "{error}");

    fold(&mut manager, &inner, &steps[3], "outer").expect("the next fold may extend the last");

    let entries = manager.build_context_entries();
    let refs: Vec<&SessionEntry> = entries.iter().collect();
    let folds = active_folds(&refs);
    assert_eq!(folds.len(), 1, "the outer fold replaces the inner one");
    assert_eq!(folds[0].record.summary, "outer");
    assert_eq!(fold_messages(&manager), 1);
}

#[test]
fn folds_go_in_order_from_the_oldest_unfolded_step() {
    let mut manager = in_memory();
    let steps = session_with_steps(&mut manager, 5);

    let error = fold(&mut manager, &steps[1], &steps[2], "skips one").expect_err("out of order");
    assert!(
        error.contains(&steps[0]) && error.contains("in order"),
        "a fold that leaves older work unfolded rewrites the cache behind it: {error}"
    );

    let first = fold(&mut manager, &steps[0], &steps[0], "first").expect("in order");
    let error = fold(&mut manager, &steps[2], &steps[3], "gap").expect_err("gap");
    assert!(
        error.contains(&steps[1]) && error.contains(&first),
        "the refusal names where to start instead: {error}"
    );
    fold(&mut manager, &steps[1], &steps[2], "second").expect("continues after the last fold");
}

#[test]
fn a_reminder_leaves_the_context_once_a_fold_answers_it() {
    let mut manager = in_memory();
    let steps = session_with_steps(&mut manager, 3);
    manager
        .append_custom_message_entry(
            FOLD_REMINDER_TYPE,
            json!("[Context reminder: fold the finished work]"),
            false,
            None,
        )
        .expect("reminder");
    manager
        .append_message(&tool_call("call-now", "folding"))
        .expect("the step making the fold call");
    assert!(
        texts(&manager)
            .iter()
            .any(|text| text.contains("Context reminder"))
    );

    fold(&mut manager, &steps[0], &steps[2], "all of it").expect("fold");
    assert!(
        !texts(&manager)
            .iter()
            .any(|text| text.contains("Context reminder")),
        "an answered reminder would otherwise pile up, one per fold"
    );

    manager.set_apply_context_folds(false);
    assert!(
        texts(&manager)
            .iter()
            .any(|text| text.contains("Context reminder")),
        "switched off, the history is shown in full"
    );
}

#[test]
fn a_compaction_supersedes_earlier_folds_and_folding_starts_after_it() {
    let mut manager = in_memory();
    let steps = session_with_steps(&mut manager, 4);
    fold(&mut manager, &steps[0], &steps[1], "before compaction").expect("fold");
    let last = manager.get_leaf_id().map(str::to_owned).expect("leaf");
    manager
        .append_compaction("compacted", &last, None, 1000, None, None, None, None)
        .expect("compaction");

    assert_eq!(
        fold_messages(&manager),
        0,
        "the compaction replaced the folded range, so the fold has nothing to apply to"
    );
    let entries = manager.build_context_entries();
    let refs: Vec<&SessionEntry> = entries.iter().collect();
    assert!(
        foldable_steps(&refs).is_empty(),
        "nothing before the compaction is offered for folding"
    );

    let after = session_with_steps(&mut manager, 3);
    fold(&mut manager, &after[0], &after[1], "after compaction").expect("fold after");
    assert_eq!(fold_messages(&manager), 1);
}

#[test]
fn search_finds_text_inside_folds_only() {
    let mut manager = in_memory();
    let steps = session_with_steps(&mut manager, 4);
    let fold_id = fold(&mut manager, &steps[0], &steps[1], "summary").expect("fold");
    let entries = manager.build_context_entries();
    let refs: Vec<&SessionEntry> = entries.iter().collect();

    let hits = search_folds(&refs, "NEEDLE-1", 20);
    assert_eq!(hits.len(), 1, "case-insensitive, inside the fold");
    assert_eq!(hits[0].fold_id, fold_id);
    assert!(
        search_folds(&refs, "needle-3", 20).is_empty(),
        "unfolded steps are already in the context and are not searched"
    );
}

#[test]
fn switched_off_the_full_history_shows_again_like_in_a_build_without_folding() {
    let mut manager = in_memory();
    let steps = session_with_steps(&mut manager, 4);
    let original = texts(&manager);
    fold(&mut manager, &steps[0], &steps[2], "summary").expect("fold");

    manager.set_apply_context_folds(false);
    assert_eq!(
        texts(&manager),
        original,
        "fold entries are plain custom entries, which every context ignores without folding"
    );
}

#[test]
fn folds_survive_a_reload_from_the_session_file() {
    let directory = tempfile::tempdir().expect("directory");
    let cwd = directory.path().to_string_lossy().into_owned();
    let mut manager = SessionManager::create(&cwd, Some(&cwd), None).expect("manager");
    let steps = session_with_steps(&mut manager, 4);
    fold(&mut manager, &steps[0], &steps[1], "persisted summary").expect("fold");
    let file = manager.get_session_file().expect("file").to_owned();
    drop(manager);

    let reopened = SessionManager::open(&file, Some(&cwd), None).expect("reopen");
    assert!(
        texts(&reopened)
            .iter()
            .any(|text| text.contains("persisted summary"))
    );
    assert_eq!(fold_messages(&reopened), 1);
}

fn tool_call_without_usage(call_id: &str, text: &str) -> AgentMessage {
    let mut value = serde_json::to_value(tool_call(call_id, text)).expect("json");
    value["usage"]["input"] = json!(0);
    value["usage"]["output"] = json!(0);
    value["usage"]["totalTokens"] = json!(0);
    serde_json::from_value(value).expect("assistant without usage")
}

fn sum(messages: &[AgentMessage]) -> i64 {
    messages.iter().map(estimate_tokens).sum::<u64>() as i64
}

#[test]
fn a_stale_provider_count_is_corrected_by_what_the_fold_changed() {
    let mut manager = in_memory();
    let steps = session_with_steps(&mut manager, 4);
    manager
        .append_message(&tool_call("call-now", "folding"))
        .expect("the step making the fold call, with the provider's count");
    let before = manager.build_session_context().messages;
    fold(&mut manager, &steps[0], &steps[2], "summary").expect("fold");
    let after = manager.build_session_context().messages;

    let correction = usage_correction_after_rebuild(&before, &after);
    assert_ne!(correction, 0, "a fold ahead of the count makes it stale");
    assert_eq!(
        correction,
        sum(&after) - sum(&before),
        "the count covers everything up to the newest answer, all of which the fold changed"
    );
}

#[test]
fn without_a_provider_count_the_estimate_is_not_corrected_twice() {
    let mut manager = in_memory();
    manager.append_message(&user("find the bug")).expect("user");
    let steps: Vec<String> = (0..4)
        .map(|index| {
            let call = format!("call-{index}");
            let id = manager
                .append_message(&tool_call_without_usage(&call, "searching"))
                .expect("assistant");
            manager
                .append_message(&tool_result(&call, &format!("result needle-{index}")))
                .expect("result");
            id
        })
        .collect();
    manager
        .append_message(&tool_call_without_usage("call-now", "folding"))
        .expect("newest");
    let before = manager.build_session_context().messages;
    fold(&mut manager, &steps[0], &steps[2], "summary").expect("fold");
    let after = manager.build_session_context().messages;

    let correction = usage_correction_after_rebuild(&before, &after);
    assert_eq!(
        correction, 0,
        "the estimate already measures the folded context; a correction would subtract the fold again"
    );
    assert_eq!(estimate_context_tokens(&after).tokens as i64, sum(&after));
}

#[test]
fn compaction_summarizes_the_context_the_agent_sees() {
    let mut manager = in_memory();
    let steps = session_with_steps(&mut manager, 4);
    fold(&mut manager, &steps[0], &steps[1], "folded summary text").expect("fold");
    let entries: Vec<SessionEntry> = manager.get_branch(None).into_iter().cloned().collect();
    let summarized = |apply: bool| {
        serde_json::to_string(
            &prepare_compaction(&entries, &DEFAULT_COMPACTION_SETTINGS, apply)
                .expect("prepared")
                .messages_to_summarize,
        )
        .expect("json")
    };

    let folded = summarized(true);
    assert!(folded.contains("folded summary text") && !folded.contains("needle-0"));
    let full = summarized(false);
    assert!(
        full.contains("needle-0") && !full.contains("folded summary text"),
        "with folding off the agent reads the original steps, so the summary must come from them"
    );
}
