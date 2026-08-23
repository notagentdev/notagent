# notagent-agent

The agent runtime: the loop that turns user input, an LLM stream, and a tool
set into a conversation.

- `agent.rs` / `agent_loop.rs` — the agent state machine: prompt in, streamed
  assistant events out, tool calls executed and fed back until the turn ends
- `harness/` — message conversion between session history and provider input,
  including context transforms
- `stream_fn.rs` — the pluggable streaming function an embedder provides
- `types.rs` — messages, events, tool definitions, queues

The crate is UI-agnostic: it emits events and leaves rendering to its caller.
Telemetry hooks are accepted but optional; without one, spans are no-ops.
