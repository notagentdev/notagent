# notagent-session-sqlite

SQLite storage backend for sessions.

Implements the session repository and storage interfaces on top of a local
SQLite database: session metadata, message history, and lookup by id or
directory.

- `session.rs` / `session_types.rs` — the repository surface and its types
- `sqlite/` — schema, migrations, and queries
