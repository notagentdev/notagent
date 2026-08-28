# Repository Guidelines

notagent is a terminal coding agent written in Rust: a TUI, a multi-provider LLM
client, and an agent loop with tools for reading, editing, searching, and
running commands. This file is what an agent needs before touching the code.

`CONVENTIONS.md` is the binding document and goes deeper on layout, error
handling, async, dependencies, and lints. It is written in German; everything
else here — code, comments, commits — is English. Where the two disagree, the
sections below reflect what the repository actually does today.

## Layout

A Cargo workspace with `members = ["crates/*"]`, so a new crate needs no change
to the root manifest.

| Crate | What it owns |
|---|---|
| `notagent` | The CLI: TUI, slash commands, sessions, tools |
| `notagent-agent` | The agent loop, tool execution, message queues |
| `notagent-ai` | Providers, streaming, auth, the model catalog |
| `notagent-tui` | Terminal UI with differential rendering |
| `notagent-client` / `-protocol` / `-server` | Remote sessions over a CBOR protocol |
| `notagent-session-sqlite` | SQLite session storage |
| `notagent-telemetry` | Telemetry contracts |
| `notagent-index` | tree-sitter symbol indexing |

Directory modules use the `mod.rs`-free form (`core/tools.rs` beside
`core/tools/`). Module, type, and constant names are established public and
internal interfaces; do not rename them merely for taste.

## Build and test

```sh
cargo check -p notagent            # the normal verification step
cargo test -p notagent-ai          # one crate
cargo test -p notagent --test agent_session_init   # one target
scripts/check.sh                   # fmt, clippy -D warnings, whole-workspace tests
cargo build --release              # only when a binary is actually wanted
```

`scripts/check.sh` is the gate: it must be green before a merge. Release builds
take minutes and are not a verification step — use `cargo check`.

Stale test binaries from earlier sessions can slow a run down badly; killing
them (`pkill -f "target/debug/deps"`) before a full suite is worth it.

Toolchain is pinned in `rust-toolchain.toml` (1.95.0 with rustfmt and clippy).
`cargo fmt` runs with defaults — there is no `rustfmt.toml`.

## Testing

- Unit tests live in the same file as the code, as `#[cfg(test)] mod tests`.
  Integration tests are `crates/<crate>/tests/<name>.rs`.
- Test names are sentences about behaviour, not about method names:
  `a_cancelled_turn_does_not_wait_for_a_tool_that_ignores_it`, not `test_abort`.
- Assertion messages state the rule that was broken, and print what was seen.
- No test may need network access or a real provider key. Provider behaviour is
  driven through the faux provider (`notagent_ai::providers::faux`) or a
  loopback server the test starts itself.
- **When a test fails after a change, decide deliberately whether the code or
  the expectation is wrong, and say which you concluded.** A pinned expectation
  that no longer matches is sometimes the bug and sometimes the point.
- A failure that predates the change is reported, not fixed in passing: verify
  it on a clean tree (`git stash`) and say so.

## Style

- No panicking `unwrap()` / `expect()` in new code. Use a fallback branch or a
  real error path. Tests may unwrap.
- One `thiserror` type per crate; `anyhow` only at the binary edges, never in a
  library API.
- Cancellation is `tokio_util::sync::CancellationToken`, checked at loop
  boundaries, around awaits, and between tool calls. Never `block_on` in
  library code.
- Dependency versions live only in `[workspace.dependencies]`; crates write
  `dep = { workspace = true }`.
- Serde formats are read by older and newer builds of this program:
  `settings.json`, `models.json`, `models-store.json`, and the session JSONL.
  Add optional fields (`#[serde(default, skip_serializing_if = …)]`), never
  rename a tag — add `#[serde(alias = …)]` instead.

## Comments

Comments carry what the code cannot: the constraint, the alternative that was
rejected, the failure that motivated the shape. Not what the next line does.

- Never write emoji, anywhere: not in code, comments, commit messages, or
  output. In a terminal an emoji is one column wide in some emulators and two
  in others, so a line carrying one cannot be laid out reliably.
- Do not annotate a change with provenance or implementation history. A reader
  six months from now needs the reason, not where the code once came from.

## Commits

- Every subject line starts with `fix:` or `feature:`.
- Subject and body are English, in prose, and say why rather than what. The
  diff already says what.
- Do not commit or push unless asked.

## Versioning

The workspace version in the root `Cargo.toml` is bumped as its own commit
(`feature: bump version to 0.1.x`) and tagged `v0.1.x`. A version bump belongs
to a change worth shipping, not to every edit.

## Working here

- Read files with the editor tools, not with `cat`, `sed`, or `head`. Shell
  commands are for builds, tests, and git.
- Do not guess at an external API. Read its documentation or probe the endpoint;
  a plausible-looking field name that does not exist is worse than an open
  question, because it fails silently.
- Never add a compatibility shim or a second code path for an old format
  without asking first.
