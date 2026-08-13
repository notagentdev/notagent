//! Port von `packages/protocol/src/cbor/index.ts`.

mod decoder;
mod encoder;
mod options;
mod value;

pub use decoder::decode_cbor;
pub use encoder::encode_cbor;
pub use options::{
    CborError, CborOptions, DEFAULT_MAX_CBOR_BYTE_LENGTH, DEFAULT_MAX_CBOR_CONTAINER_LENGTH,
    DEFAULT_MAX_CBOR_DEPTH,
};
pub use value::CborValue;
