//! 1:1 port of `packages/coding-agent/src/client/remote-session.ts` (414 LOC).
//!
//! A `RemoteSession` borrows a `PiClient`, holds exactly one exclusive session
//! lease and projects streamed progress on top of the authoritative snapshot.
//! It never disposes the client it borrowed.

use std::collections::HashSet;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use notagent_client::{
    AcquireSessionOptions, ConnectionState, ConnectionStateChange, CreateSessionOptions, PiClient,
    PiError, SessionHandle, SessionLeaseMode, Unsubscribe,
};
use notagent_protocol::{
    ModelMetadata, ModelRef, ServerEvent, SessionMetadata, SessionPhase, SessionSnapshot,
    ThinkingLevel, TranscriptItem,
};
use tokio::sync::oneshot;

use super::transcript::{
    TranscriptState, apply_transcript_progress, apply_transcript_snapshot, create_transcript_state,
    select_transcript,
};

/// `RemoteSessionOperation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteSessionOperation {
    Open,
    Create,
    Submit,
    Abort,
    SetModel,
    SetThinking,
    Reconnect,
}

impl RemoteSessionOperation {
    /// The TS union members double as the words in the error texts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Create => "create",
            Self::Submit => "submit",
            Self::Abort => "abort",
            Self::SetModel => "setModel",
            Self::SetThinking => "setThinking",
            Self::Reconnect => "reconnect",
        }
    }
}

/// `RemoteSessionLifecycle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSessionLifecycle {
    Unbound,
    Ready,
    Busy { operation: RemoteSessionOperation },
    Disposed,
}

/// `RemoteSessionState`.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteSessionState {
    pub lifecycle: RemoteSessionLifecycle,
    pub snapshot: Option<SessionSnapshot>,
    pub transcript: Vec<TranscriptItem>,
}

/// `CreateRemoteSessionOptions`.
#[derive(Debug, Clone, Default)]
pub struct CreateRemoteSessionOptions {
    pub cwd: String,
    pub model: Option<ModelRef>,
    pub thinking_level: Option<ThinkingLevel>,
}

/// Deviation class 1: the TS error classes become one enum. `Disposed` is the
/// plain `Error("Remote session is disposed")`; `DisposedDuringAttachment` is
/// `RemoteSessionDisposedError`, the only class disposal filters out.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RemoteSessionError {
    #[error("Remote session is disposed")]
    Disposed,
    #[error("Remote session is disposed")]
    DisposedDuringAttachment,
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Client(PiError),
    #[error("Failed to {context}")]
    Aggregate {
        context: String,
        errors: Vec<RemoteSessionError>,
    },
}

impl From<PiError> for RemoteSessionError {
    fn from(error: PiError) -> Self {
        Self::Client(error)
    }
}

/// `RemoteSessionOptions`.
#[derive(Clone, Default)]
pub struct RemoteSessionOptions {
    pub on_listener_error: Option<Arc<dyn Fn(RemoteSessionError) + Send + Sync>>,
}

impl std::fmt::Debug for RemoteSessionOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteSessionOptions")
            .field("on_listener_error", &self.on_listener_error.is_some())
            .finish()
    }
}

type StateListener = Arc<dyn Fn(&RemoteSessionState) + Send + Sync>;
type OperationResult = Result<(), RemoteSessionError>;
type SharedOperation = Shared<BoxFuture<'static, OperationResult>>;

struct Data {
    lifecycle: RemoteSessionLifecycle,
    lifecycle_token: u64,
    next_token: u64,
    handle: Option<SessionHandle>,
    transcript: Option<TranscriptState>,
    unsubscribe_snapshot: Option<Unsubscribe>,
    unsubscribe_events: Option<Unsubscribe>,
    listeners: Vec<(u64, StateListener)>,
    next_listener_id: u64,
    pending_attachments: Vec<(u64, SharedOperation)>,
    next_attachment_id: u64,
    active_operation_tokens: HashSet<u64>,
    dispose_promise: Option<SharedOperation>,
}

struct Inner {
    client: PiClient,
    on_listener_error: Option<Arc<dyn Fn(RemoteSessionError) + Send + Sync>>,
    data: Mutex<Data>,
    dispose_resolver: Mutex<Option<oneshot::Sender<()>>>,
    dispose_signal: Shared<BoxFuture<'static, ()>>,
}

/// `RemoteSession`.
///
/// Deviation class 1: `Clone` shares the same session (Arc), the way a JS
/// object reference is passed around.
#[derive(Clone)]
pub struct RemoteSession {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for RemoteSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteSession")
            .field("lifecycle", &self.state().lifecycle)
            .finish_non_exhaustive()
    }
}

impl RemoteSession {
    fn new(client: PiClient, options: RemoteSessionOptions) -> Self {
        let (sender, receiver) = oneshot::channel::<()>();
        let signal: BoxFuture<'static, ()> = Box::pin(async move {
            let _ = receiver.await;
        });
        Self {
            inner: Arc::new(Inner {
                client,
                on_listener_error: options.on_listener_error,
                data: Mutex::new(Data {
                    lifecycle: RemoteSessionLifecycle::Unbound,
                    lifecycle_token: 0,
                    next_token: 1,
                    handle: None,
                    transcript: None,
                    unsubscribe_snapshot: None,
                    unsubscribe_events: None,
                    listeners: Vec::new(),
                    next_listener_id: 0,
                    pending_attachments: Vec::new(),
                    next_attachment_id: 0,
                    active_operation_tokens: HashSet::new(),
                    dispose_promise: None,
                }),
                dispose_resolver: Mutex::new(Some(sender)),
                dispose_signal: signal.shared(),
            }),
        }
    }

    /// `RemoteSession.open` — the factory disposes a half-open session itself.
    pub fn open_session(
        client: PiClient,
        session_id: &str,
        options: RemoteSessionOptions,
    ) -> impl Future<Output = Result<Self, RemoteSessionError>> + Send + use<> {
        let session = Self::new(client, options);
        let opening = session.open(session_id);
        async move {
            match opening.await {
                Ok(()) => Ok(session),
                Err(error) => {
                    let _ = session.dispose().await;
                    Err(error)
                }
            }
        }
    }

    /// `RemoteSession.create`.
    pub fn create_session(
        client: PiClient,
        create_options: CreateRemoteSessionOptions,
        options: RemoteSessionOptions,
    ) -> impl Future<Output = Result<Self, RemoteSessionError>> + Send + use<> {
        let session = Self::new(client, options);
        let creating = session.create(create_options);
        async move {
            match creating.await {
                Ok(()) => Ok(session),
                Err(error) => {
                    let _ = session.dispose().await;
                    Err(error)
                }
            }
        }
    }

    pub fn id(&self) -> Option<String> {
        self.lock()
            .handle
            .as_ref()
            .map(|handle| handle.id().to_owned())
    }

    pub fn state(&self) -> RemoteSessionState {
        let data = self.lock();
        Self::state_of(&data)
    }

    pub fn snapshot(&self) -> Option<SessionSnapshot> {
        self.lock()
            .transcript
            .as_ref()
            .map(|transcript| transcript.snapshot.clone())
    }

    pub fn phase(&self) -> Option<SessionPhase> {
        self.snapshot().map(|snapshot| snapshot.phase)
    }

    pub fn operation(&self) -> Option<RemoteSessionOperation> {
        match self.lock().lifecycle {
            RemoteSessionLifecycle::Busy { operation } => Some(operation),
            _ => None,
        }
    }

    pub fn models(&self) -> Vec<ModelMetadata> {
        self.inner
            .client
            .snapshot()
            .map(|snapshot| snapshot.models)
            .unwrap_or_default()
    }

    pub fn sessions(&self) -> Vec<SessionMetadata> {
        self.inner
            .client
            .snapshot()
            .map(|snapshot| snapshot.sessions)
            .unwrap_or_default()
    }

    pub fn connection_state(&self) -> ConnectionState {
        self.inner.client.connection_state()
    }

    pub fn disposed(&self) -> bool {
        self.lock().lifecycle == RemoteSessionLifecycle::Disposed
    }

    /// `subscribe` — the new subscriber is called once with the current state.
    pub fn subscribe(&self, listener: StateListener) -> Result<Unsubscribe, RemoteSessionError> {
        self.assert_not_disposed()?;
        let (id, state) = {
            let mut data = self.lock();
            let id = data.next_listener_id;
            data.next_listener_id += 1;
            data.listeners.push((id, Arc::clone(&listener)));
            (id, Self::state_of(&data))
        };
        self.call_listener(&listener, &state);
        let inner = Arc::downgrade(&self.inner);
        Ok(Box::new(move || {
            if let Some(inner) = inner.upgrade() {
                inner
                    .data
                    .lock()
                    .expect("remote session mutex")
                    .listeners
                    .retain(|(candidate, _)| *candidate != id);
            }
        }))
    }

    pub fn on_connection_state_change(
        &self,
        listener: Arc<dyn Fn(&ConnectionStateChange) + Send + Sync>,
    ) -> Result<Unsubscribe, RemoteSessionError> {
        self.assert_not_disposed()?;
        Ok(self.inner.client.on_connection_state_change(listener)?)
    }

    /// `open` — reopening the session that is already bound is a no-op.
    pub fn open(&self, session_id: &str) -> impl Future<Output = OperationResult> + Send + use<> {
        let session_id = session_id.to_owned();
        let already_open = {
            let data = self.lock();
            data.handle
                .as_ref()
                .is_some_and(|handle| handle.id() == session_id)
                && data.lifecycle == RemoteSessionLifecycle::Ready
        };
        let this = self.clone();
        let started =
            (!already_open).then(|| this.replace(RemoteSessionOperation::Open, session_id));
        async move {
            match started {
                Some(future) => future.await,
                None => Ok(()),
            }
        }
    }

    /// `create`.
    pub fn create(
        &self,
        options: CreateRemoteSessionOptions,
    ) -> impl Future<Output = OperationResult> + Send + use<> {
        self.replace_with_create(options)
    }

    /// `submit` — an empty prompt is dropped, a running turn is steered.
    pub fn submit(&self, text: &str) -> impl Future<Output = OperationResult> + Send + use<> {
        let normalized = text.trim().to_owned();
        let prologue = (|| -> Result<Option<SharedOperation>, RemoteSessionError> {
            if normalized.is_empty() {
                return Ok(None);
            }
            self.assert_available()?;
            let handle = self.require_handle()?;
            let phase = self.phase();
            if phase != Some(SessionPhase::Idle) && phase != Some(SessionPhase::Turn) {
                return Err(RemoteSessionError::Message(format!(
                    "Session cannot accept input during {} phase",
                    phase_name(phase)
                )));
            }
            let text = normalized.clone();
            Ok(Some(self.begin(
                RemoteSessionOperation::Submit,
                false,
                move || {
                    let request: BoxFuture<'static, Result<SessionSnapshot, PiError>> =
                        if phase == Some(SessionPhase::Idle) {
                            Box::pin(handle.prompt(&text))
                        } else {
                            Box::pin(handle.steer(&text))
                        };
                    Box::pin(
                        async move { request.await.map(|_| ()).map_err(RemoteSessionError::from) },
                    )
                },
            )))
        })();
        let this = self.clone();
        async move {
            match prologue? {
                Some(running) => this.await_operation(running).await,
                None => Ok(()),
            }
        }
    }

    /// `abort` — an abort issued while a submit is in flight preempts it.
    pub fn abort(&self) -> impl Future<Output = OperationResult> + Send + use<> {
        let prologue = (|| -> Result<Option<SharedOperation>, RemoteSessionError> {
            let preempting_submit = matches!(
                self.lock().lifecycle,
                RemoteSessionLifecycle::Busy {
                    operation: RemoteSessionOperation::Submit
                }
            );
            if preempting_submit {
                self.assert_not_disposed()?;
            } else {
                self.assert_available()?;
            }
            let handle = self.require_handle()?;
            if self.phase() == Some(SessionPhase::Idle) && !preempting_submit {
                return Ok(None);
            }
            Ok(Some(self.begin(
                RemoteSessionOperation::Abort,
                preempting_submit,
                move || {
                    let request = handle.abort();
                    Box::pin(
                        async move { request.await.map(|_| ()).map_err(RemoteSessionError::from) },
                    )
                },
            )))
        })();
        let this = self.clone();
        async move {
            match prologue? {
                Some(running) => this.await_operation(running).await,
                None => Ok(()),
            }
        }
    }

    /// `setModel`.
    pub fn set_model(
        &self,
        model: ModelRef,
    ) -> impl Future<Output = OperationResult> + Send + use<> {
        self.run_idle_operation(
            RemoteSessionOperation::SetModel,
            "change model",
            move |handle| {
                let request = handle.set_model(model);
                Box::pin(async move { request.await.map(|_| ()).map_err(RemoteSessionError::from) })
            },
        )
    }

    /// `setThinking`.
    pub fn set_thinking(
        &self,
        thinking_level: ThinkingLevel,
    ) -> impl Future<Output = OperationResult> + Send + use<> {
        self.run_idle_operation(
            RemoteSessionOperation::SetThinking,
            "change thinking level",
            move |handle| {
                let request = handle.set_thinking(thinking_level);
                Box::pin(async move { request.await.map(|_| ()).map_err(RemoteSessionError::from) })
            },
        )
    }

    /// `reconnect` — reconnects the borrowed client and re-acquires the lease.
    pub fn reconnect(&self) -> impl Future<Output = OperationResult> + Send + use<> {
        let prologue = (|| -> Result<SharedOperation, RemoteSessionError> {
            self.assert_available()?;
            let session_id = self.require_handle()?.id().to_owned();
            let this = self.clone();
            Ok(
                self.begin(RemoteSessionOperation::Reconnect, false, move || {
                    let attachment = this.clone();
                    this.track_attachment(Box::pin(async move {
                        attachment.inner.client.reconnect().await?;
                        let handle = attachment
                            .inner
                            .client
                            .acquire_session(
                                &session_id,
                                AcquireSessionOptions {
                                    mode: SessionLeaseMode::Exclusive,
                                },
                            )
                            .await?;
                        attachment.assert_not_disposed_after_await(&handle).await?;
                        attachment.bind(handle, None)
                    }))
                }),
            )
        })();
        let this = self.clone();
        async move { this.await_operation(prologue?).await }
    }

    /// `dispose` — idempotent; every caller awaits the same cleanup.
    pub fn dispose(&self) -> impl Future<Output = OperationResult> + Send + use<> {
        let (promise, state) = {
            let mut data = self.lock();
            if let Some(existing) = data.dispose_promise.clone() {
                return existing;
            }
            let handle = data.handle.take();
            data.lifecycle = RemoteSessionLifecycle::Disposed;
            data.lifecycle_token = 0;
            Self::clear_subscriptions(&mut data);
            data.transcript = None;
            let mut cleanup: Vec<SharedOperation> = data
                .pending_attachments
                .iter()
                .map(|(_, pending)| pending.clone())
                .collect();
            if let Some(handle) = handle {
                // Calling `dispose()` here sends the detach request straight
                // away; only awaiting it is deferred.
                let request = handle.dispose();
                let disposing: BoxFuture<'static, OperationResult> =
                    Box::pin(async move { request.await.map_err(RemoteSessionError::from) });
                cleanup.push(disposing.shared());
            }
            let promise: BoxFuture<'static, OperationResult> =
                Box::pin(settle_remote_session_disposal(cleanup));
            let promise = promise.shared();
            data.dispose_promise = Some(promise.clone());
            (promise, Self::state_of(&data))
        };
        // Resolving the signal makes every racing operation give up.
        if let Some(sender) = self
            .inner
            .dispose_resolver
            .lock()
            .expect("dispose resolver mutex")
            .take()
        {
            let _ = sender.send(());
        }
        let listeners = self.listener_snapshot();
        for listener in &listeners {
            self.call_listener(listener, &state);
        }
        self.lock().listeners.clear();
        promise
    }

    // --- internals ----------------------------------------------------------

    fn lock(&self) -> std::sync::MutexGuard<'_, Data> {
        self.inner.data.lock().expect("remote session mutex")
    }

    fn state_of(data: &Data) -> RemoteSessionState {
        RemoteSessionState {
            lifecycle: data.lifecycle,
            snapshot: data
                .transcript
                .as_ref()
                .map(|transcript| transcript.snapshot.clone()),
            transcript: data
                .transcript
                .as_ref()
                .map(select_transcript)
                .unwrap_or_default(),
        }
    }

    /// `#replace` for `open`.
    fn replace(
        &self,
        operation: RemoteSessionOperation,
        session_id: String,
    ) -> impl Future<Output = OperationResult> + Send + use<> {
        let prologue = (|| -> Result<SharedOperation, RemoteSessionError> {
            self.assert_available()?;
            self.assert_replaceable(operation)?;
            let this = self.clone();
            Ok(self.begin(operation, false, move || {
                // The attach request leaves synchronously, as in TS.
                let attaching = this.inner.client.acquire_session(
                    &session_id,
                    AcquireSessionOptions {
                        mode: SessionLeaseMode::Exclusive,
                    },
                );
                let replacement = this.clone();
                this.track_attachment(Box::pin(async move {
                    replacement
                        .prepare_replacement(operation, Box::pin(attaching))
                        .await
                }))
            }))
        })();
        let this = self.clone();
        async move { this.await_operation(prologue?).await }
    }

    /// `#replace` for `create`.
    fn replace_with_create(
        &self,
        options: CreateRemoteSessionOptions,
    ) -> impl Future<Output = OperationResult> + Send + use<> {
        let operation = RemoteSessionOperation::Create;
        let prologue = (|| -> Result<SharedOperation, RemoteSessionError> {
            self.assert_available()?;
            self.assert_replaceable(operation)?;
            let this = self.clone();
            Ok(self.begin(operation, false, move || {
                let creating = this.inner.client.create_session(CreateSessionOptions {
                    cwd: Some(options.cwd),
                    name: None,
                    model: options.model,
                    thinking_level: options.thinking_level,
                });
                let replacement = this.clone();
                this.track_attachment(Box::pin(async move {
                    replacement
                        .prepare_replacement(operation, Box::pin(creating))
                        .await
                }))
            }))
        })();
        let this = self.clone();
        async move { this.await_operation(prologue?).await }
    }

    /// The synchronous half of TS `#replace`: a bound session that is not idle
    /// cannot be swapped out.
    fn assert_replaceable(&self, operation: RemoteSessionOperation) -> OperationResult {
        let bound = self.lock().handle.is_some();
        if bound && self.phase() != Some(SessionPhase::Idle) {
            return Err(RemoteSessionError::Message(format!(
                "Cannot {} a session while session is {}",
                operation.as_str(),
                phase_or_unavailable(self.phase())
            )));
        }
        Ok(())
    }

    /// `#prepareReplacement`.
    async fn prepare_replacement(
        &self,
        operation: RemoteSessionOperation,
        prepare: BoxFuture<'static, Result<SessionHandle, PiError>>,
    ) -> OperationResult {
        let previous = self.lock().handle.clone();
        let next = prepare.await?;
        self.assert_not_disposed_after_await(&next).await?;
        let Some(snapshot) = next.snapshot() else {
            self.detach(&next).await?;
            return Err(RemoteSessionError::Message(format!(
                "Session {} did not provide a snapshot",
                next.id()
            )));
        };
        if let Some(previous) = &previous
            && previous.id() != next.id()
            && previous.attached()
            && self.phase() != Some(SessionPhase::Idle)
        {
            self.detach(&next).await?;
            return Err(RemoteSessionError::Message(format!(
                "Cannot {} a session while session is {}",
                operation.as_str(),
                phase_or_unavailable(self.phase())
            )));
        }
        if let Some(previous) = &previous
            && previous.id() != next.id()
            && previous.attached()
            && let Err(error) = previous.detach().await
        {
            let error = RemoteSessionError::from(error);
            if let Err(cleanup_error) = self.detach(&next).await {
                return Err(RemoteSessionError::Aggregate {
                    context: "replace remote session attachment".to_owned(),
                    errors: vec![error, cleanup_error],
                });
            }
            return Err(error);
        }
        self.assert_not_disposed_after_await(&next).await?;
        self.bind(next, Some(snapshot))
    }

    /// `#runIdleOperation`.
    fn run_idle_operation<F>(
        &self,
        operation: RemoteSessionOperation,
        description: &str,
        make: F,
    ) -> impl Future<Output = OperationResult> + Send + use<F>
    where
        F: FnOnce(SessionHandle) -> BoxFuture<'static, OperationResult> + Send + 'static,
    {
        let description = description.to_owned();
        let prologue = (|| -> Result<SharedOperation, RemoteSessionError> {
            self.assert_available()?;
            let handle = self.require_handle()?;
            if self.phase() != Some(SessionPhase::Idle) {
                return Err(RemoteSessionError::Message(format!(
                    "Cannot {description} while session is {}",
                    phase_or_unavailable(self.phase())
                )));
            }
            Ok(self.begin(operation, false, move || make(handle)))
        })();
        let this = self.clone();
        async move { this.await_operation(prologue?).await }
    }

    /// The synchronous half of `#runOperation`: mark busy, notify, start the
    /// work. TS calls `run()` only after the busy state was published.
    fn begin<F>(&self, operation: RemoteSessionOperation, preempt: bool, make: F) -> SharedOperation
    where
        F: FnOnce() -> BoxFuture<'static, OperationResult>,
    {
        let (token, previous, previous_token, state) = {
            let mut data = self.lock();
            let previous = data.lifecycle;
            let previous_token = data.lifecycle_token;
            let token = data.next_token;
            data.next_token += 1;
            data.lifecycle = RemoteSessionLifecycle::Busy { operation };
            data.lifecycle_token = token;
            data.active_operation_tokens.insert(token);
            (token, previous, previous_token, Self::state_of(&data))
        };
        self.notify_with(&state);
        let running = make();
        let guard = OperationGuard {
            session: self.clone(),
            token,
            previous,
            previous_token,
            preempt,
        };
        let signal = self.inner.dispose_signal.clone();
        let future: BoxFuture<'static, OperationResult> = Box::pin(async move {
            let _guard = guard;
            tokio::select! {
                result = running => result,
                () = signal => Err(RemoteSessionError::Disposed),
            }
        });
        let shared = future.shared();
        // TS starts `run()` immediately and the promise settles on its own; the
        // task keeps the work going even when nobody awaits the operation.
        tokio::spawn(shared.clone());
        shared
    }

    /// Awaits an operation that `begin` already put on its own task.
    async fn await_operation(&self, running: SharedOperation) -> OperationResult {
        running.await
    }

    /// `#trackAttachmentOperation` — disposal awaits whatever is registered.
    fn track_attachment(
        &self,
        run: BoxFuture<'static, OperationResult>,
    ) -> BoxFuture<'static, OperationResult> {
        let pending = run.shared();
        let id = {
            let mut data = self.lock();
            let id = data.next_attachment_id;
            data.next_attachment_id += 1;
            data.pending_attachments.push((id, pending.clone()));
            id
        };
        // The attachment keeps running even after the operation that started it
        // gave up on disposal — `dispose` still has to await its cleanup.
        tokio::spawn(pending.clone());
        let this = self.clone();
        Box::pin(async move {
            let result = pending.await;
            this.lock()
                .pending_attachments
                .retain(|(candidate, _)| *candidate != id);
            result
        })
    }

    /// `#bind`.
    fn bind(
        &self,
        handle: SessionHandle,
        known_snapshot: Option<SessionSnapshot>,
    ) -> OperationResult {
        let Some(snapshot) = known_snapshot.or_else(|| handle.snapshot()) else {
            return Err(RemoteSessionError::Message(format!(
                "Session {} did not provide a snapshot",
                handle.id()
            )));
        };
        {
            let mut data = self.lock();
            Self::clear_subscriptions(&mut data);
            data.handle = Some(handle.clone());
            data.transcript = Some(create_transcript_state(snapshot));
        }
        let snapshots = Arc::downgrade(&self.inner);
        let unsubscribe_snapshot = handle.subscribe(Arc::new(move |next: &SessionSnapshot| {
            let Some(inner) = snapshots.upgrade() else {
                return;
            };
            let session = RemoteSession { inner };
            let state = {
                let mut data = session.lock();
                let Some(transcript) = data.transcript.take() else {
                    return;
                };
                data.transcript = Some(apply_transcript_snapshot(transcript, next.clone()));
                Self::state_of(&data)
            };
            session.notify_with(&state);
        }))?;
        let events = Arc::downgrade(&self.inner);
        let unsubscribe_events = handle.on_event(Arc::new(move |event: &ServerEvent| {
            let Some(inner) = events.upgrade() else {
                return;
            };
            RemoteSession { inner }.handle_event(event);
        }))?;
        let mut data = self.lock();
        data.unsubscribe_snapshot = Some(unsubscribe_snapshot);
        data.unsubscribe_events = Some(unsubscribe_events);
        Ok(())
    }

    /// `#handleEvent`.
    fn handle_event(&self, event: &ServerEvent) {
        let state = match event {
            ServerEvent::SessionRemoved(_) => {
                let mut data = self.lock();
                Self::clear_subscriptions(&mut data);
                data.handle = None;
                data.transcript = None;
                if !matches!(data.lifecycle, RemoteSessionLifecycle::Busy { .. }) {
                    data.lifecycle = RemoteSessionLifecycle::Unbound;
                    data.lifecycle_token = 0;
                }
                Self::state_of(&data)
            }
            ServerEvent::SessionProgress(progress) => {
                let mut data = self.lock();
                let Some(transcript) = data.transcript.take() else {
                    return;
                };
                data.transcript = Some(apply_transcript_progress(transcript, &progress.progress));
                Self::state_of(&data)
            }
            _ => return,
        };
        self.notify_with(&state);
    }

    fn notify_with(&self, state: &RemoteSessionState) {
        for listener in self.listener_snapshot() {
            self.call_listener(&listener, state);
        }
    }

    fn listener_snapshot(&self) -> Vec<StateListener> {
        self.lock()
            .listeners
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect()
    }

    /// A panicking subscriber is reported and cannot stop the others, the way
    /// TS catches a throwing listener.
    fn call_listener(&self, listener: &StateListener, state: &RemoteSessionState) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| listener(state))) {
            self.report_listener_error(panic_message(&payload));
        }
    }

    fn report_listener_error(&self, message: String) {
        let Some(handler) = &self.inner.on_listener_error else {
            return;
        };
        // Diagnostics cannot affect session or transport state.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            handler(RemoteSessionError::Message(message))
        }));
    }

    fn clear_subscriptions(data: &mut Data) {
        if let Some(unsubscribe) = data.unsubscribe_snapshot.take() {
            unsubscribe();
        }
        if let Some(unsubscribe) = data.unsubscribe_events.take() {
            unsubscribe();
        }
    }

    fn require_handle(&self) -> Result<SessionHandle, RemoteSessionError> {
        self.lock()
            .handle
            .clone()
            .ok_or_else(|| RemoteSessionError::Message("No remote session is attached".to_owned()))
    }

    fn assert_available(&self) -> OperationResult {
        self.assert_not_disposed()?;
        if let RemoteSessionLifecycle::Busy { operation } = self.lock().lifecycle {
            return Err(RemoteSessionError::Message(format!(
                "Remote session is busy with {}",
                operation.as_str()
            )));
        }
        Ok(())
    }

    fn assert_not_disposed(&self) -> OperationResult {
        if self.disposed() {
            return Err(RemoteSessionError::Disposed);
        }
        Ok(())
    }

    /// `#assertNotDisposedAfterAwait` — a lease acquired after disposal is
    /// handed straight back.
    async fn assert_not_disposed_after_await(&self, handle: &SessionHandle) -> OperationResult {
        if !self.disposed() {
            return Ok(());
        }
        self.detach(handle).await?;
        Err(RemoteSessionError::DisposedDuringAttachment)
    }

    async fn detach(&self, handle: &SessionHandle) -> OperationResult {
        handle.dispose().await.map_err(RemoteSessionError::from)
    }
}

/// The `finally` half of `#runOperation`.
struct OperationGuard {
    session: RemoteSession,
    token: u64,
    previous: RemoteSessionLifecycle,
    previous_token: u64,
    preempt: bool,
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        let state = {
            let mut data = self.session.lock();
            data.active_operation_tokens.remove(&self.token);
            if data.lifecycle == RemoteSessionLifecycle::Disposed
                || data.lifecycle_token != self.token
            {
                return;
            }
            // A preempted operation that is still running gets its state back.
            if self.preempt && data.active_operation_tokens.contains(&self.previous_token) {
                data.lifecycle = self.previous;
                data.lifecycle_token = self.previous_token;
            } else if data.handle.is_some() {
                data.lifecycle = RemoteSessionLifecycle::Ready;
                data.lifecycle_token = 0;
            } else {
                data.lifecycle = RemoteSessionLifecycle::Unbound;
                data.lifecycle_token = 0;
            }
            RemoteSession::state_of(&data)
        };
        self.session.notify_with(&state);
    }
}

/// `settleRemoteSessionDisposal` — `RemoteSessionDisposedError` is the expected
/// consequence of disposal and is not reported.
async fn settle_remote_session_disposal(cleanup: Vec<SharedOperation>) -> OperationResult {
    let results = futures::future::join_all(cleanup).await;
    let mut errors: Vec<RemoteSessionError> = results
        .into_iter()
        .filter_map(|result| match result {
            Err(RemoteSessionError::DisposedDuringAttachment) => None,
            Err(error) => Some(error),
            Ok(()) => None,
        })
        .collect();
    match errors.len() {
        0 => Ok(()),
        1 => Err(errors.remove(0)),
        _ => Err(RemoteSessionError::Aggregate {
            context: "dispose remote session".to_owned(),
            errors,
        }),
    }
}

fn phase_name(phase: Option<SessionPhase>) -> &'static str {
    match phase {
        Some(SessionPhase::Idle) => "idle",
        Some(SessionPhase::Turn) => "turn",
        Some(SessionPhase::Compaction) => "compaction",
        Some(SessionPhase::BranchSummary) => "branch_summary",
        Some(SessionPhase::Retry) => "retry",
        None => "unknown",
    }
}

fn phase_or_unavailable(phase: Option<SessionPhase>) -> &'static str {
    match phase {
        None => "unavailable",
        other => phase_name(other),
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    "Unknown listener failure".to_owned()
}
