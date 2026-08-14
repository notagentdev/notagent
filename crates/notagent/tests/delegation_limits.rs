//! Port of `packages/coding-agent/test/delegation-limits.test.ts`.
//!
//! What a single delegation call may ask for.
//!
//! The per-turn ceilings this file used to assert are gone: they came from a
//! different reference, and with recursion structurally impossible the thing
//! they guarded against — a tree of subagents multiplying out of one request —
//! cannot arise. What is left catches a model repeating itself inside one call,
//! and asking for more children than any answer could use.

use notagent::core::delegation::limits::{
    DelegationRefusal, MAX_DELEGATIONS_PER_CALL, check_delegation_request, delegation_fingerprint,
};

fn tasks(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("task number {index}"))
        .collect()
}

fn one(task: &str) -> Vec<String> {
    vec![task.to_owned()]
}

// ── the per-call ceiling ──────────────────────────────────────────────

#[test]
fn matches_the_reference() {
    assert_eq!(MAX_DELEGATIONS_PER_CALL, 8);
}

// ── the fingerprint ───────────────────────────────────────────────────

#[test]
fn collapses_whitespace_trailing_punctuation_and_case() {
    assert_eq!(
        delegation_fingerprint("worker", "Inspect the auth module."),
        delegation_fingerprint("worker", "inspect  the   auth module")
    );
}

#[test]
fn keeps_the_mode_part_of_the_identity() {
    assert_ne!(
        delegation_fingerprint("plan", "read x"),
        delegation_fingerprint("worker", "read x")
    );
}

#[test]
fn does_not_collapse_different_tasks() {
    assert_ne!(
        delegation_fingerprint("worker", "read a"),
        delegation_fingerprint("worker", "read b")
    );
}

#[test]
fn is_empty_for_a_task_that_normalizes_to_nothing() {
    assert_eq!(delegation_fingerprint("worker", "   "), "");
    assert_eq!(delegation_fingerprint("worker", "..."), "");
}

#[test]
fn strips_only_trailing_punctuation_not_punctuation_inside() {
    assert_eq!(
        delegation_fingerprint("worker", "read a.ts"),
        delegation_fingerprint("worker", "read a.ts!!")
    );
    assert_ne!(
        delegation_fingerprint("worker", "read a.ts"),
        delegation_fingerprint("worker", "read ats")
    );
}

// ── checking a request ────────────────────────────────────────────────

#[test]
fn accepts_an_ordinary_one() {
    assert!(check_delegation_request("worker", &one("do the thing")).is_ok());
}

#[test]
fn refuses_an_empty_request() {
    assert!(check_delegation_request("worker", &[]).is_err());
    assert!(check_delegation_request("worker", &one("  ")).is_err());
}

#[test]
fn refuses_more_tasks_than_one_call_may_carry() {
    assert!(check_delegation_request("worker", &tasks(MAX_DELEGATIONS_PER_CALL)).is_ok());
    assert!(check_delegation_request("worker", &tasks(MAX_DELEGATIONS_PER_CALL + 1)).is_err());
}

#[test]
fn refuses_the_same_task_twice_in_one_call() {
    assert!(
        check_delegation_request(
            "worker",
            &["read the parser".to_owned(), "read  the parser.".to_owned()]
        )
        .is_err()
    );
}

#[test]
fn carries_the_reason_so_the_model_can_adapt() {
    let error = check_delegation_request("worker", &tasks(MAX_DELEGATIONS_PER_CALL + 1))
        .expect_err("should have refused");
    assert_eq!(error.refusal, DelegationRefusal::PerCall { requested: 9 });
    assert!(
        error
            .message()
            .contains(&MAX_DELEGATIONS_PER_CALL.to_string()),
        "{}",
        error.message()
    );
}

#[test]
fn no_longer_counts_across_calls() {
    // The same task twice in two separate calls is a repeat a human might
    // deliberately ask for — a re-run after a change, say. Only a repeat inside
    // one call is unambiguously the model looping.
    check_delegation_request("worker", &one("inspect the parser")).expect("accepted");
    assert!(check_delegation_request("worker", &one("inspect the parser")).is_ok());
}
