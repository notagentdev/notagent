# notagent

A terminal coding agent, written in Rust. It runs as a TUI, talks to LLM
providers over their native APIs, and drives an agent loop with tools for
reading, editing, searching, and running commands.

## Why notagent?

**Save up to 50% tokens.** Two features cut the context cost of everyday
agent work:

- *Minified tools.* `read_minified` returns a compact view of a source file:
  comments removed, blank lines dropped, indentation collapsed to one space
  per nesting level. `patch_minified` and `multi_patch_minified` edit through
  that same view — a source map translates the edit back to the real file, and
  nothing is written until every edit succeeds. Surveying and navigating code
  costs a fraction of a full `read`.
- *Bash filter.* Conservative, process-free compaction of shell output before
  it enters the context (`/bash-filter on|off`). Verbose build, test, and
  package-manager output shrinks to what the agent actually needs. The filter
  never loses information: whenever a compaction would be empty, larger than
  the raw output, or uncertain, the raw output passes through untouched.

**Atomic writes in the same branch or worktree.** With `/leases on`, every
mutating file tool takes an advisory, time-bounded lease before it writes
(`.notagent/leases` in the workspace). Several agents working in one checkout
stop overwriting each other: only one holder may modify a file at a time, a
content hash taken at reservation detects a file changed underneath the
holder before the commit, and leases expire on their own so a crashed agent
never blocks a file.

**And the rest:**

- *Goal mode* — `/goal` sets an objective the agent keeps pursuing across
  turns, with optional turn and token budgets.
- *Local code index* — `/index` builds a tree-sitter symbol index the agent
  queries through a dedicated tool instead of grepping blindly.
- *Session tree* — `/fork`, `/clone`, and `/tree` branch and navigate a
  session; `/export` writes HTML or JSONL, `/import` resumes from JSONL.
- *Remote sessions* — a CBOR binary protocol, a Unix-socket session server,
  and SQLite session storage, so a session can outlive the terminal that
  started it.
- *MCP* — `/mcp` lists, adds, inspects, and authenticates MCP servers at
  runtime.
- *Single static binary* — `cargo build --release` and you are done; no
  runtime, no node_modules.

## Crates

| Crate | Description |
|-------|-------------|
| `notagent` | Interactive coding agent CLI (TUI, slash commands, session handling) |
| `notagent-agent` | Agent runtime: agent loop, tools, queues, custom messages |
| `notagent-ai` | Unified multi-provider LLM API (streaming, auth, model catalog) |
| `notagent-tui` | Terminal UI library with differential rendering |
| `notagent-client` | Transport-neutral protocol client with a lease system |
| `notagent-protocol` | CBOR binary protocol for remote sessions |
| `notagent-server` | Session server core with Unix socket transport |
| `notagent-session-sqlite` | SQLite session storage backend |
| `notagent-telemetry` | Vendor-neutral telemetry contracts (spans, events, attributes) |
| `notagent-index` | Code symbol indexing using tree-sitter |

## Build

```sh
cargo build --release
```

The binary is `target/release/notagent`. Run the test suite with
`cargo test --workspace`.

## Usage

```sh
notagent
```

Everything happens inside the TUI:

- `/login` — pick a provider and store an API key or run an OAuth flow
- `/model` — pick a model (fuzzy search, `/model <provider>/<model>` also works)
- `/leases on|off` — atomic file leases (default: off)
- `/bash-filter on|off` — shell-output compaction (default: off)
- `/index on|off` — the local codebase index
- `/goal <objective>` — goal mode
- `/settings` — everything else

Providers are also picked up from ambient environment variables
(`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, …) without a login.

## Local and custom endpoints

Three providers take their model list from a server rather than from a
built-in catalog, so they offer exactly what that server holds:

- **Ollama** — `/login ollama`, pointed at `http://127.0.0.1:11434/v1`.
- **LM Studio** — `/login lmstudio`, pointed at `http://127.0.0.1:1234/v1`.
- **Custom (OpenAI-compatible)** — `/login custom` asks for a base URL, which
  is stored with the credential, so `/logout custom` removes the endpoint
  along with the key. A URL may be typed without a scheme or without `/v1`;
  both are filled in.

The login asks for an API key in every case. On a loopback address the
question can be answered with an empty line, since a local server authorizes
everything until its own authentication is switched on — both runtimes offer
that, and a key typed here is sent as a bearer token from then on.

None of them appear in the model picker until they are activated, and
`/logout` deactivates them again. The model list comes from the server, so it
holds exactly what was pulled or loaded; an address nothing answers on is
reported rather than passed over. Where a runtime reports a model's context
window and whether it reasons, those are used, and the effort levels offered
are the ones that runtime accepts.

## Custom providers (models.json)

Additional OpenAI-compatible providers and models can be declared in
`~/.notagent-v2/agent/models.json`. Entries there are layered over the built-in
catalog: new providers appear in the picker, and known models can be
overridden per field.

## Permissions & containerization

notagent has no built-in permission system for restricting filesystem,
process, network, or credential access. It runs with the permissions of the
user and process that launched it. If you need stronger boundaries, run it in
a container or sandbox.

## Experimental

### MTPLX provider

Support for [MTPLX](https://github.com/youssofal/MTPLX), an MLX inference
server for Apple Silicon with multi-token prediction. This feature is
experimental; the login picker marks it accordingly.

Two instances exist:

- **MTPLX (local)** — built in, pointed at `http://127.0.0.1:8000/v1`.
  Activate it with `/login mtplx` (no key needed on loopback) and deactivate
  it with `/logout`. While inactive it stays out of the model picker.
- **MTPLX (remote)** — declare a second instance in `models.json` with the
  remote base URL; the key comes from the login, `MTPLX_API_KEY`, or the
  config entry.

The model list is fetched live from the server (`/v1/models`), so the picker
offers exactly the model the server is serving. Requests carry a session id
header for the server's session cache and identify themselves as `notagent`,
which keeps sampler control on the client side.

#### Starting the server

Example for a 32 GB machine (M1 Pro/Max) — this is the configuration this
feature was developed against, serving a 27B model with a 131k context:

```sh
mtplx serve --model Youssofal/Qwen3.8-27B-MTPLX-Bare-Speed \
  --paged-kv-quantization q8 \
  --context-window 131072
```

The two flags matter together: `--paged-kv-quantization q8` halves the KV
cache, and `--context-window 131072` caps the pre-allocated KV pool. Without
the cap the server wires the pool for the model's full 262k window, which
saturates a 32 GB machine. Full context needs 48 GB or more.

Note that MTPLX compacts older tool results server-side, so the effective
context stays far below what the raw conversation suggests; the context
percentage in the footer reflects what the server actually processed.

## Acknowledgements

notagent began as a Rust port of [pi](https://github.com/earendil-works/pi)
and has grown well beyond it. Thanks to the pi authors for the foundation.
