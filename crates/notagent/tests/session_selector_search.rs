//! Port of `packages/coding-agent/test/session-selector-search.test.ts` (195 LOC).

use notagent::core::session_manager::SessionInfo;
use notagent::modes::interactive::components::session_selector_search::{
    NameFilter, SortMode, filter_and_sort_sessions,
};

/// `new Date("2026-01-0NT00:00:00.000Z").getTime()`.
const JAN_01: i64 = 1_767_225_600_000;
const JAN_02: i64 = 1_767_312_000_000;
const JAN_03: i64 = 1_767_398_400_000;
const JAN_04: i64 = 1_767_484_800_000;

fn make_session(id: &str, modified: i64, all_messages_text: &str) -> SessionInfo {
    SessionInfo {
        path: format!("/tmp/{id}.jsonl"),
        id: id.to_string(),
        cwd: String::new(),
        name: None,
        parent_session_path: None,
        created: 0,
        modified,
        message_count: 1,
        first_message: "(no messages)".to_string(),
        all_messages_text: all_messages_text.to_string(),
    }
}

fn named_session(id: &str, name: &str, modified: i64, all_messages_text: &str) -> SessionInfo {
    SessionInfo {
        name: Some(name.to_string()),
        ..make_session(id, modified, all_messages_text)
    }
}

fn ids(sessions: &[SessionInfo]) -> Vec<&str> {
    sessions.iter().map(|session| session.id.as_str()).collect()
}

#[test]
fn filters_by_quoted_phrase_with_whitespace_normalization() {
    let sessions = vec![
        make_session("a", JAN_01, "node\n\n   cve was discussed"),
        make_session("b", JAN_02, "node something else"),
    ];

    let result =
        filter_and_sort_sessions(&sessions, "\"node cve\"", SortMode::Recent, NameFilter::All);
    assert_eq!(ids(&result), ["a"]);
}

#[test]
fn filters_by_regex_and_is_case_insensitive() {
    let sessions = vec![
        make_session("a", JAN_02, "Brave is great"),
        make_session("b", JAN_03, "bravery is not the same"),
    ];

    let result = filter_and_sort_sessions(
        &sessions,
        r"re:\bbrave\b",
        SortMode::Recent,
        NameFilter::All,
    );
    assert_eq!(ids(&result), ["a"]);
}

#[test]
fn recent_sort_preserves_input_order() {
    let sessions = vec![
        make_session("newer", JAN_03, "brave"),
        make_session("older", JAN_01, "brave"),
        make_session("nomatch", JAN_04, "something else"),
    ];

    let result =
        filter_and_sort_sessions(&sessions, "\"brave\"", SortMode::Recent, NameFilter::All);
    assert_eq!(ids(&result), ["newer", "older"]);
}

#[test]
fn relevance_sort_orders_by_score_and_tie_breaks_by_modified_desc() {
    let sessions = vec![
        make_session("late", JAN_03, "xxxx brave"),
        make_session("early", JAN_01, "brave xxxx"),
    ];

    let result =
        filter_and_sort_sessions(&sessions, "\"brave\"", SortMode::Relevance, NameFilter::All);
    assert_eq!(ids(&result), ["early", "late"]);

    let tie_sessions = vec![
        make_session("newer", JAN_03, "brave"),
        make_session("older", JAN_01, "brave"),
    ];

    let result = filter_and_sort_sessions(
        &tie_sessions,
        "\"brave\"",
        SortMode::Relevance,
        NameFilter::All,
    );
    assert_eq!(ids(&result), ["newer", "older"]);
}

#[test]
fn returns_empty_list_for_invalid_regex() {
    let sessions = vec![make_session("a", JAN_01, "brave")];

    let result = filter_and_sort_sessions(&sessions, "re:(", SortMode::Recent, NameFilter::All);
    assert!(result.is_empty());
}

// --- name filter -------------------------------------------------------------

fn name_filter_sessions() -> Vec<SessionInfo> {
    vec![
        named_session("named1", "My Project", JAN_03, "blueberry"),
        named_session("named2", "Another Named", JAN_02, "blueberry"),
        make_session("other1", JAN_04, "blueberry"),
        make_session("other2", JAN_01, "blueberry"),
    ]
}

#[test]
fn returns_all_sessions_when_name_filter_is_all() {
    let sessions = name_filter_sessions();
    let result = filter_and_sort_sessions(&sessions, "", SortMode::Recent, NameFilter::All);
    assert_eq!(ids(&result), ["named1", "named2", "other1", "other2"]);
}

#[test]
fn returns_only_named_sessions_when_name_filter_is_named() {
    let sessions = name_filter_sessions();
    let result = filter_and_sort_sessions(&sessions, "", SortMode::Recent, NameFilter::Named);
    assert_eq!(ids(&result), ["named1", "named2"]);
}

#[test]
fn applies_name_filter_before_search_query() {
    let sessions = name_filter_sessions();
    let result =
        filter_and_sort_sessions(&sessions, "blueberry", SortMode::Recent, NameFilter::Named);
    assert_eq!(ids(&result), ["named1", "named2"]);
}

#[test]
fn excludes_whitespace_only_names_from_named_filter() {
    let sessions = vec![
        named_session("whitespace", "   ", JAN_01, "test"),
        named_session("empty", "", JAN_02, "test"),
        named_session("named", "Real Name", JAN_03, "test"),
    ];

    let result = filter_and_sort_sessions(&sessions, "", SortMode::Recent, NameFilter::Named);
    assert_eq!(ids(&result), ["named"]);
}
