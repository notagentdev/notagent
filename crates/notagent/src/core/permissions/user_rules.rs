use std::sync::Arc;

use super::coordinator::{ApprovalCoordinator, approval_key_for};
use super::policies::is_destructive_call;
use super::policy::{FnPolicy, PermissionContext, PermissionDecision, PermissionPolicy};
use crate::core::hooks::runner::HookVerdict;

/// A hook refused. Sits ahead of yolo: an explicit user rule outranks a mode.
pub fn user_configured_deny() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "user-configured-deny",
        |context: &PermissionContext| match &context.hook_verdict {
            Some(HookVerdict::Deny { reason }) => Some(PermissionDecision::Deny {
                reason: reason.clone(),
            }),
            _ => None,
        },
    ))
}

/// A hook wants the user asked, even in a mode that would not have.
pub fn user_configured_ask() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "user-configured-ask",
        |context: &PermissionContext| match &context.hook_verdict {
            Some(HookVerdict::Ask { reason }) => Some(PermissionDecision::Ask {
                reason: Some(
                    reason
                        .clone()
                        .unwrap_or_else(|| "A hook asked for confirmation.".to_string()),
                ),
            }),
            _ => None,
        },
    ))
}

/// A hook permitted the call outright, which is why this sits ahead of the
/// sensitive-file and version-control guards: a user who wrote a rule allowing
/// a path meant it, including for the files those guards protect.
pub fn user_configured_allow() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "user-configured-allow",
        |context: &PermissionContext| {
            matches!(context.hook_verdict, Some(HookVerdict::Allow { .. }))
                .then_some(PermissionDecision::Approve)
        },
    ))
}

/// Approves what the user already approved for this session.
/// "Allow for this session" would otherwise be honoured only where a policy
/// happened to ask again, which is a hidden path through the coordinator rather
/// than a rule in the chain. As a slot it is visible, and it is reported as the
/// reason the call went through.
pub fn create_session_approval_history(
    coordinator: Arc<ApprovalCoordinator>,
) -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "session-approval-history",
        move |context: &PermissionContext| {
            // An irreversible command is decided every time. "Allow for this
            // session" answered about one `rm -rf` is not consent for the next
            // one, however alike the two look.
            if is_destructive_call(context) {
                return None;
            }
            coordinator
                .is_remembered(&approval_key_for(context))
                .then_some(PermissionDecision::Approve)
        },
    ))
}

/// The hook-backed slots, keyed by their position in the chain.
pub fn user_rule_policies() -> Vec<(&'static str, Arc<dyn PermissionPolicy>)> {
    vec![
        ("user-configured-deny", user_configured_deny()),
        ("user-configured-ask", user_configured_ask()),
        ("user-configured-allow", user_configured_allow()),
    ]
}
