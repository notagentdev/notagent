use notagent::core::session_manager::{
    FileEntry, SessionEntry, SessionManager, SessionManagerError, migrate_session_entries,
};
use notagent_agent::types::AgentMessage;
use serde_json::{Value, json};

fn user_msg(text: &str) -> AgentMessage {
    serde_json::from_value(json!({ "role": "user", "content": text, "timestamp": 1 }))
        .expect("user message")
}

fn assistant_msg(text: &str) -> AgentMessage {
    serde_json::from_value(json!({
        "role": "assistant",
        "content": [{ "type": "text", "text": text }],
        "api": "anthropic-messages",
        "provider": "anthropic",
        "model": "test",
        "usage": {
            "input": 1,
            "output": 1,
            "cacheRead": 0,
            "cacheWrite": 0,
            "totalTokens": 2,
            "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
        },
        "stopReason": "stop",
        "timestamp": 2,
    }))
    .expect("assistant message")
}

fn usage() -> Value {
    json!({
        "input": 10,
        "output": 20,
        "cacheRead": 30,
        "cacheWrite": 40,
        "totalTokens": 100,
        "cost": { "input": 0.1, "output": 0.2, "cacheRead": 0.3, "cacheWrite": 0.4, "total": 1.0 },
    })
}

fn in_memory() -> SessionManager {
    SessionManager::in_memory(Some("/tmp/notagent-session-tests"), None).expect("session")
}

fn ids(entries: &[SessionEntry]) -> Vec<String> {
    entries.iter().map(|entry| entry.id().to_owned()).collect()
}

// ---------------------------------------------------------------------------
// append operations
// ---------------------------------------------------------------------------

#[test]
fn append_message_creates_entry_with_correct_parent_id_chain() {
    let mut session = in_memory();
    let id1 = session.append_message(&user_msg("first")).expect("append");
    let id2 = session
        .append_message(&assistant_msg("second"))
        .expect("append");
    let id3 = session.append_message(&user_msg("third")).expect("append");

    let entries = session.get_entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].id(), id1);
    assert_eq!(entries[0].parent_id(), None);
    assert_eq!(entries[0].entry_type(), "message");
    assert_eq!(entries[1].id(), id2);
    assert_eq!(entries[1].parent_id(), Some(id1.as_str()));
    assert_eq!(entries[2].id(), id3);
    assert_eq!(entries[2].parent_id(), Some(id2.as_str()));
}

#[test]
fn append_thinking_level_change_integrates_into_the_tree() {
    let mut session = in_memory();
    let msg_id = session.append_message(&user_msg("hello")).expect("append");
    let thinking_id = session
        .append_thinking_level_change("high")
        .expect("append");
    session
        .append_message(&assistant_msg("response"))
        .expect("append");

    let entries = session.get_entries();
    assert_eq!(entries.len(), 3);
    let thinking = entries
        .iter()
        .find(|entry| entry.entry_type() == "thinking_level_change")
        .expect("thinking entry");
    assert_eq!(thinking.id(), thinking_id);
    assert_eq!(thinking.parent_id(), Some(msg_id.as_str()));
    assert_eq!(entries[2].parent_id(), Some(thinking_id.as_str()));
}

#[test]
fn append_model_change_integrates_into_the_tree() {
    let mut session = in_memory();
    let msg_id = session.append_message(&user_msg("hello")).expect("append");
    let model_id = session
        .append_model_change("openai", "gpt-4")
        .expect("append");
    session
        .append_message(&assistant_msg("response"))
        .expect("append");

    let entries = session.get_entries();
    let model = entries
        .iter()
        .find_map(|entry| match entry {
            SessionEntry::ModelChange(entry) => Some(entry),
            _ => None,
        })
        .expect("model entry");
    assert_eq!(model.id, model_id);
    assert_eq!(model.parent_id.as_deref(), Some(msg_id.as_str()));
    assert_eq!(model.provider, "openai");
    assert_eq!(model.model_id, "gpt-4");
    assert_eq!(entries[2].parent_id(), Some(model_id.as_str()));
}

#[test]
fn append_compaction_integrates_into_the_tree() {
    let mut session = in_memory();
    let id1 = session.append_message(&user_msg("1")).expect("append");
    let id2 = session.append_message(&assistant_msg("2")).expect("append");
    let compaction_id = session
        .append_compaction(
            "summary",
            &id1,
            None,
            1000,
            None,
            None,
            Some(false),
            Some(serde_json::from_value(usage()).expect("usage")),
        )
        .expect("append");
    session.append_message(&user_msg("3")).expect("append");

    let entries = session.get_entries();
    let compaction = entries
        .iter()
        .find_map(SessionEntry::as_compaction)
        .expect("compaction entry");
    assert_eq!(compaction.id, compaction_id);
    assert_eq!(compaction.parent_id.as_deref(), Some(id2.as_str()));
    assert_eq!(compaction.summary, "summary");
    assert_eq!(compaction.first_kept_entry_id, id1);
    assert_eq!(compaction.tokens_before, 1000);
    assert_eq!(
        compaction.usage,
        Some(serde_json::from_value(usage()).expect("usage"))
    );
    assert_eq!(entries[3].parent_id(), Some(compaction_id.as_str()));
}

#[test]
fn append_custom_entry_integrates_into_the_tree() {
    let mut session = in_memory();
    let msg_id = session.append_message(&user_msg("hello")).expect("append");
    let custom_id = session
        .append_custom_entry("my_data", Some(json!({ "key": "value" })))
        .expect("append");
    session
        .append_message(&assistant_msg("response"))
        .expect("append");

    let entries = session.get_entries();
    let custom = entries
        .iter()
        .find_map(|entry| match entry {
            SessionEntry::Custom(entry) => Some(entry),
            _ => None,
        })
        .expect("custom entry");
    assert_eq!(custom.id, custom_id);
    assert_eq!(custom.parent_id.as_deref(), Some(msg_id.as_str()));
    assert_eq!(custom.custom_type, "my_data");
    assert_eq!(custom.data, Some(json!({ "key": "value" })));
    assert_eq!(entries[2].parent_id(), Some(custom_id.as_str()));
}

#[test]
fn the_leaf_pointer_advances_after_each_append() {
    let mut session = in_memory();
    assert_eq!(session.get_leaf_id(), None);
    let id1 = session.append_message(&user_msg("1")).expect("append");
    assert_eq!(session.get_leaf_id(), Some(id1.as_str()));
    let id2 = session.append_message(&assistant_msg("2")).expect("append");
    assert_eq!(session.get_leaf_id(), Some(id2.as_str()));
    let id3 = session
        .append_thinking_level_change("high")
        .expect("append");
    assert_eq!(session.get_leaf_id(), Some(id3.as_str()));
}

// ---------------------------------------------------------------------------
// getBranch
// ---------------------------------------------------------------------------

#[test]
fn get_branch_walks_from_the_leaf_to_the_root() {
    let session = in_memory();
    assert!(session.get_branch(None).is_empty());

    let mut session = in_memory();
    let id1 = session.append_message(&user_msg("1")).expect("append");
    assert_eq!(session.get_branch(None).len(), 1);
    assert_eq!(session.get_branch(None)[0].id(), id1);

    let id2 = session.append_message(&assistant_msg("2")).expect("append");
    let id3 = session
        .append_thinking_level_change("high")
        .expect("append");
    let id4 = session.append_message(&user_msg("3")).expect("append");
    let path: Vec<String> = session
        .get_branch(None)
        .iter()
        .map(|entry| entry.id().to_owned())
        .collect();
    assert_eq!(path, vec![id1.clone(), id2.clone(), id3, id4]);

    let path: Vec<String> = session
        .get_branch(Some(&id2))
        .iter()
        .map(|entry| entry.id().to_owned())
        .collect();
    assert_eq!(path, vec![id1, id2]);
}

// ---------------------------------------------------------------------------
// getTree
// ---------------------------------------------------------------------------

#[test]
fn get_tree_returns_an_empty_list_for_an_empty_session() {
    assert!(in_memory().get_tree().is_empty());
}

#[test]
fn get_tree_returns_a_single_root_for_a_linear_session() {
    let mut session = in_memory();
    let id1 = session.append_message(&user_msg("1")).expect("append");
    let id2 = session.append_message(&assistant_msg("2")).expect("append");
    let id3 = session.append_message(&user_msg("3")).expect("append");

    let tree = session.get_tree();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].entry.id(), id1);
    assert_eq!(tree[0].children.len(), 1);
    assert_eq!(tree[0].children[0].entry.id(), id2);
    assert_eq!(tree[0].children[0].children.len(), 1);
    assert_eq!(tree[0].children[0].children[0].entry.id(), id3);
    assert!(tree[0].children[0].children[0].children.is_empty());
}

#[test]
fn get_tree_returns_siblings_after_a_branch() {
    let mut session = in_memory();
    let id1 = session.append_message(&user_msg("1")).expect("append");
    let id2 = session.append_message(&assistant_msg("2")).expect("append");
    let id3 = session.append_message(&user_msg("3")).expect("append");
    session.branch(&id2).expect("branch");
    let id4 = session
        .append_message(&user_msg("4-branch"))
        .expect("append");

    let tree = session.get_tree();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].entry.id(), id1);
    assert_eq!(tree[0].children.len(), 1);
    let node2 = &tree[0].children[0];
    assert_eq!(node2.entry.id(), id2);
    assert_eq!(node2.children.len(), 2);
    let mut child_ids: Vec<&str> = node2
        .children
        .iter()
        .map(|child| child.entry.id())
        .collect();
    child_ids.sort_unstable();
    let mut expected = vec![id3.as_str(), id4.as_str()];
    expected.sort_unstable();
    assert_eq!(child_ids, expected);
}

#[test]
fn get_tree_handles_multiple_branches_at_the_same_point() {
    let mut session = in_memory();
    session.append_message(&user_msg("root")).expect("append");
    let id2 = session
        .append_message(&assistant_msg("response"))
        .expect("append");
    let mut branch_ids: Vec<String> = Vec::new();
    for name in ["branch-A", "branch-B", "branch-C"] {
        session.branch(&id2).expect("branch");
        branch_ids.push(session.append_message(&user_msg(name)).expect("append"));
    }

    let tree = session.get_tree();
    let node2 = &tree[0].children[0];
    assert_eq!(node2.entry.id(), id2);
    assert_eq!(node2.children.len(), 3);
    let mut child_ids: Vec<String> = node2
        .children
        .iter()
        .map(|child| child.entry.id().to_owned())
        .collect();
    child_ids.sort();
    branch_ids.sort();
    assert_eq!(child_ids, branch_ids);
}

#[test]
fn get_tree_handles_deep_branching() {
    let mut session = in_memory();
    session.append_message(&user_msg("1")).expect("append");
    let id2 = session.append_message(&assistant_msg("2")).expect("append");
    let id3 = session.append_message(&user_msg("3")).expect("append");
    session.append_message(&assistant_msg("4")).expect("append");
    session.branch(&id2).expect("branch");
    let id5 = session.append_message(&user_msg("5")).expect("append");
    session.append_message(&assistant_msg("6")).expect("append");
    session.branch(&id5).expect("branch");
    session.append_message(&user_msg("7")).expect("append");

    let tree = session.get_tree();
    let node2 = &tree[0].children[0];
    assert_eq!(node2.children.len(), 2);
    let node5 = node2
        .children
        .iter()
        .find(|child| child.entry.id() == id5)
        .expect("node 5");
    assert_eq!(node5.children.len(), 2);
    let node3 = node2
        .children
        .iter()
        .find(|child| child.entry.id() == id3)
        .expect("node 3");
    assert_eq!(node3.children.len(), 1);
}

// ---------------------------------------------------------------------------
// branch
// ---------------------------------------------------------------------------

#[test]
fn branch_moves_the_leaf_pointer() {
    let mut session = in_memory();
    let id1 = session.append_message(&user_msg("1")).expect("append");
    session.append_message(&assistant_msg("2")).expect("append");
    let id3 = session.append_message(&user_msg("3")).expect("append");
    assert_eq!(session.get_leaf_id(), Some(id3.as_str()));
    session.branch(&id1).expect("branch");
    assert_eq!(session.get_leaf_id(), Some(id1.as_str()));
}

#[test]
fn branch_fails_for_a_non_existent_entry() {
    let mut session = in_memory();
    session.append_message(&user_msg("hello")).expect("append");
    assert_eq!(
        session.branch("nonexistent").expect_err("missing"),
        SessionManagerError::EntryNotFound("nonexistent".to_owned())
    );
}

#[test]
fn new_appends_become_children_of_the_branch_point() {
    let mut session = in_memory();
    let id1 = session.append_message(&user_msg("1")).expect("append");
    session.append_message(&assistant_msg("2")).expect("append");
    session.branch(&id1).expect("branch");
    let id3 = session
        .append_message(&user_msg("branched"))
        .expect("append");

    let entries = session.get_entries();
    let branched = entries
        .iter()
        .find(|entry| entry.id() == id3)
        .expect("branched entry");
    assert_eq!(branched.parent_id(), Some(id1.as_str()));
}

#[test]
fn branch_with_summary_inserts_a_summary_and_advances_the_leaf() {
    let mut session = in_memory();
    let id1 = session.append_message(&user_msg("1")).expect("append");
    session.append_message(&assistant_msg("2")).expect("append");
    session.append_message(&user_msg("3")).expect("append");

    let summary_id = session
        .branch_with_summary(
            Some(&id1),
            "Summary of abandoned work",
            None,
            Some(false),
            Some(serde_json::from_value(usage()).expect("usage")),
        )
        .expect("branch");
    assert_eq!(session.get_leaf_id(), Some(summary_id.as_str()));

    let entries = session.get_entries();
    let summary = entries
        .iter()
        .find_map(|entry| match entry {
            SessionEntry::BranchSummary(entry) => Some(entry),
            _ => None,
        })
        .expect("summary entry");
    assert_eq!(summary.parent_id.as_deref(), Some(id1.as_str()));
    assert_eq!(summary.summary, "Summary of abandoned work");
    assert_eq!(
        summary.usage,
        Some(serde_json::from_value(usage()).expect("usage"))
    );
}

#[test]
fn branch_with_summary_fails_for_a_non_existent_entry() {
    let mut session = in_memory();
    session.append_message(&user_msg("hello")).expect("append");
    assert_eq!(
        session
            .branch_with_summary(Some("nonexistent"), "summary", None, None, None)
            .expect_err("missing"),
        SessionManagerError::EntryNotFound("nonexistent".to_owned())
    );
}

// ---------------------------------------------------------------------------
// leaf and entry lookup
// ---------------------------------------------------------------------------

#[test]
fn leaf_and_entry_lookup() {
    let session = in_memory();
    assert!(session.get_leaf_entry().is_none());
    assert!(session.get_entry("nonexistent").is_none());

    let mut session = in_memory();
    let id1 = session.append_message(&user_msg("first")).expect("append");
    let id2 = session
        .append_message(&assistant_msg("second"))
        .expect("append");
    assert_eq!(session.get_leaf_entry().expect("leaf").id(), id2);

    let entry1 = session
        .get_entry(&id1)
        .expect("entry")
        .as_message()
        .expect("message");
    assert_eq!(entry1.message["content"], json!("first"));
    let entry2 = session
        .get_entry(&id2)
        .expect("entry")
        .as_message()
        .expect("message");
    assert_eq!(entry2.message["content"][0]["text"], json!("second"));
}

#[test]
fn build_session_context_returns_messages_from_the_current_branch_only() {
    let mut session = in_memory();
    session.append_message(&user_msg("msg1")).expect("append");
    let id2 = session
        .append_message(&assistant_msg("msg2"))
        .expect("append");
    session.append_message(&user_msg("msg3")).expect("append");
    session.branch(&id2).expect("branch");
    session
        .append_message(&assistant_msg("msg4-branch"))
        .expect("append");

    let context = session.build_session_context();
    assert_eq!(context.messages.len(), 3);
    let messages: Vec<Value> = context
        .messages
        .iter()
        .map(|message| serde_json::to_value(message).expect("message"))
        .collect();
    assert_eq!(messages[0]["content"], json!("msg1"));
    assert_eq!(messages[1]["content"][0]["text"], json!("msg2"));
    assert_eq!(messages[2]["content"][0]["text"], json!("msg4-branch"));
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

#[test]
fn saves_custom_entries_and_includes_them_in_tree_traversal() {
    let mut session = in_memory();
    let msg_id = session.append_message(&user_msg("hello")).expect("append");
    let custom_id = session
        .append_custom_entry("my_data", Some(json!({ "foo": "bar" })))
        .expect("append");
    let msg2_id = session
        .append_message(&assistant_msg("hi"))
        .expect("append");

    let entries = session.get_entries();
    assert_eq!(entries.len(), 3);
    let custom = entries
        .iter()
        .find_map(|entry| match entry {
            SessionEntry::Custom(entry) => Some(entry),
            _ => None,
        })
        .expect("custom entry");
    assert_eq!(custom.custom_type, "my_data");
    assert_eq!(custom.data, Some(json!({ "foo": "bar" })));
    assert_eq!(custom.id, custom_id);
    assert_eq!(custom.parent_id.as_deref(), Some(msg_id.as_str()));

    let path = session.get_branch(None);
    assert_eq!(
        ids(&path.into_iter().cloned().collect::<Vec<_>>()),
        vec![msg_id, custom_id, msg2_id]
    );
    // Custom entries do not participate in the LLM context.
    assert_eq!(session.build_session_context().messages.len(), 2);
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

fn file_entries(values: Vec<Value>) -> Vec<FileEntry> {
    values.into_iter().map(FileEntry::from_value).collect()
}

#[test]
fn migration_adds_id_and_parent_id_to_v1_entries() {
    let mut entries = file_entries(vec![
        json!({ "type": "session", "id": "sess-1", "timestamp": "2025-01-01T00:00:00Z", "cwd": "/tmp" }),
        json!({
            "type": "message",
            "timestamp": "2025-01-01T00:00:01Z",
            "message": { "role": "user", "content": "hi", "timestamp": 1 },
        }),
        json!({
            "type": "message",
            "timestamp": "2025-01-01T00:00:02Z",
            "message": {
                "role": "assistant",
                "content": [{ "type": "text", "text": "hello" }],
                "api": "test",
                "provider": "test",
                "model": "test",
                "usage": { "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0 },
                "stopReason": "stop",
                "timestamp": 2,
            },
        }),
    ]);

    migrate_session_entries(&mut entries);

    // v3 is current after the hookMessage → custom migration.
    assert_eq!(entries[0].as_header().expect("header").version, Some(3));
    let first = entries[1].as_entry().expect("entry");
    let second = entries[2].as_entry().expect("entry");
    assert_eq!(first.id().len(), 8);
    assert_eq!(first.parent_id(), None);
    assert_eq!(second.id().len(), 8);
    assert_eq!(second.parent_id(), Some(first.id()));
}

#[test]
fn migration_is_idempotent() {
    let mut entries = file_entries(vec![
        json!({ "type": "session", "id": "sess-1", "version": 2, "timestamp": "2025-01-01T00:00:00Z", "cwd": "/tmp" }),
        json!({
            "type": "message",
            "id": "abc12345",
            "parentId": null,
            "timestamp": "2025-01-01T00:00:01Z",
            "message": { "role": "user", "content": "hi", "timestamp": 1 },
        }),
        json!({
            "type": "message",
            "id": "def67890",
            "parentId": "abc12345",
            "timestamp": "2025-01-01T00:00:02Z",
            "message": { "role": "user", "content": "there", "timestamp": 2 },
        }),
    ]);

    migrate_session_entries(&mut entries);

    assert_eq!(entries[1].as_entry().expect("entry").id(), "abc12345");
    assert_eq!(entries[2].as_entry().expect("entry").id(), "def67890");
    assert_eq!(
        entries[2].as_entry().expect("entry").parent_id(),
        Some("abc12345")
    );
}

#[test]
fn migration_renames_the_hook_message_role() {
    let mut entries = file_entries(vec![
        json!({ "type": "session", "id": "sess-1", "version": 2, "timestamp": "2025-01-01T00:00:00Z", "cwd": "/tmp" }),
        json!({
            "type": "message",
            "id": "abc12345",
            "parentId": null,
            "timestamp": "2025-01-01T00:00:01Z",
            "message": { "role": "hookMessage", "customType": "hook", "content": "hi", "display": true, "timestamp": 1 },
        }),
    ]);

    migrate_session_entries(&mut entries);

    assert_eq!(entries[0].as_header().expect("header").version, Some(3));
    let message = entries[1]
        .as_entry()
        .expect("entry")
        .as_message()
        .expect("message");
    assert_eq!(message.message["role"], json!("custom"));
}

#[test]
fn migration_resolves_the_compaction_index_into_an_id() {
    let mut entries = file_entries(vec![
        json!({ "type": "session", "id": "sess-1", "timestamp": "2025-01-01T00:00:00Z", "cwd": "/tmp" }),
        json!({ "type": "message", "timestamp": "2025-01-01T00:00:01Z", "message": { "role": "user", "content": "one", "timestamp": 1 } }),
        json!({ "type": "message", "timestamp": "2025-01-01T00:00:02Z", "message": { "role": "user", "content": "two", "timestamp": 2 } }),
        json!({ "type": "compaction", "timestamp": "2025-01-01T00:00:03Z", "summary": "s", "firstKeptEntryIndex": 2, "tokensBefore": 10 }),
    ]);

    migrate_session_entries(&mut entries);

    let second_id = entries[2].as_entry().expect("entry").id().to_owned();
    let compaction = entries[3]
        .as_entry()
        .expect("entry")
        .as_compaction()
        .expect("compaction");
    assert_eq!(compaction.first_kept_entry_id, second_id);
    assert!(!compaction.extra.contains_key("firstKeptEntryIndex"));
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

use notagent::core::session_manager::{find_most_recent_session, load_entries_from_file};
use std::path::{Path, PathBuf};

const HEADER_SCAN_LIMIT_BYTES: usize = 1024 * 1024;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("notagent-session-test-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        Self { path }
    }

    fn path(&self) -> &str {
        self.path.to_str().expect("utf-8 path")
    }

    fn join(&self, name: &str) -> String {
        self.path.join(name).to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, contents: &str) -> String {
        let path = self.join(name);
        std::fs::write(&path, contents).expect("writes");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn session_header_line(cwd: &str, id: &str) -> String {
    format!(
        "{}\n",
        json!({ "type": "session", "version": 3, "id": id, "timestamp": "2025-01-01T00:00:00Z", "cwd": cwd })
    )
}

#[test]
fn load_entries_from_file_rejects_files_without_a_valid_header() {
    let directory = TempDir::new();
    assert!(load_entries_from_file(&directory.join("nonexistent.jsonl")).is_empty());
    assert!(load_entries_from_file(&directory.write("empty.jsonl", "")).is_empty());
    assert!(
        load_entries_from_file(
            &directory.write("no-header.jsonl", "{\"type\":\"message\",\"id\":\"1\"}\n")
        )
        .is_empty()
    );
    assert!(load_entries_from_file(&directory.write("malformed.jsonl", "not json\n")).is_empty());
}

#[test]
fn load_entries_from_file_loads_a_valid_session() {
    let directory = TempDir::new();
    let file = directory.write(
        "valid.jsonl",
        "{\"type\":\"session\",\"id\":\"abc\",\"timestamp\":\"2025-01-01T00:00:00Z\",\"cwd\":\"/tmp\"}\n\
         {\"type\":\"message\",\"id\":\"1\",\"parentId\":null,\"timestamp\":\"2025-01-01T00:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"hi\",\"timestamp\":1}}\n",
    );
    let entries = load_entries_from_file(&file);
    assert_eq!(entries.len(), 2);
    assert!(entries[0].as_header().is_some());
    assert_eq!(
        entries[1].as_entry().expect("entry").entry_type(),
        "message"
    );
}

#[test]
fn load_entries_from_file_skips_malformed_lines() {
    let directory = TempDir::new();
    let file = directory.write(
        "mixed.jsonl",
        "{\"type\":\"session\",\"id\":\"abc\",\"timestamp\":\"2025-01-01T00:00:00Z\",\"cwd\":\"/tmp\"}\n\
         not valid json\n\
         {\"type\":\"message\",\"id\":\"1\",\"parentId\":null,\"timestamp\":\"2025-01-01T00:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"hi\",\"timestamp\":1}}\n",
    );
    assert_eq!(load_entries_from_file(&file).len(), 2);
}

#[test]
fn reads_cwd_from_a_session_with_leading_noise_or_a_multi_buffer_header() {
    let cases: [(&str, String); 3] = [
        ("\n  \n", "leading-blank".to_owned()),
        ("not json\n{broken json\n", "leading-malformed".to_owned()),
        ("", "a".repeat(8192)),
    ];
    for (prefix, session_id) in cases {
        let directory = TempDir::new();
        let stored_cwd = directory.join("stored-project");
        let file = directory.write(
            "header.jsonl",
            &format!("{prefix}{}", session_header_line(&stored_cwd, &session_id)),
        );

        let session = SessionManager::open(&file, Some(directory.path()), None).expect("open");
        assert_eq!(session.get_session_id(), session_id);
        assert_eq!(session.get_cwd(), stored_cwd);
    }
}

#[test]
fn opens_compatible_sessions_beyond_the_discovery_scan_limit() {
    let directory = TempDir::new();
    let stored_cwd = directory.join("stored-project");
    let override_cwd = directory.join("override-project");
    let cases = [
        (
            "large-header",
            "a".repeat(HEADER_SCAN_LIMIT_BYTES + 1),
            String::new(),
        ),
        (
            "large-prefix",
            "large-prefix".to_owned(),
            format!("{}\n", "x".repeat(HEADER_SCAN_LIMIT_BYTES + 1)),
        ),
    ];
    for (name, id, prefix) in cases {
        let file = directory.write(
            &format!("{name}.jsonl"),
            &format!("{prefix}{}", session_header_line(&stored_cwd, &id)),
        );
        for cwd_override in [None, Some(override_cwd.as_str())] {
            let session =
                SessionManager::open(&file, Some(directory.path()), cwd_override).expect("open");
            assert_eq!(session.get_session_id(), id);
            assert_eq!(session.get_cwd(), cwd_override.unwrap_or(&stored_cwd));
        }
    }
}

#[test]
fn opens_session_files_with_gaps_larger_than_the_read_buffer() {
    // such limit, so this exercises the same incremental read loop with gaps that
    // span several 1 MiB buffers.
    use std::io::{Seek, SeekFrom, Write};
    let directory = TempDir::new();
    let file_path = directory.write(
        "large.jsonl",
        "{\"type\":\"session\",\"version\":3,\"id\":\"abc\",\"timestamp\":\"2025-01-01T00:00:00Z\",\"cwd\":\"/tmp\"}\n",
    );
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&file_path)
            .expect("open");
        let stride = 4 * 1024 * 1024u64;
        for offset in (stride..=stride * 4).step_by(stride as usize) {
            file.seek(SeekFrom::Start(offset)).expect("seek");
            file.write_all(b"\n").expect("write");
        }
    }
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&file_path)
            .expect("open");
        file.write_all(b"{\"type\":\"message\",\"id\":\"1\",\"parentId\":null,\"timestamp\":\"2025-01-01T00:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"hi\",\"timestamp\":1}}\n").expect("write");
    }

    let session = SessionManager::open(&file_path, Some(directory.path()), None).expect("open");
    assert_eq!(session.get_session_id(), "abc");
    assert_eq!(session.get_entries().len(), 1);
    let messages: Vec<Value> = session
        .build_session_context()
        .messages
        .iter()
        .map(|message| serde_json::to_value(message).expect("message"))
        .collect();
    assert_eq!(
        messages,
        vec![json!({ "role": "user", "content": "hi", "timestamp": 1 })]
    );
}

#[test]
fn find_most_recent_session_ignores_files_that_are_not_sessions() {
    let directory = TempDir::new();
    assert_eq!(find_most_recent_session(directory.path(), None), None);
    assert_eq!(
        find_most_recent_session(&directory.join("nonexistent"), None),
        None
    );
    directory.write("file.txt", "hello");
    directory.write("file.json", "{}");
    assert_eq!(find_most_recent_session(directory.path(), None), None);
    directory.write("invalid.jsonl", "{\"type\":\"message\"}\n");
    assert_eq!(find_most_recent_session(directory.path(), None), None);
}

#[test]
fn find_most_recent_session_returns_the_newest_valid_session() {
    let directory = TempDir::new();
    let file = directory.write("session.jsonl", &session_header_line("/tmp", "abc"));
    assert_eq!(
        find_most_recent_session(directory.path(), None).as_deref(),
        Some(file.as_str())
    );

    let older = directory.write("older.jsonl", &session_header_line("/tmp", "old"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    let newer = directory.write("newer.jsonl", &session_header_line("/tmp", "new"));
    let most_recent = find_most_recent_session(directory.path(), None).expect("session");
    assert_eq!(most_recent, newer);
    assert_ne!(most_recent, older);
}

#[test]
fn find_most_recent_session_skips_invalid_and_oversized_files() {
    let directory = TempDir::new();
    directory.write("invalid.jsonl", "{\"type\":\"not-session\"}\n");
    directory.write("oversized.jsonl", &"x".repeat(HEADER_SCAN_LIMIT_BYTES + 1));
    std::thread::sleep(std::time::Duration::from_millis(20));
    let valid = directory.write("valid.jsonl", &session_header_line("/tmp", "abc"));
    assert_eq!(
        find_most_recent_session(directory.path(), None).as_deref(),
        Some(valid.as_str())
    );
}

#[test]
fn find_most_recent_session_filters_by_cwd() {
    let directory = TempDir::new();
    let project_a = directory.join("project-a");
    let project_b = directory.join("project-b");
    let file_a = directory.write("a.jsonl", &session_header_line(&project_a, "a"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    let file_b = directory.write("b.jsonl", &session_header_line(&project_b, "b"));

    assert_eq!(
        find_most_recent_session(directory.path(), Some(&project_a)).as_deref(),
        Some(file_a.as_str())
    );
    assert_eq!(
        find_most_recent_session(directory.path(), Some(&project_b)).as_deref(),
        Some(file_b.as_str())
    );
}

#[tokio::test]
async fn scopes_current_folder_apis_by_cwd_while_listing_all_flat_sessions() {
    let directory = TempDir::new();
    let project_a = directory.join("project-a");
    let project_b = directory.join("project-b");
    std::fs::create_dir_all(&project_a).expect("creates");
    std::fs::create_dir_all(&project_b).expect("creates");

    let create_persisted = |cwd: &str, label: &str| -> String {
        let mut session =
            SessionManager::create(cwd, Some(directory.path()), None).expect("session");
        session.append_message(&user_msg(label)).expect("append");
        session
            .append_message(&assistant_msg(&format!("reply to {label}")))
            .expect("append");
        session.get_session_file().expect("session file").to_owned()
    };

    let session_a = create_persisted(&project_a, "from A");
    std::thread::sleep(std::time::Duration::from_millis(20));
    let session_b = create_persisted(&project_b, "from B");

    let current_a = SessionManager::list(&project_a, Some(directory.path()), None).await;
    assert_eq!(
        current_a
            .iter()
            .map(|session| session.path.clone())
            .collect::<Vec<_>>(),
        vec![session_a.clone()]
    );

    let all = SessionManager::list_all(Some(directory.path()), None).await;
    let mut paths: Vec<String> = all.iter().map(|session| session.path.clone()).collect();
    paths.sort();
    let mut expected = vec![session_a.clone(), session_b];
    expected.sort();
    assert_eq!(paths, expected);

    let continued =
        SessionManager::continue_recent(&project_a, Some(directory.path())).expect("session");
    assert_eq!(continued.get_session_file(), Some(session_a.as_str()));
}

#[test]
fn opening_an_empty_file_rewrites_it_with_a_valid_header() {
    let directory = TempDir::new();
    let empty_file = directory.write("empty.jsonl", "");

    let session = SessionManager::open(&empty_file, Some(directory.path()), None).expect("open");
    assert!(!session.get_session_id().is_empty());
    let header = session.get_header().expect("header");
    assert_eq!(header.id, session.get_session_id());
    assert_eq!(session.get_session_file(), Some(empty_file.as_str()));

    let content = std::fs::read_to_string(&empty_file).expect("read");
    let lines: Vec<&str> = content
        .trim()
        .lines()
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 1);
    let written: Value = serde_json::from_str(lines[0]).expect("json");
    assert_eq!(written["type"], json!("session"));
    assert_eq!(written["id"], json!(session.get_session_id()));

    // A second open reuses the written session.
    let reopened = SessionManager::open(&empty_file, Some(directory.path()), None).expect("open");
    assert_eq!(reopened.get_session_id(), session.get_session_id());
    assert!(reopened.get_header().is_some());
}

#[test]
fn opening_a_non_session_file_fails_and_preserves_it() {
    let directory = TempDir::new();
    let no_header = directory.write(
        "no-header.jsonl",
        "{\"type\":\"message\",\"id\":\"abc\",\"parentId\":\"orphaned\",\"timestamp\":\"2025-01-01T00:00:00Z\",\"message\":{\"role\":\"assistant\",\"content\":\"test\"}}\n",
    );
    let original = std::fs::read_to_string(&no_header).expect("read");
    assert_eq!(
        SessionManager::open(&no_header, Some(directory.path()), None).expect_err("invalid"),
        SessionManagerError::InvalidSessionFile(no_header.clone())
    );
    assert_eq!(std::fs::read_to_string(&no_header).expect("read"), original);

    let not_a_session = directory.write(
        "not-a-session.log",
        "{\"type\":\"event\",\"data\":\"not a session\"}\n",
    );
    let original = std::fs::read_to_string(&not_a_session).expect("read");
    assert_eq!(
        SessionManager::open(&not_a_session, Some(directory.path()), None).expect_err("invalid"),
        SessionManagerError::InvalidSessionFile(not_a_session.clone())
    );
    assert_eq!(
        std::fs::read_to_string(&not_a_session).expect("read"),
        original
    );
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

#[test]
fn create_branched_session_extracts_the_path_in_memory() {
    let mut session = in_memory();
    let id1 = session.append_message(&user_msg("1")).expect("append");
    let id2 = session.append_message(&assistant_msg("2")).expect("append");
    let id3 = session.append_message(&user_msg("3")).expect("append");
    session.append_message(&assistant_msg("4")).expect("append");
    session.branch(&id3).expect("branch");
    session.append_message(&user_msg("5")).expect("append");

    assert_eq!(session.create_branched_session(&id2).expect("branch"), None);
    let entries = session.get_entries();
    assert_eq!(ids(&entries), vec![id1, id2]);
}

#[test]
fn create_branched_session_extracts_the_path_from_a_branched_tree() {
    let mut session = in_memory();
    let id1 = session.append_message(&user_msg("1")).expect("append");
    let id2 = session.append_message(&assistant_msg("2")).expect("append");
    session.append_message(&user_msg("3")).expect("append");
    session.branch(&id2).expect("branch");
    let id4 = session.append_message(&user_msg("4")).expect("append");
    let id5 = session.append_message(&assistant_msg("5")).expect("append");

    session.create_branched_session(&id5).expect("branch");
    assert_eq!(ids(&session.get_entries()), vec![id1, id2, id4, id5]);
}

#[test]
fn create_branched_session_fails_for_a_non_existent_entry() {
    let mut session = in_memory();
    session.append_message(&user_msg("hello")).expect("append");
    assert_eq!(
        session
            .create_branched_session("nonexistent")
            .expect_err("missing"),
        SessionManagerError::EntryNotFound("nonexistent".to_owned())
    );
}

#[test]
fn create_branched_session_does_not_duplicate_entries_when_forking_the_first_user_message() {
    let directory = TempDir::new();
    let mut session =
        SessionManager::create(directory.path(), Some(directory.path()), None).expect("session");
    let id1 = session
        .append_message(&user_msg("first question"))
        .expect("append");
    session
        .append_message(&assistant_msg("first answer"))
        .expect("append");
    session
        .append_message(&user_msg("second question"))
        .expect("append");
    session
        .append_message(&assistant_msg("second answer"))
        .expect("append");

    let new_file = session
        .create_branched_session(&id1)
        .expect("branch")
        .expect("file");
    // The branched path has no assistant message, so the file is deferred.
    assert!(!Path::new(&new_file).exists());

    session
        .append_custom_entry("preset-state", Some(json!({ "name": "plan" })))
        .expect("append");
    session
        .append_message(&assistant_msg("new answer"))
        .expect("append");

    assert!(Path::new(&new_file).exists());
    let content = std::fs::read_to_string(&new_file).expect("read");
    let records: Vec<Value> = content
        .trim()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    assert_eq!(
        records
            .iter()
            .filter(|record| record["type"] == json!("session"))
            .count(),
        1
    );
    let entry_ids: Vec<&str> = records
        .iter()
        .filter(|record| record["type"] != json!("session"))
        .filter_map(|record| record["id"].as_str())
        .collect();
    let unique: std::collections::HashSet<&&str> = entry_ids.iter().collect();
    assert_eq!(unique.len(), entry_ids.len());
}

#[test]
fn create_branched_session_writes_immediately_when_the_path_has_an_assistant_message() {
    let directory = TempDir::new();
    let mut session =
        SessionManager::create(directory.path(), Some(directory.path()), None).expect("session");
    session
        .append_message(&user_msg("first question"))
        .expect("append");
    let id2 = session
        .append_message(&assistant_msg("first answer"))
        .expect("append");
    session
        .append_message(&user_msg("second question"))
        .expect("append");
    session
        .append_message(&assistant_msg("second answer"))
        .expect("append");

    let new_file = session
        .create_branched_session(&id2)
        .expect("branch")
        .expect("file");
    assert!(Path::new(&new_file).exists());
    let content = std::fs::read_to_string(&new_file).expect("read");
    let records: Vec<Value> = content
        .trim()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    assert_eq!(
        records
            .iter()
            .filter(|record| record["type"] == json!("session"))
            .count(),
        1
    );
}

#[test]
fn preserves_tool_and_summary_usage_across_a_file_backed_reload() {
    let directory = TempDir::new();
    let mut session =
        SessionManager::create(directory.path(), Some(directory.path()), None).expect("session");
    let root_id = session
        .append_message(&user_msg("question"))
        .expect("append");
    session
        .append_message(&assistant_msg("answer"))
        .expect("append");
    let tool_result: AgentMessage = serde_json::from_value(json!({
        "role": "toolResult",
        "toolCallId": "call-1",
        "toolName": "nested-model",
        "content": [{ "type": "text", "text": "result" }],
        "isError": false,
        "usage": usage(),
        "timestamp": 3,
    }))
    .expect("tool result");
    session.append_message(&tool_result).expect("append");
    let expected_usage = serde_json::from_value(usage()).expect("usage");
    session
        .append_compaction(
            "summary",
            &root_id,
            None,
            100,
            None,
            None,
            Some(false),
            Some(expected_usage),
        )
        .expect("append");
    let expected_usage = serde_json::from_value(usage()).expect("usage");
    session
        .branch_with_summary(
            Some(&root_id),
            "branch summary",
            None,
            Some(false),
            Some(expected_usage),
        )
        .expect("branch");

    let file = session.get_session_file().expect("session file").to_owned();
    let reopened = SessionManager::open(&file, Some(directory.path()), None).expect("open");
    let entries = reopened.get_entries();
    let expected_usage: notagent_ai::types::Usage = serde_json::from_value(usage()).expect("usage");
    assert!(entries.iter().any(|entry| matches!(
        entry,
        SessionEntry::Compaction(entry) if entry.usage.as_ref() == Some(&expected_usage)
    )));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        SessionEntry::BranchSummary(entry) if entry.usage.as_ref() == Some(&expected_usage)
    )));
    assert!(entries.iter().any(|entry| {
        entry.as_message().is_some_and(|entry| {
            entry.message["role"] == json!("toolResult")
                && entry.message["usage"]["totalTokens"] == json!(100)
        })
    }));
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

use notagent::core::session_manager::{LeafSelector, build_context_entries, build_session_context};

fn entry(value: Value) -> SessionEntry {
    SessionEntry::from_value(value)
}

fn msg(id: &str, parent_id: Option<&str>, role: &str, text: &str) -> SessionEntry {
    let message = if role == "user" {
        json!({ "role": "user", "content": text, "timestamp": 1 })
    } else {
        json!({
            "role": "assistant",
            "content": [{ "type": "text", "text": text }],
            "api": "anthropic-messages",
            "provider": "anthropic",
            "model": "claude-test",
            "usage": {
                "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
            },
            "stopReason": "stop",
            "timestamp": 1,
        })
    };
    entry(json!({
        "type": "message",
        "id": id,
        "parentId": parent_id,
        "timestamp": "2025-01-01T00:00:00Z",
        "message": message,
    }))
}

fn compaction_entry(
    id: &str,
    parent_id: Option<&str>,
    summary: &str,
    first_kept: &str,
) -> SessionEntry {
    entry(json!({
        "type": "compaction",
        "id": id,
        "parentId": parent_id,
        "timestamp": "2025-01-01T00:00:00Z",
        "summary": summary,
        "firstKeptEntryId": first_kept,
        "tokensBefore": 1000,
    }))
}

/// A compaction in the current shape: it names the messages it carried through
/// and how much of each survived.
fn compaction_entry_retaining(
    id: &str,
    parent_id: Option<&str>,
    summary: &str,
    head: &[&str],
    tail: &[&str],
    omitted_tokens: u64,
) -> SessionEntry {
    let first_kept = head.first().or_else(|| tail.first()).copied().unwrap_or("");
    entry(json!({
        "type": "compaction",
        "id": id,
        "parentId": parent_id,
        "timestamp": "2025-01-01T00:00:00Z",
        "summary": summary,
        "firstKeptEntryId": first_kept,
        "retained": {
            "head": head.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
            "tail": tail.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
            "omittedTokens": omitted_tokens,
        },
        "tokensBefore": 1000,
        "tokensAfter": 120,
    }))
}

fn branch_summary_entry(
    id: &str,
    parent_id: Option<&str>,
    summary: &str,
    from_id: &str,
) -> SessionEntry {
    entry(json!({
        "type": "branch_summary",
        "id": id,
        "parentId": parent_id,
        "timestamp": "2025-01-01T00:00:00Z",
        "summary": summary,
        "fromId": from_id,
    }))
}

fn custom_entry(id: &str, parent_id: Option<&str>, custom_type: &str, data: Value) -> SessionEntry {
    entry(json!({
        "type": "custom",
        "id": id,
        "parentId": parent_id,
        "timestamp": "2025-01-01T00:00:00Z",
        "customType": custom_type,
        "data": data,
    }))
}

fn thinking_level_entry(id: &str, parent_id: Option<&str>, level: &str) -> SessionEntry {
    entry(json!({
        "type": "thinking_level_change",
        "id": id,
        "parentId": parent_id,
        "timestamp": "2025-01-01T00:00:00Z",
        "thinkingLevel": level,
    }))
}

fn model_change_entry(
    id: &str,
    parent_id: Option<&str>,
    provider: &str,
    model_id: &str,
) -> SessionEntry {
    entry(json!({
        "type": "model_change",
        "id": id,
        "parentId": parent_id,
        "timestamp": "2025-01-01T00:00:00Z",
        "provider": provider,
        "modelId": model_id,
    }))
}

fn roles(messages: &[AgentMessage]) -> Vec<String> {
    messages
        .iter()
        .map(|message| {
            serde_json::to_value(message).expect("message")["role"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

fn message_values(messages: &[AgentMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| serde_json::to_value(message).expect("message"))
        .collect()
}

#[test]
fn build_session_context_handles_trivial_cases() {
    let context = build_session_context(&[], LeafSelector::Undefined);
    assert!(context.messages.is_empty());
    assert_eq!(context.thinking_level, "off");
    assert_eq!(context.model, None);

    let entries = vec![msg("1", None, "user", "hello")];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    assert_eq!(roles(&context.messages), vec!["user"]);

    let entries = vec![
        msg("1", None, "user", "hello"),
        msg("2", Some("1"), "assistant", "hi there"),
        msg("3", Some("2"), "user", "how are you"),
        msg("4", Some("3"), "assistant", "great"),
    ];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    assert_eq!(
        roles(&context.messages),
        vec!["user", "assistant", "user", "assistant"]
    );
}

#[test]
fn build_session_context_tracks_thinking_level_and_model() {
    let entries = vec![
        msg("1", None, "user", "hello"),
        thinking_level_entry("2", Some("1"), "high"),
        msg("3", Some("2"), "assistant", "thinking hard"),
    ];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    assert_eq!(context.thinking_level, "high");
    assert_eq!(context.messages.len(), 2);

    let entries = vec![
        msg("1", None, "user", "hello"),
        msg("2", Some("1"), "assistant", "hi"),
    ];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    let model = context.model.expect("model");
    assert_eq!(
        (model.provider.as_str(), model.model_id.as_str()),
        ("anthropic", "claude-test")
    );

    // An assistant message overwrites an earlier model change.
    let entries = vec![
        msg("1", None, "user", "hello"),
        model_change_entry("2", Some("1"), "openai", "gpt-4"),
        msg("3", Some("2"), "assistant", "hi"),
    ];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    let model = context.model.expect("model");
    assert_eq!(
        (model.provider.as_str(), model.model_id.as_str()),
        ("anthropic", "claude-test")
    );
}

#[test]
fn build_session_context_keeps_the_retained_messages_and_ends_with_the_summary() {
    let entries = vec![
        msg("1", None, "user", "first"),
        msg("2", Some("1"), "assistant", "response1"),
        msg("3", Some("2"), "user", "second"),
        msg("4", Some("3"), "assistant", "response2"),
        compaction_entry_retaining("5", Some("4"), "Note", &[], &["1", "3"], 0),
        msg("6", Some("5"), "user", "third"),
        msg("7", Some("6"), "assistant", "response3"),
    ];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    let messages = message_values(&context.messages);

    // The user's two retained messages, the note, then the turn that followed.
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0]["content"], json!("first"));
    assert_eq!(messages[1]["content"], json!("second"));
    assert!(
        messages[2]["summary"]
            .as_str()
            .expect("summary")
            .contains("Note")
    );
    assert_eq!(messages[3]["content"], json!("third"));
    assert_eq!(messages[4]["content"][0]["text"], json!("response3"));
}

#[test]
fn build_session_context_marks_the_gap_between_head_and_tail() {
    let entries = vec![
        msg("1", None, "user", "first"),
        msg("2", Some("1"), "user", "middle"),
        msg("3", Some("2"), "user", "last"),
        msg("4", Some("3"), "assistant", "response"),
        compaction_entry_retaining("5", Some("4"), "Note", &["1"], &["3"], 4200),
    ];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    let messages = message_values(&context.messages);

    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["content"], json!("first"));
    let elision = messages[1]["content"][0]["text"].as_str().expect("text");
    assert!(elision.contains("4200"), "{elision}");
    assert_eq!(messages[2]["content"], json!("last"));
    assert!(messages[3]["summary"].as_str().is_some());
}

#[test]
fn build_session_context_applies_the_recorded_truncation() {
    let long = "abcdefgh".repeat(8); // 64 characters
    let entries = vec![
        entry(json!({
            "type": "message",
            "id": "1",
            "parentId": null,
            "timestamp": "2025-01-01T00:00:00Z",
            "message": { "role": "user", "content": long, "timestamp": 0 },
        })),
        msg("2", Some("1"), "assistant", "response"),
        entry(json!({
            "type": "compaction",
            "id": "3",
            "parentId": Some("2"),
            "timestamp": "2025-01-01T00:00:00Z",
            "summary": "Note",
            "firstKeptEntryId": "1",
            "retained": { "head": [], "tail": [{ "id": "1", "suffixTokens": 4 }], "omittedTokens": 0 },
            "tokensBefore": 1000,
        })),
    ];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    let messages = message_values(&context.messages);

    let kept = messages[0]["content"][0]["text"].as_str().expect("text");
    assert!(kept.ends_with("abcdefghabcdefgh"), "{kept}");
    assert!(kept.contains("dropped during compaction"), "{kept}");
    // The entry itself still holds the whole message.
    assert!(matches!(&entries[0], SessionEntry::Message(_)));
}

/// Written before the retained selection existed: the suffix rule still
/// applies, summary first, so an old session keeps opening the way it did.
#[test]
fn build_session_context_includes_the_summary_before_kept_messages() {
    let entries = vec![
        msg("1", None, "user", "first"),
        msg("2", Some("1"), "assistant", "response1"),
        msg("3", Some("2"), "user", "second"),
        msg("4", Some("3"), "assistant", "response2"),
        compaction_entry("5", Some("4"), "Summary of first two turns", "3"),
        msg("6", Some("5"), "user", "third"),
        msg("7", Some("6"), "assistant", "response3"),
    ];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    let messages = message_values(&context.messages);
    assert_eq!(messages.len(), 5);
    assert!(
        messages[0]["summary"]
            .as_str()
            .expect("summary")
            .contains("Summary of first two turns")
    );
    assert_eq!(messages[1]["content"], json!("second"));
    assert_eq!(messages[2]["content"][0]["text"], json!("response2"));
    assert_eq!(messages[3]["content"], json!("third"));
    assert_eq!(messages[4]["content"][0]["text"], json!("response3"));
}

#[test]
fn build_session_context_handles_compaction_keeping_from_the_first_message() {
    let entries = vec![
        msg("1", None, "user", "first"),
        msg("2", Some("1"), "assistant", "response"),
        compaction_entry("3", Some("2"), "Empty summary", "1"),
        msg("4", Some("3"), "user", "second"),
    ];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    let messages = message_values(&context.messages);
    assert_eq!(messages.len(), 4);
    assert!(
        messages[0]["summary"]
            .as_str()
            .expect("summary")
            .contains("Empty summary")
    );
}

#[test]
fn build_session_context_uses_the_latest_compaction() {
    let entries = vec![
        msg("1", None, "user", "a"),
        msg("2", Some("1"), "assistant", "b"),
        compaction_entry("3", Some("2"), "First summary", "1"),
        msg("4", Some("3"), "user", "c"),
        msg("5", Some("4"), "assistant", "d"),
        compaction_entry("6", Some("5"), "Second summary", "4"),
        msg("7", Some("6"), "user", "e"),
    ];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    let messages = message_values(&context.messages);
    assert_eq!(messages.len(), 4);
    assert!(
        messages[0]["summary"]
            .as_str()
            .expect("summary")
            .contains("Second summary")
    );
}

#[test]
fn build_context_entries_returns_compaction_aware_entries_including_custom_entries() {
    let entries = vec![
        msg("1", None, "user", "first"),
        custom_entry("2", Some("1"), "old-state", json!({ "hidden": true })),
        msg("3", Some("2"), "assistant", "response1"),
        custom_entry("4", Some("3"), "kept-card", json!({ "title": "Kept" })),
        msg("5", Some("4"), "user", "second"),
        compaction_entry("6", Some("5"), "Summary", "4"),
        custom_entry("7", Some("6"), "after-card", json!({ "title": "After" })),
        msg("8", Some("7"), "assistant", "response2"),
    ];
    let context_ids: Vec<&str> = build_context_entries(&entries, LeafSelector::Undefined)
        .iter()
        .map(|entry| entry.id())
        .collect();
    assert_eq!(context_ids, vec!["6", "4", "5", "7", "8"]);
    let context = build_session_context(&entries, LeafSelector::Undefined);
    assert_eq!(
        roles(&context.messages),
        vec!["compactionSummary", "user", "assistant"]
    );
}

#[test]
fn build_session_context_keeps_settings_from_the_full_path_after_compaction() {
    let entries = vec![
        msg("1", None, "user", "first"),
        thinking_level_entry("2", Some("1"), "high"),
        msg("3", Some("2"), "assistant", "response1"),
        msg("4", Some("3"), "user", "second"),
        compaction_entry("5", Some("4"), "Summary", "4"),
    ];
    let context = build_session_context(&entries, LeafSelector::Undefined);
    assert_eq!(context.thinking_level, "high");
    assert_eq!(roles(&context.messages), vec!["compactionSummary", "user"]);
}

#[test]
fn build_session_context_follows_the_path_to_the_selected_leaf() {
    let entries = vec![
        msg("1", None, "user", "start"),
        msg("2", Some("1"), "assistant", "response"),
        msg("3", Some("2"), "user", "branch A"),
        msg("4", Some("2"), "user", "branch B"),
    ];
    let context = build_session_context(&entries, LeafSelector::Id("3"));
    assert_eq!(context.messages.len(), 3);
    assert_eq!(
        message_values(&context.messages)[2]["content"],
        json!("branch A")
    );

    let context = build_session_context(&entries, LeafSelector::Id("4"));
    assert_eq!(context.messages.len(), 3);
    assert_eq!(
        message_values(&context.messages)[2]["content"],
        json!("branch B")
    );
}

#[test]
fn build_session_context_includes_a_branch_summary_in_the_path() {
    let entries = vec![
        msg("1", None, "user", "start"),
        msg("2", Some("1"), "assistant", "response"),
        msg("3", Some("2"), "user", "abandoned path"),
        branch_summary_entry("4", Some("2"), "Summary of abandoned work", "3"),
        msg("5", Some("4"), "user", "new direction"),
    ];
    let context = build_session_context(&entries, LeafSelector::Id("5"));
    let messages = message_values(&context.messages);
    assert_eq!(messages.len(), 4);
    assert!(
        messages[2]["summary"]
            .as_str()
            .expect("summary")
            .contains("Summary of abandoned work")
    );
    assert_eq!(messages[3]["content"], json!("new direction"));
}

#[test]
fn build_session_context_handles_a_complex_tree_with_branches_and_compaction() {
    let entries = vec![
        msg("1", None, "user", "start"),
        msg("2", Some("1"), "assistant", "r1"),
        msg("3", Some("2"), "user", "q2"),
        msg("4", Some("3"), "assistant", "r2"),
        compaction_entry("5", Some("4"), "Compacted history", "3"),
        msg("6", Some("5"), "user", "q3"),
        msg("7", Some("6"), "assistant", "r3"),
        msg("8", Some("3"), "user", "wrong path"),
        msg("9", Some("8"), "assistant", "wrong response"),
        branch_summary_entry("10", Some("3"), "Tried wrong approach", "9"),
        msg("11", Some("10"), "user", "better approach"),
    ];

    let main = message_values(&build_session_context(&entries, LeafSelector::Id("7")).messages);
    assert_eq!(main.len(), 5);
    assert!(
        main[0]["summary"]
            .as_str()
            .expect("summary")
            .contains("Compacted history")
    );
    assert_eq!(main[1]["content"], json!("q2"));
    assert_eq!(main[2]["content"][0]["text"], json!("r2"));
    assert_eq!(main[3]["content"], json!("q3"));
    assert_eq!(main[4]["content"][0]["text"], json!("r3"));

    let branch = message_values(&build_session_context(&entries, LeafSelector::Id("11")).messages);
    assert_eq!(branch.len(), 5);
    assert_eq!(branch[0]["content"], json!("start"));
    assert_eq!(branch[1]["content"][0]["text"], json!("r1"));
    assert_eq!(branch[2]["content"], json!("q2"));
    assert!(
        branch[3]["summary"]
            .as_str()
            .expect("summary")
            .contains("Tried wrong approach")
    );
    assert_eq!(branch[4]["content"], json!("better approach"));
}

#[test]
fn build_session_context_handles_missing_leaves_and_orphans() {
    let entries = vec![
        msg("1", None, "user", "hello"),
        msg("2", Some("1"), "assistant", "hi"),
    ];
    let context = build_session_context(&entries, LeafSelector::Id("nonexistent"));
    assert_eq!(context.messages.len(), 2);

    let entries = vec![
        msg("1", None, "user", "hello"),
        msg("2", Some("missing"), "assistant", "orphan"),
    ];
    let context = build_session_context(&entries, LeafSelector::Id("2"));
    assert_eq!(context.messages.len(), 1);
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

fn label_entries(session: &SessionManager) -> Vec<notagent::core::session_manager::LabelEntry> {
    session
        .get_entries()
        .into_iter()
        .filter_map(|entry| match entry {
            SessionEntry::Label(entry) => Some(entry),
            _ => None,
        })
        .collect()
}

#[test]
fn sets_and_gets_labels() {
    let mut session = in_memory();
    let msg_id = session.append_message(&user_msg("hello")).expect("append");
    assert_eq!(session.get_label(&msg_id), None);

    let label_id = session
        .append_label_change(&msg_id, Some("checkpoint"))
        .expect("label");
    assert_eq!(session.get_label(&msg_id), Some("checkpoint"));

    let labels = label_entries(&session);
    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0].id, label_id);
    assert_eq!(labels[0].target_id, msg_id);
    assert_eq!(labels[0].label.as_deref(), Some("checkpoint"));
}

#[test]
fn clears_labels_with_none() {
    let mut session = in_memory();
    let msg_id = session.append_message(&user_msg("hello")).expect("append");
    session
        .append_label_change(&msg_id, Some("checkpoint"))
        .expect("label");
    assert_eq!(session.get_label(&msg_id), Some("checkpoint"));
    session.append_label_change(&msg_id, None).expect("label");
    assert_eq!(session.get_label(&msg_id), None);
}

#[test]
fn the_last_label_wins() {
    let mut session = in_memory();
    let msg_id = session.append_message(&user_msg("hello")).expect("append");
    session
        .append_label_change(&msg_id, Some("first"))
        .expect("label");
    session
        .append_label_change(&msg_id, Some("second"))
        .expect("label");
    let last_label_id = session
        .append_label_change(&msg_id, Some("third"))
        .expect("label");
    assert_eq!(session.get_label(&msg_id), Some("third"));

    let entries = session.get_entries();
    let last = entries
        .iter()
        .find(|entry| entry.id() == last_label_id)
        .expect("label entry");
    let tree = session.get_tree();
    let node = tree
        .iter()
        .find(|node| node.entry.id() == msg_id)
        .expect("message node");
    assert_eq!(node.label_timestamp.as_deref(), Some(last.timestamp()));
}

#[test]
fn labels_are_included_in_tree_nodes() {
    let mut session = in_memory();
    let msg1_id = session.append_message(&user_msg("hello")).expect("append");
    let msg2_id = session
        .append_message(&assistant_msg("hi"))
        .expect("append");
    let label1_id = session
        .append_label_change(&msg1_id, Some("start"))
        .expect("label");
    let label2_id = session
        .append_label_change(&msg2_id, Some("response"))
        .expect("label");

    let entries = session.get_entries();
    let label1 = entries
        .iter()
        .find(|entry| entry.id() == label1_id)
        .expect("label");
    let label2 = entries
        .iter()
        .find(|entry| entry.id() == label2_id)
        .expect("label");
    let tree = session.get_tree();
    let node1 = tree
        .iter()
        .find(|node| node.entry.id() == msg1_id)
        .expect("node");
    assert_eq!(node1.label.as_deref(), Some("start"));
    assert_eq!(node1.label_timestamp.as_deref(), Some(label1.timestamp()));
    let node2 = node1
        .children
        .iter()
        .find(|node| node.entry.id() == msg2_id)
        .expect("node");
    assert_eq!(node2.label.as_deref(), Some("response"));
    assert_eq!(node2.label_timestamp.as_deref(), Some(label2.timestamp()));
}

#[test]
fn labels_are_preserved_in_create_branched_session() {
    let mut session = in_memory();
    let msg1_id = session.append_message(&user_msg("hello")).expect("append");
    let msg2_id = session
        .append_message(&assistant_msg("hi"))
        .expect("append");
    let label1_id = session
        .append_label_change(&msg1_id, Some("important"))
        .expect("label");
    let label2_id = session
        .append_label_change(&msg2_id, Some("also-important"))
        .expect("label");
    let original = session.get_entries();
    let label1_timestamp = original
        .iter()
        .find(|entry| entry.id() == label1_id)
        .expect("label")
        .timestamp()
        .to_owned();
    let label2_timestamp = original
        .iter()
        .find(|entry| entry.id() == label2_id)
        .expect("label")
        .timestamp()
        .to_owned();

    session.create_branched_session(&msg2_id).expect("branch");

    assert_eq!(session.get_label(&msg1_id), Some("important"));
    assert_eq!(session.get_label(&msg2_id), Some("also-important"));
    assert_eq!(label_entries(&session).len(), 2);

    let tree = session.get_tree();
    let node1 = tree
        .iter()
        .find(|node| node.entry.id() == msg1_id)
        .expect("node");
    let node2 = node1
        .children
        .iter()
        .find(|node| node.entry.id() == msg2_id)
        .expect("node");
    assert_eq!(
        node1.label_timestamp.as_deref(),
        Some(label1_timestamp.as_str())
    );
    assert_eq!(
        node2.label_timestamp.as_deref(),
        Some(label2_timestamp.as_str())
    );
}

#[test]
fn rewires_children_of_removed_labels_when_forking() {
    let mut session = in_memory();
    let msg1_id = session.append_message(&user_msg("hello")).expect("append");
    session
        .append_label_change(&msg1_id, Some("checkpoint"))
        .expect("label");
    let model_change_id = session
        .append_model_change("anthropic", "claude-test")
        .expect("append");
    let msg2_id = session
        .append_message(&user_msg("followup"))
        .expect("append");

    session.create_branched_session(&msg2_id).expect("branch");

    assert_eq!(
        session
            .get_entry(&model_change_id)
            .expect("entry")
            .parent_id(),
        Some(msg1_id.as_str())
    );
}

#[test]
fn labels_off_the_path_are_not_preserved_when_forking() {
    let mut session = in_memory();
    let msg1_id = session.append_message(&user_msg("hello")).expect("append");
    let msg2_id = session
        .append_message(&assistant_msg("hi"))
        .expect("append");
    let msg3_id = session
        .append_message(&user_msg("followup"))
        .expect("append");
    session
        .append_label_change(&msg1_id, Some("first"))
        .expect("label");
    session
        .append_label_change(&msg2_id, Some("second"))
        .expect("label");
    session
        .append_label_change(&msg3_id, Some("third"))
        .expect("label");

    session.create_branched_session(&msg2_id).expect("branch");

    assert_eq!(session.get_label(&msg1_id), Some("first"));
    assert_eq!(session.get_label(&msg2_id), Some("second"));
    assert_eq!(session.get_label(&msg3_id), None);
}

#[test]
fn labels_are_not_included_in_the_session_context() {
    let mut session = in_memory();
    let msg_id = session.append_message(&user_msg("hello")).expect("append");
    session
        .append_label_change(&msg_id, Some("checkpoint"))
        .expect("label");
    let context = session.build_session_context();
    assert_eq!(roles(&context.messages), vec!["user"]);
}

#[test]
fn labeling_a_non_existent_entry_fails() {
    let mut session = in_memory();
    assert_eq!(
        session
            .append_label_change("non-existent", Some("label"))
            .expect_err("missing"),
        SessionManagerError::EntryNotFound("non-existent".to_owned())
    );
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

use notagent::core::session_manager::NewSessionOptions;

fn is_uuid_v7(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    let groups = [8usize, 4, 4, 4, 12];
    let mut index = 0;
    for (group, length) in groups.iter().enumerate() {
        if group > 0 {
            if bytes[index] != b'-' {
                return false;
            }
            index += 1;
        }
        for offset in 0..*length {
            let byte = bytes[index + offset];
            if !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase() {
                return false;
            }
        }
        if group == 2 && bytes[index] != b'7' {
            return false;
        }
        if group == 3 && !matches!(bytes[index], b'8' | b'9' | b'a' | b'b') {
            return false;
        }
        index += length;
    }
    true
}

fn with_id(id: &str) -> Option<NewSessionOptions> {
    Some(NewSessionOptions {
        id: Some(id.to_owned()),
        parent_session: None,
    })
}

#[test]
fn new_session_uses_a_provided_id() {
    let mut session = in_memory();
    session
        .new_session(with_id("my-custom-id"))
        .expect("new session");
    assert_eq!(session.get_session_id(), "my-custom-id");

    let session =
        SessionManager::in_memory(Some("/tmp"), with_id("memory-session-id")).expect("session");
    assert_eq!(session.get_session_id(), "memory-session-id");
    assert_eq!(
        session.get_header().expect("header").id,
        "memory-session-id"
    );
    assert_eq!(session.get_session_file(), None);

    let mut session = in_memory();
    session
        .new_session(with_id("abc-123_def.456"))
        .expect("new session");
    assert_eq!(session.get_session_id(), "abc-123_def.456");

    let mut session = in_memory();
    session
        .new_session(with_id("header-test-id"))
        .expect("new session");
    assert_eq!(session.get_header().expect("header").id, "header-test-id");
}

#[test]
fn new_session_rejects_invalid_ids() {
    for id in [
        "", "-abc", "abc-", "_abc", "abc_", ".abc", "abc.", "abc/def", "abc\\def", "abc def",
    ] {
        let mut session = in_memory();
        let error = session.new_session(with_id(id)).expect_err("invalid id");
        assert!(
            error
                .to_string()
                .contains("Session id must be non-empty, contain only alphanumeric characters"),
            "{error}"
        );
    }
}

#[test]
fn new_session_generates_a_uuid_v7_when_no_id_is_given() {
    let mut session = in_memory();
    session.new_session(None).expect("new session");
    assert!(
        is_uuid_v7(session.get_session_id()),
        "{}",
        session.get_session_id()
    );

    session
        .new_session(Some(NewSessionOptions {
            id: None,
            parent_session: Some("parent.jsonl".to_owned()),
        }))
        .expect("new session");
    assert!(
        is_uuid_v7(session.get_session_id()),
        "{}",
        session.get_session_id()
    );

    let session = in_memory();
    assert!(is_uuid_v7(session.get_session_id()));
    assert_eq!(
        session.get_header().expect("header").id,
        session.get_session_id()
    );
}

#[test]
fn create_uses_a_provided_id_for_the_session_file_name() {
    let directory = TempDir::new();
    let session = SessionManager::create(
        directory.path(),
        Some(directory.path()),
        with_id("created-session-id"),
    )
    .expect("session");
    assert_eq!(session.get_session_id(), "created-session-id");
    assert_eq!(
        session.get_header().expect("header").id,
        "created-session-id"
    );
    let session_file = session.get_session_file().expect("session file");
    assert!(session_file.contains("created-session-id"));
    let base_name = Path::new(session_file)
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    let (timestamp, rest) = base_name.split_once('_').expect("timestamp separator");
    assert_eq!(rest, "created-session-id.jsonl");
    assert_eq!(timestamp.len(), "2025-01-01T00-00-00-000Z".len());
    assert!(timestamp.ends_with('Z'));
    // The file is only written once the first assistant message arrives.
    assert!(!Path::new(session_file).exists());
}

#[test]
fn create_branched_session_generates_a_uuid_v7() {
    let mut session = in_memory();
    let first_id = session.append_message(&user_msg("hello")).expect("append");
    session.create_branched_session(&first_id).expect("branch");
    assert!(
        is_uuid_v7(session.get_session_id()),
        "{}",
        session.get_session_id()
    );
    assert_eq!(
        session.get_header().expect("header").id,
        session.get_session_id()
    );
}

#[test]
fn fork_from_generates_or_uses_the_session_id() {
    let directory = TempDir::new();
    let source_path = directory.write(
        "source.jsonl",
        &format!(
            "{}\n{}\n",
            json!({
                "type": "session",
                "version": 3,
                "id": "legacy-session-id",
                "timestamp": "2025-01-01T00:00:00.000Z",
                "cwd": directory.path(),
            }),
            json!({
                "type": "message",
                "id": "entry-1",
                "parentId": null,
                "timestamp": "2025-01-01T00:00:01.000Z",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "hello" }],
                    "api": "openai-responses",
                    "provider": "openai",
                    "model": "gpt-5.4",
                    "usage": {
                        "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
                    },
                    "stopReason": "stop",
                    "timestamp": 1,
                },
            })
        ),
    );

    let forked =
        SessionManager::fork_from(&source_path, directory.path(), Some(directory.path()), None)
            .expect("fork");
    let header = forked.get_header().expect("header");
    assert!(is_uuid_v7(&header.id), "{}", header.id);
    assert_eq!(header.parent_session.as_deref(), Some(source_path.as_str()));

    let forked = SessionManager::fork_from(
        &source_path,
        directory.path(),
        Some(directory.path()),
        with_id("forked-session-id"),
    )
    .expect("fork");
    let header = forked.get_header().expect("header");
    assert_eq!(header.id, "forked-session-id");
    assert_eq!(header.parent_session.as_deref(), Some(source_path.as_str()));
    let session_file = forked.get_session_file().expect("session file");
    assert!(session_file.contains("forked-session-id"));
    // The fork copies the source entries.
    assert_eq!(forked.get_entries().len(), 1);
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_info_uses_the_last_message_timestamp_instead_of_the_file_mtime() {
    let directory = TempDir::new();
    let file_path = directory.write(
        "session.jsonl",
        &format!(
            "{}\n",
            json!({
                "type": "session",
                "id": "test-session",
                "version": 3,
                "timestamp": "1970-01-01T00:00:00.000Z",
                "cwd": "/tmp",
            })
        ),
    );

    // The session only persists once it has seen an assistant message.
    let mut session = SessionManager::open(&file_path, None, None).expect("open");
    session
        .append_message(&assistant_msg("hi"))
        .expect("append");
    let before = std::fs::metadata(&file_path)
        .expect("stat")
        .modified()
        .expect("mtime");
    std::thread::sleep(std::time::Duration::from_millis(20));

    let mut session = SessionManager::open(&file_path, None, None).expect("open");
    let message_time = 1_760_000_000_000i64;
    let later: AgentMessage = serde_json::from_value(json!({
        "role": "assistant",
        "content": [{ "type": "text", "text": "later" }],
        "api": "openai-completions",
        "provider": "openai",
        "model": "test",
        "usage": {
            "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
            "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
        },
        "stopReason": "stop",
        "timestamp": message_time,
    }))
    .expect("assistant message");
    session.append_message(&later).expect("append");

    let sessions = SessionManager::list("/tmp", Some(directory.path()), None).await;
    let info = sessions
        .iter()
        .find(|info| info.path == file_path)
        .expect("session info");
    assert_eq!(info.modified, message_time);
    let before_ms = before
        .duration_since(std::time::UNIX_EPOCH)
        .expect("epoch")
        .as_millis() as i64;
    assert_ne!(info.modified, before_ms);
    assert_eq!(info.id, "test-session");
    assert_eq!(info.message_count, 2);
}

#[tokio::test]
async fn session_info_reports_names_first_messages_and_parents() {
    let directory = TempDir::new();
    let mut session =
        SessionManager::create(directory.path(), Some(directory.path()), None).expect("session");
    session
        .append_message(&user_msg("the first question"))
        .expect("append");
    session
        .append_message(&assistant_msg("an answer"))
        .expect("append");
    session
        .append_session_info("  My \n session  ")
        .expect("append");
    let file = session.get_session_file().expect("file").to_owned();

    let sessions = SessionManager::list(directory.path(), Some(directory.path()), None).await;
    let info = sessions
        .iter()
        .find(|info| info.path == file)
        .expect("session info");
    assert_eq!(info.name.as_deref(), Some("My   session"));
    assert_eq!(info.first_message, "the first question");
    assert!(info.all_messages_text.contains("the first question"));
    assert!(info.all_messages_text.contains("an answer"));
    assert_eq!(info.parent_session_path, None);
    assert_eq!(session.get_session_name().as_deref(), Some("My   session"));
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// Read a fixture the way the loader does, so both sides skip the same lines.
fn fixture_lines(path: &str) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("fixture")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

#[test]
fn reads_and_reserializes_session_fixtures_losslessly() {
    for name in ["before-compaction.jsonl", "large-session.jsonl"] {
        let path = fixture(name);
        let original = fixture_lines(&path);
        let mut entries = load_entries_from_file(&path);
        assert_eq!(entries.len(), original.len(), "{name}");

        // Both fixtures carry a v1 header, so the migration adds `id`/`parentId`
        // and resolves `firstKeptEntryIndex`; everything else must survive.
        migrate_session_entries(&mut entries);
        for (index, (entry, expected)) in entries.iter().zip(original.iter()).enumerate() {
            let written: Value = serde_json::from_str(&entry.to_json()).expect("re-serialized");
            for (key, value) in expected.as_object().expect("object") {
                if key == "version" || key == "firstKeptEntryIndex" {
                    continue;
                }
                assert_eq!(
                    written.get(key),
                    Some(value),
                    "{name} line {index} key {key}"
                );
            }
        }

        // A migrated (v3) session round-trips byte for byte.
        let migrated: Vec<String> = entries.iter().map(FileEntry::to_json).collect();
        let reserialized: Vec<String> = migrated
            .iter()
            .map(|line| FileEntry::from_value(serde_json::from_str(line).expect("json")).to_json())
            .collect();
        assert_eq!(reserialized, migrated, "{name}");
    }
}

#[test]
fn opens_continues_and_forks_a_session_fixture() {
    let directory = TempDir::new();
    let source = directory.join("before-compaction.jsonl");
    std::fs::copy(fixture("before-compaction.jsonl"), &source).expect("copy fixture");
    let original_lines = fixture_lines(&source).len();

    // The fixture has a v1 header, so opening it migrates and rewrites the file.
    let mut session = SessionManager::open(&source, Some(directory.path()), None).expect("open");
    assert_eq!(session.get_header().expect("header").version, Some(3));
    assert_eq!(session.get_entries().len(), original_lines - 1);
    let migrated = fixture_lines(&source);
    assert_eq!(migrated.len(), original_lines);
    assert_eq!(migrated[0]["version"], json!(3));
    // Every migrated entry now carries an 8-character id and a parent chain.
    let entries = session.get_entries();
    assert!(entries.iter().all(|entry| entry.id().len() == 8));
    assert_eq!(entries[0].parent_id(), None);
    assert_eq!(entries[1].parent_id(), Some(entries[0].id()));
    // The unknown header fields of the old format survive the rewrite.
    assert_eq!(migrated[0]["provider"], json!("anthropic"));
    assert_eq!(migrated[0]["thinkingLevel"], json!("off"));

    // Continuing appends to the migrated file.
    let leaf_before = session.get_leaf_id().expect("leaf").to_owned();
    let appended = session
        .append_message(&user_msg("continue please"))
        .expect("append");
    assert_eq!(
        session.get_entry(&appended).expect("entry").parent_id(),
        Some(leaf_before.as_str())
    );
    let after_append = fixture_lines(&source);
    assert_eq!(after_append.len(), original_lines + 1);

    // Forking copies every entry into a new file with a fresh id.
    let forked = SessionManager::fork_from(&source, directory.path(), Some(directory.path()), None)
        .expect("fork");
    assert_eq!(forked.get_entries().len(), original_lines);
    let forked_file = forked.get_session_file().expect("file");
    assert_eq!(fixture_lines(forked_file).len(), original_lines + 1);
    assert_eq!(
        forked
            .get_header()
            .expect("header")
            .parent_session
            .as_deref(),
        Some(source.as_str())
    );
    assert_ne!(forked.get_session_id(), session.get_session_id());

    // The context of the fixture rebuilds from its latest compaction.
    let context = forked.build_session_context();
    assert!(!context.messages.is_empty());
}
