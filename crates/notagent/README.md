# notagent

The coding agent itself: the `notagent` binary and its library surface.

Contains the interactive TUI mode (slash commands, model picker, session
resume, compaction UI), the non-interactive CLI mode, the core runtime
(settings, model catalog and `models.json` layering, auth storage, session
persistence, compaction, hooks), and the package/extension manager CLI.

Key areas:

- `src/modes/interactive/` — the TUI: components, selectors, status
  indicators, the message loop
- `src/core/` — settings, model runtime, session handling, compaction,
  telemetry ping, provider attribution
- `src/cli/` — argument parsing and the non-interactive entry points
- `src/package_manager_cli.rs` — install/update/remove of packages, themes,
  and the self-update flow

Build the binary with `cargo build --release`; it lands in
`target/release/notagent`. See the repository README for usage.
