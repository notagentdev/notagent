use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, Weak};

use notagent_protocol::{CommandResult, ServerEvent, ServerSnapshot, SessionSnapshot};

use crate::errors::PiError;
use crate::types::{ListenerErrorHandler, Unsubscribe};

pub type Listener<T> = Arc<dyn Fn(&T) + Send + Sync>;

/// individual listeners can unsubscribe again.
struct ListenerSet<T> {
    next_id: u64,
    listeners: Vec<(u64, Listener<T>)>,
}

impl<T> Default for ListenerSet<T> {
    fn default() -> Self {
        Self {
            next_id: 0,
            listeners: Vec::new(),
        }
    }
}

impl<T> ListenerSet<T> {
    fn add(&mut self, listener: Listener<T>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.listeners.push((id, listener));
        id
    }

    fn remove(&mut self, id: u64) {
        self.listeners.retain(|(candidate, _)| *candidate != id);
    }

    fn snapshot(&self) -> Vec<Listener<T>> {
        self.listeners
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect()
    }

    fn is_empty(&self) -> bool {
        self.listeners.is_empty()
    }

    fn clear(&mut self) {
        self.listeners.clear();
    }
}

#[derive(Default)]
struct StateData {
    session_snapshots: HashMap<String, SessionSnapshot>,
    attached_session_ids: HashSet<String>,
    snapshot_listeners: ListenerSet<ServerSnapshot>,
    event_listeners: ListenerSet<ServerEvent>,
    session_snapshot_listeners: HashMap<String, ListenerSet<SessionSnapshot>>,
    session_event_listeners: HashMap<String, ListenerSet<ServerEvent>>,
    snapshot: Option<ServerSnapshot>,
}

pub(crate) struct ClientState {
    data: Arc<Mutex<StateData>>,
    on_listener_error: Option<ListenerErrorHandler>,
}

impl ClientState {
    pub(crate) fn new(on_listener_error: Option<ListenerErrorHandler>) -> Self {
        Self {
            data: Arc::new(Mutex::new(StateData::default())),
            on_listener_error,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StateData> {
        self.data.lock().expect("client state mutex")
    }

    pub(crate) fn snapshot(&self) -> Option<ServerSnapshot> {
        self.lock().snapshot.clone()
    }

    pub(crate) fn reset(&self) {
        let mut data = self.lock();
        data.snapshot = None;
        data.session_snapshots.clear();
        data.attached_session_ids.clear();
    }

    pub(crate) fn clear_attachments(&self) {
        self.lock().attached_session_ids.clear();
    }

    pub(crate) fn dispose(&self) {
        self.reset();
        let mut data = self.lock();
        data.snapshot_listeners.clear();
        data.event_listeners.clear();
        data.session_snapshot_listeners.clear();
        data.session_event_listeners.clear();
    }

    pub(crate) fn get_session_snapshot(&self, session_id: &str) -> Option<SessionSnapshot> {
        self.lock().session_snapshots.get(session_id).cloned()
    }

    pub(crate) fn is_session_attached(&self, session_id: &str) -> bool {
        self.lock().attached_session_ids.contains(session_id)
    }

    pub(crate) fn forget_session_snapshot(&self, session_id: &str) -> Option<SessionSnapshot> {
        self.lock().session_snapshots.remove(session_id)
    }

    pub(crate) fn restore_session_snapshot(&self, snapshot: SessionSnapshot) {
        let mut data = self.lock();
        data.session_snapshots
            .entry(snapshot.id.clone())
            .or_insert(snapshot);
    }

    pub(crate) fn subscribe(&self, listener: Listener<ServerSnapshot>) -> Unsubscribe {
        let id = self.lock().snapshot_listeners.add(listener);
        let data = Arc::downgrade(&self.data);
        Box::new(move || {
            if let Some(data) = data.upgrade() {
                data.lock()
                    .expect("client state mutex")
                    .snapshot_listeners
                    .remove(id);
            }
        })
    }

    pub(crate) fn on_event(&self, listener: Listener<ServerEvent>) -> Unsubscribe {
        let id = self.lock().event_listeners.add(listener);
        let data = Arc::downgrade(&self.data);
        Box::new(move || {
            if let Some(data) = data.upgrade() {
                data.lock()
                    .expect("client state mutex")
                    .event_listeners
                    .remove(id);
            }
        })
    }

    pub(crate) fn subscribe_session(
        &self,
        session_id: &str,
        listener: Listener<SessionSnapshot>,
    ) -> Unsubscribe {
        let id = self
            .lock()
            .session_snapshot_listeners
            .entry(session_id.to_owned())
            .or_default()
            .add(listener);
        remove_mapped_listener(Arc::downgrade(&self.data), session_id.to_owned(), id, true)
    }

    pub(crate) fn on_session_event(
        &self,
        session_id: &str,
        listener: Listener<ServerEvent>,
    ) -> Unsubscribe {
        let id = self
            .lock()
            .session_event_listeners
            .entry(session_id.to_owned())
            .or_default()
            .add(listener);
        remove_mapped_listener(Arc::downgrade(&self.data), session_id.to_owned(), id, false)
    }

    pub(crate) fn apply_result(&self, result: &CommandResult) {
        match result {
            CommandResult::List(_) => (),
            CommandResult::Detach(detach) => {
                let previous = {
                    let mut data = self.lock();
                    data.attached_session_ids.remove(&detach.session_id);
                    data.session_snapshots.get(&detach.session_id).cloned()
                };
                if let Some(mut snapshot) = previous {
                    snapshot.attached = false;
                    self.apply_session_snapshot(snapshot, true);
                }
            }
            CommandResult::Create(result) => {
                self.apply_session_snapshot(result.session.clone(), false)
            }
            CommandResult::Attach(result) => {
                self.apply_session_snapshot(result.session.clone(), false)
            }
            CommandResult::Prompt(result) => {
                self.apply_session_snapshot(result.session.clone(), false)
            }
            CommandResult::Steer(result) => {
                self.apply_session_snapshot(result.session.clone(), false)
            }
            CommandResult::Abort(result) => {
                self.apply_session_snapshot(result.session.clone(), false)
            }
            CommandResult::SetModel(result) => {
                self.apply_session_snapshot(result.session.clone(), false)
            }
            CommandResult::SetThinking(result) => {
                self.apply_session_snapshot(result.session.clone(), false)
            }
        }
    }

    pub(crate) fn apply_event(&self, event: &ServerEvent) {
        match event {
            ServerEvent::ServerSnapshot(server) => {
                self.apply_server_snapshot(server.snapshot.clone())
            }
            ServerEvent::SessionSnapshot(session) => {
                self.apply_session_snapshot(session.snapshot.clone(), false)
            }
            ServerEvent::SessionRemoved(removed) => {
                let mut data = self.lock();
                data.session_snapshots.remove(&removed.session_id);
                data.attached_session_ids.remove(&removed.session_id);
            }
            ServerEvent::SessionProgress(_) => (),
        }
        let listeners = self.lock().event_listeners.snapshot();
        self.notify(listeners, event);
        if let Some(session_id) = event_session_id(event) {
            let listeners = self
                .lock()
                .session_event_listeners
                .get(&session_id)
                .map(ListenerSet::snapshot)
                .unwrap_or_default();
            self.notify(listeners, event);
        }
    }

    pub(crate) fn apply_server_snapshot(&self, snapshot: ServerSnapshot) {
        let listeners = {
            let mut data = self.lock();
            if let Some(current) = &data.snapshot
                && snapshot.revision < current.revision
            {
                return;
            }
            data.snapshot = Some(snapshot.clone());
            data.snapshot_listeners.snapshot()
        };
        self.notify(listeners, &snapshot);
    }

    fn apply_session_snapshot(&self, snapshot: SessionSnapshot, force: bool) {
        let listeners = {
            let mut data = self.lock();
            if !force
                && let Some(current) = data.session_snapshots.get(&snapshot.id)
                && snapshot.revision < current.revision
            {
                return;
            }
            data.session_snapshots
                .insert(snapshot.id.clone(), snapshot.clone());
            if snapshot.attached {
                data.attached_session_ids.insert(snapshot.id.clone());
            } else {
                data.attached_session_ids.remove(&snapshot.id);
            }
            data.session_snapshot_listeners
                .get(&snapshot.id)
                .map(ListenerSet::snapshot)
                .unwrap_or_default()
        };
        self.notify(listeners, &snapshot);
    }

    /// re-enter the client. A panicking listener is caught and reported just
    /// like a JS exception.
    fn notify<T>(&self, listeners: Vec<Listener<T>>, value: &T) {
        for listener in listeners {
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| listener(value))) {
                self.report_listener_error(panic_message(&payload));
            }
        }
    }

    pub(crate) fn report_listener_error(&self, message: String) {
        let Some(handler) = &self.on_listener_error else {
            return;
        };
        // Diagnostics cannot affect client state.
        let _ = catch_unwind(AssertUnwindSafe(|| handler(PiError::Other(message))));
    }
}

fn remove_mapped_listener(
    data: Weak<Mutex<StateData>>,
    session_id: String,
    id: u64,
    snapshots: bool,
) -> Unsubscribe {
    Box::new(move || {
        let Some(data) = data.upgrade() else { return };
        let mut data = data.lock().expect("client state mutex");
        if snapshots {
            if let Some(listeners) = data.session_snapshot_listeners.get_mut(&session_id) {
                listeners.remove(id);
                if listeners.is_empty() {
                    data.session_snapshot_listeners.remove(&session_id);
                }
            }
        } else if let Some(listeners) = data.session_event_listeners.get_mut(&session_id) {
            listeners.remove(id);
            if listeners.is_empty() {
                data.session_event_listeners.remove(&session_id);
            }
        }
    })
}

pub(crate) fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    "Unknown listener failure".to_owned()
}

fn event_session_id(event: &ServerEvent) -> Option<String> {
    match event {
        ServerEvent::SessionSnapshot(session) => Some(session.snapshot.id.clone()),
        ServerEvent::SessionProgress(progress) => Some(progress.session_id.clone()),
        ServerEvent::SessionRemoved(removed) => Some(removed.session_id.clone()),
        ServerEvent::ServerSnapshot(_) => None,
    }
}
