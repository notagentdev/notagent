//! CBOR-Binaerprotokoll fuer Remote-Sessions (Version 1).
//!
//! 1:1-Port von `packages/protocol` (siehe crates/notagent-protocol/PARITY.md).
//! Port von `packages/protocol/src/index.ts`.

pub mod cbor;
pub mod codec;
pub mod framing;
pub mod schemas;

pub use cbor::{
    CborError, CborOptions, CborValue, DEFAULT_MAX_CBOR_BYTE_LENGTH,
    DEFAULT_MAX_CBOR_CONTAINER_LENGTH, DEFAULT_MAX_CBOR_DEPTH, decode_cbor, encode_cbor,
};
pub use codec::*;
pub use framing::*;
pub use schemas::*;
