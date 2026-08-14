//! Behaviour of the two dialogs that workstream C's task 9 unblocked
//! (`components/approval-selector.ts`, `components/trust-selector.ts`).
//! Neither has a TypeScript suite, so the expectations come from the sources.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::permissions::request::{ApprovalAnswer, ApprovalRequest};
use notagent::core::trust_manager::ProjectTrustStoreEntry;
use notagent::modes::interactive::components::approval_selector::ApprovalSelectorComponent;
use notagent::modes::interactive::components::trust_selector::{
    TrustSelection, TrustSelectorComponent, TrustSelectorOptions,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::tui::Component;

use notagent::core::keybindings::KeybindingsManager;

/// The theme and the keybindings registry are process globals.
fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    init_theme(Some("dark"), false);
    set_keybindings(KeybindingsManager::default().to_tui());
    guard
}

const ENTER: &str = "\r";
const ESCAPE: &str = "\x1b";
const ARROW_DOWN: &str = "\x1b[B";
const ARROW_UP: &str = "\x1b[A";

fn request() -> ApprovalRequest {
    ApprovalRequest {
        tool_name: "write".to_string(),
        target: Some("src/main.rs".to_string()),
        policy_name: "deny-writes".to_string(),
        reason: Some("outside the workspace".to_string()),
        mode_id: Some("plan".to_string()),
    }
}

// --- approval selector ------------------------------------------------------------

#[test]
fn shows_the_deciding_policy_and_the_active_mode() {
    let _guard = test_lock();
    let mut selector = ApprovalSelectorComponent::new(&request(), Box::new(|_| {}));

    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("Permission required"), "{rendered}");
    assert!(rendered.contains("write"), "{rendered}");
    assert!(rendered.contains("deny-writes"), "{rendered}");
    assert!(rendered.contains("mode: plan"), "{rendered}");
    // The three answers, in dialog order.
    assert!(rendered.contains("Allow once"), "{rendered}");
    assert!(rendered.contains("Allow for this session"), "{rendered}");
    assert!(rendered.contains("Deny"), "{rendered}");
}

#[test]
fn omits_the_mode_row_without_a_mode() {
    let _guard = test_lock();
    let mut selector = ApprovalSelectorComponent::new(
        &ApprovalRequest {
            mode_id: None,
            ..request()
        },
        Box::new(|_| {}),
    );

    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(!rendered.contains("mode:"), "{rendered}");
}

#[test]
fn answers_with_the_confirmed_entry() {
    let _guard = test_lock();
    let answers: Rc<RefCell<Vec<ApprovalAnswer>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&answers);
    let mut selector = ApprovalSelectorComponent::new(
        &request(),
        Box::new(move |answer| sink.borrow_mut().push(answer)),
    );

    selector.handle_input(ARROW_DOWN);
    selector.handle_input(ENTER);

    assert_eq!(*answers.borrow(), [ApprovalAnswer::ApproveAlways]);
}

#[test]
fn a_dismissed_prompt_denies_rather_than_answering_nothing() {
    let _guard = test_lock();
    let answers: Rc<RefCell<Vec<ApprovalAnswer>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&answers);
    let mut selector = ApprovalSelectorComponent::new(
        &request(),
        Box::new(move |answer| sink.borrow_mut().push(answer)),
    );

    selector.handle_input(ESCAPE);

    assert_eq!(*answers.borrow(), [ApprovalAnswer::Deny]);
}

// --- trust selector ---------------------------------------------------------------

fn trust_selector(
    saved_decision: Option<ProjectTrustStoreEntry>,
    project_trusted: bool,
    selections: &Rc<RefCell<Vec<TrustSelection>>>,
    cancels: &Rc<RefCell<usize>>,
) -> TrustSelectorComponent {
    let sink = Rc::clone(selections);
    let cancel_sink = Rc::clone(cancels);
    TrustSelectorComponent::new(TrustSelectorOptions {
        cwd: "/tmp/notagent-trust-project".to_string(),
        saved_decision,
        project_trusted,
        on_select: Box::new(move |selection| sink.borrow_mut().push(selection)),
        on_cancel: Box::new(move || *cancel_sink.borrow_mut() += 1),
    })
}

#[test]
fn reports_no_saved_decision_and_the_session_state() {
    let _guard = test_lock();
    let selections = Rc::new(RefCell::new(Vec::new()));
    let cancels = Rc::new(RefCell::new(0));
    let mut selector = trust_selector(None, false, &selections, &cancels);

    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("Project trust"), "{rendered}");
    assert!(
        rendered.contains("/tmp/notagent-trust-project"),
        "{rendered}"
    );
    assert!(rendered.contains("Saved decision: none"), "{rendered}");
    assert!(
        rendered.contains("Current session: untrusted"),
        "{rendered}"
    );
    // Without a saved decision the cursor starts on the first option.
    assert!(rendered.contains("→ Trust"), "{rendered}");
    assert!(!rendered.contains('✓'), "{rendered}");
}

#[test]
fn marks_and_preselects_the_saved_option() {
    let _guard = test_lock();
    let selections = Rc::new(RefCell::new(Vec::new()));
    let cancels = Rc::new(RefCell::new(0));
    let saved = ProjectTrustStoreEntry {
        path: "/tmp/notagent-trust-project".to_string(),
        decision: true,
    };
    let mut selector = trust_selector(Some(saved), true, &selections, &cancels);

    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("Current session: trusted"), "{rendered}");
    assert!(
        rendered.contains("trusted (/tmp/notagent-trust-project)"),
        "{rendered}"
    );
    assert!(rendered.contains("→ Trust ✓"), "{rendered}");
}

#[test]
fn confirms_the_selected_option_and_cancels_on_escape() {
    let _guard = test_lock();
    let selections: Rc<RefCell<Vec<TrustSelection>>> = Rc::new(RefCell::new(Vec::new()));
    let cancels = Rc::new(RefCell::new(0));
    let mut selector = trust_selector(None, false, &selections, &cancels);

    // `j`/`k` move like the arrow keys.
    selector.handle_input("j");
    selector.handle_input(ARROW_UP);
    selector.handle_input(ENTER);
    assert_eq!(selections.borrow().len(), 1);
    assert!(selections.borrow()[0].trusted, "the first option trusts");
    assert!(!selections.borrow()[0].updates.is_empty());

    selector.handle_input(ESCAPE);
    assert_eq!(*cancels.borrow(), 1);
}
