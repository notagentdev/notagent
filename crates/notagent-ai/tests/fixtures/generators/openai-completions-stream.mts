// Captures the event sequence the TS openai-completions stream emits for scripted SSE
// bodies. Each case records every event (without the `partial` snapshots, which are just
// the message under construction) plus the final message or error.
import { stream } from "/Users/dev/projects/notagent-main/packages/ai/src/api/openai-completions.ts";

const baseModel: any = {
  id: "gpt-5", name: "gpt-5", api: "openai-completions", provider: "openai",
  baseUrl: "https://api.openai.com/v1", reasoning: true, input: ["text", "image"],
  cost: { input: 1, output: 2, cacheRead: 0.1, cacheWrite: 0 },
  contextWindow: 128000, maxTokens: 4096,
};

const grammarTool = {
  name: "grammar_tool", description: "Grammar constrained",
  parameters: { type: "object", properties: { input: { type: "string" } }, required: ["input"] },
  constrainedSampling: { type: "grammar", variants: { openai_lark: "start: /.+/" } },
};

function sse(chunks: unknown[], terminate = true): string {
  const lines = chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`);
  if (terminate) lines.push("data: [DONE]\n\n");
  return lines.join("");
}

const chunk = (delta: unknown, extra: Record<string, unknown> = {}) => ({
  id: "chatcmpl-1", model: "gpt-5", choices: [{ index: 0, delta, ...extra }],
});
const finish = (reason: string) => ({ id: "chatcmpl-1", model: "gpt-5", choices: [{ index: 0, delta: {}, finish_reason: reason }] });

interface Case { name: string; model?: any; context?: any; options?: any; body: string; status?: number }

const cases: Case[] = [
  { name: "text", body: sse([chunk({ role: "assistant" }), chunk({ content: "Hel" }), chunk({ content: "lo" }), finish("stop")]) },
  { name: "empty-content-deltas-are-ignored", body: sse([chunk({ content: "" }), chunk({ content: null }), chunk({ content: "x" }), finish("stop")]) },
  { name: "usage", body: sse([chunk({ content: "hi" }), { id: "chatcmpl-1", model: "gpt-5", choices: [], usage: { prompt_tokens: 100, completion_tokens: 20, prompt_tokens_details: { cached_tokens: 40, cache_write_tokens: 10 }, completion_tokens_details: { reasoning_tokens: 5 } } }, finish("stop")]) },
  { name: "usage-deepseek-cache-hit", body: sse([chunk({ content: "hi" }), { id: "1", choices: [], usage: { prompt_tokens: 100, completion_tokens: 20, prompt_cache_hit_tokens: 30 } }, finish("stop")]) },
  { name: "usage-on-choice", body: sse([{ id: "1", choices: [{ index: 0, delta: { content: "hi" }, usage: { prompt_tokens: 7, completion_tokens: 3 } }] }, finish("stop")]) },
  { name: "response-model", body: sse([{ id: "1", model: "gpt-5-2025", choices: [{ index: 0, delta: { content: "hi" } }] }, finish("stop")]) },
  { name: "response-model-same-as-request", body: sse([{ id: "1", model: "gpt-5", choices: [{ index: 0, delta: { content: "hi" } }] }, finish("stop")]) },
  { name: "reasoning-content", body: sse([chunk({ reasoning_content: "thin" }), chunk({ reasoning_content: "king" }), chunk({ content: "answer" }), finish("stop")]) },
  { name: "reasoning-field", body: sse([chunk({ reasoning: "hmm" }), finish("stop")]) },
  { name: "reasoning-text-field", body: sse([chunk({ reasoning_text: "hmm" }), finish("stop")]) },
  { name: "reasoning-duplicate-fields", body: sse([chunk({ reasoning_content: "a", reasoning: "a" }), finish("stop")]) },
  { name: "reasoning-opencode-go", model: { provider: "opencode-go", baseUrl: "https://opencode.ai/zen/v1" }, body: sse([chunk({ reasoning: "hmm" }), finish("stop")]) },
  {
    name: "tool-call",
    body: sse([
      chunk({ tool_calls: [{ index: 0, id: "call_1", type: "function", function: { name: "read", arguments: "" } }] }),
      chunk({ tool_calls: [{ index: 0, function: { arguments: '{"path":' } }] }),
      chunk({ tool_calls: [{ index: 0, function: { arguments: '"a.txt"}' } }] }),
      finish("tool_calls"),
    ]),
  },
  {
    name: "tool-call-two-parallel",
    body: sse([
      chunk({ tool_calls: [{ index: 0, id: "call_1", function: { name: "read", arguments: '{"a":1}' } }, { index: 1, id: "call_2", function: { name: "write", arguments: '{"b":2}' } }] }),
      finish("tool_calls"),
    ]),
  },
  {
    name: "tool-call-without-index",
    body: sse([
      chunk({ tool_calls: [{ id: "call_1", function: { name: "read", arguments: "{}" } }] }),
      chunk({ tool_calls: [{ id: "call_1", function: { arguments: "" } }] }),
      finish("tool_calls"),
    ]),
  },
  {
    name: "tool-call-id-arrives-late",
    body: sse([
      chunk({ tool_calls: [{ index: 0, function: { name: "read" } }] }),
      chunk({ tool_calls: [{ index: 0, id: "call_late", function: { arguments: '{"x":1}' } }] }),
      finish("tool_calls"),
    ]),
  },
  {
    name: "tool-call-truncated-arguments",
    body: sse([
      chunk({ tool_calls: [{ index: 0, id: "call_1", function: { name: "read", arguments: '{"path":"a' } }] }),
      finish("tool_calls"),
    ]),
  },
  {
    name: "custom-tool-call",
    context: { messages: [{ role: "user", content: "hi", timestamp: 1 }], tools: [grammarTool] },
    model: { compat: { supportsOpenAIGrammarTools: true } },
    body: sse([
      chunk({ tool_calls: [{ index: 0, id: "call_1", type: "custom", custom: { name: "grammar_tool", input: "ab" } }] }),
      chunk({ tool_calls: [{ index: 0, custom: { input: "cd" } }] }),
      finish("tool_calls"),
    ]),
  },
  {
    name: "custom-tool-call-unknown-tool",
    body: sse([
      chunk({ tool_calls: [{ index: 0, id: "call_1", type: "custom", custom: { name: "made_up", input: "x" } }] }),
      finish("tool_calls"),
    ]),
  },
  {
    name: "reasoning-details-for-known-tool-call",
    body: sse([
      chunk({ tool_calls: [{ index: 0, id: "call_1", function: { name: "read", arguments: "{}" } }] }),
      chunk({ reasoning_details: [{ type: "reasoning.encrypted", id: "call_1", data: "enc" }] }),
      finish("tool_calls"),
    ]),
  },
  {
    name: "reasoning-details-before-tool-call",
    body: sse([
      chunk({ reasoning_details: [{ type: "reasoning.encrypted", id: "call_1", data: "enc" }] }),
      chunk({ tool_calls: [{ index: 0, id: "call_1", function: { name: "read", arguments: "{}" } }] }),
      finish("tool_calls"),
    ]),
  },
  {
    name: "reasoning-details-ignored-when-incomplete",
    body: sse([
      chunk({ tool_calls: [{ index: 0, id: "call_1", function: { name: "read", arguments: "{}" } }] }),
      chunk({ reasoning_details: [{ type: "reasoning.encrypted", id: "call_1" }, { type: "reasoning.text", id: "call_1", data: "x" }] }),
      finish("tool_calls"),
    ]),
  },
  { name: "finish-length", body: sse([chunk({ content: "hi" }), finish("length")]) },
  { name: "finish-content-filter", body: sse([chunk({ content: "hi" }), finish("content_filter")]) },
  { name: "finish-network-error", body: sse([finish("network_error")]) },
  { name: "finish-unknown", body: sse([finish("weird_reason")]) },
  { name: "finish-function-call", body: sse([chunk({ tool_calls: [{ index: 0, id: "c", function: { name: "read", arguments: "{}" } }] }), finish("function_call")]) },
  { name: "finish-end", body: sse([chunk({ content: "hi" }), finish("end")]) },
  { name: "missing-finish-reason", body: sse([chunk({ content: "hi" })]) },
  { name: "missing-finish-reason-tolerated", model: { compat: { supportsFinishReason: false } }, body: sse([chunk({ content: "hi" })]) },
  { name: "missing-finish-reason-tolerated-tool-call", model: { compat: { supportsFinishReason: false } }, body: sse([chunk({ tool_calls: [{ index: 0, id: "c", function: { name: "read", arguments: "{}" } }] })]) },
  { name: "null-chunks-are-ignored", body: sse([null, chunk({ content: "OK" }, { finish_reason: null }), { id: "chatcmpl-test", choices: [{ delta: {}, finish_reason: "stop" }], usage: { prompt_tokens: 3, completion_tokens: 1, prompt_tokens_details: { cached_tokens: 0 }, completion_tokens_details: { reasoning_tokens: 0 } } }]) },
  { name: "only-null-finish-reasons", body: sse([chunk({ content: "partial answer" }, { finish_reason: null })]) },
  {
    name: "tool-call-ids-mutate-mid-stream",
    body: sse([
      chunk({ tool_calls: [{ index: 0, id: "functions.read:0", type: "function", function: { name: "read", arguments: "" } }] }, { finish_reason: null }),
      chunk({ tool_calls: [{ index: 0, id: "chatcmpl-tool-a", type: "function", function: { name: null, arguments: '{"path":"README' } }] }, { finish_reason: null }),
      chunk({ tool_calls: [{ index: 0, id: "chatcmpl-tool-b", type: "function", function: { name: null, arguments: '.md"}' } }] }, { finish_reason: "tool_calls" }),
    ]),
  },
  {
    name: "empty-custom-object-on-a-function-tool-call",
    body: sse([
      chunk({ tool_calls: [{ index: 0, id: "call_1", type: "function", function: { name: "read", arguments: '{"path":"README.md"}' }, custom: {} }] }, { finish_reason: "tool_calls" }),
    ]),
  },
  {
    name: "mixed-content-reasoning-and-parallel-tool-calls",
    body: sse([
      chunk({ reasoning_content: "let me think" }),
      chunk({ content: "I will " }),
      chunk({ tool_calls: [{ index: 0, id: "call_a", function: { name: "read", arguments: '{"p":' } }] }),
      chunk({ content: "read two files" }),
      chunk({ tool_calls: [{ index: 1, id: "call_b", function: { name: "read", arguments: '{"p":"b"}' } }] }),
      chunk({ tool_calls: [{ index: 0, function: { arguments: '"a"}' } }] }),
      finish("tool_calls"),
    ]),
  },
  { name: "empty-stream", body: sse([]) },
  { name: "no-choices", body: sse([{ id: "1", choices: [] }, finish("stop")]) },
  // No unparsable-chunk case: the thrown text comes from V8's JSON.parse and has no
  // portable wording (see the dedicated Rust test).
  { name: "chunk-error-field", body: sse([chunk({ content: "hi" }), { error: { message: "upstream blew up" } }, finish("stop")]) },
  { name: "chunk-error-field-without-message", body: sse([{ error: { code: 500 } }]) },
  { name: "http-error", status: 429, body: JSON.stringify({ error: { message: "slow down", metadata: { raw: "upstream said no" } } }) },
  { name: "http-error-plain-text", status: 500, body: "gateway exploded" },
  { name: "http-error-nested-message", status: 400, body: JSON.stringify({ error: { message: "bad request" } }) },
];

const out: string[] = [];
for (const testCase of cases) {
  const model: any = { ...baseModel, ...(testCase.model ?? {}) };
  const context = testCase.context ?? { messages: [{ role: "user", content: "hi", timestamp: 1 }] };
  const status = testCase.status ?? 200;
  const events: unknown[] = [];
  const s = stream(model, context as any, {
    apiKey: "k",
    maxRetries: 0,
    fetch: (async () =>
      new Response(testCase.body, {
        status,
        headers: { "content-type": status === 200 ? "text/event-stream" : "application/json" },
      })) as any,
    ...(testCase.options ?? {}),
  } as any);
  for await (const event of s) {
    const { partial, ...rest } = event as any;
    events.push(rest);
  }
  out.push(JSON.stringify({ name: testCase.name, model, context, body: testCase.body, status, events }));
}
process.stdout.write(out.join("\n") + "\n");
