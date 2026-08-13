//! Port of `packages/server/src/snapshots.ts`.

use std::sync::{Arc, Mutex};

use notagent_protocol::{
    EventEnvelope, EventTag, ModelMetadata, ProtocolVersionTag, ServerEvent, ServerSnapshot,
    ServerSnapshotEvent, ServerSnapshotTag, SessionMetadata,
};

use crate::connection::{ConnectionStage, ConnectionState};

pub(crate) struct ServerSnapshotPublisher {
    server_id: String,
    revision: Mutex<u64>,
    /// TS serializes broadcasts through a promise queue; Rust uses a fair async mutex.
    broadcast_queue: tokio::sync::Mutex<()>,
}

impl ServerSnapshotPublisher {
    pub(crate) fn new(server_id: String) -> Self {
        Self {
            server_id,
            revision: Mutex::new(0),
            broadcast_queue: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn current_revision(&self) -> u64 {
        *self.revision.lock().expect("revision mutex")
    }

    /// TS builds the snapshot as an object literal: `revision: this.revision` is
    /// evaluated *before* the awaited `listSessions()`/`listModels()` calls, so a
    /// concurrent broadcast during those awaits cannot change it. Callers
    /// therefore capture the revision first and pass it in.
    pub(crate) fn build(
        &self,
        revision: u64,
        sessions: Vec<SessionMetadata>,
        models: Vec<ModelMetadata>,
    ) -> ServerSnapshot {
        ServerSnapshot {
            server_id: self.server_id.clone(),
            protocol_version: ProtocolVersionTag,
            revision,
            sessions,
            models,
        }
    }

    pub(crate) fn next_revision(&self) -> u64 {
        let mut revision = self.revision.lock().expect("revision mutex");
        *revision += 1;
        *revision
    }

    pub(crate) fn ready_connections(
        connections: Vec<Arc<ConnectionState>>,
    ) -> Vec<Arc<ConnectionState>> {
        connections
            .into_iter()
            .filter(|connection| {
                let data = connection.lock();
                data.stage == ConnectionStage::Ready && !data.disconnected
            })
            .collect()
    }

    pub(crate) fn envelope(snapshot: ServerSnapshot) -> EventEnvelope {
        EventEnvelope {
            kind: EventTag,
            event: ServerEvent::ServerSnapshot(ServerSnapshotEvent {
                kind: ServerSnapshotTag,
                snapshot,
            }),
        }
    }

    pub(crate) async fn broadcast_lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.broadcast_queue.lock().await
    }
}
