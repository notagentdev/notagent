use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use notagent_protocol::{
    AttachResult, AttachTag, Command, CommandResult, CreateResult, CreateTag, DetachResult,
    DetachTag, EventEnvelope, EventTag, ListResult, ListTag, ServerEvent, SessionMetadata,
    SessionPhase, SessionProgressEvent, SessionProgressTag, SessionSnapshot, SessionSnapshotEvent,
    SessionSnapshotTag,
};

use crate::connection::{ConnectionStage, ConnectionState};
use crate::errors::{PiServerError, ServerError};
use crate::server::PiServerInner;
use crate::types::{
    CreateSessionOptions, PiSessionRuntime, PiSessionRuntimeEvent, PromptInput, Unsubscribe,
};

type RuntimeFactory = Arc<
    dyn Fn() -> BoxFuture<'static, Result<Arc<dyn PiSessionRuntime>, ServerError>> + Send + Sync,
>;
type DisposePromise = Shared<BoxFuture<'static, ()>>;
type OpeningPromise = Shared<BoxFuture<'static, Result<Arc<LiveSession>, ServerError>>>;

pub(crate) struct LiveSession {
    pub(crate) id: String,
    pub(crate) runtime: Arc<dyn PiSessionRuntime>,
    connections: Mutex<Vec<Arc<ConnectionState>>>,
    unsubscribe: Mutex<Option<Unsubscribe>>,
    operation_count: AtomicI64,
    ready: AtomicBool,
    terminal: AtomicBool,
    disposing: Mutex<Option<DisposePromise>>,
}

impl LiveSession {
    fn connection_count(&self) -> usize {
        self.connections.lock().expect("live session mutex").len()
    }

    fn add_connection(&self, connection: &Arc<ConnectionState>) {
        let mut connections = self.connections.lock().expect("live session mutex");
        if !connections
            .iter()
            .any(|candidate| Arc::ptr_eq(candidate, connection))
        {
            connections.push(Arc::clone(connection));
        }
    }

    fn remove_connection(&self, connection: &Arc<ConnectionState>) {
        self.connections
            .lock()
            .expect("live session mutex")
            .retain(|candidate| !Arc::ptr_eq(candidate, connection));
    }

    fn connection_list(&self) -> Vec<Arc<ConnectionState>> {
        self.connections.lock().expect("live session mutex").clone()
    }

    fn unsubscribe(&self) {
        if let Some(unsubscribe) = self.unsubscribe.lock().expect("live session mutex").take() {
            unsubscribe();
        }
    }

    fn disposing(&self) -> Option<DisposePromise> {
        self.disposing.lock().expect("live session mutex").clone()
    }
}

fn to_metadata(snapshot: &SessionSnapshot) -> SessionMetadata {
    SessionMetadata {
        id: snapshot.id.clone(),
        created_at: snapshot.created_at,
        updated_at: Some(snapshot.updated_at),
        parent_session_id: None,
        session_name: snapshot.name.clone(),
        cwd: Some(snapshot.cwd.clone()),
    }
}

pub(crate) struct LiveSessionManager {
    host: Weak<PiServerInner>,
    live_sessions: Mutex<HashMap<String, Arc<LiveSession>>>,
    opening_sessions: Mutex<HashMap<String, OpeningPromise>>,
}

impl LiveSessionManager {
    pub(crate) fn new(host: Weak<PiServerInner>) -> Self {
        Self {
            host,
            live_sessions: Mutex::new(HashMap::new()),
            opening_sessions: Mutex::new(HashMap::new()),
        }
    }

    fn host(&self) -> Option<Arc<PiServerInner>> {
        self.host.upgrade()
    }

    fn is_closing(&self) -> bool {
        self.host().is_some_and(|host| host.is_closing())
    }

    fn report_error(&self, error: ServerError) {
        if let Some(host) = self.host() {
            host.report_error(error);
        }
    }

    pub(crate) async fn execute_command(
        self: &Arc<Self>,
        connection: &Arc<ConnectionState>,
        command: Command,
    ) -> Result<CommandResult, ServerError> {
        let host = self
            .host()
            .ok_or_else(|| ServerError::other("PiServer is gone"))?;
        match command {
            Command::List(_) => Ok(CommandResult::List(ListResult {
                command: ListTag,
                sessions: self.list_metadata().await?,
            })),
            Command::Create(create) => {
                let id = uuid::Uuid::new_v4().to_string();
                let options = CreateSessionOptions {
                    id: id.clone(),
                    cwd: create.cwd,
                    name: create.name,
                    model: create.model,
                    thinking_level: create.thinking_level,
                };
                let service = Arc::clone(&host.service);
                let factory: RuntimeFactory = Arc::new(move || {
                    let service = Arc::clone(&service);
                    let options = options.clone();
                    Box::pin(async move { service.create_session(options).await })
                });
                let live = self.acquire(&id, factory).await?;
                self.attach(connection, &live).await?;
                let session =
                    Self::for_connection(self.broadcast_snapshot(&live).await?, connection);
                host.broadcast_server_snapshot();
                Ok(CommandResult::Create(CreateResult {
                    command: CreateTag,
                    session,
                }))
            }
            Command::Attach(attach) => {
                let session_id = attach.session_id;
                let service = Arc::clone(&host.service);
                let factory_id = session_id.clone();
                let factory: RuntimeFactory = Arc::new(move || {
                    let service = Arc::clone(&service);
                    let session_id = factory_id.clone();
                    Box::pin(async move { service.open_session(&session_id).await })
                });
                let live = self.acquire(&session_id, factory).await?;
                self.attach(connection, &live).await?;
                let session =
                    Self::for_connection(self.broadcast_snapshot(&live).await?, connection);
                host.broadcast_server_snapshot();
                Ok(CommandResult::Attach(AttachResult {
                    command: AttachTag,
                    session,
                }))
            }
            Command::Detach(detach) => {
                let session_id = detach.session_id;
                let live = self
                    .live_sessions
                    .lock()
                    .expect("sessions mutex")
                    .get(&session_id)
                    .cloned();
                let attached = connection.lock().session_ids.contains(&session_id);
                if attached {
                    connection.lock().session_ids.remove(&session_id);
                    if let Some(live) = live {
                        live.remove_connection(connection);
                        if live.connection_count() > 0
                            && !live.terminal.load(Ordering::SeqCst)
                            && live.disposing().is_none()
                        {
                            self.broadcast_snapshot(&live).await?;
                        }
                        self.maybe_dispose(&live).await;
                    }
                    host.broadcast_server_snapshot();
                }
                Ok(CommandResult::Detach(DetachResult {
                    command: DetachTag,
                    session_id,
                }))
            }
            Command::Prompt(prompt) => {
                let live = self.require_attached(connection, &prompt.session_id)?;
                let runtime = Arc::clone(&live.runtime);
                let input = PromptInput { text: prompt.text };
                let session = self
                    .run_operation(
                        connection,
                        &live,
                        Box::pin(async move { runtime.prompt(input).await }),
                    )
                    .await?;
                Ok(CommandResult::Prompt(notagent_protocol::PromptResult {
                    command: notagent_protocol::PromptTag,
                    session,
                }))
            }
            Command::Steer(steer) => {
                let live = self.require_attached(connection, &steer.session_id)?;
                let runtime = Arc::clone(&live.runtime);
                let input = PromptInput { text: steer.text };
                let session = self
                    .run_operation(
                        connection,
                        &live,
                        Box::pin(async move { runtime.steer(input).await }),
                    )
                    .await?;
                Ok(CommandResult::Steer(notagent_protocol::SteerResult {
                    command: notagent_protocol::SteerTag,
                    session,
                }))
            }
            Command::Abort(abort) => {
                let live = self.require_attached(connection, &abort.session_id)?;
                let runtime = Arc::clone(&live.runtime);
                let session = self
                    .run_operation(
                        connection,
                        &live,
                        Box::pin(async move { runtime.abort().await }),
                    )
                    .await?;
                Ok(CommandResult::Abort(notagent_protocol::AbortResult {
                    command: notagent_protocol::AbortTag,
                    session,
                }))
            }
            Command::SetModel(set_model) => {
                let live = self.require_attached(connection, &set_model.session_id)?;
                let runtime = Arc::clone(&live.runtime);
                let model = set_model.model;
                let session = self
                    .run_operation(
                        connection,
                        &live,
                        Box::pin(async move { runtime.set_model(model).await }),
                    )
                    .await?;
                Ok(CommandResult::SetModel(notagent_protocol::SetModelResult {
                    command: notagent_protocol::SetModelTag,
                    session,
                }))
            }
            Command::SetThinking(set_thinking) => {
                let live = self.require_attached(connection, &set_thinking.session_id)?;
                let runtime = Arc::clone(&live.runtime);
                let thinking_level = set_thinking.thinking_level;
                let session = self
                    .run_operation(
                        connection,
                        &live,
                        Box::pin(async move { runtime.set_thinking(thinking_level).await }),
                    )
                    .await?;
                Ok(CommandResult::SetThinking(
                    notagent_protocol::SetThinkingResult {
                        command: notagent_protocol::SetThinkingTag,
                        session,
                    },
                ))
            }
        }
    }

    pub(crate) async fn disconnect(self: &Arc<Self>, connection: &Arc<ConnectionState>) {
        let session_ids: Vec<String> = {
            let mut data = connection.lock();
            let ids = data.session_ids.iter().cloned().collect();
            data.session_ids.clear();
            ids
        };
        let sessions: Vec<Arc<LiveSession>> = {
            let live_sessions = self.live_sessions.lock().expect("sessions mutex");
            session_ids
                .iter()
                .filter_map(|id| live_sessions.get(id).cloned())
                .collect()
        };
        for live in &sessions {
            live.remove_connection(connection);
        }
        for live in &sessions {
            self.maybe_dispose(live).await;
        }
    }

    pub(crate) async fn list_metadata(
        self: &Arc<Self>,
    ) -> Result<Vec<SessionMetadata>, ServerError> {
        let host = self
            .host()
            .ok_or_else(|| ServerError::other("PiServer is gone"))?;
        let stored = host.service.list_sessions().await?;
        let live: Vec<Arc<LiveSession>> = self
            .live_sessions
            .lock()
            .expect("sessions mutex")
            .values()
            .filter(|live| live.disposing().is_none())
            .cloned()
            .collect();
        let mut live_by_id: HashMap<String, SessionSnapshot> = HashMap::new();
        for session in live {
            let snapshot = self.normalized_snapshot(&session).await?;
            live_by_id.insert(session.id.clone(), snapshot);
        }
        let mut metadata: Vec<SessionMetadata> = Vec::with_capacity(stored.len());
        for item in stored {
            match live_by_id.remove(&item.id) {
                Some(snapshot) => {
                    let mut merged = item;
                    let live_metadata = to_metadata(&snapshot);
                    merged.id = live_metadata.id;
                    merged.created_at = live_metadata.created_at;
                    merged.updated_at = live_metadata.updated_at;
                    merged.session_name = live_metadata.session_name;
                    merged.cwd = live_metadata.cwd;
                    metadata.push(merged);
                }
                None => metadata.push(item),
            }
        }
        for snapshot in live_by_id.values() {
            metadata.push(to_metadata(snapshot));
        }
        Ok(metadata)
    }

    pub(crate) async fn close(self: &Arc<Self>) {
        let opening: Vec<OpeningPromise> = self
            .opening_sessions
            .lock()
            .expect("sessions mutex")
            .values()
            .cloned()
            .collect();
        for pending in opening {
            if let Err(error) = pending.await {
                self.report_error(error);
            }
        }
        let sessions: Vec<Arc<LiveSession>> = {
            let mut live_sessions = self.live_sessions.lock().expect("sessions mutex");
            let sessions = live_sessions.values().cloned().collect();
            live_sessions.clear();
            sessions
        };
        for live in sessions {
            if let Some(disposing) = live.disposing() {
                disposing.await;
                continue;
            }
            live.unsubscribe();
            if let Err(error) = live.runtime.dispose().await {
                self.report_error(error);
            }
        }
    }

    async fn run_operation(
        self: &Arc<Self>,
        connection: &Arc<ConnectionState>,
        live: &Arc<LiveSession>,
        operation: BoxFuture<'static, Result<(), ServerError>>,
    ) -> Result<SessionSnapshot, ServerError> {
        live.operation_count.fetch_add(1, Ordering::SeqCst);
        let result = async {
            operation.await?;
            Ok(Self::for_connection(
                self.broadcast_snapshot(live).await?,
                connection,
            ))
        }
        .await;
        live.operation_count.fetch_sub(1, Ordering::SeqCst);
        self.schedule_maybe_dispose(live);
        result
    }

    async fn acquire(
        self: &Arc<Self>,
        id: &str,
        factory: RuntimeFactory,
    ) -> Result<Arc<LiveSession>, ServerError> {
        loop {
            let existing = self
                .live_sessions
                .lock()
                .expect("sessions mutex")
                .get(id)
                .cloned();
            if let Some(existing) = existing {
                if existing.terminal.load(Ordering::SeqCst) {
                    return Err(PiServerError::locked(format!(
                        "Session runtime is terminating: {id}"
                    ))
                    .into());
                }
                if let Some(disposing) = existing.disposing() {
                    disposing.await;
                    continue;
                }
                return Ok(existing);
            }
            let opening = self
                .opening_sessions
                .lock()
                .expect("sessions mutex")
                .get(id)
                .cloned();
            if let Some(opening) = opening {
                return opening.await;
            }
            let manager = Arc::clone(self);
            let session_id = id.to_owned();
            let factory = Arc::clone(&factory);
            let pending: OpeningPromise =
                (Box::pin(async move { manager.create(&session_id, factory).await })
                    as BoxFuture<'static, Result<Arc<LiveSession>, ServerError>>)
                    .shared();
            self.opening_sessions
                .lock()
                .expect("sessions mutex")
                .insert(id.to_owned(), pending.clone());
            let result = pending.clone().await;
            {
                let mut opening_sessions = self.opening_sessions.lock().expect("sessions mutex");
                if opening_sessions
                    .get(id)
                    .is_some_and(|current| current.ptr_eq(&pending))
                {
                    opening_sessions.remove(id);
                }
            }
            return result;
        }
    }

    async fn create(
        self: &Arc<Self>,
        id: &str,
        factory: RuntimeFactory,
    ) -> Result<Arc<LiveSession>, ServerError> {
        let runtime = factory().await?;
        if self.is_closing() {
            let _ = runtime.dispose().await;
            return Err(ServerError::other(
                "PiServer closed while acquiring a session runtime",
            ));
        }
        let snapshot = match runtime.snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if let Err(dispose_error) = runtime.dispose().await {
                    self.report_error(dispose_error);
                }
                return Err(error);
            }
        };
        if snapshot.id != id {
            if let Err(dispose_error) = runtime.dispose().await {
                self.report_error(dispose_error);
            }
            return Err(PiServerError::invalid_request(format!(
                "Service returned session {} for server-assigned session {id}",
                snapshot.id
            ))
            .into());
        }
        let live = Arc::new(LiveSession {
            id: id.to_owned(),
            runtime: Arc::clone(&runtime),
            connections: Mutex::new(Vec::new()),
            unsubscribe: Mutex::new(None),
            operation_count: AtomicI64::new(0),
            ready: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
            disposing: Mutex::new(None),
        });
        let manager = Arc::downgrade(self);
        let event_session = Arc::downgrade(&live);
        let unsubscribe = runtime.subscribe(Arc::new(move |event: &PiSessionRuntimeEvent| {
            let (Some(manager), Some(live)) = (manager.upgrade(), event_session.upgrade()) else {
                return;
            };
            manager.handle_runtime_event(live, event.clone());
        }));
        *live.unsubscribe.lock().expect("live session mutex") = Some(unsubscribe);
        self.live_sessions
            .lock()
            .expect("sessions mutex")
            .insert(id.to_owned(), Arc::clone(&live));
        live.ready.store(true, Ordering::SeqCst);
        Ok(live)
    }

    fn handle_runtime_event(
        self: &Arc<Self>,
        live: Arc<LiveSession>,
        event: PiSessionRuntimeEvent,
    ) {
        let manager = Arc::clone(self);
        match event {
            PiSessionRuntimeEvent::Error(error) => {
                tokio::spawn(async move { manager.terminate(&live, error).await });
            }
            PiSessionRuntimeEvent::Progress(progress) => {
                let Some(host) = self.host() else { return };
                let envelope = EventEnvelope {
                    kind: EventTag,
                    event: ServerEvent::SessionProgress(SessionProgressEvent {
                        kind: SessionProgressTag,
                        session_id: live.id.clone(),
                        progress,
                    }),
                };
                for connection in live.connection_list() {
                    let host = Arc::clone(&host);
                    let envelope = envelope.clone();
                    tokio::spawn(async move {
                        host.send_event(&connection, envelope).await;
                    });
                }
                self.schedule_maybe_dispose(&live);
            }
            PiSessionRuntimeEvent::Snapshot => {
                let broadcast_manager = Arc::clone(self);
                let broadcast_live = Arc::clone(&live);
                tokio::spawn(async move {
                    if let Err(error) = broadcast_manager.broadcast_snapshot(&broadcast_live).await
                    {
                        broadcast_manager.report_error(error);
                    }
                });
                self.schedule_maybe_dispose(&live);
            }
        }
    }

    async fn terminate(self: &Arc<Self>, live: &Arc<LiveSession>, error: PiServerError) {
        if live.terminal.swap(true, Ordering::SeqCst) {
            return;
        }
        self.report_error(error.into());
        live.unsubscribe();
        let connections = live.connection_list();
        if let Some(host) = self.host() {
            for connection in &connections {
                host.close_byte_connection(&connection.connection, None)
                    .await;
            }
            for connection in &connections {
                host.disconnect(connection).await;
            }
        }
        self.maybe_dispose(live).await;
    }

    async fn normalized_snapshot(
        &self,
        live: &Arc<LiveSession>,
    ) -> Result<SessionSnapshot, ServerError> {
        let snapshot = live.runtime.snapshot().await?;
        if snapshot.id != live.id {
            return Err(PiServerError::invalid_request(format!(
                "Runtime session ID changed from {} to {}",
                live.id, snapshot.id
            ))
            .into());
        }
        Ok(SessionSnapshot {
            phase: live.runtime.get_phase(),
            attached: live.connection_count() > 0,
            locked: true,
            ..snapshot
        })
    }

    fn for_connection(
        snapshot: SessionSnapshot,
        connection: &Arc<ConnectionState>,
    ) -> SessionSnapshot {
        let attached = connection.lock().session_ids.contains(&snapshot.id);
        SessionSnapshot {
            attached,
            ..snapshot
        }
    }

    async fn broadcast_snapshot(
        self: &Arc<Self>,
        live: &Arc<LiveSession>,
    ) -> Result<SessionSnapshot, ServerError> {
        let snapshot = self.normalized_snapshot(live).await?;
        if let Some(host) = self.host() {
            let envelope = EventEnvelope {
                kind: EventTag,
                event: ServerEvent::SessionSnapshot(SessionSnapshotEvent {
                    kind: SessionSnapshotTag,
                    snapshot: snapshot.clone(),
                }),
            };
            for connection in live.connection_list() {
                host.send_event(&connection, envelope.clone()).await;
            }
        }
        Ok(snapshot)
    }

    async fn attach(
        self: &Arc<Self>,
        connection: &Arc<ConnectionState>,
        live: &Arc<LiveSession>,
    ) -> Result<(), ServerError> {
        let unusable = {
            let data = connection.lock();
            data.disconnected || data.stage != ConnectionStage::Ready
        } || connection.connection.closed();
        if unusable {
            self.maybe_dispose(live).await;
            return Err(PiServerError::invalid_request(
                "Connection closed while attaching to a session",
            )
            .into());
        }
        connection.lock().session_ids.insert(live.id.clone());
        live.add_connection(connection);
        Ok(())
    }

    fn require_attached(
        &self,
        connection: &Arc<ConnectionState>,
        session_id: &str,
    ) -> Result<Arc<LiveSession>, ServerError> {
        if !connection.lock().session_ids.contains(session_id) {
            return Err(PiServerError::invalid_request(format!(
                "Connection is not attached to session {session_id}"
            ))
            .into());
        }
        let live = self
            .live_sessions
            .lock()
            .expect("sessions mutex")
            .get(session_id)
            .cloned();
        match live {
            Some(live) if !live.terminal.load(Ordering::SeqCst) && live.disposing().is_none() => {
                Ok(live)
            }
            _ => Err(PiServerError::not_found(format!("Session is not live: {session_id}")).into()),
        }
    }

    fn schedule_maybe_dispose(self: &Arc<Self>, live: &Arc<LiveSession>) {
        let manager = Arc::clone(self);
        let live = Arc::clone(live);
        tokio::spawn(async move {
            manager.maybe_dispose(&live).await;
        });
    }

    async fn maybe_dispose(self: &Arc<Self>, live: &Arc<LiveSession>) {
        let disposing = {
            let existing = live.disposing();
            if self.is_closing()
                || !live.ready.load(Ordering::SeqCst)
                || existing.is_some()
                || live.connection_count() > 0
                || live.operation_count.load(Ordering::SeqCst) > 0
                || (!live.terminal.load(Ordering::SeqCst)
                    && live.runtime.get_phase() != SessionPhase::Idle)
            {
                if let Some(existing) = existing {
                    existing.await;
                }
                return;
            }
            live.unsubscribe();
            let manager = Arc::clone(self);
            let dispose_live = Arc::clone(live);
            let promise: DisposePromise = (Box::pin(async move {
                if let Err(error) = dispose_live.runtime.dispose().await {
                    manager.report_error(error);
                }
                let mut live_sessions = manager.live_sessions.lock().expect("sessions mutex");
                if live_sessions
                    .get(&dispose_live.id)
                    .is_some_and(|current| Arc::ptr_eq(current, &dispose_live))
                {
                    live_sessions.remove(&dispose_live.id);
                }
            }) as BoxFuture<'static, ()>)
                .shared();
            *live.disposing.lock().expect("live session mutex") = Some(promise.clone());
            promise
        };
        disposing.await;
        if !self.is_closing()
            && let Some(host) = self.host()
        {
            host.broadcast_server_snapshot();
        }
    }
}
