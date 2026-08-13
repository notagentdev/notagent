# Fixture generators

These scripts produce the differential fixtures in `../` from the TypeScript original in
`/Users/dev/projects/notagent-main`. They are kept here so the fixtures can be
regenerated when the TS source changes — the fixtures are the oracle, not hand-written
expectations.

| Script | Fixture | Run with |
|---|---|---|
| `partial-json.cjs` | `partial-json.jsonl` | `node partial-json.cjs > ../partial-json.jsonl` |
| `sse-decoder.cjs` | `sse-decoder.jsonl` | `node sse-decoder.cjs > ../sse-decoder.jsonl` |
| `anthropic-payloads.mts` | `anthropic-payloads.jsonl` | `node --experimental-strip-types anthropic-payloads.mts > ../anthropic-payloads.jsonl` |

`partial-json.cjs` and `sse-decoder.cjs` carry a verbatim copy of the TS functions
(`json-parse.ts`, the SSE decoder of `anthropic-messages.ts`) and delegate to the
installed `partial-json` package. `anthropic-payloads.mts` imports the real
`anthropic-messages.ts` and captures request bodies through its `onPayload` hook.

`session-messages.jsonl` is not generated: it holds unmodified message entries from
`packages/coding-agent/test/fixtures/*.jsonl`.

`openai-completions-payloads.mts` and `openai-completions-stream.mts` do the same for
`openai-completions.ts`: the first captures request bodies through `onPayload`, the
second records the event sequence for scripted SSE bodies through a fake `fetch`.

| Script | Fixture | Run with |
|---|---|---|
| `openai-completions-payloads.mts` | `openai-completions-payloads.jsonl` | `node --experimental-strip-types openai-completions-payloads.mts > ../openai-completions-payloads.jsonl` |
| `openai-completions-stream.mts` | `openai-completions-stream.jsonl` | `node --experimental-strip-types openai-completions-stream.mts > ../openai-completions-stream.jsonl` |

| Script | Fixture | Run with |
|---|---|---|
| `openai-responses-payloads.mts` | `openai-responses-payloads.jsonl` | `node --experimental-strip-types openai-responses-payloads.mts > ../openai-responses-payloads.jsonl` |
| `openai-responses-stream.mts` | `openai-responses-stream.jsonl` | `node --experimental-strip-types openai-responses-stream.mts > ../openai-responses-stream.jsonl` |

| Script | Fixture | Run with |
|---|---|---|
| `azure-openai-responses.mts` | `azure-openai-responses.jsonl` | `node --experimental-strip-types azure-openai-responses.mts > ../azure-openai-responses.jsonl` |

`azure-openai-responses.mts` also records the URL and the headers the `AzureOpenAI`
client sends, because both are built inside the SDK rather than by the adapter.

| Script | Fixture | Run with |
|---|---|---|
| `openai-codex-responses.mts` | `openai-codex-responses.jsonl` | `node --experimental-strip-types openai-codex-responses.mts > ../openai-codex-responses.jsonl` |

`openai-codex-responses.mts` forces `transport: "sse"`; the WebSocket transport needs a
real socket and is covered by a local server in `tests/openai_codex_responses.rs`.

| Script | Fixture | Run with |
|---|---|---|
| `google-generative-ai.mts` | `google-generative-ai.jsonl` | `node --experimental-strip-types google-generative-ai.mts > ../google-generative-ai.jsonl` |

`google-generative-ai.mts` swaps `globalThis.fetch`, because the `@google/genai` SDK
takes no fetch option; it records the URL, headers and the body the SDK serializes.

| Script | Fixture | Run with |
|---|---|---|
| `google-generative-ai.mts` | `google-generative-ai.jsonl` | `node --experimental-strip-types google-generative-ai.mts > ../google-generative-ai.jsonl` |

`google-generative-ai.mts` swaps `globalThis.fetch`, because the `@google/genai` SDK
takes no fetch option; it records the URL, the headers and the body the SDK serializes.
The Vertex adapter has no generator: its endpoint rules are covered by
`tests/google_vertex.rs` against the URLs the SDK's own `getBaseUrl`/`constructUrl` build.

| Script | Fixture | Run with |
|---|---|---|
| `bedrock-converse-stream.mts` | `bedrock-converse-stream.jsonl` | copy into `packages/ai/test/tmpgen/gen.test.ts`, wrap the loop in an `it(...)` that writes the file, then `node ../../node_modules/vitest/vitest.mjs --run test/tmpgen/gen.test.ts` |

`bedrock-converse-stream.mts` needs vitest because it mocks the AWS SDK client with
`vi.mock`; the other generators run under plain `node --experimental-strip-types`.

| Script | Fixture | Run with |
|---|---|---|
| `mistral-conversations.mts` | `mistral-conversations.jsonl` | `node --experimental-strip-types mistral-conversations.mts > ../mistral-conversations.jsonl` |

`mistral-conversations.mts` injects a fake `fetch` through the adapter's own `fetch`
option and records the URL, the headers and the body next to the event sequence.

| Script | Fixture | Run with |
|---|---|---|
| `pi-messages.mts` | `pi-messages.jsonl` | `node --experimental-strip-types pi-messages.mts > ../pi-messages.jsonl` |

`pi-messages.mts` injects a fake `fetch` and records the URL, the headers, the body and
the status text next to the event sequence.
