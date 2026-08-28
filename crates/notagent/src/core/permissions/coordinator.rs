use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::policy::PermissionContext;
use super::request::{ApprovalAnswer, ApprovalRequest, answer_allows, answer_persists};

/// Presents one request and resolves when the user answers.
/// Deviation (class 1): a presenter that fails answers `Deny` itself, where the
pub type ApprovalPresenter =
    Arc<dyn Fn(ApprovalRequest) -> BoxFuture<'static, ApprovalAnswer> + Send + Sync>;

/// Watches the moments a human is involved.
/// Only actual prompts are reported. A remembered answer and an aborted session
/// both settle without anyone being asked, and reporting those as requests
/// would tell a supervisor the agent is waiting when it is not — which is the
/// one thing this signal exists to say.
pub trait ApprovalObserver: Send + Sync {
    fn requested(&self, request: ApprovalRequest) -> BoxFuture<'static, ()>;
    fn resolved(&self, request: ApprovalRequest, answer: ApprovalAnswer) -> BoxFuture<'static, ()>;
}

/// Identifies a call for the purpose of remembering a session-wide answer.
pub fn approval_key(tool_name: &str, input: &serde_json::Map<String, Value>) -> String {
    let target = ["path", "file_path", "command"]
        .into_iter()
        .find_map(|key| input.get(key))
        .and_then(Value::as_str);
    match target {
        Some(target) => format!("{tool_name}:{target}"),
        None => tool_name.to_string(),
    }
}

/// `approvalKey(context)` for a full context.
pub fn approval_key_for(context: &PermissionContext) -> String {
    approval_key(&context.tool_name, &context.input)
}

struct CoordinatorState {
    /// Cancelled while the session is aborted; replaced by `reset`, since the
    aborted: CancellationToken,
    remembered: HashSet<String>,
}

pub struct ApprovalCoordinator {
    present: ApprovalPresenter,
    state: Mutex<CoordinatorState>,
    observer: Mutex<Option<Arc<dyn ApprovalObserver>>>,
    /// time, and tokio's mutex hands it on in arrival order.
    queue: tokio::sync::Mutex<()>,
}

impl ApprovalCoordinator {
    pub fn new(present: ApprovalPresenter) -> Self {
        Self {
            present,
            state: Mutex::new(CoordinatorState {
                aborted: CancellationToken::new(),
                remembered: HashSet::new(),
            }),
            observer: Mutex::new(None),
            queue: tokio::sync::Mutex::new(()),
        }
    }

    pub fn with_observer(present: ApprovalPresenter, observer: Arc<dyn ApprovalObserver>) -> Self {
        let coordinator = Self::new(present);
        coordinator.observe(Some(observer));
        coordinator
    }

    /// Attaches the observer after construction, since the coordinator is built
    /// before the session whose identity the observer reports.
    pub fn observe(&self, observer: Option<Arc<dyn ApprovalObserver>>) {
        *self.observer.lock().expect("approval observer") = observer;
    }

    /// Whether a session-wide answer already covers this call.
    pub fn is_remembered(&self, key: &str) -> bool {
        self.state
            .lock()
            .expect("approval state")
            .remembered
            .contains(key)
    }

    fn aborted_token(&self) -> CancellationToken {
        self.state.lock().expect("approval state").aborted.clone()
    }

    /// Observing must not be able to break approval, so the observer runs
    /// detached and a panic on either side of the await is swallowed: a
    /// supervisor that crashed is not consent, and it is not a denial either.
    fn notify(&self, run: impl FnOnce(Arc<dyn ApprovalObserver>) -> BoxFuture<'static, ()>) {
        let observer = self.observer.lock().expect("approval observer").clone();
        let Some(observer) = observer else { return };
        let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(observer)));
        if let Ok(future) = started {
            tokio::spawn(future);
        }
    }

    /// Asks, queueing behind any request already on screen. Returns a denial
    /// immediately once the session has been aborted, so a queued request never
    /// appears after the work it belonged to is gone.
    pub async fn request(
        &self,
        request: ApprovalRequest,
        key: &str,
        signal: Option<&CancellationToken>,
    ) -> ApprovalAnswer {
        let aborted = self.aborted_token();
        if aborted.is_cancelled() || signal.is_some_and(CancellationToken::is_cancelled) {
            return ApprovalAnswer::Deny;
        }
        if self.is_remembered(key) {
            return ApprovalAnswer::ApproveAlways;
        }

        let _turn = self.queue.lock().await;
        // Checked again here: a request that waited in the queue may have been
        // interrupted while the one ahead of it was on screen, and showing a
        // prompt for work that is already gone would be worse than useless.
        if aborted.is_cancelled() || signal.is_some_and(CancellationToken::is_cancelled) {
            return ApprovalAnswer::Deny;
        }
        let answer = self.race(request, &aborted, signal).await;
        if answer_persists(answer) && answer_allows(answer) {
            self.state
                .lock()
                .expect("approval state")
                .remembered
                .insert(key.to_string());
        }
        answer
    }

    /// Settles the presented request as soon as either the user answers or the
    /// session aborts, whichever comes first.
    async fn race(
        &self,
        request: ApprovalRequest,
        aborted: &CancellationToken,
        signal: Option<&CancellationToken>,
    ) -> ApprovalAnswer {
        let announced = request.clone();
        self.notify(move |observer| observer.requested(announced));

        let presented = (self.present)(request.clone());
        let interrupted = async {
            match signal {
                Some(signal) => {
                    tokio::select! {
                        () = aborted.cancelled() => {}
                        () = signal.cancelled() => {}
                    }
                }
                None => aborted.cancelled().await,
            }
        };
        let answer = tokio::select! {
            answer = presented => answer,
            () = interrupted => ApprovalAnswer::Deny,
        };

        let resolved = request;
        self.notify(move |observer| observer.resolved(resolved, answer));
        answer
    }

    /// Denies everything outstanding and every later request. Called when the
    /// session is aborted: an unanswered prompt is not consent, and a prompt
    /// that outlives its turn would approve work nobody is waiting for.
    pub fn abort(&self) {
        self.state.lock().expect("approval state").aborted.cancel();
    }

    /// Clears the abort state and remembered answers for a fresh turn.
    pub fn reset(&self) {
        let mut state = self.state.lock().expect("approval state");
        state.aborted = CancellationToken::new();
        state.remembered.clear();
    }
}
