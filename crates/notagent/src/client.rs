//! Client layer of the coding agent — port of `packages/coding-agent/src/client/`.
//!
//! Ported by workstream B under O-12 (`plans/interface-requests.md`); the ledger
//! section is "B: client layer" in `crates/notagent/PARITY.md`.
//! Port of `packages/coding-agent/src/client/index.ts` (15 LOC): the barrel
//! re-exports exactly the symbols the public subpath `@notagent/coding-agent/client`
//! exposes.

pub mod remote_session;
pub mod transcript;

pub use remote_session::{
    CreateRemoteSessionOptions, RemoteSession, RemoteSessionError, RemoteSessionLifecycle,
    RemoteSessionOperation, RemoteSessionOptions, RemoteSessionState,
};
pub use transcript::{
    TranscriptState, apply_transcript_progress, apply_transcript_snapshot, create_transcript_state,
    select_transcript,
};
