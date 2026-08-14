//! Port of `packages/coding-agent/src/core/permissions/chain.ts`.
//!
//! The policy chain and the mode-driven policies in it.
//!
//! The order below is the safety logic. Three positions carry guarantees that
//! are asserted in tests, because a refactor could reorder this list without
//! anything else failing:
//!
//!  - The auto-mode question denial is first, so a model cannot substitute
//!    questions for the approval prompts auto removed.
//!  - User-authored denials sit ahead of auto-approval, so turning on auto
//!    never overrides a restriction the user set deliberately.
//!  - Yolo approval sits ahead of every guard except explicit user denials and
//!    the irreversible-command check: yolo means yolo about supervision, not
//!    about whether a mistake can be taken back. Auto sits behind the
//!    sensitive-file and version-control checks instead, so the gradient is
//!    real — manual asks, auto proceeds but still stops at credentials and
//!    version control, yolo proceeds.
//!  - The irreversible-command check sits ahead of every approving policy,
//!    including yolo. Every other guard reads a path argument, and a shell
//!    command has none, so without this one the chain is blind precisely where
//!    the damage is.
//!
//! The user-authored slots are filled by PreToolUse hooks, which run once per
//! call before the chain and leave their verdict on the context. The session
//! history slot needs the coordinator and is therefore supplied from outside.
//!
//! Slots whose policies are not implemented yet are simply absent from the
//! assembled chain; their position in ORDER is what fixes where they land once
//! they arrive.

use std::sync::Arc;

use super::policies::self_contained_policies;
use super::policy::{FnPolicy, PermissionContext, PermissionDecision, PermissionPolicy};
use super::user_rules::user_rule_policies;
use crate::core::modes::shells::ApprovalLevel;

/// Every slot in the chain, in evaluation order.
pub const POLICY_ORDER: [&str; 13] = [
    "auto-mode-ask-user-deny",
    "user-configured-deny",
    "destructive-command-ask",
    "yolo-mode-approve",
    "session-approval-history",
    "user-configured-ask",
    "user-configured-allow",
    "sensitive-file-access-ask",
    "git-control-path-access-ask",
    "auto-mode-approve",
    "default-tool-approve",
    "git-cwd-write-approve",
    "fallback-ask",
];

/// Tools that ask the user something rather than acting on the workspace.
const QUESTION_TOOLS: [&str; 3] = ["ask_question", "ask_user_question", "followup"];

/// Auto mode removes approval prompts. Without this, a model would replace
/// them with questions and the autonomy gain would vanish, so asking is
/// refused with an instruction to decide instead.
pub fn auto_mode_ask_user_deny() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "auto-mode-ask-user-deny",
        |context: &PermissionContext| {
            if context.approval != ApprovalLevel::Auto {
                return None;
            }
            if !QUESTION_TOOLS.contains(&context.tool_name.as_str()) {
                return None;
            }
            Some(PermissionDecision::Deny {
            reason:
                "Asking the user is disabled while auto mode is active. Make a reasonable decision and continue without asking."
                    .to_string(),
        })
        },
    ))
}

/// Auto approves after the guards have had their say, so credentials and the
/// version-control directory still stop it. Autonomy without supervision is
/// exactly where an unnoticed write hurts most.
pub fn auto_mode_approve() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "auto-mode-approve",
        |context: &PermissionContext| {
            (context.approval == ApprovalLevel::Auto).then_some(PermissionDecision::Approve)
        },
    ))
}

/// Yolo approves what is left once the protective checks ahead of it have
/// abstained. It is narrower than its name suggests, which is deliberate.
pub fn yolo_mode_approve() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "yolo-mode-approve",
        |context: &PermissionContext| {
            (context.approval == ApprovalLevel::Yolo).then_some(PermissionDecision::Approve)
        },
    ))
}

fn implemented() -> Vec<(&'static str, Arc<dyn PermissionPolicy>)> {
    let mut policies: Vec<(&'static str, Arc<dyn PermissionPolicy>)> = vec![
        ("auto-mode-ask-user-deny", auto_mode_ask_user_deny()),
        ("auto-mode-approve", auto_mode_approve()),
        ("yolo-mode-approve", yolo_mode_approve()),
    ];
    policies.extend(user_rule_policies());
    policies.extend(self_contained_policies());
    policies
}

/// Builds the chain in POLICY_ORDER. Additional policies may be supplied by
/// name; anything not supplied and not built in is left out, keeping the
/// relative order of everything that is present.
pub fn build_policy_chain(
    extra: &[(&str, Arc<dyn PermissionPolicy>)],
) -> Vec<Arc<dyn PermissionPolicy>> {
    let built_in = implemented();
    let mut chain: Vec<Arc<dyn PermissionPolicy>> = Vec::new();
    for name in POLICY_ORDER {
        let policy = extra
            .iter()
            .find(|(slot, _)| *slot == name)
            .or_else(|| built_in.iter().find(|(slot, _)| *slot == name))
            .map(|(_, policy)| Arc::clone(policy));
        if let Some(policy) = policy {
            chain.push(policy);
        }
    }
    chain
}
