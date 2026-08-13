//! Port von `packages/protocol/src/cbor/options.ts`.

pub const UINT32_BASE: u64 = 0x1_0000_0000;
pub const MAX_UINT32: u64 = 0xffff_ffff;
const MAX_CONFIGURED_DEPTH: u64 = 512;

/// Safe defaults for untrusted protocol payloads.
pub const DEFAULT_MAX_CBOR_BYTE_LENGTH: u64 = 16 * 1024 * 1024;
pub const DEFAULT_MAX_CBOR_CONTAINER_LENGTH: u64 = 1_000_000;
pub const DEFAULT_MAX_CBOR_DEPTH: u64 = 64;

/// Optionale Limits. `None` bedeutet „Default verwenden" (TS: `undefined`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CborOptions {
    /// Maximum encoded input/output bytes and maximum byte/text string length.
    pub max_byte_length: Option<u64>,
    /// Maximum number of elements in an array or entries in a map.
    pub max_container_length: Option<u64>,
    /// Maximum recursive item depth.
    pub max_depth: Option<u64>,
}

impl CborOptions {
    pub fn with_max_byte_length(max_byte_length: u64) -> Self {
        Self {
            max_byte_length: Some(max_byte_length),
            ..Self::default()
        }
    }

    pub fn with_max_container_length(max_container_length: u64) -> Self {
        Self {
            max_container_length: Some(max_container_length),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedCborOptions {
    pub max_byte_length: u64,
    pub max_container_length: u64,
    pub max_depth: u64,
}

/// TS kennt zwei Fehlerklassen: `CborError` (Kodierungs-/Dekodierungsfehler) und
/// `RangeError` (ungültige Limit-Optionen). Beide sind hier Varianten eines Typs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CborError {
    /// Entspricht `class CborError extends Error`.
    #[error("{0}")]
    Cbor(String),
    /// Entspricht dem `RangeError` aus `resolveLimit`.
    #[error("{0}")]
    Range(String),
}

impl CborError {
    pub(crate) fn cbor(message: impl Into<String>) -> Self {
        Self::Cbor(message.into())
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Cbor(message) | Self::Range(message) => message,
        }
    }
}

fn resolve_limit(name: &str, value: u64, maximum: u64) -> Result<u64, CborError> {
    // Nicht-Integer und negative Werte sind in Rust nicht darstellbar (u64).
    if value > maximum {
        return Err(CborError::Range(format!(
            "{name} must be an integer between 0 and {maximum}"
        )));
    }
    Ok(value)
}

pub fn resolve_options(options: Option<CborOptions>) -> Result<ResolvedCborOptions, CborError> {
    let options = options.unwrap_or_default();
    Ok(ResolvedCborOptions {
        max_byte_length: resolve_limit(
            "maxByteLength",
            options
                .max_byte_length
                .unwrap_or(DEFAULT_MAX_CBOR_BYTE_LENGTH),
            MAX_UINT32,
        )?,
        max_container_length: resolve_limit(
            "maxContainerLength",
            options
                .max_container_length
                .unwrap_or(DEFAULT_MAX_CBOR_CONTAINER_LENGTH),
            MAX_UINT32,
        )?,
        max_depth: resolve_limit(
            "maxDepth",
            options.max_depth.unwrap_or(DEFAULT_MAX_CBOR_DEPTH),
            MAX_CONFIGURED_DEPTH,
        )?,
    })
}
