//! CBOR binary protocol for remote sessions (version 1).
//!
//! 1:1 port of `packages/protocol` (see crates/notagent-protocol/PARITY.md).
//! Port of `packages/protocol/src/index.ts`.

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
