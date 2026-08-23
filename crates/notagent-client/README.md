# notagent-client

Transport-neutral client for remote sessions.

Connects to a session server over a pluggable transport (Unix domain sockets
in `unix.rs`), speaks the binary protocol from `notagent-protocol`, and
exposes sessions through `session_handle.rs`. A lease system arbitrates which
client currently drives a session while others observe.

- `client.rs` / `connection.rs` — connection lifecycle and request routing
- `state.rs` — client-side session state tracking
- `transport.rs` — the transport abstraction
- `errors.rs` — typed connection and protocol errors
