use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::coordinator::{ApprovalCoordinator, approval_key_for};
use super::policy::{PermissionContext, PermissionDecision, PermissionPolicy, evaluate_policies};
use super::request::{answer_allows, build_approval_request};
use crate::core::hooks::runner::HookVerdict;
use crate::core::modes::shells::{ApprovalLevel, ShellId};

/// What the handler returns; mirrors the gate's own result shape.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PermissionHookResult {
    pub block: bool,
    pub reason: Option<String>,
}

/// Session state the handler needs to build a decision context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionSessionState {
    pub mode_id: Option<String>,
    pub shell: Option<ShellId>,
    pub approval: ApprovalLevel,
    pub cwd: String,
}

impl Default for PermissionSessionState {
    /// Before a session exists, assume the supervised level rather than the
    /// permissive one.
    fn default() -> Self {
        Self {
            mode_id: None,
            shell: None,
            approval: ApprovalLevel::Manual,
            cwd: String::new(),
        }
    }
}

pub type PermissionStateSource = Arc<dyn Fn() -> PermissionSessionState + Send + Sync>;

/// Runs the user's PreToolUse hooks. Called for every tool call, before the
/// chain, because PreToolUse is also how a supervisor sees that a tool was
/// attempted — a hook that only fired when the chain reached its slot would
/// be a partial signal, and worse, the run would then depend on which mode
/// the user happened to be in.
pub type PermissionDecideHook =
    Arc<dyn Fn(String, Map<String, Value>) -> BoxFuture<'static, HookVerdict> + Send + Sync>;

pub struct PermissionHookOptions {
    pub policies: Vec<Arc<dyn PermissionPolicy>>,
    pub coordinator: Arc<ApprovalCoordinator>,
    pub state: PermissionStateSource,
    pub decide: Option<PermissionDecideHook>,
}

/// The handler. Returns `None` to let a call through, or a blocking result
/// carrying the reason and the policy that produced it.
pub struct PermissionHandler {
    options: PermissionHookOptions,
}

pub fn create_permission_handler(options: PermissionHookOptions) -> PermissionHandler {
    PermissionHandler { options }
}

impl PermissionHandler {
    pub async fn call(
        &self,
        tool_name: &str,
        input: &Map<String, Value>,
        signal: Option<&CancellationToken>,
    ) -> Option<PermissionHookResult> {
        let session = (self.options.state)();
        let hook_verdict = match &self.options.decide {
            Some(decide) => Some(decide(tool_name.to_string(), input.clone()).await),
            None => None,
        };
        let context = PermissionContext {
            tool_name: tool_name.to_string(),
            input: input.clone(),
            mode_id: session.mode_id,
            shell: session.shell,
            approval: session.approval,
            cwd: session.cwd,
            // Read here rather than carried in `PermissionSessionState`: the
            // state source describes the *session*, and the requester is a
            // property of the individual call.
            requester: super::requester::current_requester(),
            hook_verdict,
        };

        let Some(evaluation) = evaluate_policies(&self.options.policies, &context) else {
            return Some(PermissionHookResult {
                block: true,
                reason: Some(
                    "No permission policy decided this call; refusing rather than assuming permission."
                        .to_string(),
                ),
            });
        };

        let policy_name = evaluation.policy_name.clone();
        match &evaluation.decision {
            PermissionDecision::Approve => return None,
            PermissionDecision::Deny { reason } => {
                return Some(PermissionHookResult {
                    block: true,
                    reason: Some(format!("{reason} ({policy_name})")),
                });
            }
            PermissionDecision::Ask { .. } => {}
        }

        let Some(request) = build_approval_request(&context, &evaluation) else {
            return Some(PermissionHookResult {
                block: true,
                reason: Some(format!(
                    "Could not present an approval request ({policy_name})."
                )),
            });
        };

        let answer = self
            .options
            .coordinator
            .request(request, &approval_key_for(&context), signal)
            .await;
        if answer_allows(answer) {
            return None;
        }
        // An interrupted turn settles as a denial, so say which it was rather
        // than blaming the user for a prompt they never saw.
        if signal.is_some_and(CancellationToken::is_cancelled) {
            return Some(PermissionHookResult {
                block: true,
                reason: Some("The turn was interrupted before this was answered.".to_string()),
            });
        }
        Some(PermissionHookResult {
            block: true,
            reason: Some(format!("Denied by the user ({policy_name}).")),
        })
    }
}
