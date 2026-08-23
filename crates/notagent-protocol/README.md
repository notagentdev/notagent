# notagent-protocol

CBOR binary protocol for remote sessions (version 1).

Defines the wire format shared by `notagent-server` and `notagent-client`:

- `cbor/` — CBOR encoding and decoding
- `framing.rs` — length-prefixed message framing over a byte stream
- `codec.rs` — typed encode/decode of protocol messages
- `schemas.rs` — the message schemas themselves

The crate has no I/O of its own; transports live in the client and server
crates.
