//! Port of `packages/coding-agent/test/session-selector-path-delete.test.ts` (354 LOC)
//! and `packages/coding-agent/test/session-selector-rename.test.ts` (111 LOC).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::keybindings::KeybindingsManager;
use notagent::core::session_manager::SessionInfo;
use notagent::modes::interactive::components::session_selector::{
    SessionScope, SessionSelectorComponent, SessionSelectorOptions,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::tui::Component;

/// The theme and the keybindings registry are process globals.
fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // session selector uses the global theme instance
    init_theme(Some("dark"), false);
    // Ensure test isolation: keybindings are a global singleton
    set_keybindings(KeybindingsManager::default().to_tui());
    guard
}

const CTRL_D: &str = "\x04";
const CTRL_BACKSPACE: &str = "\x1b[127;5u";
/// Kitty keyboard protocol encoding for Ctrl+R
const CTRL_R: &str = "\x1b[114;5u";
const TAB: &str = "\t";
const ENTER: &str = "\r";

fn make_session(id: &str) -> SessionInfo {
    SessionInfo {
        path: format!("/tmp/{id}.jsonl"),
        id: id.to_string(),
        cwd: String::new(),
        name: None,
        parent_session_path: None,
        created: 0,
        modified: 0,
        message_count: 1,
        first_message: "hello".to_string(),
        all_messages_text: "hello".to_string(),
    }
}

fn keybindings() -> Rc<RefCell<KeybindingsManager>> {
    Rc::new(RefCell::new(KeybindingsManager::default()))
}

/// Builds a selector and settles the initial `current` load, which is what
/// `await flushPromises()` does in the TypeScript suites.
fn selector_with(
    sessions: Vec<SessionInfo>,
    options: SessionSelectorOptions,
    current_session_file_path: Option<&str>,
) -> SessionSelectorComponent {
    let mut selector = SessionSelectorComponent::new(
        Box::new(|_| {}),
        Box::new(|| {}),
        Box::new(|| {}),
        Rc::new(|| {}),
        options,
        current_session_file_path,
    );
    let request = selector
        .take_pending_load()
        .expect("the constructor starts the current load");
    assert_eq!(request.scope, SessionScope::Current);
    selector.apply_load_result(request, Ok(sessions));
    selector
}

fn options() -> SessionSelectorOptions {
    SessionSelectorOptions {
        keybindings: Some(keybindings()),
        ..Default::default()
    }
}

// --- path / delete interactions -------------------------------------------------

#[test]
fn does_not_treat_ctrl_backspace_as_delete_when_search_query_is_non_empty() {
    let _guard = test_lock();
    let sessions = vec![make_session("a"), make_session("b")];
    let mut selector = selector_with(sessions, options(), None);

    let confirmation_changes: Rc<RefCell<Vec<Option<String>>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&confirmation_changes);
    selector
        .get_session_list()
        .borrow_mut()
        .on_delete_confirmation_change = Some(Box::new(move |path| {
        sink.borrow_mut().push(path.map(str::to_string));
    }));

    selector.handle_input("a");
    selector.handle_input(CTRL_BACKSPACE);

    assert!(confirmation_changes.borrow().is_empty());
}

#[test]
fn enters_confirmation_mode_on_ctrl_d_even_with_a_non_empty_search_query() {
    let _guard = test_lock();
    let sessions = vec![make_session("a"), make_session("b")];
    let first_path = sessions[0].path.clone();
    let mut selector = selector_with(sessions, options(), None);

    let confirmation_changes: Rc<RefCell<Vec<Option<String>>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&confirmation_changes);
    selector
        .get_session_list()
        .borrow_mut()
        .on_delete_confirmation_change = Some(Box::new(move |path| {
        sink.borrow_mut().push(path.map(str::to_string));
    }));

    selector.handle_input("a");
    selector.handle_input(CTRL_D);

    assert_eq!(*confirmation_changes.borrow(), [Some(first_path)]);
}

#[test]
fn enters_confirmation_mode_on_ctrl_backspace_when_search_query_is_empty() {
    let _guard = test_lock();
    let sessions = vec![make_session("a"), make_session("b")];
    let first_path = sessions[0].path.clone();
    let mut selector = selector_with(sessions, options(), None);

    let confirmation_changes: Rc<RefCell<Vec<Option<String>>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&confirmation_changes);
    let deleted_path: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let deleted_sink = Rc::clone(&deleted_path);
    {
        let mut list = selector.get_session_list().borrow_mut();
        list.on_delete_confirmation_change = Some(Box::new(move |path| {
            sink.borrow_mut().push(path.map(str::to_string));
        }));
        list.on_delete_session = Some(Box::new(move |session_path| {
            *deleted_sink.borrow_mut() = Some(session_path.to_string());
        }));
    }

    selector.handle_input(CTRL_BACKSPACE);
    assert_eq!(*confirmation_changes.borrow(), [Some(first_path.clone())]);

    selector.handle_input(ENTER);
    assert_eq!(
        *confirmation_changes.borrow(),
        [Some(first_path.clone()), None]
    );
    assert_eq!(deleted_path.borrow().as_deref(), Some(first_path.as_str()));
}

#[test]
fn does_not_switch_scope_back_to_all_when_all_load_resolves_after_toggling_back_to_current() {
    let _guard = test_lock();
    let mut selector = selector_with(vec![make_session("current")], options(), None);

    selector.handle_input(TAB); // current -> all (starts async load)
    let all_request = selector
        .take_pending_load()
        .expect("the toggle starts the all load");
    assert_eq!(all_request.scope, SessionScope::All);

    selector.handle_input(TAB); // all -> current

    // The load resolves only now, after the scope moved back.
    selector.apply_load_result(all_request, Ok(vec![make_session("all")]));

    assert!(
        selector.take_pending_load().is_none(),
        "no second all load was started"
    );
    let output = selector.render(120).join("\n");
    assert!(
        output.contains("Resume Session (Current Folder)"),
        "{output}"
    );
    assert!(!output.contains("Resume Session (All)"), "{output}");
}

#[test]
fn does_not_start_redundant_all_loads_when_toggling_scopes_while_all_is_already_loading() {
    let _guard = test_lock();
    let mut selector = selector_with(vec![make_session("current")], options(), None);

    selector.handle_input(TAB); // current -> all (starts async load)
    let all_request = selector
        .take_pending_load()
        .expect("the toggle starts the all load");
    selector.handle_input(TAB); // all -> current
    selector.handle_input(TAB); // current -> all again while load pending

    assert!(
        selector.take_pending_load().is_none(),
        "the pending load is not started twice"
    );

    selector.apply_load_result(all_request, Ok(vec![make_session("all")]));
}

/// `createSymlinkedSessionPaths()`
struct SymlinkedSessionPaths {
    _base_dir: tempfile::TempDir,
    parent_alias_a: String,
    parent_alias_b: String,
    child_alias_b: String,
}

fn create_symlinked_session_paths() -> SymlinkedSessionPaths {
    let base_dir = tempfile::Builder::new()
        .prefix("notagent-session-selector-")
        .tempdir()
        .expect("temp dir");
    let root = base_dir.path();
    let real_dir = root.join("real");
    let alias_a_dir = root.join("alias-a");
    let alias_b_dir = root.join("alias-b");
    std::fs::create_dir_all(&real_dir).expect("creates");
    std::fs::create_dir_all(&alias_a_dir).expect("creates");
    std::fs::create_dir_all(&alias_b_dir).expect("creates");

    let shared_dir = real_dir.join("sessions");
    std::fs::create_dir_all(&shared_dir).expect("creates");
    let alias_a_sessions = alias_a_dir.join("sessions");
    let alias_b_sessions = alias_b_dir.join("sessions");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&shared_dir, &alias_a_sessions).expect("symlink");
        std::os::unix::fs::symlink(&shared_dir, &alias_b_sessions).expect("symlink");
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(&shared_dir, &alias_a_sessions).expect("symlink");
        std::os::windows::fs::symlink_dir(&shared_dir, &alias_b_sessions).expect("symlink");
    }

    std::fs::write(shared_dir.join("parent.jsonl"), "parent\n").expect("writes");
    std::fs::write(shared_dir.join("child.jsonl"), "child\n").expect("writes");

    SymlinkedSessionPaths {
        parent_alias_a: alias_a_sessions
            .join("parent.jsonl")
            .to_string_lossy()
            .into_owned(),
        parent_alias_b: alias_b_sessions
            .join("parent.jsonl")
            .to_string_lossy()
            .into_owned(),
        child_alias_b: alias_b_sessions
            .join("child.jsonl")
            .to_string_lossy()
            .into_owned(),
        _base_dir: base_dir,
    }
}

#[test]
fn threads_sessions_when_parent_and_child_paths_use_different_symlink_aliases() {
    let _guard = test_lock();
    let paths = create_symlinked_session_paths();

    let sessions = vec![
        SessionInfo {
            path: paths.parent_alias_b.clone(),
            name: Some("Parent".to_string()),
            // `new Date("2026-01-01T00:00:00.000Z")`
            modified: 1_767_225_600_000,
            ..make_session("parent")
        },
        SessionInfo {
            path: paths.child_alias_b.clone(),
            parent_session_path: Some(paths.parent_alias_a.clone()),
            name: Some("Child".to_string()),
            // `new Date("2025-12-31T00:00:00.000Z")`
            modified: 1_767_139_200_000,
            ..make_session("child")
        },
    ];

    let mut selector = selector_with(sessions, options(), None);

    let output = strip_ansi(&selector.render(120).join("\n"));
    assert!(output.contains("Parent"), "{output}");
    assert!(output.contains("└─ Child"), "{output}");
}

#[test]
fn sorts_threaded_sessions_by_latest_activity_in_their_subtree() {
    let _guard = test_lock();
    let parent_one = SessionInfo {
        name: Some("Parent one".to_string()),
        modified: 1_767_312_000_000, // 2026-01-02
        ..make_session("parent-one")
    };
    let parent_two = SessionInfo {
        name: Some("Parent two".to_string()),
        modified: 1_767_225_600_000, // 2026-01-01
        ..make_session("parent-two")
    };
    let child_two = SessionInfo {
        name: Some("Child two".to_string()),
        parent_session_path: Some(parent_two.path.clone()),
        modified: 1_767_398_400_000, // 2026-01-03
        ..make_session("child-two")
    };

    let mut selector = selector_with(vec![parent_one, parent_two, child_two], options(), None);

    let output = strip_ansi(&selector.render(120).join("\n"));
    let parent_two_index = output.find("Parent two").expect("parent two is listed");
    let child_two_index = output.find("└─ Child two").expect("child two is listed");
    let parent_one_index = output.find("Parent one").expect("parent one is listed");

    assert!(child_two_index > parent_two_index, "{output}");
    assert!(parent_one_index > child_two_index, "{output}");
}

#[test]
fn treats_the_current_session_as_active_across_symlink_aliases() {
    let _guard = test_lock();
    let paths = create_symlinked_session_paths();

    let sessions = vec![SessionInfo {
        path: paths.parent_alias_b.clone(),
        name: Some("Parent".to_string()),
        ..make_session("parent")
    }];
    let mut selector = selector_with(sessions, options(), Some(&paths.parent_alias_a));

    let confirmation_changes: Rc<RefCell<Vec<Option<String>>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&confirmation_changes);
    let error_message: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let error_sink = Rc::clone(&error_message);
    {
        let mut list = selector.get_session_list().borrow_mut();
        list.on_delete_confirmation_change = Some(Box::new(move |path| {
            sink.borrow_mut().push(path.map(str::to_string));
        }));
        list.on_error = Some(Box::new(move |message| {
            *error_sink.borrow_mut() = Some(message.to_string());
        }));
    }

    selector.handle_input(CTRL_D);

    assert!(confirmation_changes.borrow().is_empty());
    assert_eq!(
        error_message.borrow().as_deref(),
        Some("Cannot delete the currently active session")
    );
}

// --- rename -----------------------------------------------------------------------

#[test]
fn shows_rename_hint_in_interactive_resume_picker_configuration() {
    let _guard = test_lock();
    let mut selector = selector_with(
        vec![make_session("a")],
        SessionSelectorOptions {
            show_rename_hint: Some(true),
            keybindings: Some(keybindings()),
            ..Default::default()
        },
        None,
    );

    let output = selector.render(120).join("\n");
    assert!(output.contains("ctrl+r"), "{output}");
    assert!(output.contains("rename"), "{output}");
}

#[test]
fn does_not_show_rename_hint_in_resume_flag_picker_configuration() {
    let _guard = test_lock();
    let mut selector = selector_with(
        vec![make_session("a")],
        SessionSelectorOptions {
            show_rename_hint: Some(false),
            keybindings: Some(keybindings()),
            ..Default::default()
        },
        None,
    );

    let output = selector.render(120).join("\n");
    assert!(!output.contains("ctrl+r"), "{output}");
    assert!(!output.contains("rename"), "{output}");
}

#[test]
fn enters_rename_mode_on_ctrl_r_and_submits_with_enter() {
    let _guard = test_lock();
    let session = SessionInfo {
        name: Some("Old".to_string()),
        ..make_session("a")
    };
    let session_path = session.path.clone();
    let renames: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&renames);

    let mut selector = selector_with(
        vec![session],
        SessionSelectorOptions {
            rename_session: Some(Box::new(move |path, name| {
                sink.borrow_mut().push((path.to_string(), name.to_string()));
            })),
            show_rename_hint: Some(true),
            keybindings: Some(keybindings()),
        },
        None,
    );

    selector.handle_input(CTRL_R);

    // Rename mode layout
    let output = selector.render(120).join("\n");
    assert!(output.contains("Rename Session"), "{output}");
    assert!(!output.contains("Resume Session"), "{output}");

    // Type and submit
    selector.handle_input("X");
    selector.handle_input(ENTER);

    assert_eq!(*renames.borrow(), [(session_path, "XOld".to_string())]);
}
