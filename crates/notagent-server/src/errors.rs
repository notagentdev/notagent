use notagent_protocol::{JsonValue, ProtocolErrorCode};

/// `Extract<ProtocolErrorCode, "busy" | "session_locked" | "not_found" | "invalid_request" | "not_implemented">`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PiServerOperationErrorCode {
    Busy,
    SessionLocked,
    NotFound,
    InvalidRequest,
    NotImplemented,
}

impl PiServerOperationErrorCode {
    pub fn to_protocol(self) -> ProtocolErrorCode {
        match self {
            Self::Busy => ProtocolErrorCode::Busy,
            Self::SessionLocked => ProtocolErrorCode::SessionLocked,
            Self::NotFound => ProtocolErrorCode::NotFound,
            Self::InvalidRequest => ProtocolErrorCode::InvalidRequest,
            Self::NotImplemented => ProtocolErrorCode::NotImplemented,
        }
    }
}

pub const INTERNAL_SERVER_ERROR_MESSAGE: &str = "Internal server error";
pub const NOT_IMPLEMENTED_MESSAGE: &str = "Operation is not implemented";

/// A service/runtime error that can safely cross the protocol boundary.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct PiServerError {
    pub code: PiServerOperationErrorCode,
    pub message: String,
    pub details: Option<JsonValue>,
}

impl PiServerError {
    pub fn new(code: PiServerOperationErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: JsonValue) -> Self {
        self.details = Some(details);
        self
    }

    /// `SessionBusyError`
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(PiServerOperationErrorCode::Busy, message)
    }

    /// `SessionLockedError`
    pub fn locked(message: impl Into<String>) -> Self {
        Self::new(PiServerOperationErrorCode::SessionLocked, message)
    }

    /// `SessionNotFoundError`
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(PiServerOperationErrorCode::NotFound, message)
    }

    /// `NotImplementedError`
    pub fn not_implemented() -> Self {
        Self::new(
            PiServerOperationErrorCode::NotImplemented,
            NOT_IMPLEMENTED_MESSAGE,
        )
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(PiServerOperationErrorCode::InvalidRequest, message)
    }
}

/// Every error that can reach the protocol boundary.
/// (`PiServerError`, `InternalServerError`, `ProtocolValidationError`, plain
/// `Error`); Rust models the same distinction as variants of one enum.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ServerError {
    #[error("{0}")]
    Server(#[from] PiServerError),
    /// `InternalServerError` — the cause is reported but never serialized.
    #[error("{INTERNAL_SERVER_ERROR_MESSAGE}")]
    Internal { cause: String },
    #[error("{0}")]
    ProtocolValidation(String),
    #[error("{0}")]
    Other(String),
}

impl ServerError {
    pub fn internal(cause: impl Into<String>) -> Self {
        Self::Internal {
            cause: cause.into(),
        }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }
}
