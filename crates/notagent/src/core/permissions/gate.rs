use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::chain::build_policy_chain;
use super::coordinator::{ApprovalCoordinator, ApprovalObserver, ApprovalPresenter};
use super::hook::{
    PermissionCall, PermissionDecideHook, PermissionHandler, PermissionHookOptions,
    PermissionStateSource, create_permission_handler,
};
use super::policy::PermissionPolicy;
use super::user_rules::create_session_approval_history;

pub struct PermissionGateOptions {
    /// Current mode, shell and approval level at the moment of the call.
    pub state: PermissionStateSource,
    /// Presents an approval request; typically the interactive dialog.
    pub present: ApprovalPresenter,
    /// Extra policies by slot name, for the rules this project does not ship.
    pub policies: Vec<(&'static str, Arc<dyn PermissionPolicy>)>,
    /// Runs the user's PreToolUse hooks; their verdict fills the user slots.
    pub decide: Option<PermissionDecideHook>,
}

/// A refused call, as the agent loop reports it.
/// `terminate` ends the batch, so later calls in the same batch do not run
/// against a state the user just refused to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionBlock {
    pub reason: Option<String>,
    pub terminate: bool,
}

pub struct PermissionGate {
    coordinator: Arc<ApprovalCoordinator>,
    handler: PermissionHandler,
}

impl PermissionGate {
    pub fn new(options: PermissionGateOptions) -> Self {
        let coordinator = Arc::new(ApprovalCoordinator::new(options.present));
        let mut supplied = options.policies;
        // The history slot needs the coordinator that holds the answers, so it is
        // built here rather than shipped as a standalone policy.
        if !supplied
            .iter()
            .any(|(name, _)| *name == "session-approval-history")
        {
            supplied.push((
                "session-approval-history",
                create_session_approval_history(Arc::clone(&coordinator)),
            ));
        }
        let handler = create_permission_handler(PermissionHookOptions {
            policies: build_policy_chain(&supplied),
            coordinator: Arc::clone(&coordinator),
            state: options.state,
            decide: options.decide,
        });
        Self {
            coordinator,
            handler,
        }
    }

    /// The pre-tool gate. `None` lets the call proceed.
    /// The turn's cancellation token is what settles a prompt nobody will
    /// answer. Without it, interrupting the agent while the dialog is up
    /// leaves this call waiting forever, and the interrupt waits on the
    /// call — the session never goes idle.
    pub async fn before_tool_call(
        &self,
        call: PermissionCall,
        signal: Option<&CancellationToken>,
    ) -> Option<PermissionBlock> {
        let result = self.handler.call(call, signal).await?;
        if !result.block {
            return None;
        }
        Some(PermissionBlock {
            reason: result.reason,
            terminate: true,
        })
    }

    /// A replaced session starts over: an answer given for the previous one
    /// is not consent for this one, and a stale abort must not outlive it.
    pub fn session_start(&self) {
        self.coordinator.reset();
    }

    /// Teardown denies whatever is still on screen, for the same reason an
    /// interrupt does — nobody is going to answer it now.
    pub fn session_shutdown(&self) {
        self.coordinator.abort();
    }

    /// Deny everything outstanding. Session teardown does this on its own; this
    /// is for a host that tears down by some other route.
    pub fn abort(&self) {
        self.coordinator.abort();
    }

    /// Forget remembered answers and clear the abort state.
    pub fn reset(&self) {
        self.coordinator.reset();
    }

    /// Watch the moments a human is asked. Bound once the session exists.
    pub fn observe(&self, observer: Option<Arc<dyn ApprovalObserver>>) {
        self.coordinator.observe(observer);
    }

    /// The coordinator behind the gate, for the hook observer that reports
    /// prompts and for tests.
    pub fn coordinator(&self) -> Arc<ApprovalCoordinator> {
        Arc::clone(&self.coordinator)
    }
}
