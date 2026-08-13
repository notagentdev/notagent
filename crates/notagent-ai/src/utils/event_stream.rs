//! Push-Queue mit Waiter-Liste, asynchron iterierbar, plus Ergebnis-Future.
//!
//! 1:1-Port von `packages/ai/src/utils/event-stream.ts` (88 LOC). Die TS-Klasse
//! kombiniert eine Warteschlange mit einer Liste wartender Konsumenten und einem
//! `result()`-Promise; in Rust übernimmt `Notify` die Waiter-Liste.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::types::{AssistantMessage, AssistantMessageEvent};

type IsComplete<T> = Box<dyn Fn(&T) -> bool + Send + Sync>;
type ExtractResult<T, R> = Box<dyn Fn(&T) -> R + Send + Sync>;

struct State<T, R> {
    queue: VecDeque<T>,
    done: bool,
    result: Option<R>,
}

struct Shared<T, R> {
    state: Mutex<State<T, R>>,
    notify: Notify,
    is_complete: IsComplete<T>,
    extract_result: ExtractResult<T, R>,
}

/// `EventStream<T, R>` — generischer Ereignisstrom mit Endergebnis.
///
/// Klonen liefert ein weiteres Handle auf denselben Strom (TS: dieselbe Objektreferenz).
pub struct EventStream<T, R> {
    shared: Arc<Shared<T, R>>,
}

impl<T, R> Clone for EventStream<T, R> {
    fn clone(&self) -> Self {
        EventStream {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T, R> EventStream<T, R> {
    pub fn new(
        is_complete: impl Fn(&T) -> bool + Send + Sync + 'static,
        extract_result: impl Fn(&T) -> R + Send + Sync + 'static,
    ) -> Self {
        EventStream {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    queue: VecDeque::new(),
                    done: false,
                    result: None,
                }),
                notify: Notify::new(),
                is_complete: Box::new(is_complete),
                extract_result: Box::new(extract_result),
            }),
        }
    }

    /// `push(event)` — nach dem Abschluss-Ereignis werden weitere Pushes ignoriert.
    pub fn push(&self, event: T) {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("EventStream-Zustand vergiftet");
        if state.done {
            return;
        }
        if (self.shared.is_complete)(&event) {
            state.done = true;
            state.result = Some((self.shared.extract_result)(&event));
        }
        state.queue.push_back(event);
        drop(state);
        self.shared.notify.notify_waiters();
    }

    /// `end(result?)` — beendet den Strom; ein übergebenes Ergebnis löst `result()` auf.
    pub fn end(&self, result: Option<R>) {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("EventStream-Zustand vergiftet");
        state.done = true;
        if result.is_some() {
            state.result = result;
        }
        drop(state);
        self.shared.notify.notify_waiters();
    }

    /// Nächstes Ereignis; `None`, sobald der Strom beendet und die Queue geleert ist.
    pub async fn next(&self) -> Option<T> {
        loop {
            let notified = self.shared.notify.notified();
            {
                let mut state = self
                    .shared
                    .state
                    .lock()
                    .expect("EventStream-Zustand vergiftet");
                if let Some(event) = state.queue.pop_front() {
                    return Some(event);
                }
                if state.done {
                    return None;
                }
            }
            notified.await;
        }
    }

    /// `result(): Promise<R>` — wartet auf das Abschluss-Ereignis.
    ///
    /// Abweichung Klasse 1: TS gibt dieselbe Objektreferenz an jeden Awaiter; in Rust wird
    /// je Aufruf geklont. Beobachtbarer Wert identisch.
    pub async fn result(&self) -> R
    where
        R: Clone,
    {
        loop {
            let notified = self.shared.notify.notified();
            {
                let state = self
                    .shared
                    .state
                    .lock()
                    .expect("EventStream-Zustand vergiftet");
                if let Some(result) = state.result.clone() {
                    return result;
                }
            }
            notified.await;
        }
    }

    /// True, sobald das Abschluss-Ereignis verarbeitet oder `end()` aufgerufen wurde.
    pub fn is_done(&self) -> bool {
        self.shared
            .state
            .lock()
            .expect("EventStream-Zustand vergiftet")
            .done
    }
}

/// `AssistantMessageEventStream` — vollständig bei `done`/`error`.
pub type AssistantMessageEventStream = EventStream<AssistantMessageEvent, AssistantMessage>;

/// `createAssistantMessageEventStream()`
pub fn create_assistant_message_event_stream() -> AssistantMessageEventStream {
    EventStream::new(
        |event| {
            matches!(
                event,
                AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
            )
        },
        |event| match event {
            AssistantMessageEvent::Done { message, .. } => message.clone(),
            AssistantMessageEvent::Error { error, .. } => error.clone(),
            // TS wirft hier; unerreichbar, weil `is_complete` genau diese beiden Varianten prüft.
            _ => unreachable!("Unexpected event type for final result"),
        },
    )
}
