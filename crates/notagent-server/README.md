# notagent-server

Session server core: hosts live agent sessions and serves them to clients.

- `server.rs` — the server object tying listener, sessions, and protocol
  together
- `listener.rs` / `transports/` — accepting connections (Unix domain sockets)
- `sessions.rs` — the live session manager: one agent session, many clients
- `snapshots.rs` — session snapshots for late-joining clients
- `protocol.rs` — request handling on top of `notagent-protocol`
- `testing/` — in-process harness for exercising server and client together

Used to run sessions detached from a single terminal, with clients attaching
and detaching over the socket.
