# MCP Client Support

## Objective

Give this port the ability to use MCP servers: read their configuration, connect
over stdio, SSE and streamable HTTP, discover their tools, and offer those tools
to the model beside the built-in ones — with OAuth where a server wants it and a
trust decision before a project-local config is ever honoured.

The frame comes from `../notagent-main-rust`, which is Rust and uses the
official `rmcp` SDK. Two things it does worse are taken from
`../kimi-code-main` instead:

- **The failure triage on a tool call.** The reference retries any transport
  error five times with backoff. That is both too much and too little: it
  retries where the server gave a real answer, and it never reconnects a
  transport that is genuinely gone. The replacement decides between three
  cases — the server answered, the failure is ambiguous, the transport is dead —
  and does the right thing for each.
- **Authentication the model can resolve.** In the reference a server needing
  OAuth is a dead end until the user leaves the session and runs a CLI command.
  The replacement swaps that server's tool list for a single synthetic
  `authenticate` tool: the model calls it, hands the URL to the user, and the
  real tools appear when the callback lands.

### Out of scope

`../notagent-main-rust` also *serves* its own tools to an external agent over
MCP (`crates/notagent_app/src/mcp_lend.rs`, 530 LOC): an HTTP endpoint on
loopback behind a bearer token, chosen over stdio because a freshly spawned
server process knows nothing of the running session — no leases, no file
tracking, no metrics. That is a genuinely good idea and a genuinely different
feature: it makes this a server, where this plan makes it a client. It belongs
in its own plan, after a client exists to prove the protocol wiring.

Neither reference implements prompts, resources, sampling or elicitation. Both
are tools-only clients, and so is this.

### Assumptions

- **Version.** The work lands as v0.1.22 (current workspace version is 0.1.21,
  `Cargo.toml:6`).
- **`rmcp` is the client.** The reference pins `rmcp 0.10` with stdio, SSE and
  streamable-HTTP transports plus its OAuth state machine
  (`../notagent-main-rust/Cargo.toml:136`). Writing a JSON-RPC client by hand to
  avoid one dependency would be the wrong trade for a protocol that is still
  moving.
- **Config lives where the reference puts it.** A project-local `.mcp.json` next
  to the workspace and a user-level one under the agent directory, merged with
  the project's entries winning, matching `CONFIG_DIR_NAME`
  (`crates/notagent/src/config.rs:21`) and the two paths the reference resolves.
- **Servers connect lazily, not at startup.** A session that never calls an MCP
  tool should not pay for spawning three processes. Discovery happens on first
  need and its result is cached for the session.
- **Off unless configured.** No setting and no gate: with no `.mcp.json` there
  is nothing to connect to, and the feature is inert. This follows goal mode
  (v0.1.21) rather than the leases and bash-filter pattern.

## Background

### What comes from `../notagent-main-rust`

| Source | LOC | What it holds |
|---|---|---|
| `crates/notagent_domain/src/mcp.rs` | 683 | The config schema at line 21, untagged stdio-or-HTTP; mustache-templated headers at 96; the three-state OAuth setting at 149 with its flexible deserializer; the trust store at 326 |
| `crates/notagent_infra/src/mcp_client.rs` | 894 | Connection construction at `:145`, the standard-then-OAuth auto-detect at `:186`, streamable-HTTP-then-SSE fallback at `:218`, stderr draining for stdio children at `:162`, and the full OAuth flow at `:638` |
| `crates/notagent_infra/src/auth/mcp_credentials.rs` | 308 | Credential shapes and their persistence |
| `crates/notagent_infra/src/auth/mcp_token_storage.rs` | 267 | The token store the OAuth state machine writes through |
| `crates/notagent_app/src/dto/anthropic/transforms/mcp_tool_names.rs` | 116 | Qualifying and unqualifying a server's tool names |

Two details in there are easy to lose. A stdio child's stderr is drained on its
own task, because a server that fills the pipe blocks forever
(`mcp_client.rs:162`). And header values carry mustache templates over
environment variables, so a token never has to be written into a config file
(`mcp.rs:102`).

### What comes from `../kimi-code-main`

The triage lives in
`packages/agent-core-v2/src/agent/mcp/tools/mcp.ts:92`, and its shape is the
whole point:

- The server answered — a JSON-RPC error, or a response that failed schema
  validation. Rethrow; reconnecting cannot change the answer.
- Ambiguous — a raw socket or fetch error. Probe the client with a ping. Alive
  means a blip, so retry once in place. Dead means the transport is gone.
- Provably dead — the SDK fired its close callback, or the probe failed.
  Reconnect once and retry on the fresh client, so a dropped connection surfaces
  as a slow call rather than a failed turn.

Retries are at-least-once: a transport that died after the server processed the
call may duplicate side effects. MCP has no dedup across reconnects, so this is
a property of the protocol rather than of the implementation.

The authenticate tool is `tools/auth.ts:75`, and it exists because of a status
the connection manager tracks: `needs-auth`, one of six
(`packages/agent-core-v2/src/mcpCore/connection-manager.ts:35`, with `removed`
as the tombstone that lets a held tool fail with a clear notice rather than a
confusing one). A server in that state contributes exactly one tool. Calling it
runs discovery, streams the authorization URL back, blocks on the loopback
callback, and reconnects — after which the real tools replace it.

### The one structural obstacle in this port

`ToolName` here is a fixed enum of the built-ins
(`crates/notagent/src/core/tools.rs:37`), because the TypeScript original it
ports has a closed union. The reference's is a string newtype
(`../notagent-main-rust/crates/notagent_domain/src/tools/definition/name.rs:9`),
which is why MCP tools cost it nothing. Modes are typed against the enum too
(`crates/notagent/src/core/modes.rs:85`).

The resolution is not to widen the enum — it is `Copy`, it appears in const
arrays, and every mode file would have to learn about servers that may not
exist. It is that the registry underneath is already name-keyed
(`crates/notagent/src/core/agent_session.rs:444`), the allowlist check already
takes a string (`:1314`), and the permission chain already gates on the tool
name as text (`crates/notagent/src/core/permissions/chain.rs:69`). MCP tools
therefore live beside the enum-named built-ins as ordinary registry entries,
outside the mode allowlist and governed by permissions. Task 6 is that decision
and nothing else, because getting it wrong late would be expensive.

## Implementation Plan

- [ ] 1. **Add the config schema and its two files.** Port
  `../notagent-main-rust/crates/notagent_domain/src/mcp.rs`'s configuration half
  into a new `crates/notagent/src/core/mcp.rs`: the untagged stdio-or-HTTP
  server config, per-server `disable` and `timeout`, mustache-templated headers
  resolved against the environment, and the three-state OAuth setting with the
  flexible deserializer that accepts an absent value, `false`, or an object.
  Resolve a project-local `.mcp.json` and a user-level one, project winning on a
  name collision. Rationale: everything downstream is shaped by what a server
  can be configured as, and the templated headers are what keeps a token out of
  a file that gets committed.

- [ ] 2. **Port the trust store and its prompt.** Bring the trust half of the
  same file over: a project-local config is not honoured until the user accepts
  it, the decision is stored against a content hash of the file, and any edit to
  that file revokes the decision and asks again
  (`../notagent-main-rust/crates/notagent_domain/src/mcp.rs:326`). Wire the
  prompt into startup beside the existing project-trust flow
  (`crates/notagent/src/core/project_trust.rs`). Rationale: a `.mcp.json` in a
  cloned repository is a list of programs to run on the user's machine; the
  content hash is what stops "trusted once" from meaning "trusted whatever it
  says next".

- [ ] 3. **Add the client and its transports.** Port
  `../notagent-main-rust/crates/notagent_infra/src/mcp_client.rs`'s connection
  half into `crates/notagent/src/core/mcp/client.rs`, adding `rmcp` to the
  workspace dependencies: a stdio child with its arguments and environment and
  its stderr drained on a separate task, streamable HTTP falling back to SSE,
  and the initialize handshake announcing this client's name and version.
  Rationale: `rmcp` provides most of this; the stderr drain and the transport
  fallback are the parts it does not.

- [ ] 4. **Add the connection manager and its six states.** A per-session owner
  of the configured servers and their live clients, modelled on
  `../kimi-code-main/packages/agent-core-v2/src/mcpCore/connection-manager.ts:35`:
  pending, connected, failed, disabled, needs-auth, removed. Connect on first
  need rather than at startup, cache the discovered tools for the session, and
  keep a removed server as a tombstone so a tool the model still holds fails
  with a notice that says the server is gone instead of one that says the call
  failed. Rationale: the states are not bookkeeping — `needs-auth` is what task
  9 keys off, and `removed` is the difference between a clear message and a
  confusing one.

- [ ] 5. **Qualify tool names and unqualify them again.** Adopt the
  `mcp__<server>__<tool>` convention both references use, ported from
  `../notagent-main-rust/crates/notagent_app/src/dto/anthropic/transforms/mcp_tool_names.rs`,
  with the inverse mapping for dispatch. Decide and document what happens when a
  server's name or tool name already contains the separator, and when two
  servers offer the same tool. Rationale: the qualified name is what the model
  sees, what the permission rules match on, and what the registry keys on, so an
  ambiguity here is an ambiguity in all three.

- [ ] 6. **Give MCP tools a place in the registry.** Append the discovered tools
  to the session's base definitions
  (`crates/notagent/src/core/agent_session.rs:1446`) as ordinary name-keyed
  entries, leaving `ToolName` and the mode allowlists untouched, and let the
  permission chain gate them by their qualified name
  (`crates/notagent/src/core/permissions/chain.rs:69`). Keep them out of a
  subagent unless its parent had them, following the reasoning already applied
  to the task, todo and goal tools
  (`crates/notagent/src/core/delegation/run.rs:37`). Rationale: this is the one
  decision that shapes every later task, and widening the closed enum instead
  would push knowledge of servers that may not exist into every mode file.

- [ ] 7. **Convert results.** Map MCP tool results
  into this port's result shape: text through, images as attachments where the
  session accepts them, and everything else — audio, embedded resources —
  refused with a message naming what came back
  (`../notagent-main-rust/crates/notagent_infra/src/mcp_client.rs:556`). Apply
  the same truncation the bash tool applies, so a server that returns a megabyte
  costs what a command returning a megabyte costs. Rationale: a dropped content
  block is a wrong answer the model cannot see is wrong, and an untruncated
  result undoes the work the bash filter did.

- [ ] 8. **Replace the retry with the three-way triage.** Instead of the
  reference's blanket five-attempt backoff
  (`../notagent-main-rust/crates/notagent_infra/src/mcp_client.rs:566`),
  implement the decision from
  `../kimi-code-main/packages/agent-core-v2/src/agent/mcp/tools/mcp.ts:92`: a
  server-side answer is rethrown, an ambiguous failure is probed with a ping and
  retried once in place if the client is alive, and a provably dead transport is
  reconnected once and the call retried on the fresh client. Honour the caller's
  cancellation at every step, and record in the module header that retries are
  at-least-once and may duplicate side effects. Rationale: the reference retries
  where the server already answered, which wastes time and can duplicate work,
  and never reconnects, which is the case that actually needs recovering.

- [ ] 9. **Add the synthetic authenticate tool.** When a server lands in
  `needs-auth`, contribute exactly one tool for it instead of its real ones,
  after
  `../kimi-code-main/packages/agent-core-v2/src/agent/mcp/tools/auth.ts:75`: it
  runs discovery, streams the authorization URL back through the tool's update
  channel so the user can see it while the call is still open, waits on the
  loopback callback with a timeout, and reconnects — after which the real tools
  replace it. Handle the already-authorized case by reconnecting instead of
  starting a flow. Rationale: this is the difference between a server that needs
  OAuth being a dead end and being a two-message detour; the reference makes the
  user leave the session for it.

- [ ] 10. **Port the OAuth flow and its token storage.** Bring
  `../notagent-main-rust/crates/notagent_infra/src/mcp_client.rs:638` and the two
  auth files over: metadata discovery, dynamic client registration, the browser
  handoff, the loopback callback listener and token persistence, plus the
  auto-detect path that tries an unauthenticated connection first and falls back
  to stored credentials on a 401. Bind the callback listener on a port the OS
  picks rather than the reference's fixed one, so two sessions authenticating at
  once do not collide. Rationale: the flow is the same either way, and the fixed
  port is a defect this port does not need to inherit.

- [ ] 11. **Add the `/mcp` command.** A single command over the server list:
  bare `/mcp` reports every configured server with its status and tool count,
  `/mcp auth <server>` starts the flow from the user's side, `/mcp logout
  <server>` drops its credentials, and `/mcp reconnect <server>` retries a failed
  one. Register it in `crates/notagent/src/core/slash_commands.rs:62` and
  dispatch it beside `/goal` and `/bash-filter`. Rationale: six states are
  worth nothing if the user cannot see which one a server is in, and a failed
  server with no way to retry is a restart.

- [ ] 12. **Show connected servers in the UI.** Report the count of connected
  servers and any that are failed or awaiting authentication where the user will
  see it without asking, following the footer badge added for goal mode
  (`crates/notagent/src/modes/interactive/components/footer.rs`). Rationale: a
  server that quietly failed at connect looks exactly like a server whose tools
  the model chose not to use.

- [ ] 13. **Test against a real server, not only a mock.** Cover the config
  schema and its merge, the trust store's hash invalidation, name qualification
  and its inverse, and each of the three triage branches with a client scripted
  to fail that way. Then add an end-to-end case against a minimal stdio server
  spawned by the test itself, so the handshake, discovery, a call and a result
  conversion run for real at least once. Add a `/mcp` scenario to
  `crates/notagent/tests/interactive_e2e/`. Rationale: every mock in this
  feature encodes an assumption about a protocol this port has never spoken, and
  one real handshake is worth more than ten mocked ones.

- [ ] 14. **Bump the version and run the gate.** Record in the module header
  that this is a client only — no prompts, resources, sampling or elicitation —
  and that lending our own tools out is a separate feature. Bump to v0.1.22 and
  run `./scripts/check.sh` until format, clippy with warnings denied and the
  whole suite are green. Rationale: the absent halves of the protocol are the
  first thing a reader will look for.

## Verification Criteria

- `./scripts/check.sh` passes: `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, and `cargo test --workspace`.
- With no `.mcp.json` anywhere, every existing test passes unchanged and no
  process is spawned.
- A project-local `.mcp.json` is ignored until accepted; editing the file after
  acceptance asks again; rejecting it leaves the servers unconfigured.
- A stdio server configured in the test spawns, completes the handshake,
  reports its tools under `mcp__<server>__<tool>`, answers a call, and its
  result reaches the model converted.
- A server whose child writes continuously to stderr does not block.
- A JSON-RPC error from the server is reported once, with no reconnect and no
  second call.
- An ambiguous socket failure against a live client retries once in place; the
  same failure against a dead one reconnects once and succeeds on the fresh
  client.
- A server answering 401 with no stored credentials contributes exactly one
  tool, whose name ends in `authenticate`; calling it surfaces an authorization
  URL, and completing the flow replaces it with the server's real tools.
- Two sessions authenticating at the same time both complete.
- `/mcp` lists every configured server with its status; `/mcp reconnect` moves a
  failed server to connected when the cause is gone.
- A removed server's still-held tools fail with a message that says the server
  was removed.
- MCP tools are absent from a subagent's tool set.
- An MCP result carrying audio or an embedded resource is refused with a message
  naming what arrived, not silently dropped.

## Potential Risks and Mitigations

1. **An MCP server is arbitrary code from a config file.**
   A `.mcp.json` in a cloned repository names programs to spawn with the user's
   environment.
   Mitigation: task 2 — nothing project-local runs before the user accepts it,
   and the acceptance is bound to a content hash, so an edit revokes it.

2. **A retry duplicates a side effect.**
   A transport that dies after the server processed a call but before the
   response arrives cannot be distinguished from one that died before, so the
   retry may run the call twice.
   Mitigation: none is available at this layer — MCP has no dedup across
   reconnects. Record it in the module header, and confine retries to the two
   branches where the transport is actually suspect rather than the blanket
   retry the reference applies.

3. **The closed `ToolName` enum makes MCP tools awkward everywhere.**
   Modes, permissions and the registry each name tools differently.
   Mitigation: task 6 settles it before anything depends on it — the registry is
   already name-keyed and the permission chain already gates on text, so MCP
   tools become ordinary entries outside the mode allowlist. Widening the enum
   is the alternative that looks cheaper and is not.

4. **A slow or hanging server stalls a turn.**
   A stdio child that never answers, or an HTTP server that accepts and never
   replies, blocks the tool call.
   Mitigation: honour the per-server `timeout` from the config on every call and
   on the handshake, default it conservatively, and cover a hanging server with
   a test. The connection manager's `failed` state is what the user then sees.

5. **Connecting at startup costs every session.**
   Three configured servers mean three processes for a session that asks none of
   them anything.
   Mitigation: connect lazily on first need and cache for the session, which is
   also what makes a broken server cost nothing until it is used.

6. **`rmcp` moves.**
   The Rust SDK is younger than the TypeScript one and its API is still
   changing.
   Mitigation: pin the version the reference pins, keep the client's own surface
   narrow — connect, list, call, close — so an upgrade touches one file, and
   cover that surface with the real-server test from task 13.

## Alternative Approaches

1. **Take `../kimi-code-main` whole and translate it.** Around 7,600 lines of
   TypeScript across two generations, including the workspace config service,
   ephemeral per-session servers and the VS Code and ACP surfaces. It is the
   better implementation overall and this plan says so. Rejected because most of
   that volume is product surface this port does not have, and translating a
   TypeScript service graph into Rust is a larger and riskier job than porting
   Rust that already uses the same SDK family — for the two behaviours that
   actually matter, which are grafted here instead.

2. **Take `../notagent-main-rust` unchanged.** Cheapest by a wide margin: same
   language, same idioms, roughly 2,800 lines. Rejected because its two
   weaknesses are both in the path a user hits daily — a blanket retry that
   retries the wrong failures, and an OAuth dead end that forces the user out of
   the session. The grafts are small next to the port.

3. **Write a JSON-RPC client by hand instead of using `rmcp`.** Would remove a
   young dependency and give full control of the wire. Rejected: MCP is still
   changing, and tracking it by hand is a standing cost paid to avoid a
   dependency both references decided to take.

4. **Include `mcp_lend` in this plan.** Serving this session's own tools to an
   external agent over loopback HTTP — the reference's genuinely unique idea.
   Rejected as scope: it is a server rather than a client, it needs its own
   token and lifetime handling, and it is easier to build and to review once the
   client half exists and the protocol wiring is proven.

5. **Connect every configured server at startup.** Discovery would be complete
   before the model's first turn, so the tool list never changes mid-session.
   Rejected because it charges every session for servers it will not use, and
   the changing tool list is something the registry already handles for the
   agent's own tools.
