# MCP Client Support

Supersedes `plans/2026-08-18-mcp-client-v1.md`, which ordered the work by
component. This version orders it by what has to hold for the feature to keep
working, and adds the four things v1 assumed rather than checked.

## Objective

Give this port the ability to use MCP servers: read their configuration, connect
over stdio, SSE and streamable HTTP, discover their tools, and offer those tools
to the model beside the built-in ones, with OAuth where a server wants it and a
trust decision before a project-local config is ever honoured.

The working definition of done is not "MCP tools appear". It is:

- A configured server that is slow, hung, crashed, malicious or absent costs the
  session a bounded amount of time and nothing else.
- Every process this feature starts is gone when the session ends, on every exit
  path.
- A dropped connection surfaces as a slow call, not a failed turn.
- A server needing OAuth is resolvable without leaving the session.
- One real MCP server, spawned by the test suite, completes a handshake,
  discovery, a call and a result conversion on every CI run.

### Where the code comes from

The frame is `../notagent-main-rust`: Rust, the official `rmcp` SDK, roughly
2,800 lines. Four behaviours come from elsewhere or are new, listed in the
robustness table below.

`../notagent-main-rust` also serves its own tools to an external agent over MCP
(`crates/notagent_app/src/mcp_lend.rs`, 530 LOC). That makes this a server where
this plan makes it a client; separate plan, after the client works.

Neither reference implements prompts, resources, sampling or elicitation. This
is a tools-only client.

### What the reference already gets right

Verified by reading, not assumed:

| Property | Where |
|---|---|
| A failing server does not break the session; failures are collected and reported alongside the working servers | `crates/notagent_services/src/mcp/service.rs:191` |
| Connections are lazy and guarded against races, so concurrent callers cannot observe a half-built tool map | `service.rs:220` |
| The trust gate runs before the cache key is computed, so a rejected server cannot re-enter through a stale cache entry | `service.rs:241` |
| Trust is bound to a content hash, so editing an accepted config revokes the decision | `crates/notagent_domain/src/mcp.rs:326` |
| A stdio child's stderr is drained on its own task; a server that fills the pipe would otherwise block forever | `crates/notagent_infra/src/mcp_client.rs:162` |
| Child processes are killed on drop | `mcp_client.rs:155` |
| Tool calls are bounded by the global tool timeout | `crates/notagent_app/src/tool_registry.rs:568` |
| Header values carry mustache templates over environment variables, so a token stays out of the config file | `mcp.rs:102` |

### The four gaps this plan closes

| Gap | Evidence | Task |
|---|---|---|
| The per-server `timeout` in `.mcp.json` is parsed and never applied; the documented "default 300 seconds" does not exist. Connect and discovery are outside any timeout, so a server that hangs during handshake hangs tool discovery for every later call. The reference calls the non-cancellable request path throughout — no `cancel` or `abort` appears anywhere in its MCP code — so an in-flight call cannot be stopped either | `mcp.rs:85` documents the field; no enforcement anywhere in `mcp_client.rs` or `mcp/service.rs` | 4, 8, 11 |
| The retry is a blanket five-attempt backoff on any transport error: it retries where the server already answered, and never reconnects a transport that is gone | `mcp_client.rs:566`; the replacement is `../kimi-code-main/packages/agent-core-v2/src/agent/mcp/tools/mcp.ts:92` | 8 |
| A server needing OAuth is a dead end until the user leaves the session for a CLI command | `mcp_client.rs:638`; the replacement is `../kimi-code-main/.../tools/auth.ts:75` | 9 |
| Nothing validates what a server sends: a tool whose input schema is not a JSON object reaches the model's tool list | `../kimi-code-main/packages/agent-core-v2/src/mcpCore/types.ts:52` rejects it | 6 |

### The structural obstacle in this port

`ToolName` here is a closed enum of the built-ins
(`crates/notagent/src/core/tools.rs:37`), because the TypeScript original has a
closed union; modes are typed against it (`crates/notagent/src/core/modes.rs:85`).
The reference's is a string newtype, which is why MCP costs it nothing.

The registry underneath is already name-keyed
(`crates/notagent/src/core/agent_session.rs:444`), the allowlist check takes a
string (`:1314`), and the permission chain gates on the tool name as text
(`crates/notagent/src/core/permissions/chain.rs:69`). MCP tools therefore live
beside the enum-named built-ins as ordinary registry entries, outside the mode
allowlist and governed by permissions. Task 5 settles this before anything
depends on it.

### Assumptions

- **Version.** v0.1.22 (current is 0.1.21, `Cargo.toml:6`).
- **`rmcp` is the client**, pinned at the version the reference pins
  (`../notagent-main-rust/Cargo.toml:136`).
- **Config paths.** A project-local `.mcp.json` and a user-level one under the
  agent directory, project winning on a name collision, following
  `CONFIG_DIR_NAME` (`crates/notagent/src/config.rs:21`).
- **Off unless configured.** No setting and no gate; with no `.mcp.json` nothing
  connects and nothing is registered.

## Implementation Plan

- [x] 1. **Add the config schema, the two files and their merge.** Port the
  configuration half of
  `../notagent-main-rust/crates/notagent_domain/src/mcp.rs` into a new
  `crates/notagent/src/core/mcp.rs`: the untagged stdio-or-HTTP server config,
  per-server `disable` and `timeout`, mustache-templated headers resolved
  against the environment, and the three-state OAuth setting with the flexible
  deserializer that accepts an absent value, `false`, or an object. Reject a
  config file that does not parse with a message naming the file and the
  problem, rather than falling back to an empty server list — a typo that
  silently disables every server is worse than an error. Rationale: everything
  downstream is shaped by what a server can be configured as, and the templated
  headers keep a token out of a file that gets committed.

- [x] 2. **Port the trust store and its prompt.** Bring the trust half of the
  same file over (`mcp.rs:326`): a project-local config is not honoured until
  the user accepts it, the decision is stored against a content hash, and any
  edit revokes it. Apply the gate before the cache key is computed, as the
  reference does (`../notagent-main-rust/crates/notagent_services/src/mcp/service.rs:241`),
  so a rejected server cannot re-enter through a stale cache entry. Wire the
  prompt into startup beside the existing project-trust flow
  (`crates/notagent/src/core/project_trust.rs`). Rationale: a `.mcp.json` in a
  cloned repository is a list of programs to run on the user's machine, and the
  content hash is what stops "trusted once" from meaning "trusted whatever it
  says next".

- [x] 3. **Add the client and its transports.** Port the connection half of
  `../notagent-main-rust/crates/notagent_infra/src/mcp_client.rs` into
  `crates/notagent/src/core/mcp/client.rs`, adding `rmcp` to the workspace
  dependencies: a stdio child with its arguments and environment and its stderr
  drained on a separate task (`:162`), streamable HTTP falling back to SSE
  (`:218`), and the initialize handshake announcing this client's name and
  version. Rationale: `rmcp` provides most of this; the stderr drain and the
  transport fallback are the parts it does not.

- [x] 4. **Bound every operation in time.** Apply a deadline to the handshake, to
  discovery and to each tool call, taking the per-server `timeout` from the
  config when present and a conservative default otherwise. The mechanism
  exists in the SDK and the reference does not use it: `send_cancellable_request`
  with a `timeout` in `PeerRequestOptions`
  (`rmcp-0.10.0/src/service.rs:378` and `:332`) bounds the wait and, on expiry,
  sends the server a cancellation notification so the call is dropped on both
  ends rather than only locally (`:261`). This is therefore setting a parameter
  the reference leaves unset, not building a mechanism. Tool calls happen to be
  covered by the registry's global tool timeout
  (`../notagent-main-rust/crates/notagent_app/src/tool_registry.rs:568`), but
  connect and discovery are outside any deadline, so a server that hangs during
  handshake hangs tool discovery for every later call in the session. A
  timed-out server becomes `failed` with a message naming the phase that
  expired. Rationale: this is the difference between one broken server costing a
  bounded wait and costing the session.

- [x] 5. **Give MCP tools a place in the registry.** Append the discovered tools
  to the session's base definitions
  (`crates/notagent/src/core/agent_session.rs:1446`) as name-keyed entries,
  leaving `ToolName` and the mode allowlists untouched, and let the permission
  chain gate them by qualified name
  (`crates/notagent/src/core/permissions/chain.rs:69`). Adopt the
  `mcp__<server>__<tool>` convention both references use, ported from
  `../notagent-main-rust/crates/notagent_app/src/dto/anthropic/transforms/mcp_tool_names.rs`,
  with the inverse mapping for dispatch. Decide what happens when a server's
  name or a tool name contains the separator, when two servers offer the same
  tool, and when a qualified name collides with a built-in: a built-in always
  wins and the shadowed MCP tool is dropped with a warning. Keep MCP tools out
  of a subagent unless its parent had them, following the task, todo and goal
  tools (`crates/notagent/src/core/delegation/run.rs:37`). Rationale: widening
  the closed enum instead would push knowledge of servers that may not exist
  into every mode file, and a collision that silently shadows a built-in is a
  hijack.

- [x] 6. **Validate everything a server sends.** A server is remote input.
  Reject a tool whose name is empty or not usable as a tool name, or which
  duplicates another tool of the same server — dropping that tool and keeping
  the rest of the server. Corrected during implementation: a tool whose input
  schema is not a JSON object cannot be dropped in isolation, because `rmcp`
  deserializes a `tools/list` response whole and one malformed entry fails the
  page. The server therefore loses its whole list, and the recorded error says
  so (`crates/notagent/src/core/mcp/client.rs`, pinned by
  `tests/mcp_client.rs::one_malformed_tool_costs_its_server_the_whole_list_and_says_so`).
  A schema that arrives readable but is not an object still becomes an empty
  object schema at registration rather than refusing every call. Cap the number of tools accepted from one server and the size of
  a tool description, so a server cannot fill the context with its own manifest
  before a single call happens. Rationale: an invalid schema reaching the model's
  tool list is a malformed request to the provider, and the tool list is sent on
  every turn — an oversized manifest is a permanent tax.

- [x] 7. **Convert results.** Map MCP tool results into this port's result shape:
  text through, images as attachments where the session accepts them, and audio
  or embedded resources refused with a message naming what came back
  (`../notagent-main-rust/crates/notagent_infra/src/mcp_client.rs:556`). Apply
  the same truncation the bash tool applies, so a server returning a megabyte
  costs what a command returning a megabyte costs. Rationale: a dropped content
  block is a wrong answer the model cannot see is wrong, and an untruncated
  result undoes the work the bash filter did.

- [x] 8. **Replace the retry with the three-way triage.** Instead of the
  reference's blanket five-attempt backoff (`mcp_client.rs:566`), implement the
  decision from
  `../kimi-code-main/packages/agent-core-v2/src/agent/mcp/tools/mcp.ts:92`: a
  server-side answer — a JSON-RPC error, or a response that failed schema
  validation — is rethrown, because reconnecting cannot change it; an ambiguous
  socket failure is probed with a ping and retried once in place if the client
  is alive; a provably dead transport is reconnected once and the call retried
  on the fresh client. Honour cancellation at every step, through the same
  cancellable request path task 4 introduces. Retries are
  at-least-once: a transport that died after the server processed the call may
  duplicate side effects, and MCP has no dedup across reconnects, so record that
  in the module header. Rationale: the reference retries where the server
  already answered, and never reconnects, which is the case that needs
  recovering.

- [x] 9. **Add the synthetic authenticate tool.** Track the six connection states
  `../kimi-code-main/packages/agent-core-v2/src/mcpCore/connection-manager.ts:35`
  tracks — pending, connected, failed, disabled, needs-auth, removed. A server
  in `needs-auth` contributes exactly one tool instead of its real ones, after
  `../kimi-code-main/.../tools/auth.ts:75`: it runs discovery, streams the
  authorization URL back through the tool's update channel while the call is
  still open, waits on the loopback callback with a timeout, and reconnects, at
  which point the real tools replace it. Handle the already-authorized case by
  reconnecting instead of starting a flow. Keep `removed` as a tombstone so a
  tool the model still holds fails with a notice that the server is gone.
  Rationale: in the reference a server needing OAuth is a dead end until the
  user leaves the session; here it is a two-message detour.

- [x] 10. **Port the OAuth flow and its token storage.** Bring
  `mcp_client.rs:638` and the two auth files
  (`crates/notagent_infra/src/auth/mcp_credentials.rs`,
  `mcp_token_storage.rs`) over: metadata discovery, dynamic client registration,
  the browser handoff, the loopback callback listener and token persistence,
  plus the auto-detect path that tries an unauthenticated connection first and
  falls back to stored credentials on a 401 (`:186`). Bind the callback listener
  on a port the OS picks rather than the reference's fixed 8765. Rationale: the
  flow is the same either way, and a fixed port collides when two sessions
  authenticate at once.

- [x] 11. **Make process lifetime explicit.** `kill_on_drop` covers the ordinary
  path (`mcp_client.rs:155`); cover the rest. Close every client and reap every
  child when the session ends, when a server is removed from the config, and
  when a call is cancelled mid-flight — cancelling the in-flight request through
  `RequestHandle::cancel` (`rmcp-0.10.0/src/service.rs:280`) so the server is
  told, then closing the client. Give a child that ignores the polite stop a
  bounded grace period and then a hard kill, following the pattern the hook
  runner already uses (`crates/notagent/src/core/hooks/runner.rs:30`). Rationale:
  a stdio server is a process this feature started, and a leaked one keeps
  running after the session that owned it is gone.

- [x] 12. **Add the `/mcp` command and the status display.** Bare `/mcp` lists
  every configured server with its state and tool count, `/mcp auth <server>`
  starts the flow from the user's side, `/mcp logout <server>` drops its
  credentials, and `/mcp reconnect <server>` retries a failed one. Register it in
  `crates/notagent/src/core/slash_commands.rs:62` and dispatch it beside `/goal`
  and `/bash-filter`. Surface connected, failed and awaiting-authentication
  counts where the user sees them without asking, following the goal badge
  (`crates/notagent/src/modes/interactive/components/footer.rs`). Rationale: six
  states are worth nothing if the user cannot see which one a server is in, and
  a server that quietly failed at connect looks exactly like a server whose
  tools the model chose not to use.

- [x] 13. **Build the fault-injection server the tests need.** A minimal MCP
  server binary under `crates/notagent/tests/`, driven by its arguments, that can
  behave in each way the implementation claims to survive: answer normally, hang
  during handshake, hang during a call, exit mid-call, close the transport
  between calls, return a JSON-RPC error, return an invalid input schema, return
  a hundred megabytes, return audio, spam stderr without reading stdin, demand
  OAuth with a 401. Rationale: every other test in this feature is a mock over a
  protocol this port has never spoken, and each behaviour above corresponds to a
  claim in tasks 4, 6, 7, 8, 9 and 11 that is otherwise untested.

- [x] 14. **Run the implementation against it.** One integration suite per claim:
  a normal handshake, discovery, call and result conversion end to end; each
  hang bounded by its deadline and reported as `failed` naming the phase; a
  crashed server reconnected on the next call; a JSON-RPC error surfacing once
  with no reconnect; an invalid schema dropping one tool and keeping the rest;
  an oversized result truncated; audio refused with its type named; two calls to
  one server concurrently, which the SDK provides for by giving each request its
  own id over a cloneable peer (`rmcp-0.10.0/src/service.rs:313`); a cancelled
  call leaving no child behind; every child gone after the session ends. Add unit coverage for the config merge, the trust
  hash invalidation, name qualification and its inverse, and the collision rules,
  plus a `/mcp` scenario in `crates/notagent/tests/interactive_e2e/`. Rationale:
  this suite is the definition of done from the objective, expressed as tests.

- [x] 15. **Bump the version and run the gate.** Record in the module header that
  this is a client only — no prompts, resources, sampling or elicitation — and
  that lending our own tools out is a separate feature. Bump to v0.1.22 and run
  `./scripts/check.sh` until format, clippy with warnings denied and the whole
  suite are green.

## Verification Criteria

- `./scripts/check.sh` passes: `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, and `cargo test --workspace`.
- With no `.mcp.json` anywhere, every existing test passes unchanged and no
  process is spawned.
- A `.mcp.json` that does not parse produces an error naming the file, not an
  empty server list.
- A project-local `.mcp.json` is ignored until accepted; editing it after
  acceptance asks again; rejecting it leaves the servers unconfigured and no
  cached entry re-admits them.
- The fault-injection server, driven through all eleven behaviours, produces the
  outcome each of tasks 4, 6, 7, 8, 9 and 11 claims.
- A server hanging during handshake fails within its configured timeout, and the
  other configured servers still contribute their tools.
- A server hanging during a call fails that call within its configured timeout
  and the session continues.
- A per-server `timeout` in the config is the deadline actually used, verified by
  a server that hangs longer than it and shorter than the default.
- A JSON-RPC error is reported once, with no reconnect and no second call.
- An ambiguous socket failure against a live client retries once in place; the
  same failure against a dead one reconnects once and succeeds on the fresh
  client.
- A server answering 401 with no stored credentials contributes exactly one tool,
  whose name ends in `authenticate`; completing the flow replaces it with the
  server's real tools.
- Two sessions authenticating at the same time both complete.
- A tool with a non-object input schema is dropped with a warning; the server's
  other tools remain callable.
- A qualified MCP name that collides with a built-in leaves the built-in
  reachable.
- After the session ends, no child process started by this feature is running;
  the same holds after a call is cancelled mid-flight.
- Two concurrent calls to one server both return correct results.
- MCP tools are absent from a subagent's tool set.
- `/mcp` lists every configured server with its state; `/mcp reconnect` moves a
  failed server to connected when the cause is gone.

## Potential Risks and Mitigations

1. **An MCP server is arbitrary code from a config file.**
   A `.mcp.json` in a cloned repository names programs to spawn with the user's
   environment.
   Mitigation: task 2 — nothing project-local runs before the user accepts it,
   the acceptance is bound to a content hash, and the gate runs before the cache
   key so a rejection cannot be bypassed by a stale entry.

2. **A hung server costs the session, not just itself.**
   Connect and discovery run before any tool call, so one server hanging during
   handshake blocks the tool list for every later call.
   Mitigation: task 4 puts a deadline on the handshake, on discovery and on each
   call, and task 13 provides a server that hangs in each of those phases so the
   claim is tested rather than asserted.

3. **A leaked child process outlives its session.**
   `kill_on_drop` covers the ordinary path and not cancellation or an abrupt
   session end.
   Mitigation: task 11 makes shutdown explicit with a grace period and a hard
   kill; task 14 checks for surviving children after both a cancelled call and a
   session end.

4. **A retry duplicates a side effect.**
   A transport that died after the server processed a call cannot be
   distinguished from one that died before.
   Mitigation: none is available at this layer — MCP has no dedup across
   reconnects. Record it in the module header, and confine retries to the two
   branches where the transport is actually suspect rather than the blanket
   retry the reference applies.

5. **A server's manifest is a permanent context tax.**
   Tool names and descriptions are sent on every turn, and a server chooses them.
   Mitigation: task 6 caps the tool count per server and the description size,
   and drops what does not validate rather than passing it through.

6. **The closed `ToolName` enum makes MCP tools awkward everywhere.**
   Mitigation: task 5 settles it before anything depends on it — the registry is
   already name-keyed and the permission chain already gates on text, so MCP
   tools become ordinary entries outside the mode allowlist, with built-ins
   winning any collision.

7. **`rmcp` moves.**
   The Rust SDK is younger than the TypeScript one and its API is still
   changing.
   Mitigation: pin the version the reference pins, keep the client's own surface
   narrow — connect, list, call, cancel, close — so an upgrade touches one file,
   and let the task 13 server prove an upgrade did not break the wire. The parts
   of that surface this plan depends on were read before it was written:
   cancellable requests with timeouts at `rmcp-0.10.0/src/service.rs:378`,
   per-request cancellation at `:280`, and per-request ids over a cloneable peer
   at `:313`. What remains unverified is behaviour under load and against
   half-closed sockets, which is what task 13 exercises.

## Alternative Approaches

1. **Take `../kimi-code-main` whole and translate it.** Around 7,600 lines of
   TypeScript across two generations, including the workspace config service,
   ephemeral per-session servers and the VS Code and ACP surfaces. It is the
   better implementation overall. Rejected because most of that volume is
   product surface this port does not have, and translating a TypeScript service
   graph into Rust is a larger and riskier job than porting Rust that already
   uses the same SDK family, for the four behaviours that matter.

2. **Take `../notagent-main-rust` unchanged.** Cheapest by a wide margin: same
   language, same idioms. Rejected because its four gaps are all in the path a
   user hits daily, and two of them — the unbounded handshake and the missing
   per-server timeout — are exactly the kind of defect that only shows up under
   a server having a bad day.

3. **Skip the fault-injection server and mock the failures.** Faster to write
   and the tests would run in milliseconds. Rejected: a mock encodes what the
   implementation believes about the protocol, which is what the tests exist to
   check. The eleven behaviours are cheap to write once and are the only
   evidence the recovery paths work at all.

4. **Connect every configured server at startup.** Discovery would be complete
   before the model's first turn, so the tool list never changes mid-session.
   Rejected because it charges every session for servers it will not use, and
   because it turns one slow server into slow startup for everyone.

5. **Include `mcp_lend` here.** Serving this session's own tools to an external
   agent over loopback HTTP. Rejected as scope: it is a server rather than a
   client, it needs its own token and lifetime handling, and it is easier to
   build and review once the client half exists.
