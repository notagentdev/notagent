# notagent

A terminal coding agent, written in Rust. It runs as a TUI, talks to LLM
providers over their native APIs, and drives an agent loop with tools for
reading, editing, searching, and running commands.

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
- `/logout` — remove a stored credential
- `/model` — pick a model (fuzzy search, `/model <provider>/<model>` also works)
- `/compact` — compact the conversation context manually

Providers are also picked up from ambient environment variables
(`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, …) without a login.

## Custom providers (models.json)

Additional OpenAI-compatible providers and models can be declared in
`~/.notagent-v2/agent/models.json`. Entries there are layered over the built-in
catalog: new providers appear in the picker, and known models can be
overridden per field.

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
