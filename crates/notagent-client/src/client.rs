use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, Weak};

use notagent_protocol::{
    Command, CommandResult, ServerEvent, ServerSnapshot, SessionMetadata, SessionSnapshot,
    encode_client_message,
};

use crate::connection::{Connection, ConnectionCallbacks, ServerNonHandshakeMessage};
use crate::errors::PiError;
use crate::promise::{
    Resolver, SettleResult, SharedPromise, create_promise_resolvers, promise, rejected, resolved,
};
use crate::session_handle::{
    AcquireSessionOptions, SessionHandle, SessionHandleCallbacks, SessionLeaseMode,
};
use crate::state::{ClientState, Listener, panic_message};
use crate::types::{
    ConnectionState, ConnectionStateChange, CreateSessionOptions, ListenerErrorHandler,
    PiClientOptions, Unsubscribe,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionLeaseState {
    Active,
    Releasing,
    Released,
    Invalidated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionLeaseToken {
    id: u64,
    mode: SessionLeaseMode,
}

struct PendingRequest {
    command: Command,
    resolver: Arc<Resolver<CommandResult>>,
}

/// identity, Rust compares the sequence number).
struct Tracked {
    id: u64,
    promise: SharedPromise<()>,
}

#[derive(Default)]
struct ClientData {
    pending_requests: HashMap<String, PendingRequest>,
    session_lease_counts: HashMap<String, u64>,
    exclusive_session_leases: HashMap<String, u64>,
    session_lease_generations: HashMap<String, u64>,
    session_attachments: HashMap<String, Tracked>,
    session_detachments: HashMap<String, Tracked>,
    session_cleanup_required: HashSet<String>,
    session_reconciliations: HashMap<String, Tracked>,
    connection_state_listeners: Vec<(u64, Listener<ConnectionStateChange>)>,
    next_listener_id: u64,
    request_sequence: u64,
    lease_token_sequence: u64,
    tracked_sequence: u64,
    disposed: bool,
    dispose_promise: Option<SharedPromise<()>>,
}

pub(crate) struct ClientInner {
    connection: Arc<Connection>,
    state: ClientState,
    on_listener_error: Option<ListenerErrorHandler>,
    data: Mutex<ClientData>,
}

/// tasks can pass the client around like a JS object reference.
#[derive(Clone)]
pub struct PiClient {
    inner: Arc<ClientInner>,
}

impl std::fmt::Debug for PiClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PiClient")
            .field("state", &self.connection_state())
            .finish_non_exhaustive()
    }
}

impl PiClient {
    pub fn new(options: PiClientOptions) -> Result<Self, PiError> {
        let PiClientOptions {
            transport_factory,
            max_frame_length,
            on_listener_error,
        } = options;
        // Validate up front so that `Arc::new_cyclic` cannot fail.
        Connection::new(
            Arc::clone(&transport_factory),
            max_frame_length,
            Weak::<ClientInner>::new(),
        )?;
        let inner = Arc::new_cyclic(|weak: &Weak<ClientInner>| {
            let callbacks: Weak<dyn ConnectionCallbacks> = weak.clone();
            ClientInner {
                connection: Arc::new(
                    Connection::new(transport_factory, max_frame_length, callbacks)
                        .expect("validated options"),
                ),
                state: ClientState::new(on_listener_error.clone()),
                on_listener_error,
                data: Mutex::new(ClientData::default()),
            }
        });
        Ok(Self { inner })
    }

    pub async fn connect_with(options: PiClientOptions) -> Result<Self, PiError> {
        let client = Self::new(options)?;
        match client.connect().await {
            Ok(_) => Ok(client),
            Err(error) => {
                let _ = client.dispose().await;
                Err(error)
            }
        }
    }

    pub fn disposed(&self) -> bool {
        self.inner.data.lock().expect("client mutex").disposed
    }

    pub fn connection_state(&self) -> ConnectionState {
        self.inner.connection.state()
    }

    pub fn connected(&self) -> bool {
        self.inner.connection.state() == ConnectionState::Connected
    }

    pub fn snapshot(&self) -> Option<ServerSnapshot> {
        self.inner.state.snapshot()
    }

    pub fn connect(&self) -> impl Future<Output = Result<ServerSnapshot, PiError>> + Send + use<> {
        if self.disposed() {
            return rejected::<ServerSnapshot>(PiError::Disposed);
        }
        if self.inner.connection.state() == ConnectionState::Disconnected {
            self.inner.state.reset();
        }
        self.inner.connection.connect()
    }

    pub fn reconnect(
        &self,
    ) -> impl Future<Output = Result<ServerSnapshot, PiError>> + Send + use<> {
        self.connect()
    }

    pub fn disconnect(&self, reason: &str) {
        self.inner
            .connection
            .disconnect(PiError::Disconnected(reason.to_owned()));
    }

    pub fn subscribe(&self, listener: Listener<ServerSnapshot>) -> Result<Unsubscribe, PiError> {
        self.inner.assert_not_disposed()?;
        Ok(self.inner.state.subscribe(listener))
    }

    pub fn on_event(&self, listener: Listener<ServerEvent>) -> Result<Unsubscribe, PiError> {
        self.inner.assert_not_disposed()?;
        Ok(self.inner.state.on_event(listener))
    }

    pub fn on_connection_state_change(
        &self,
        listener: Listener<ConnectionStateChange>,
    ) -> Result<Unsubscribe, PiError> {
        self.inner.assert_not_disposed()?;
        let id = {
            let mut data = self.inner.data.lock().expect("client mutex");
            let id = data.next_listener_id;
            data.next_listener_id += 1;
            data.connection_state_listeners.push((id, listener));
            id
        };
        let inner = Arc::downgrade(&self.inner);
        Ok(Box::new(move || {
            if let Some(inner) = inner.upgrade() {
                inner
                    .data
                    .lock()
                    .expect("client mutex")
                    .connection_state_listeners
                    .retain(|(candidate, _)| *candidate != id);
            }
        }))
    }

    pub fn list_sessions(
        &self,
    ) -> impl Future<Output = Result<Vec<SessionMetadata>, PiError>> + Send + use<> {
        let response = self
            .inner
            .request(Command::List(notagent_protocol::ListCommand {
                command: notagent_protocol::ListTag,
            }));
        async move {
            match response.await? {
                CommandResult::List(result) => Ok(result.sessions),
                _ => Err(PiError::ProtocolValidation(
                    "Response has no session list".to_owned(),
                )),
            }
        }
    }

    pub fn create_session(
        &self,
        options: CreateSessionOptions,
    ) -> impl Future<Output = Result<SessionHandle, PiError>> + Send + use<> {
        let inner = Arc::clone(&self.inner);
        let response = inner.request(Command::Create(notagent_protocol::CreateCommand {
            command: notagent_protocol::CreateTag,
            cwd: options.cwd,
            name: options.name,
            model: options.model,
            thinking_level: options.thinking_level,
        }));
        async move {
            let session = match response.await? {
                CommandResult::Create(result) => result.session,
                _ => {
                    return Err(PiError::ProtocolValidation(
                        "Response has no session snapshot".to_owned(),
                    ));
                }
            };
            let token = inner.reserve_session_lease(&session.id, SessionLeaseMode::Exclusive)?;
            Ok(inner.create_session_lease(session.id, token))
        }
    }

    pub fn attach_session(
        &self,
        session_id: &str,
    ) -> impl Future<Output = Result<SessionHandle, PiError>> + Send + use<> {
        self.acquire_session(
            session_id,
            AcquireSessionOptions {
                mode: SessionLeaseMode::Shared,
            },
        )
    }

    pub fn acquire_session(
        &self,
        session_id: &str,
        options: AcquireSessionOptions,
    ) -> impl Future<Output = Result<SessionHandle, PiError>> + Send + use<> {
        let inner = Arc::clone(&self.inner);
        let session_id = session_id.to_owned();
        // sends the attach request before the first `await`.
        let started = inner.begin_acquire_session(&session_id, options.mode);
        async move {
            let step = match started {
                Ok(step) => step,
                Err(error) => return Err(error),
            };
            let token = step.token();
            let result = inner.finish_acquire_session(&session_id, step).await;
            match result {
                Ok(handle) => Ok(handle),
                Err(error) => {
                    inner.release_session_lease(&session_id, token);
                    Err(error)
                }
            }
        }
    }

    pub fn dispose(&self) -> impl Future<Output = Result<(), PiError>> + Send + use<> {
        self.inner.dispose()
    }
}

enum AcquireStep {
    Ready(SessionLeaseToken),
    Attach {
        token: SessionLeaseToken,
        attachment: SharedPromise<()>,
        attachment_id: u64,
    },
    Reconcile {
        token: SessionLeaseToken,
        reconciliation: SharedPromise<()>,
    },
    Wait {
        token: SessionLeaseToken,
        detachment: SharedPromise<()>,
    },
}

impl AcquireStep {
    fn token(&self) -> SessionLeaseToken {
        match self {
            Self::Ready(token)
            | Self::Attach { token, .. }
            | Self::Reconcile { token, .. }
            | Self::Wait { token, .. } => *token,
        }
    }
}

impl ClientInner {
    fn lock(&self) -> std::sync::MutexGuard<'_, ClientData> {
        self.data.lock().expect("client mutex")
    }

    fn assert_not_disposed(&self) -> Result<(), PiError> {
        if self.lock().disposed {
            Err(PiError::Disposed)
        } else {
            Ok(())
        }
    }

    fn next_tracked_id(&self) -> u64 {
        let mut data = self.lock();
        data.tracked_sequence += 1;
        data.tracked_sequence
    }

    fn request(self: &Arc<Self>, command: Command) -> SharedPromise<CommandResult> {
        if self.lock().disposed {
            return rejected(PiError::Disposed);
        }
        if self.connection.state() != ConnectionState::Connected {
            return rejected(PiError::disconnected());
        }
        let (resolver, response) = create_promise_resolvers::<CommandResult>();
        let id = {
            let mut data = self.lock();
            data.request_sequence += 1;
            let id = format!("request-{}", data.request_sequence);
            data.pending_requests.insert(
                id.clone(),
                PendingRequest {
                    command: command.clone(),
                    resolver: Arc::clone(&resolver),
                },
            );
            id
        };
        let frame = match encode_client_message(
            &notagent_protocol::ClientMessage::Request(notagent_protocol::RequestEnvelope {
                kind: notagent_protocol::RequestTag,
                id: id.clone(),
                request: command,
            }),
            Some(
                notagent_protocol::FrameDecoderOptions::with_max_frame_length(
                    self.connection.max_frame_length(),
                ),
            ),
        ) {
            Ok(frame) => frame,
            Err(error) => {
                if let Some(pending) = self.take_pending_request(&id) {
                    pending
                        .resolver
                        .reject(PiError::ProtocolValidation(error.message().to_owned()));
                }
                return response;
            }
        };
        if let Err(error) = self.connection.send(frame) {
            resolver.reject(error);
        }
        response
    }

    fn take_pending_request(&self, id: &str) -> Option<PendingRequest> {
        self.lock().pending_requests.remove(id)
    }

    fn reject_pending_requests(&self, error: PiError) {
        let requests: Vec<PendingRequest> = self
            .lock()
            .pending_requests
            .drain()
            .map(|(_, value)| value)
            .collect();
        for request in requests {
            request.resolver.reject(error.clone());
        }
    }

    fn reserve_session_lease(
        &self,
        session_id: &str,
        mode: SessionLeaseMode,
    ) -> Result<SessionLeaseToken, PiError> {
        let mut data = self.lock();
        let count = data
            .session_lease_counts
            .get(session_id)
            .copied()
            .unwrap_or(0);
        if mode == SessionLeaseMode::Exclusive && count > 0 {
            return Err(PiError::SessionOwnership {
                session_id: session_id.to_owned(),
                message: format!("Session {session_id} already has an active lease"),
            });
        }
        if mode == SessionLeaseMode::Shared
            && data.exclusive_session_leases.contains_key(session_id)
        {
            return Err(PiError::SessionOwnership {
                session_id: session_id.to_owned(),
                message: format!("Session {session_id} has an exclusive lease"),
            });
        }
        data.lease_token_sequence += 1;
        let token = SessionLeaseToken {
            id: data.lease_token_sequence,
            mode,
        };
        data.session_lease_counts
            .insert(session_id.to_owned(), count + 1);
        if mode == SessionLeaseMode::Exclusive {
            data.exclusive_session_leases
                .insert(session_id.to_owned(), token.id);
        }
        Ok(token)
    }

    fn release_session_lease(&self, session_id: &str, token: SessionLeaseToken) {
        let mut data = self.lock();
        let count = data
            .session_lease_counts
            .get(session_id)
            .copied()
            .unwrap_or(0);
        if count <= 1 {
            data.session_lease_counts.remove(session_id);
        } else {
            data.session_lease_counts
                .insert(session_id.to_owned(), count - 1);
        }
        if data.exclusive_session_leases.get(session_id) == Some(&token.id) {
            data.exclusive_session_leases.remove(session_id);
        }
    }

    fn invalidate_session_leases(&self, session_id: &str) {
        let mut data = self.lock();
        data.session_lease_counts.remove(session_id);
        data.exclusive_session_leases.remove(session_id);
        data.session_cleanup_required.remove(session_id);
        let generation = data
            .session_lease_generations
            .get(session_id)
            .copied()
            .unwrap_or(0);
        data.session_lease_generations
            .insert(session_id.to_owned(), generation + 1);
    }

    fn invalidate_all_session_leases(&self) {
        let session_ids: Vec<String> = self.lock().session_lease_counts.keys().cloned().collect();
        for session_id in session_ids {
            self.invalidate_session_leases(&session_id);
        }
        self.lock().session_cleanup_required.clear();
    }

    fn begin_acquire_session(
        self: &Arc<Self>,
        session_id: &str,
        mode: SessionLeaseMode,
    ) -> Result<AcquireStep, PiError> {
        self.assert_not_disposed()?;
        let token = self.reserve_session_lease(session_id, mode)?;
        if let Some(detachment) = self
            .lock()
            .session_detachments
            .get(session_id)
            .map(|tracked| tracked.promise.clone())
        {
            return Ok(AcquireStep::Wait { token, detachment });
        }
        if self.lock().session_cleanup_required.contains(session_id) {
            return Ok(AcquireStep::Reconcile {
                token,
                reconciliation: self.reconcile_session_cleanup(session_id),
            });
        }
        if self.state.is_session_attached(session_id) {
            return Ok(AcquireStep::Ready(token));
        }
        let (attachment, attachment_id) = self.start_attachment(session_id);
        Ok(AcquireStep::Attach {
            token,
            attachment,
            attachment_id,
        })
    }

    async fn finish_acquire_session(
        self: &Arc<Self>,
        session_id: &str,
        step: AcquireStep,
    ) -> Result<SessionHandle, PiError> {
        match step {
            AcquireStep::Ready(token) => {
                Ok(self.create_session_lease(session_id.to_owned(), token))
            }
            AcquireStep::Attach {
                token,
                attachment,
                attachment_id,
            } => {
                let result = attachment.await;
                self.forget_attachment(session_id, attachment_id);
                result?;
                Ok(self.create_session_lease(session_id.to_owned(), token))
            }
            AcquireStep::Reconcile {
                token,
                reconciliation,
            } => {
                reconciliation.await?;
                self.attach_and_create_lease(session_id, token, true).await
            }
            AcquireStep::Wait { token, detachment } => {
                let _ = detachment.await;
                let reconciled = if self.lock().session_cleanup_required.contains(session_id) {
                    self.reconcile_session_cleanup(session_id).await?;
                    true
                } else {
                    false
                };
                self.attach_and_create_lease(session_id, token, reconciled)
                    .await
            }
        }
    }

    async fn attach_and_create_lease(
        self: &Arc<Self>,
        session_id: &str,
        token: SessionLeaseToken,
        reconciled: bool,
    ) -> Result<SessionHandle, PiError> {
        if reconciled || !self.state.is_session_attached(session_id) {
            let (attachment, attachment_id) = self.start_attachment(session_id);
            let result = attachment.await;
            self.forget_attachment(session_id, attachment_id);
            result?;
        }
        Ok(self.create_session_lease(session_id.to_owned(), token))
    }

    fn start_attachment(self: &Arc<Self>, session_id: &str) -> (SharedPromise<()>, u64) {
        if let Some(tracked) = self.lock().session_attachments.get(session_id) {
            return (tracked.promise.clone(), tracked.id);
        }
        let previous = self.state.forget_session_snapshot(session_id);
        let response = self.request(Command::Attach(notagent_protocol::AttachCommand {
            command: notagent_protocol::AttachTag,
            session_id: session_id.to_owned(),
        }));
        let inner = Arc::clone(self);
        let attachment = promise(async move {
            match response.await {
                Ok(_) => Ok(()),
                Err(error) => {
                    if let Some(previous) = previous {
                        inner.state.restore_session_snapshot(previous);
                    }
                    Err(error)
                }
            }
        });
        let id = self.next_tracked_id();
        self.lock().session_attachments.insert(
            session_id.to_owned(),
            Tracked {
                id,
                promise: attachment.clone(),
            },
        );
        (attachment, id)
    }

    fn forget_attachment(&self, session_id: &str, attachment_id: u64) {
        let mut data = self.lock();
        if data
            .session_attachments
            .get(session_id)
            .is_some_and(|tracked| tracked.id == attachment_id)
        {
            data.session_attachments.remove(session_id);
        }
    }

    fn reconcile_session_cleanup(self: &Arc<Self>, session_id: &str) -> SharedPromise<()> {
        if let Some(tracked) = self.lock().session_reconciliations.get(session_id) {
            return tracked.promise.clone();
        }
        let response = self.request(Command::Detach(notagent_protocol::DetachCommand {
            command: notagent_protocol::DetachTag,
            session_id: session_id.to_owned(),
        }));
        let inner = Arc::clone(self);
        let id = self.next_tracked_id();
        let session = session_id.to_owned();
        let reconciliation = promise(async move {
            let result = response.await;
            {
                let mut data = inner.lock();
                if data
                    .session_reconciliations
                    .get(&session)
                    .is_some_and(|tracked| tracked.id == id)
                {
                    data.session_reconciliations.remove(&session);
                }
            }
            result?;
            inner.lock().session_cleanup_required.remove(&session);
            Ok(())
        });
        self.lock().session_reconciliations.insert(
            session_id.to_owned(),
            Tracked {
                id,
                promise: reconciliation.clone(),
            },
        );
        reconciliation
    }

    fn create_session_lease(
        self: &Arc<Self>,
        session_id: String,
        token: SessionLeaseToken,
    ) -> SessionHandle {
        let generation = {
            let mut data = self.lock();
            let generation = data
                .session_lease_generations
                .get(&session_id)
                .copied()
                .unwrap_or(0);
            data.session_lease_generations
                .insert(session_id.clone(), generation);
            generation
        };
        let lease = Arc::new(Mutex::new(LeaseCell {
            state: SessionLeaseState::Active,
            release: None,
        }));

        let refresh = {
            let inner = Arc::clone(self);
            let session_id = session_id.clone();
            let lease = Arc::clone(&lease);
            Arc::new(move || {
                let mut cell = lease.lock().expect("lease mutex");
                if matches!(
                    cell.state,
                    SessionLeaseState::Active | SessionLeaseState::Releasing
                ) && inner
                    .lock()
                    .session_lease_generations
                    .get(&session_id)
                    .copied()
                    .unwrap_or(0)
                    != generation
                {
                    cell.state = SessionLeaseState::Invalidated;
                }
            })
        };
        let is_active = {
            let inner = Arc::clone(self);
            let session_id = session_id.clone();
            let lease = Arc::clone(&lease);
            let refresh = Arc::clone(&refresh);
            Arc::new(move || {
                refresh();
                lease.lock().expect("lease mutex").state == SessionLeaseState::Active
                    && inner.state.is_session_attached(&session_id)
            })
        };
        let assert_active: Arc<dyn Fn() -> Result<(), PiError> + Send + Sync> = {
            let inner = Arc::clone(self);
            let session_id = session_id.clone();
            let is_active = Arc::clone(&is_active);
            Arc::new(move || {
                inner.assert_not_disposed()?;
                if inner.connection.state() != ConnectionState::Connected {
                    return Err(PiError::disconnected());
                }
                if !is_active() {
                    return Err(PiError::session_detached(&session_id));
                }
                Ok(())
            })
        };
        let release: Arc<dyn Fn(bool) -> SharedPromise<()> + Send + Sync> = {
            let inner = Arc::clone(self);
            let session_id = session_id.clone();
            let lease = Arc::clone(&lease);
            let refresh = Arc::clone(&refresh);
            let assert_active = Arc::clone(&assert_active);
            Arc::new(move |relinquish_on_failure: bool| {
                refresh();
                {
                    let cell = lease.lock().expect("lease mutex");
                    if matches!(
                        cell.state,
                        SessionLeaseState::Released | SessionLeaseState::Invalidated
                    ) {
                        return resolved(());
                    }
                    if let Some(release) = &cell.release {
                        return release.clone();
                    }
                }
                if let Err(error) = assert_active() {
                    return rejected(error);
                }
                lease.lock().expect("lease mutex").state = SessionLeaseState::Releasing;

                let count = inner
                    .lock()
                    .session_lease_counts
                    .get(&session_id)
                    .copied()
                    .unwrap_or(0);
                let detachment = if count <= 1 {
                    let response =
                        inner.request(Command::Detach(notagent_protocol::DetachCommand {
                            command: notagent_protocol::DetachTag,
                            session_id: session_id.clone(),
                        }));
                    let id = inner.next_tracked_id();
                    let detachment = promise(async move { response.await.map(|_| ()) });
                    inner.lock().session_detachments.insert(
                        session_id.clone(),
                        Tracked {
                            id,
                            promise: detachment.clone(),
                        },
                    );
                    Some((detachment, id))
                } else {
                    None
                };

                let release_inner = Arc::clone(&inner);
                let release_session = session_id.clone();
                let release_lease = Arc::clone(&lease);
                let release_refresh = Arc::clone(&refresh);
                let release_promise = promise(async move {
                    let result: SettleResult<()> = match detachment {
                        Some((detachment, id)) => {
                            let result = detachment.await;
                            {
                                let mut data = release_inner.lock();
                                if data
                                    .session_detachments
                                    .get(&release_session)
                                    .is_some_and(|t| t.id == id)
                                {
                                    data.session_detachments.remove(&release_session);
                                }
                            }
                            match result {
                                Ok(()) => {
                                    release_inner.release_session_lease(&release_session, token);
                                    Ok(())
                                }
                                Err(error) => Err(error),
                            }
                        }
                        None => {
                            release_inner.release_session_lease(&release_session, token);
                            Ok(())
                        }
                    };
                    match result {
                        Ok(()) => {
                            release_lease.lock().expect("lease mutex").state =
                                SessionLeaseState::Released;
                            Ok(())
                        }
                        Err(error) => {
                            release_refresh();
                            let mut cell = release_lease.lock().expect("lease mutex");
                            if cell.state == SessionLeaseState::Invalidated {
                                return Ok(());
                            }
                            if relinquish_on_failure {
                                drop(cell);
                                release_inner.release_session_lease(&release_session, token);
                                release_inner
                                    .lock()
                                    .session_cleanup_required
                                    .insert(release_session.clone());
                                release_lease.lock().expect("lease mutex").state =
                                    SessionLeaseState::Released;
                            } else {
                                cell.state = SessionLeaseState::Active;
                                cell.release = None;
                            }
                            Err(error)
                        }
                    }
                });
                lease.lock().expect("lease mutex").release = Some(release_promise.clone());
                release_promise
            })
        };

        let callbacks = SessionHandleCallbacks {
            is_attached: {
                let is_active = Arc::clone(&is_active);
                Arc::new(move || is_active())
            },
            get_snapshot: {
                let inner = Arc::clone(self);
                let session_id = session_id.clone();
                let is_active = Arc::clone(&is_active);
                Arc::new(move || {
                    if is_active() {
                        inner.state.get_session_snapshot(&session_id)
                    } else {
                        None
                    }
                })
            },
            subscribe: {
                let inner = Arc::clone(self);
                let session_id = session_id.clone();
                let is_active = Arc::clone(&is_active);
                let assert_active = Arc::clone(&assert_active);
                Arc::new(move |listener: Listener<SessionSnapshot>| {
                    assert_active()?;
                    let is_active = Arc::clone(&is_active);
                    Ok(inner.state.subscribe_session(
                        &session_id,
                        Arc::new(move |snapshot: &SessionSnapshot| {
                            if is_active() {
                                listener(snapshot);
                            }
                        }),
                    ))
                })
            },
            on_event: {
                let inner = Arc::clone(self);
                let session_id = session_id.clone();
                let is_active = Arc::clone(&is_active);
                let assert_active = Arc::clone(&assert_active);
                Arc::new(move |listener: Listener<ServerEvent>| {
                    assert_active()?;
                    let is_active = Arc::clone(&is_active);
                    Ok(inner.state.on_session_event(
                        &session_id,
                        Arc::new(move |event: &ServerEvent| {
                            if is_active() || matches!(event, ServerEvent::SessionRemoved(_)) {
                                listener(event);
                            }
                        }),
                    ))
                })
            },
            detach: {
                let release = Arc::clone(&release);
                Arc::new(move || release(false))
            },
            dispose: {
                let release = Arc::clone(&release);
                Arc::new(move || release(true))
            },
            request: {
                let inner = Arc::clone(self);
                let assert_active = Arc::clone(&assert_active);
                Arc::new(move |command: Command| match assert_active() {
                    Ok(()) => inner.request(command),
                    Err(error) => rejected(error),
                })
            },
        };
        SessionHandle::new(session_id, callbacks)
    }

    fn dispose(self: &Arc<Self>) -> SharedPromise<()> {
        {
            let mut data = self.lock();
            if let Some(promise) = &data.dispose_promise {
                return promise.clone();
            }
            data.disposed = true;
            data.dispose_promise = Some(resolved(()));
        }
        let error = PiError::Disposed;
        self.reject_pending_requests(error.clone());
        self.connection.disconnect(error);
        self.state.dispose();
        self.invalidate_all_session_leases();
        self.lock().connection_state_listeners.clear();
        self.lock()
            .dispose_promise
            .clone()
            .expect("dispose promise")
    }

    fn notify_connection_state_listeners(&self, change: &ConnectionStateChange) {
        let listeners: Vec<Listener<ConnectionStateChange>> = self
            .lock()
            .connection_state_listeners
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        for listener in listeners {
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| listener(change))) {
                self.report_listener_error(panic_message(&payload));
            }
        }
    }

    fn report_listener_error(&self, message: String) {
        let Some(handler) = &self.on_listener_error else {
            return;
        };
        // Diagnostics cannot affect protocol or transport state.
        let _ = catch_unwind(AssertUnwindSafe(|| handler(PiError::Other(message))));
    }
}

struct LeaseCell {
    state: SessionLeaseState,
    release: Option<SharedPromise<()>>,
}

impl ConnectionCallbacks for ClientInner {
    fn on_handshake(&self, snapshot: ServerSnapshot) {
        self.state.apply_server_snapshot(snapshot);
    }

    fn on_message(&self, message: ServerNonHandshakeMessage) {
        match message {
            ServerNonHandshakeMessage::Event(envelope) => {
                if let ServerEvent::SessionRemoved(removed) = &envelope.event {
                    self.invalidate_session_leases(&removed.session_id);
                }
                self.state.apply_event(&envelope.event);
            }
            ServerNonHandshakeMessage::Response(response) => {
                let (id, ok) = match &response {
                    notagent_protocol::ResponseEnvelope::Ok(ok) => (ok.id.clone(), true),
                    notagent_protocol::ResponseEnvelope::Error(error) => (error.id.clone(), false),
                };
                let Some(pending) = self.take_pending_request(&id) else {
                    self.connection.fail(PiError::ProtocolValidation(
                        "Response has no matching request".to_owned(),
                    ));
                    return;
                };
                if !ok {
                    let notagent_protocol::ResponseEnvelope::Error(error) = response else {
                        unreachable!()
                    };
                    pending.resolver.reject(PiError::server(error.error));
                    return;
                }
                let notagent_protocol::ResponseEnvelope::Ok(ok) = response else {
                    unreachable!()
                };
                if ok.result.command() != pending.command.name() {
                    let error = PiError::ProtocolValidation(format!(
                        "Response command {} does not match {}",
                        command_name(ok.result.command()),
                        command_name(pending.command.name())
                    ));
                    pending.resolver.reject(error.clone());
                    self.connection.fail(error);
                    return;
                }
                self.state.apply_result(&ok.result);
                pending.resolver.resolve(ok.result);
            }
        }
    }

    fn on_state_change(&self, change: ConnectionStateChange) {
        if change.state == ConnectionState::Disconnected {
            self.state.clear_attachments();
            self.invalidate_all_session_leases();
            self.reject_pending_requests(
                change.error.clone().unwrap_or_else(PiError::disconnected),
            );
        }
        self.notify_connection_state_listeners(&change);
    }
}

fn command_name(name: notagent_protocol::CommandName) -> &'static str {
    match name {
        notagent_protocol::CommandName::List => "list",
        notagent_protocol::CommandName::Create => "create",
        notagent_protocol::CommandName::Attach => "attach",
        notagent_protocol::CommandName::Detach => "detach",
        notagent_protocol::CommandName::Prompt => "prompt",
        notagent_protocol::CommandName::Steer => "steer",
        notagent_protocol::CommandName::Abort => "abort",
        notagent_protocol::CommandName::SetModel => "set_model",
        notagent_protocol::CommandName::SetThinking => "set_thinking",
    }
}
