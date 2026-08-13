//! Port of `packages/coding-agent/test/tree-selector.test.ts` (702 LOC).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::keybindings::KeybindingsManager;
use notagent::core::session_manager::{
    ModelChangeEntry, SessionEntry, SessionMessageEntry, SessionTreeNode, ThinkingLevelChangeEntry,
};
use notagent::modes::interactive::components::tree_selector::{
    TreeSelectorComponent, TreeSelectorOptions,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::tui::Component;
use notagent_tui::utils::visible_width;
use serde_json::json;

/// The theme and the keybindings registry are process globals.
fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    init_theme(Some("dark"), false);
    // Ensure test isolation: keybindings are a global singleton
    set_keybindings(KeybindingsManager::default().to_tui());
    guard
}

const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";
const CTRL_LEFT: &str = "\x1b[1;5D";
const CTRL_RIGHT: &str = "\x1b[1;5C";
const ALT_LEFT: &str = "\x1b[1;3D";
const ALT_RIGHT: &str = "\x1b[1;3C";

// Helper to create a user message entry
fn user_message(id: &str, parent_id: Option<&str>, content: &str) -> SessionEntry {
    SessionEntry::Message(SessionMessageEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-08-13T00:00:00.000Z".to_string(),
        message: json!({ "role": "user", "content": content, "timestamp": 0 }),
        ..Default::default()
    })
}

// Helper to create an assistant message entry
fn assistant_message(id: &str, parent_id: Option<&str>, text: &str) -> SessionEntry {
    SessionEntry::Message(SessionMessageEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-08-13T00:00:00.000Z".to_string(),
        message: json!({
            "role": "assistant",
            "content": [{ "type": "text", "text": text }],
            "api": "anthropic-messages",
            "provider": "anthropic",
            "model": "claude-sonnet-4",
            "stopReason": "stop",
            "timestamp": 0,
        }),
        ..Default::default()
    })
}

// Helper to create a tool-call-only assistant message (filtered out in default mode)
fn tool_call_only_assistant(id: &str, parent_id: Option<&str>) -> SessionEntry {
    SessionEntry::Message(SessionMessageEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-08-13T00:00:00.000Z".to_string(),
        message: json!({
            "role": "assistant",
            "content": [{
                "type": "toolCall",
                "id": format!("tc-{id}"),
                "name": "read",
                "arguments": { "path": "test.ts" },
            }],
            "stopReason": "toolUse",
            "timestamp": 0,
        }),
        ..Default::default()
    })
}

// Helper to create a model_change entry
fn model_change(id: &str, parent_id: Option<&str>) -> SessionEntry {
    SessionEntry::ModelChange(ModelChangeEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-08-13T00:00:00.000Z".to_string(),
        provider: "anthropic".to_string(),
        model_id: "claude-sonnet-4".to_string(),
        ..Default::default()
    })
}

// Helper to build a tree from entries using parentId relationships
fn build_tree(entries: Vec<SessionEntry>) -> Vec<SessionTreeNode> {
    fn attach(entries: &[SessionEntry], parent_id: Option<&str>) -> Vec<SessionTreeNode> {
        entries
            .iter()
            .filter(|entry| entry.parent_id() == parent_id)
            .map(|entry| SessionTreeNode {
                entry: entry.clone(),
                children: attach(entries, Some(entry.id())),
                label: None,
                label_timestamp: None,
            })
            .collect()
    }
    attach(&entries, None)
}

fn selector(tree: &[SessionTreeNode], current_leaf_id: &str) -> TreeSelectorComponent {
    TreeSelectorComponent::new(
        tree,
        Some(current_leaf_id),
        24,
        Box::new(|_| {}),
        Box::new(|| {}),
        TreeSelectorOptions::default(),
    )
}

fn selected_id(selector: &TreeSelectorComponent) -> Option<String> {
    selector
        .get_tree_list()
        .borrow()
        .get_selected_node()
        .map(|node| node.entry.id().to_string())
}

// --- initial selection with metadata entries ----------------------------------

#[test]
fn focuses_nearest_visible_ancestor_when_current_leaf_is_a_model_change_with_sibling_branch() {
    let _guard = test_lock();
    // Tree structure:
    // user-1
    // └── asst-1
    //     ├── user-2 (active branch)
    //     │   └── model-1 (model_change, CURRENT LEAF)
    //     └── user-3 (sibling branch, added later chronologically)
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "active branch"),
        model_change("model-1", Some("user-2")),
        user_message("user-3", Some("asst-1"), "sibling branch"),
    ]);

    let selector = selector(&tree, "model-1");

    // Should focus on user-2 (parent of model-1), not user-3 (last item)
    assert_eq!(selected_id(&selector).as_deref(), Some("user-2"));
}

#[test]
fn focuses_nearest_visible_ancestor_when_current_leaf_is_a_thinking_level_change() {
    let _guard = test_lock();
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "active branch"),
        SessionEntry::ThinkingLevelChange(ThinkingLevelChangeEntry {
            id: "thinking-1".to_string(),
            parent_id: Some("user-2".to_string()),
            timestamp: "2026-08-13T00:00:00.000Z".to_string(),
            thinking_level: "high".to_string(),
            ..Default::default()
        }),
        user_message("user-3", Some("asst-1"), "sibling branch"),
    ]);

    let selector = selector(&tree, "thinking-1");

    assert_eq!(selected_id(&selector).as_deref(), Some("user-2"));
}

// --- filter switching with parent traversal -----------------------------------

fn branching_filter_tree() -> Vec<SessionTreeNode> {
    // In user-only filter: [user-1, user-2, user-3]
    build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "active branch"),
        assistant_message("asst-2", Some("user-2"), "response"),
        user_message("user-3", Some("asst-1"), "sibling branch"),
    ])
}

#[test]
fn switches_to_nearest_visible_user_message_when_changing_to_user_only_filter() {
    let _guard = test_lock();
    let tree = branching_filter_tree();
    let mut selector = selector(&tree, "asst-2");

    assert_eq!(selected_id(&selector).as_deref(), Some("asst-2"));

    // Simulate Ctrl+U (user-only filter)
    selector.handle_input("\x15");

    // Should now be on user-2 (the parent user message), not user-3
    assert_eq!(selected_id(&selector).as_deref(), Some("user-2"));
}

#[test]
fn returns_to_nearest_visible_ancestor_when_switching_back_to_default_filter() {
    let _guard = test_lock();
    let tree = branching_filter_tree();
    let mut selector = selector(&tree, "asst-2");

    assert_eq!(selected_id(&selector).as_deref(), Some("asst-2"));

    // Switch to user-only
    selector.handle_input("\x15"); // Ctrl+U
    assert_eq!(selected_id(&selector).as_deref(), Some("user-2"));

    // Switch back to default - should stay on user-2
    // (since that's what we navigated to via parent traversal)
    selector.handle_input("\x04"); // Ctrl+D
    assert_eq!(selected_id(&selector).as_deref(), Some("user-2"));
}

// --- help ---------------------------------------------------------------------

#[test]
fn renders_semantic_help_rows_without_truncating_narrow_terminal_controls() {
    let _guard = test_lock();
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
    ]);
    let mut selector = selector(&tree, "asst-1");

    let plain_lines: Vec<String> = selector
        .render(30)
        .iter()
        .map(|line| strip_ansi(line))
        .collect();
    let plain = plain_lines.join("\n");
    assert!(plain.contains("branch"), "{plain}");
    assert!(plain.contains("copy"), "{plain}");
    assert!(plain.contains("filters"), "{plain}");
    assert!(plain.contains("cycle"), "{plain}");
    assert!(plain.contains("label time"), "{plain}");
    assert!(!plain.contains("..."), "{plain}");
    assert!(
        plain_lines.iter().all(|line| visible_width(line) <= 30),
        "{plain}"
    );
}

// --- copy ---------------------------------------------------------------------

#[test]
fn copies_the_full_selected_message_with_ctrl_x() {
    let _guard = test_lock();
    let message = format!("{}\nsecond line", "long message ".repeat(30));
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), &message),
    ]);
    let mut selector = selector(&tree, "asst-1");
    let copied: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&copied);
    selector.on_copy = Some(Box::new(move |text| {
        *sink.borrow_mut() = text.map(str::to_string);
    }));

    selector.handle_input("\x18");

    assert_eq!(copied.borrow().as_deref(), Some(message.as_str()));
}

// --- label timestamps ---------------------------------------------------------

#[test]
fn toggles_label_timestamps_for_labeled_nodes() {
    use chrono::TimeZone;

    let _guard = test_lock();
    let mut tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
    ]);
    // `new Date(2026, 2, 28, 14, 32, 0)` — local time, month is zero-based.
    let label_date = chrono::Local
        .with_ymd_and_hms(2026, 3, 28, 14, 32, 0)
        .single()
        .expect("local timestamp");
    tree[0].label = Some("checkpoint".to_string());
    tree[0].label_timestamp = Some(label_date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));

    let mut selector = selector(&tree, "asst-1");
    let tree_list = Rc::clone(selector.get_tree_list());

    let render = tree_list.borrow_mut().render(200).join("\n");
    assert!(render.contains("[checkpoint]"), "{render}");
    assert!(!render.contains("3/28 14:32"), "{render}");
    assert!(!render.contains("[+label time]"), "{render}");

    selector.handle_input("T");

    let render = tree_list.borrow_mut().render(200).join("\n");
    assert!(render.contains("3/28 14:32"), "{render}");
    assert!(render.contains("[+label time]"), "{render}");
}

// --- empty filter preservation ------------------------------------------------

#[test]
fn preserves_selection_when_switching_to_empty_labeled_filter_and_back() {
    let _guard = test_lock();
    // Tree with no labels
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "bye"),
        assistant_message("asst-2", Some("user-2"), "goodbye"),
    ]);
    let mut selector = selector(&tree, "asst-2");

    assert_eq!(selected_id(&selector).as_deref(), Some("asst-2"));

    // Switch to labeled-only filter (no labels exist, so empty result)
    selector.handle_input("\x0c"); // Ctrl+L

    // The list should be empty, getSelectedNode returns undefined
    assert_eq!(selected_id(&selector), None);

    // Switch back to default filter
    selector.handle_input("\x04"); // Ctrl+D

    // Should restore to asst-2 (the selection before we switched to empty filter)
    assert_eq!(selected_id(&selector).as_deref(), Some("asst-2"));
}

#[test]
fn preserves_selection_through_multiple_empty_filter_switches() {
    let _guard = test_lock();
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
    ]);
    let mut selector = selector(&tree, "asst-1");

    assert_eq!(selected_id(&selector).as_deref(), Some("asst-1"));

    // Switch to labeled-only (empty) - Ctrl+L toggles labeled ↔ default
    selector.handle_input("\x0c");
    assert_eq!(selected_id(&selector), None);

    // Switch to default, then back to labeled-only
    selector.handle_input("\x0c");
    assert_eq!(selected_id(&selector).as_deref(), Some("asst-1"));

    selector.handle_input("\x0c");
    assert_eq!(selected_id(&selector), None);

    // Switch back to default with Ctrl+D
    selector.handle_input("\x04");
    assert_eq!(selected_id(&selector).as_deref(), Some("asst-1"));
}

// --- branch navigation and folding with ctrl+arrow keys -----------------------

// Tree structure:
//
// user-1
// asst-1
// user-2
// asst-2          ← branch point (has 2 children)
// ├─ user-3a      ← branch A (active: leaf is asst-4a)
// │  asst-3a
// │  user-4a
// │  asst-4a
// └─ user-3b      ← branch B
//    asst-3b
//    user-4b
//
// Foldable nodes: user-1 (root), user-3a (segment start), user-3b (segment start)
fn build_branching_tree() -> Vec<SessionTreeNode> {
    build_tree(vec![
        user_message("user-1", None, "first message"),
        assistant_message("asst-1", Some("user-1"), "response 1"),
        user_message("user-2", Some("asst-1"), "second message"),
        assistant_message("asst-2", Some("user-2"), "response 2"),
        // Branch A (active)
        user_message("user-3a", Some("asst-2"), "branch A start"),
        assistant_message("asst-3a", Some("user-3a"), "branch A response"),
        user_message("user-4a", Some("asst-3a"), "branch A deep"),
        assistant_message("asst-4a", Some("user-4a"), "branch A leaf"),
        // Branch B
        user_message("user-3b", Some("asst-2"), "branch B start"),
        assistant_message("asst-3b", Some("user-3b"), "branch B response"),
        user_message("user-4b", Some("asst-3b"), "branch B deep"),
    ])
}

#[test]
fn ctrl_right_unfolds_a_folded_node_then_does_segment_jump_when_unfolded() {
    let _guard = test_lock();
    let tree = build_branching_tree();
    let mut selector = selector(&tree, "asst-4a");

    selector.handle_input(CTRL_LEFT); // asst-4a → user-3a
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3a"));

    selector.handle_input(CTRL_LEFT); // fold user-3a
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3a"));

    selector.handle_input(DOWN); // user-3a → user-3b (children hidden)
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3b"));

    selector.handle_input(UP); // user-3b → user-3a
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3a"));

    selector.handle_input(CTRL_RIGHT); // unfold user-3a
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3a"));

    selector.handle_input(DOWN); // user-3a → asst-3a (children restored)
    assert_eq!(selected_id(&selector).as_deref(), Some("asst-3a"));

    selector.handle_input(CTRL_LEFT); // asst-3a → user-3a
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3a"));

    selector.handle_input(CTRL_RIGHT); // user-3a → asst-4a (segment jump to leaf)
    assert_eq!(selected_id(&selector).as_deref(), Some("asst-4a"));
}

#[test]
fn alt_left_right_are_aliases_for_fold_and_unfold_navigation() {
    let _guard = test_lock();
    let tree = build_branching_tree();
    let mut selector = selector(&tree, "asst-4a");

    selector.handle_input(ALT_LEFT); // asst-4a → user-3a
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3a"));

    selector.handle_input(ALT_LEFT); // fold user-3a
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3a"));

    selector.handle_input(ALT_RIGHT); // unfold user-3a
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3a"));

    selector.handle_input(ALT_RIGHT); // user-3a → asst-4a
    assert_eq!(selected_id(&selector).as_deref(), Some("asst-4a"));
}

#[test]
fn folding_root_hides_entire_subtree_nested_fold_preserved_on_unfold() {
    let _guard = test_lock();
    let tree = build_branching_tree();
    let mut selector = selector(&tree, "asst-4a");

    selector.handle_input(CTRL_LEFT); // asst-4a → user-3a
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3a"));

    selector.handle_input(CTRL_LEFT); // fold user-3a
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3a"));

    selector.handle_input(CTRL_LEFT); // user-3a (folded) → user-1
    assert_eq!(selected_id(&selector).as_deref(), Some("user-1"));

    selector.handle_input(CTRL_LEFT); // fold user-1
    assert_eq!(selected_id(&selector).as_deref(), Some("user-1"));

    selector.handle_input(DOWN); // wrap (only visible node)
    assert_eq!(selected_id(&selector).as_deref(), Some("user-1"));

    selector.handle_input(CTRL_RIGHT); // unfold user-1
    assert_eq!(selected_id(&selector).as_deref(), Some("user-1"));

    selector.handle_input(CTRL_RIGHT); // user-1 → user-3a (segment jump, user-3a still folded)
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3a"));

    selector.handle_input(DOWN); // user-3a → user-3b (user-3a still folded)
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3b"));
}

#[test]
fn fold_and_navigate_on_non_active_branch() {
    let _guard = test_lock();
    let tree = build_branching_tree();
    let mut selector = selector(&tree, "asst-4a");

    // Navigate down to user-3b (branch B)
    let mut found = false;
    for _ in 0..20 {
        selector.handle_input(DOWN);
        if selected_id(&selector).as_deref() == Some("user-3b") {
            found = true;
            break;
        }
    }
    assert!(found);

    selector.handle_input(CTRL_RIGHT); // user-3b → user-4b (segment jump to leaf)
    assert_eq!(selected_id(&selector).as_deref(), Some("user-4b"));

    selector.handle_input(CTRL_LEFT); // user-4b → user-3b
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3b"));

    selector.handle_input(CTRL_LEFT); // fold user-3b
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3b"));

    selector.handle_input(CTRL_LEFT); // user-3b (folded) → user-1
    assert_eq!(selected_id(&selector).as_deref(), Some("user-1"));
}

#[test]
fn fold_and_navigate_with_multiple_roots() {
    let _guard = test_lock();
    let tree = build_tree(vec![
        user_message("user-1", None, "first root"),
        assistant_message("asst-1", Some("user-1"), "response 1"),
        user_message("user-2", None, "second root"),
        assistant_message("asst-2", Some("user-2"), "response 2"),
    ]);
    let mut selector = selector(&tree, "asst-1");

    assert_eq!(selected_id(&selector).as_deref(), Some("asst-1"));

    selector.handle_input(CTRL_LEFT); // asst-1 → user-1
    assert_eq!(selected_id(&selector).as_deref(), Some("user-1"));

    selector.handle_input(CTRL_LEFT); // fold user-1
    assert_eq!(selected_id(&selector).as_deref(), Some("user-1"));

    selector.handle_input(DOWN); // user-1 → user-2 (children hidden)
    assert_eq!(selected_id(&selector).as_deref(), Some("user-2"));

    selector.handle_input(CTRL_RIGHT); // user-2 → asst-2 (segment jump to leaf)
    assert_eq!(selected_id(&selector).as_deref(), Some("asst-2"));

    selector.handle_input(CTRL_LEFT); // asst-2 → user-2
    assert_eq!(selected_id(&selector).as_deref(), Some("user-2"));

    selector.handle_input(CTRL_LEFT); // fold user-2
    assert_eq!(selected_id(&selector).as_deref(), Some("user-2"));

    selector.handle_input(CTRL_LEFT); // user-2 (folded, root) → stays on user-2
    assert_eq!(selected_id(&selector).as_deref(), Some("user-2"));
}

#[test]
fn folding_root_hides_descendants_even_when_intermediate_nodes_are_filtered_out() {
    let _guard = test_lock();
    // user-1 → toolCallOnly-1 (filtered out) → user-2 → asst-2
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        tool_call_only_assistant("tool-asst-1", Some("user-1")),
        user_message("user-2", Some("tool-asst-1"), "follow up"),
        assistant_message("asst-2", Some("user-2"), "response"),
    ]);
    let mut selector = selector(&tree, "asst-2");

    selector.handle_input(CTRL_LEFT); // asst-2 → user-1
    assert_eq!(selected_id(&selector).as_deref(), Some("user-1"));

    selector.handle_input(CTRL_LEFT); // fold user-1
    assert_eq!(selected_id(&selector).as_deref(), Some("user-1"));

    selector.handle_input(DOWN); // wrap (only visible node)
    assert_eq!(selected_id(&selector).as_deref(), Some("user-1"));
}

#[test]
fn search_resets_fold_state() {
    let _guard = test_lock();
    let tree = build_branching_tree();
    let mut selector = selector(&tree, "asst-4a");

    selector.handle_input(CTRL_LEFT); // asst-4a → user-3a
    selector.handle_input(CTRL_LEFT); // fold user-3a

    selector.handle_input(DOWN); // user-3a → user-3b (children hidden)
    assert_eq!(selected_id(&selector).as_deref(), Some("user-3b"));

    selector.handle_input("b"); // search resets folds
    selector.handle_input("\x1b"); // clear search

    // Navigate to user-3a to verify fold was reset
    let mut current_id = String::new();
    for _ in 0..20 {
        selector.handle_input(DOWN);
        current_id = selected_id(&selector).unwrap_or_default();
        if current_id == "user-3a" {
            break;
        }
    }
    assert_eq!(current_id, "user-3a");

    selector.handle_input(DOWN); // user-3a → asst-3a (not user-3b)
    assert_eq!(selected_id(&selector).as_deref(), Some("asst-3a"));
}

#[test]
fn filter_mode_change_resets_fold_state() {
    let _guard = test_lock();
    let tree = build_branching_tree();
    let mut selector = selector(&tree, "asst-4a");

    selector.handle_input(CTRL_LEFT); // asst-4a → user-3a
    selector.handle_input(CTRL_LEFT); // fold user-3a

    selector.handle_input("\x15"); // ctrl+u: user-only filter resets folds
    selector.handle_input("\x04"); // ctrl+d: back to default

    // Navigate to user-3a to verify fold was reset
    let mut current_id = String::new();
    for _ in 0..20 {
        selector.handle_input(DOWN);
        current_id = selected_id(&selector).unwrap_or_default();
        if current_id == "user-3a" {
            break;
        }
    }
    assert_eq!(current_id, "user-3a");

    selector.handle_input(DOWN); // user-3a → asst-3a (not user-3b)
    assert_eq!(selected_id(&selector).as_deref(), Some("asst-3a"));
}
