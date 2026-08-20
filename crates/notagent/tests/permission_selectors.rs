//! Port of `packages/coding-agent/test/approval-selector.test.ts` (75 LOC) and
//! `packages/coding-agent/test/trust-selector.test.ts` (87 LOC), plus the
//! behaviour of both dialogs those suites do not reach.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::permissions::request::{ApprovalAnswer, ApprovalRequest};
use notagent::core::trust_manager::{ProjectTrustStoreEntry, ProjectTrustUpdate};
use notagent::modes::interactive::components::approval_selector::{
    ApprovalSelectorComponent, create_pending_approval,
};
use notagent::modes::interactive::components::trust_selector::{
    TrustSelection, TrustSelectorComponent, TrustSelectorOptions,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_tui::components::select_list::SelectItem;
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
        target: Some(".env".to_string()),
        policy_name: "sensitive-file-access-ask".to_string(),
        reason: Some("looks like a credentials file".to_string()),
        mode_id: Some("manual".to_string()),
        requester: None,
    }
}

/// `answered(component, index)` of the TypeScript suite: drives the list the
/// way the TUI would, without a terminal.
fn answered(component: &ApprovalSelectorComponent, index: usize) {
    let mut list = component.get_select_list().borrow_mut();
    list.set_selected_index(index);
    let item = SelectItem {
        value: ["approve-once", "approve-always", "deny"][index].to_string(),
        label: String::new(),
        description: None,
    };
    if let Some(on_select) = list.on_select.as_mut() {
        on_select(&item);
    }
}

// --- approval selector ------------------------------------------------------------

#[test]
fn renders_the_tool_the_target_the_reason_and_the_deciding_policy() {
    let _guard = test_lock();
    let mut component = ApprovalSelectorComponent::new(&request(), Box::new(|_| {}));

    let rendered = component.render(100).join("\n");
    assert!(rendered.contains("write .env"), "{rendered}");
    assert!(
        rendered.contains("looks like a credentials file"),
        "{rendered}"
    );
    assert!(rendered.contains("sensitive-file-access-ask"), "{rendered}");
}

#[test]
fn shows_the_active_mode_so_the_prompt_is_legible_in_context() {
    let _guard = test_lock();
    let mut component = ApprovalSelectorComponent::new(&request(), Box::new(|_| {}));
    assert!(component.render(100).join("\n").contains("mode: manual"));
}

#[test]
fn offers_exactly_the_three_answers() {
    let _guard = test_lock();
    let mut component = ApprovalSelectorComponent::new(&request(), Box::new(|_| {}));

    let rendered = component.render(100).join("\n");
    assert!(rendered.contains("Allow once"), "{rendered}");
    assert!(rendered.contains("Allow for this session"), "{rendered}");
    assert!(rendered.contains("Deny"), "{rendered}");
}

#[test]
fn reports_the_chosen_answer() {
    let _guard = test_lock();
    let answers: Rc<RefCell<Vec<ApprovalAnswer>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&answers);
    let component = ApprovalSelectorComponent::new(
        &request(),
        Box::new(move |answer| sink.borrow_mut().push(answer)),
    );

    answered(&component, 0);

    assert_eq!(*answers.borrow(), [ApprovalAnswer::ApproveOnce]);
}

#[test]
fn treats_cancelling_as_a_denial_never_as_consent() {
    let _guard = test_lock();
    let answers: Rc<RefCell<Vec<ApprovalAnswer>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&answers);
    let component = ApprovalSelectorComponent::new(
        &request(),
        Box::new(move |answer| sink.borrow_mut().push(answer)),
    );

    {
        let mut list = component.get_select_list().borrow_mut();
        if let Some(on_cancel) = list.on_cancel.as_mut() {
            on_cancel();
        }
    }

    assert_eq!(*answers.borrow(), [ApprovalAnswer::Deny]);
}

// --- pending approval ---------------------------------------------------------------

#[tokio::test]
async fn settles_with_the_answer_the_dialog_reports() {
    let mut pending = create_pending_approval();
    pending.resolve(ApprovalAnswer::ApproveAlways);
    assert_eq!(pending.answer.await, Ok(ApprovalAnswer::ApproveAlways));
}

#[tokio::test]
async fn ignores_a_second_answer_instead_of_failing() {
    let mut pending = create_pending_approval();
    pending.resolve(ApprovalAnswer::ApproveOnce);
    // The TypeScript expectation is `not.toThrow()`; the port drops the second
    // answer just as `settle = undefined` does.
    pending.resolve(ApprovalAnswer::Deny);
    assert_eq!(pending.answer.await, Ok(ApprovalAnswer::ApproveOnce));
}

// --- behaviour the TypeScript suite does not cover ------------------------------------

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
fn answers_with_the_entry_confirmed_through_the_keyboard() {
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
    cwd: &str,
    saved_decision: Option<ProjectTrustStoreEntry>,
    project_trusted: bool,
    selections: &Rc<RefCell<Vec<TrustSelection>>>,
    cancels: &Rc<RefCell<usize>>,
) -> TrustSelectorComponent {
    let sink = Rc::clone(selections);
    let cancel_sink = Rc::clone(cancels);
    TrustSelectorComponent::new(TrustSelectorOptions {
        cwd: cwd.to_string(),
        saved_decision,
        project_trusted,
        on_select: Box::new(move |selection| sink.borrow_mut().push(selection)),
        on_cancel: Box::new(move || *cancel_sink.borrow_mut() += 1),
    })
}

#[test]
fn marks_the_saved_trusted_decision() {
    let _guard = test_lock();
    let selections = Rc::new(RefCell::new(Vec::new()));
    let cancels = Rc::new(RefCell::new(0));
    let mut selector = trust_selector(
        "/project",
        Some(ProjectTrustStoreEntry {
            path: "/project".to_string(),
            decision: true,
        }),
        true,
        &selections,
        &cancels,
    );

    let output = strip_ansi(&selector.render(120).join("\n"));

    assert!(
        output.contains("Saved decision: trusted (/project)"),
        "{output}"
    );
    assert!(output.contains("Current session: trusted"), "{output}");
    assert!(output.contains("Trust ✓"), "{output}");
    assert!(!output.contains("Do not trust ✓"), "{output}");
}

#[test]
fn selects_a_trust_decision() {
    let _guard = test_lock();
    let selections: Rc<RefCell<Vec<TrustSelection>>> = Rc::new(RefCell::new(Vec::new()));
    let cancels = Rc::new(RefCell::new(0));
    let mut selector = trust_selector("/project", None, false, &selections, &cancels);

    selector.handle_input("\n");

    assert_eq!(
        *selections.borrow(),
        [TrustSelection {
            trusted: true,
            updates: vec![ProjectTrustUpdate {
                path: "/project".to_string(),
                decision: Some(true),
            }],
        }]
    );
}

#[test]
fn labels_saved_ancestor_decisions_as_inherited() {
    let _guard = test_lock();
    let selections = Rc::new(RefCell::new(Vec::new()));
    let cancels = Rc::new(RefCell::new(0));
    let mut selector = trust_selector(
        "/parent/project/nested",
        Some(ProjectTrustStoreEntry {
            path: "/parent".to_string(),
            decision: true,
        }),
        true,
        &selections,
        &cancels,
    );

    let output = strip_ansi(&selector.render(120).join("\n"));

    assert!(
        output.contains("Saved decision: trusted (inherited from /parent)"),
        "{output}"
    );
}

#[test]
fn adds_a_trust_parent_option() {
    let _guard = test_lock();
    let selections: Rc<RefCell<Vec<TrustSelection>>> = Rc::new(RefCell::new(Vec::new()));
    let cancels = Rc::new(RefCell::new(0));
    let mut selector = trust_selector(
        "/parent/project",
        Some(ProjectTrustStoreEntry {
            path: "/parent".to_string(),
            decision: true,
        }),
        true,
        &selections,
        &cancels,
    );

    let output = strip_ansi(&selector.render(120).join("\n"));
    assert!(
        output.contains("Saved decision: trusted (inherited from /parent)"),
        "{output}"
    );
    assert!(
        output.contains("Trust parent folder (/parent) ✓"),
        "{output}"
    );

    selector.handle_input("\n");

    assert_eq!(
        *selections.borrow(),
        [TrustSelection {
            trusted: true,
            updates: vec![
                ProjectTrustUpdate {
                    path: "/parent".to_string(),
                    decision: Some(true),
                },
                ProjectTrustUpdate {
                    path: "/parent/project".to_string(),
                    decision: None,
                },
            ],
        }]
    );
}

// --- behaviour the TypeScript suite does not cover ------------------------------------

#[test]
fn reports_no_saved_decision_and_the_session_state() {
    let _guard = test_lock();
    let selections = Rc::new(RefCell::new(Vec::new()));
    let cancels = Rc::new(RefCell::new(0));
    let mut selector = trust_selector("/project", None, false, &selections, &cancels);

    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("Project trust"), "{rendered}");
    assert!(rendered.contains("/project"), "{rendered}");
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
fn moves_with_j_and_k_and_cancels_on_escape() {
    let _guard = test_lock();
    let selections: Rc<RefCell<Vec<TrustSelection>>> = Rc::new(RefCell::new(Vec::new()));
    let cancels = Rc::new(RefCell::new(0));
    let mut selector = trust_selector("/project", None, false, &selections, &cancels);

    // `j`/`k` move like the arrow keys; the cursor cannot leave the list.
    // `/project` sits directly under the root, so the second option is the
    // parent-folder one rather than "Do not trust".
    selector.handle_input("j");
    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("→ Trust parent folder (/)"), "{rendered}");

    selector.handle_input("k");
    selector.handle_input(ARROW_UP);
    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("→ Trust"), "{rendered}");

    selector.handle_input(ESCAPE);
    assert_eq!(*cancels.borrow(), 1);
    assert!(selections.borrow().is_empty());
}
