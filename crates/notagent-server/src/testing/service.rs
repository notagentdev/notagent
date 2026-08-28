use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use notagent_protocol::{
    AssistantStopReason, AssistantTag, AssistantTranscriptItem, CompleteAssistantTranscriptItem,
    CompleteTag, ModelCost, ModelInput, ModelMetadata, ModelRef, SessionMetadata, SessionPhase,
    SessionSnapshot, TextContent, TextTag, ThinkingLevel, TranscriptItem, TranscriptProgress,
    UserContent, UserTag, UserTranscriptItem,
};
use tokio::sync::oneshot;

use crate::errors::{PiServerError, ServerError};
use crate::types::{
    CreateSessionOptions, PiServerService, PiSessionRuntime, PiSessionRuntimeEvent, PromptInput,
    RuntimeEventListener, Unsubscribe,
};

pub fn test_model() -> ModelMetadata {
    ModelMetadata {
        provider: "test".to_owned(),
        id: "small".to_owned(),
        name: "Test Small".to_owned(),
        api: "test-api".to_owned(),
        reasoning: true,
        input: vec![ModelInput::Text, ModelInput::Image],
        context_window: 16_000,
        max_tokens: 2_000,
        cost: ModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
        supported_thinking_levels: vec![
            ThinkingLevel::Off,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ],
        authenticated: true,
    }
}

pub struct Deferred<T: Clone + Send + 'static> {
    sender: Mutex<Option<oneshot::Sender<T>>>,
    promise: Shared<BoxFuture<'static, T>>,
}

impl<T: Clone + Send + 'static> Default for Deferred<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + Send + 'static> Deferred<T> {
    pub fn new() -> Self {
        let (sender, receiver) = oneshot::channel::<T>();
        let promise: BoxFuture<'static, T> = Box::pin(async move {
            receiver
                .await
                .expect("deferred was dropped without resolving")
        });
        Self {
            sender: Mutex::new(Some(sender)),
            promise: promise.shared(),
        }
    }

    pub fn resolve(&self, value: T) {
        if let Some(sender) = self.sender.lock().expect("deferred mutex").take() {
            let _ = sender.send(value);
        }
    }

    pub fn promise(&self) -> Shared<BoxFuture<'static, T>> {
        self.promise.clone()
    }
}

type StoredSession = Arc<Mutex<SessionSnapshot>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptOutcome {
    Complete,
    Aborted,
}

pub struct TestSessionRuntime {
    pub disposed: Deferred<()>,
    dispose_count: AtomicU64,
    steers: Mutex<Vec<PromptInput>>,
    stored: StoredSession,
    on_dispose: Box<dyn Fn() + Send + Sync>,
    listeners: Arc<Mutex<Vec<(u64, RuntimeEventListener)>>>,
    next_listener_id: AtomicU64,
    pending_prompt: Mutex<Option<Arc<Deferred<PromptOutcome>>>>,
}

impl TestSessionRuntime {
    fn new(stored: StoredSession, on_dispose: Box<dyn Fn() + Send + Sync>) -> Self {
        Self {
            disposed: Deferred::new(),
            dispose_count: AtomicU64::new(0),
            steers: Mutex::new(Vec::new()),
            stored,
            on_dispose,
            listeners: Arc::new(Mutex::new(Vec::new())),
            next_listener_id: AtomicU64::new(0),
            pending_prompt: Mutex::new(None),
        }
    }

    pub fn dispose_count(&self) -> u64 {
        self.dispose_count.load(Ordering::SeqCst)
    }

    pub fn steers(&self) -> Vec<PromptInput> {
        self.steers.lock().expect("runtime mutex").clone()
    }

    pub fn stored_snapshot(&self) -> SessionSnapshot {
        self.stored.lock().expect("stored mutex").clone()
    }

    pub fn set_phase(&self, phase: SessionPhase) {
        self.stored.lock().expect("stored mutex").phase = phase;
    }

    pub fn finish_prompt(&self) {
        let pending = self.pending_prompt.lock().expect("runtime mutex").clone();
        let pending = pending.expect("No prompt is pending");
        pending.resolve(PromptOutcome::Complete);
    }

    pub fn emit_progress(&self, progress: TranscriptProgress) {
        self.emit(PiSessionRuntimeEvent::Progress(progress));
    }

    pub fn emit_error(&self, error: PiServerError) {
        self.emit(PiSessionRuntimeEvent::Error(error));
    }

    pub fn emit_snapshot(&self) {
        self.emit(PiSessionRuntimeEvent::Snapshot);
    }

    fn emit(&self, event: PiSessionRuntimeEvent) {
        let listeners: Vec<RuntimeEventListener> = self
            .listeners
            .lock()
            .expect("runtime mutex")
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        for listener in listeners {
            listener(&event);
        }
    }

    fn update(&self, apply: impl FnOnce(&mut SessionSnapshot)) {
        {
            let mut snapshot = self.stored.lock().expect("stored mutex");
            apply(&mut snapshot);
            snapshot.revision += 1;
            snapshot.updated_at += 1;
        }
        self.emit_snapshot();
    }
}

fn user_item(id: String, text: String, timestamp: u64) -> TranscriptItem {
    TranscriptItem::User(UserTranscriptItem {
        id,
        role: UserTag,
        content: vec![UserContent::Text(TextContent {
            kind: TextTag,
            text,
        })],
        timestamp,
    })
}

#[async_trait]
impl PiSessionRuntime for TestSessionRuntime {
    async fn snapshot(&self) -> Result<SessionSnapshot, ServerError> {
        Ok(self.stored_snapshot())
    }

    fn get_phase(&self) -> SessionPhase {
        self.stored.lock().expect("stored mutex").phase
    }

    async fn prompt(&self, input: PromptInput) -> Result<(), ServerError> {
        if self.get_phase() != SessionPhase::Idle {
            return Err(PiServerError::busy("A prompt is already running").into());
        }
        let done = Arc::new(Deferred::<PromptOutcome>::new());
        *self.pending_prompt.lock().expect("runtime mutex") = Some(Arc::clone(&done));
        let text = input.text.clone();
        self.update(|snapshot| {
            let next = snapshot.revision + 1;
            snapshot.phase = SessionPhase::Turn;
            snapshot
                .transcript
                .push(user_item(format!("user-{next}"), text, next));
        });
        let outcome = done.promise().await;
        self.update(|snapshot| {
            let next = snapshot.revision + 1;
            let (text, status_complete) = match outcome {
                PromptOutcome::Complete => (format!("reply:{}", input.text), true),
                PromptOutcome::Aborted => (String::new(), false),
            };
            let content = vec![notagent_protocol::AssistantContent::Text(TextContent {
                kind: TextTag,
                text,
            })];
            let assistant = if status_complete {
                AssistantTranscriptItem::Complete(CompleteAssistantTranscriptItem {
                    id: format!("assistant-{next}"),
                    role: AssistantTag,
                    content,
                    model: snapshot.model.clone(),
                    response_model: None,
                    usage: None,
                    timestamp: next,
                    status: CompleteTag,
                    stop_reason: AssistantStopReason::Stop,
                })
            } else {
                AssistantTranscriptItem::Aborted(
                    notagent_protocol::AbortedAssistantTranscriptItem {
                        id: format!("assistant-{next}"),
                        role: AssistantTag,
                        content,
                        model: snapshot.model.clone(),
                        response_model: None,
                        usage: None,
                        timestamp: next,
                        status: notagent_protocol::AbortedTag,
                        stop_reason: notagent_protocol::AbortedTag,
                        error_message: None,
                    },
                )
            };
            snapshot.phase = SessionPhase::Idle;
            snapshot
                .transcript
                .push(TranscriptItem::Assistant(assistant));
        });
        *self.pending_prompt.lock().expect("runtime mutex") = None;
        Ok(())
    }

    async fn steer(&self, input: PromptInput) -> Result<(), ServerError> {
        if self.get_phase() == SessionPhase::Idle {
            return Err(PiServerError::busy("There is no active prompt to steer").into());
        }
        self.steers
            .lock()
            .expect("runtime mutex")
            .push(input.clone());
        self.update(|snapshot| {
            let next = snapshot.revision + 1;
            snapshot.queued_steer_count += 1;
            snapshot.queued_steer.push(UserTranscriptItem {
                id: format!("steer-{next}"),
                role: UserTag,
                content: vec![UserContent::Text(TextContent {
                    kind: TextTag,
                    text: input.text,
                })],
                timestamp: next,
            });
        });
        Ok(())
    }

    async fn abort(&self) -> Result<(), ServerError> {
        let pending = self.pending_prompt.lock().expect("runtime mutex").clone();
        let Some(pending) = pending else {
            return Err(PiServerError::busy("There is no active prompt to abort").into());
        };
        pending.resolve(PromptOutcome::Aborted);
        Ok(())
    }

    async fn set_model(&self, model: ModelRef) -> Result<(), ServerError> {
        if self.get_phase() != SessionPhase::Idle {
            return Err(PiServerError::busy("Session is busy").into());
        }
        self.update(|snapshot| snapshot.model = model);
        Ok(())
    }

    async fn set_thinking(&self, thinking_level: ThinkingLevel) -> Result<(), ServerError> {
        if self.get_phase() != SessionPhase::Idle {
            return Err(PiServerError::busy("Session is busy").into());
        }
        self.update(|snapshot| snapshot.thinking_level = thinking_level);
        Ok(())
    }

    fn subscribe(&self, listener: RuntimeEventListener) -> Unsubscribe {
        let id = self.next_listener_id.fetch_add(1, Ordering::SeqCst);
        self.listeners
            .lock()
            .expect("runtime mutex")
            .push((id, listener));
        let listeners = Arc::clone(&self.listeners);
        Box::new(move || {
            listeners
                .lock()
                .expect("runtime mutex")
                .retain(|(candidate, _)| *candidate != id);
        })
    }

    async fn dispose(&self) -> Result<(), ServerError> {
        self.dispose_count.fetch_add(1, Ordering::SeqCst);
        (self.on_dispose)();
        self.disposed.resolve(());
        Ok(())
    }
}

pub struct ListDelay {
    pub entered: Arc<Deferred<()>>,
    pub release: Arc<Deferred<()>>,
}

#[derive(Default)]
struct TestServerServiceInner {
    sessions: Mutex<HashMap<String, StoredSession>>,
    runtimes: Mutex<HashMap<String, Vec<Arc<TestSessionRuntime>>>>,
    locked: Mutex<HashSet<String>>,
    last_created_id: Mutex<Option<String>>,
    next_list_delay: Mutex<Option<Arc<ListDelay>>>,
}

/// cheap `Clone` over shared state so tests can hold it and hand it to the
/// server at the same time.
#[derive(Clone, Default)]
pub struct TestServerService {
    inner: Arc<TestServerServiceInner>,
}

impl TestServerService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_service(&self) -> Arc<dyn PiServerService> {
        Arc::new(self.clone())
    }

    pub fn last_created_id(&self) -> Option<String> {
        self.inner
            .last_created_id
            .lock()
            .expect("service mutex")
            .clone()
    }

    pub fn locked(&self) -> HashSet<String> {
        self.inner.locked.lock().expect("service mutex").clone()
    }

    pub fn lock_session(&self, id: &str) {
        self.inner
            .locked
            .lock()
            .expect("service mutex")
            .insert(id.to_owned());
    }

    pub fn runtime_count(&self, id: &str) -> usize {
        self.inner
            .runtimes
            .lock()
            .expect("service mutex")
            .get(id)
            .map(Vec::len)
            .unwrap_or(0)
    }

    pub fn latest_runtime(&self, id: &str) -> Arc<TestSessionRuntime> {
        self.inner
            .runtimes
            .lock()
            .expect("service mutex")
            .get(id)
            .and_then(|runtimes| runtimes.last().cloned())
            .unwrap_or_else(|| panic!("No runtime for {id}"))
    }

    pub fn seed(&self, id: &str) {
        self.seed_with(
            id,
            &format!("Session {id}"),
            "/tmp/notagent-server-conformance",
            None,
            None,
        );
    }

    pub fn seed_with(
        &self,
        id: &str,
        name: &str,
        cwd: &str,
        model: Option<ModelRef>,
        thinking_level: Option<ThinkingLevel>,
    ) {
        let model = model.unwrap_or_else(|| {
            let test_model = test_model();
            ModelRef {
                provider: test_model.provider,
                id: test_model.id,
            }
        });
        let snapshot = SessionSnapshot {
            id: id.to_owned(),
            name: Some(name.to_owned()),
            cwd: cwd.to_owned(),
            created_at: 1,
            updated_at: 1,
            phase: SessionPhase::Idle,
            model,
            thinking_level: thinking_level.unwrap_or(ThinkingLevel::Off),
            attached: false,
            locked: false,
            revision: 0,
            transcript: vec![],
            queued_steer: vec![],
            queued_steer_count: 0,
        };
        self.inner
            .sessions
            .lock()
            .expect("service mutex")
            .insert(id.to_owned(), Arc::new(Mutex::new(snapshot)));
    }

    pub fn delay_next_list(&self) -> Arc<ListDelay> {
        let delay = Arc::new(ListDelay {
            entered: Arc::new(Deferred::new()),
            release: Arc::new(Deferred::new()),
        });
        *self.inner.next_list_delay.lock().expect("service mutex") = Some(Arc::clone(&delay));
        delay
    }

    pub async fn stored_sessions(&self) -> Vec<SessionMetadata> {
        let delay = self
            .inner
            .next_list_delay
            .lock()
            .expect("service mutex")
            .take();
        if let Some(delay) = delay {
            delay.entered.resolve(());
            delay.release.promise().await;
        }
        let snapshots: Vec<SessionSnapshot> = {
            let sessions = self.inner.sessions.lock().expect("service mutex");
            let mut snapshots: Vec<SessionSnapshot> = sessions
                .values()
                .map(|stored| stored.lock().expect("stored mutex").clone())
                .collect();
            snapshots.sort_by(|left, right| left.id.cmp(&right.id));
            snapshots
        };
        snapshots
            .into_iter()
            .map(|snapshot| SessionMetadata {
                id: snapshot.id,
                created_at: snapshot.created_at,
                updated_at: Some(snapshot.updated_at),
                parent_session_id: None,
                session_name: snapshot.name,
                cwd: Some(snapshot.cwd),
            })
            .collect()
    }

    fn acquire(&self, id: &str) -> Result<Arc<dyn PiSessionRuntime>, ServerError> {
        let stored = self
            .inner
            .sessions
            .lock()
            .expect("service mutex")
            .get(id)
            .cloned()
            .ok_or_else(|| ServerError::other(format!("Unknown session: {id}")))?;
        self.inner
            .locked
            .lock()
            .expect("service mutex")
            .insert(id.to_owned());
        let service = self.clone();
        let unlock_id = id.to_owned();
        let runtime = Arc::new(TestSessionRuntime::new(
            stored,
            Box::new(move || {
                service
                    .inner
                    .locked
                    .lock()
                    .expect("service mutex")
                    .remove(&unlock_id);
            }),
        ));
        self.inner
            .runtimes
            .lock()
            .expect("service mutex")
            .entry(id.to_owned())
            .or_default()
            .push(Arc::clone(&runtime));
        Ok(runtime)
    }
}

#[async_trait]
impl PiServerService for TestServerService {
    async fn list_sessions(&self) -> Result<Vec<SessionMetadata>, ServerError> {
        Ok(self.stored_sessions().await)
    }

    async fn list_models(&self) -> Result<Vec<ModelMetadata>, ServerError> {
        Ok(vec![test_model()])
    }

    async fn create_session(
        &self,
        options: CreateSessionOptions,
    ) -> Result<Arc<dyn PiSessionRuntime>, ServerError> {
        *self.inner.last_created_id.lock().expect("service mutex") = Some(options.id.clone());
        if self
            .inner
            .sessions
            .lock()
            .expect("service mutex")
            .contains_key(&options.id)
        {
            return Err(PiServerError::locked("Session already exists").into());
        }
        let test_model = test_model();
        self.seed_with(
            &options.id,
            &options
                .name
                .unwrap_or_else(|| format!("Session {}", options.id)),
            &options
                .cwd
                .unwrap_or_else(|| "/tmp/notagent-server-conformance".to_owned()),
            Some(options.model.unwrap_or(ModelRef {
                provider: test_model.provider,
                id: test_model.id,
            })),
            options.thinking_level,
        );
        self.acquire(&options.id)
    }

    async fn open_session(
        &self,
        session_id: &str,
    ) -> Result<Arc<dyn PiSessionRuntime>, ServerError> {
        if !self
            .inner
            .sessions
            .lock()
            .expect("service mutex")
            .contains_key(session_id)
        {
            return Err(PiServerError::not_found(format!("Unknown session: {session_id}")).into());
        }
        if self
            .inner
            .locked
            .lock()
            .expect("service mutex")
            .contains(session_id)
        {
            return Err(PiServerError::locked(format!("Session is locked: {session_id}")).into());
        }
        self.acquire(session_id)
    }
}
