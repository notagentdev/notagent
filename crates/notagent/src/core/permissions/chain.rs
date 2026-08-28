use std::sync::Arc;

use super::policies::self_contained_policies;
use super::policy::{FnPolicy, PermissionContext, PermissionDecision, PermissionPolicy};
use super::user_rules::user_rule_policies;
use crate::core::modes::shells::ApprovalLevel;

/// Every slot in the chain, in evaluation order.
pub const POLICY_ORDER: [&str; 14] = [
    "auto-mode-ask-user-deny",
    "user-configured-deny",
    "destructive-command-ask",
    "yolo-mode-approve",
    "session-approval-history",
    "user-configured-ask",
    "user-configured-allow",
    "sensitive-file-access-ask",
    "git-control-path-access-ask",
    // After the two guards above and before the mode slots: taking a change
    // back needs no permission, but a secret is still a secret, and the guards
    // that stop at one have to see the call first.
    "undo-approve",
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
