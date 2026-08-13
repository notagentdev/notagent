//! Port of `packages/client/src/errors.ts`.
//!
//! Deviation class 1: the five TS error classes become variants of one enum.
//! `name` and `message` stay verbatim so consumers see the same texts.

use notagent_protocol::{JsonValue, ProtocolError, ProtocolErrorCode};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PiError {
    /// `PiServerError`
    #[error("{message}")]
    Server {
        code: ProtocolErrorCode,
        message: String,
        details: Option<JsonValue>,
    },
    /// `PiDisconnectedError`
    #[error("{0}")]
    Disconnected(String),
    /// `PiClientDisposedError`
    #[error("Notagent client is disposed")]
    Disposed,
    /// `PiSessionOwnershipError`
    #[error("{message}")]
    SessionOwnership { session_id: String, message: String },
    /// `PiSessionDetachedError`
    #[error("Session {session_id} is not attached")]
    SessionDetached { session_id: String },
    /// `ProtocolValidationError` from `@notagent/protocol`
    #[error("{0}")]
    ProtocolValidation(String),
    /// Byte transport or listener failure (TS: generic `Error`).
    #[error("{0}")]
    Other(String),
}

impl PiError {
    pub fn server(error: ProtocolError) -> Self {
        Self::Server {
            code: error.code,
            message: error.message,
            details: error.details,
        }
    }

    pub fn disconnected() -> Self {
        Self::Disconnected("Notagent client is disconnected".to_owned())
    }

    pub fn session_detached(session_id: impl Into<String>) -> Self {
        Self::SessionDetached {
            session_id: session_id.into(),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Server { .. } => "PiServerError",
            Self::Disconnected(_) => "PiDisconnectedError",
            Self::Disposed => "PiClientDisposedError",
            Self::SessionOwnership { .. } => "PiSessionOwnershipError",
            Self::SessionDetached { .. } => "PiSessionDetachedError",
            Self::ProtocolValidation(_) => "ProtocolValidationError",
            Self::Other(_) => "Error",
        }
    }

    pub fn code(&self) -> Option<ProtocolErrorCode> {
        match self {
            Self::Server { code, .. } => Some(*code),
            _ => None,
        }
    }

    pub fn message(&self) -> String {
        self.to_string()
    }
}

/// Port of `toDisconnectedError`.
pub(crate) fn to_disconnected_error(error: PiError) -> PiError {
    match error {
        PiError::Disconnected(_) => error,
        other => PiError::Disconnected(other.to_string()),
    }
}
