use std::sync::Arc;

use serde_json::{Map, Value};

use crate::core::hooks::runner::HookVerdict;
use crate::core::modes::shells::{ApprovalLevel, ShellId};

/// What a policy decided about one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Approve,
    Deny { reason: String },
    Ask { reason: Option<String> },
}

/// Everything a policy may consider. Deliberately read-only.
#[derive(Debug, Clone, Default)]
pub struct PermissionContext {
    /// Stable identity shared by permission and tool lifecycle events.
    pub tool_call_id: String,
    /// Name of the tool being called.
    pub tool_name: String,
    /// Validated tool arguments.
    pub input: Map<String, Value>,
    /// Active mode id, for reporting rather than for deciding.
    pub mode_id: Option<String>,
    /// Shell of the active mode: what tools exist at all.
    pub shell: Option<ShellId>,
    /// Approval level of the active mode: whether an existing tool runs unattended.
    pub approval: ApprovalLevel,
    /// Absolute working directory of the session.
    pub cwd: String,
    /// The delegated child behind this call, when there is one. Reporting only — a child is governed by exactly the rules its
    /// parent is, so no policy decides on this. It exists so the prompt can say
    /// who is asking, which with several children running is the first thing
    /// the user needs to know.
    pub requester: Option<crate::core::permissions::requester::Requester>,
    /// What the user's PreToolUse hooks decided about this call, run once before
    /// the chain. Precomputed rather than run from inside a policy because a
    /// policy decides synchronously and a hook is a child process — and because
    /// PreToolUse is also how an external supervisor sees that a tool was
    /// attempted, so it must fire for every call rather than only for those that
    /// reach a particular slot.
    pub hook_verdict: Option<HookVerdict>,
}

impl PermissionContext {
    /// The `Default` for `approval` is the supervised end, as everywhere else.
    pub fn new(
        tool_name: impl Into<String>,
        input: Map<String, Value>,
        cwd: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: String::new(),
            tool_name: tool_name.into(),
            input,
            cwd: cwd.into(),
            approval: ApprovalLevel::Manual,
            ..Self::default()
        }
    }
}

pub trait PermissionPolicy: Send + Sync {
    /// Stable identifier, reported with the decision so an outcome is traceable.
    fn name(&self) -> &str;
    /// Decide, or return `None` to abstain and let the next policy run.
    fn evaluate(&self, context: &PermissionContext) -> Option<PermissionDecision>;
}

/// A decision together with the policy that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionEvaluation {
    pub policy_name: String,
    pub decision: PermissionDecision,
}

/// Evaluates policies in order, first decision wins.
/// Returns `None` only when every policy abstained. Callers must treat
/// that as unresolved rather than as permission — the chain is expected to end
/// in a policy that always decides, and a missing one is a configuration error,
/// not an allowance.
pub fn evaluate_policies(
    policies: &[Arc<dyn PermissionPolicy>],
    context: &PermissionContext,
) -> Option<PermissionEvaluation> {
    for policy in policies {
        if let Some(decision) = policy.evaluate(context) {
            return Some(PermissionEvaluation {
                policy_name: policy.name().to_string(),
                decision,
            });
        }
    }
    None
}

/// What [`resolve_without_dialog`] concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWithoutDialog {
    pub allowed: bool,
    pub reason: Option<String>,
    pub policy_name: String,
}

/// Collapses a decision into what the runtime can currently carry out.
/// The tool-call gate can allow or block; there is no channel yet for waiting
/// on a user's answer. Until that exists, `ask` must resolve to a refusal: a
/// prompt that cannot be shown but lets the call through would look like
/// supervision while providing none. An unresolved chain is refused for the
/// same reason.
pub fn resolve_without_dialog(evaluation: Option<&PermissionEvaluation>) -> ResolvedWithoutDialog {
    let Some(evaluation) = evaluation else {
        return ResolvedWithoutDialog {
            allowed: false,
            reason: Some(
                "No permission policy decided this call; refusing rather than assuming permission."
                    .to_string(),
            ),
            policy_name: "unresolved".to_string(),
        };
    };
    let policy_name = evaluation.policy_name.clone();
    match &evaluation.decision {
        PermissionDecision::Approve => {
            ResolvedWithoutDialog { allowed: true, reason: None, policy_name }
        }
        PermissionDecision::Deny { reason } => ResolvedWithoutDialog {
            allowed: false,
            reason: Some(reason.clone()),
            policy_name,
        },
        PermissionDecision::Ask { reason } => ResolvedWithoutDialog {
            allowed: false,
            reason: Some(reason.clone().unwrap_or_else(|| {
                "This call requires confirmation, which cannot be requested yet; refusing rather than proceeding unsupervised."
                    .to_string()
            })),
            policy_name,
        },
    }
}

/// A policy expressed as a closure, which is how the chain's small rules are
/// spelled out without a type each.
pub struct FnPolicy<F>
where
    F: Fn(&PermissionContext) -> Option<PermissionDecision> + Send + Sync,
{
    name: &'static str,
    evaluate: F,
}

impl<F> FnPolicy<F>
where
    F: Fn(&PermissionContext) -> Option<PermissionDecision> + Send + Sync,
{
    pub const fn new(name: &'static str, evaluate: F) -> Self {
        Self { name, evaluate }
    }
}

impl<F> PermissionPolicy for FnPolicy<F>
where
    F: Fn(&PermissionContext) -> Option<PermissionDecision> + Send + Sync,
{
    fn name(&self) -> &str {
        self.name
    }

    fn evaluate(&self, context: &PermissionContext) -> Option<PermissionDecision> {
        (self.evaluate)(context)
    }
}
