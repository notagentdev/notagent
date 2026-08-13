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
