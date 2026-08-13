// Captures the request URL, headers and body the TS mistral-conversations adapter
// sends, plus the event sequence for scripted SSE bodies. The adapter takes a
// `fetch` option, so a fake fetch records everything without a network call.
import { stream } from "/Users/dev/projects/notagent-main/packages/ai/src/api/mistral-conversations.ts";

const baseModel: any = {
  id: "mistral-medium-3.5", name: "mistral-medium-3.5", api: "mistral-conversations", provider: "mistral",
  baseUrl: "https://api.mistral.ai", reasoning: true, input: ["text", "image"],
  cost: { input: 1, output: 2, cacheRead: 0.1, cacheWrite: 0 },
  contextWindow: 128000, maxTokens: 4096,
};

const tool = {
  name: "read", description: "Reads a file",
  parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
};
const strictTool = { ...tool, name: "strict_read", constrainedSampling: { type: "json_schema", strict: "require" } };

const user = (text: string) => ({ role: "user", content: text, timestamp: 1 });
const assistantBase = {
  role: "assistant", timestamp: 2, api: "mistral-conversations", provider: "mistral", model: "mistral-medium-3.5",
  usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
  stopReason: "toolUse", content: [],
};
const toolResult = (extra: Record<string, unknown> = {}) => ({
  role: "toolResult", timestamp: 3, toolCallId: "call_1", toolName: "read",
  content: [{ type: "text", text: "file body" }], isError: false, ...extra,
});

function sse(chunks: unknown[], terminate = true): string {
  const lines = chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`);
  if (terminate) lines.push("data: [DONE]\n\n");
  return lines.join("");
}
const chunk = (delta: unknown, extra: Record<string, unknown> = {}) => ({
  id: "cmpl-1", choices: [{ index: 0, delta, ...extra }],
});
const finish = (reason: string) => ({ id: "cmpl-1", choices: [{ index: 0, delta: {}, finish_reason: reason }] });

interface Case { name: string; model?: any; context?: any; options?: any; body?: string; status?: number }

const cases: Case[] = [
  // --- Request bodies. ---
  { name: "minimal" },
  { name: "system-prompt", context: { systemPrompt: "be nice", messages: [user("hi")] } },
  { name: "temperature-and-max-tokens", options: { temperature: 0.3, maxTokens: 512 } },
  { name: "tools", context: { messages: [user("hi")], tools: [tool] } },
  { name: "tools-strict", context: { messages: [user("hi")], tools: [strictTool] } },
  { name: "tool-choice-auto", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: "auto" } },
  { name: "tool-choice-any", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: "any" } },
  { name: "tool-choice-function", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: { type: "function", function: { name: "read" } } } },
  { name: "prompt-mode-reasoning", options: { promptMode: "reasoning" } },
  { name: "reasoning-effort", options: { reasoningEffort: "high" } },
  { name: "prompt-cache", options: { sessionId: "session-1" } },
  { name: "prompt-cache-disabled", options: { sessionId: "session-1", cacheRetention: "none" } },
  { name: "prompt-cache-explicit-affinity", options: { sessionId: "session-1", headers: { "x-affinity": "manual" } } },
  // `Model.headers` is `Record<string, string>`; only `ProviderHeaders` may delete with null.
  { name: "header-overrides", model: { headers: { "x-model": "yes" } }, options: { headers: { "x-request": "1", accept: null } } },
  { name: "images", context: { messages: [{ role: "user", content: [{ type: "text", text: "look" }, { type: "image", data: "AAA", mimeType: "image/png" }], timestamp: 1 }] } },
  { name: "images-unsupported", model: { input: ["text"] }, context: { messages: [{ role: "user", content: [{ type: "text", text: "look" }, { type: "image", data: "AAA", mimeType: "image/png" }], timestamp: 1 }] } },
  { name: "images-only-unsupported", model: { input: ["text"] }, context: { messages: [{ role: "user", content: [{ type: "image", data: "AAA", mimeType: "image/png" }], timestamp: 1 }] } },
  { name: "empty-user-blocks", context: { messages: [{ role: "user", content: [], timestamp: 1 }, user("hi")] } },
  {
    name: "assistant-replay",
    context: {
      messages: [
        user("hi"),
        { ...assistantBase, content: [
          { type: "thinking", thinking: "hmm" },
          { type: "text", text: "answer" },
          { type: "toolCall", id: "call_1", name: "read", arguments: { path: "a.txt" } },
        ] },
        toolResult(),
      ],
    },
  },
  {
    name: "assistant-empty-blocks",
    context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "text", text: "  " }, { type: "thinking", thinking: " " }] }, user("again")] },
  },
  {
    name: "tool-result-error",
    context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_1", name: "read", arguments: {} }] }, toolResult({ isError: true })] },
  },
  {
    name: "tool-result-empty",
    context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_1", name: "read", arguments: {} }] }, toolResult({ content: [] })] },
  },
  {
    name: "tool-result-image",
    context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_1", name: "read", arguments: {} }] }, toolResult({ content: [{ type: "image", data: "AAA", mimeType: "image/png" }] })] },
  },
  {
    name: "tool-result-image-unsupported",
    model: { input: ["text"] },
    context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_1", name: "read", arguments: {} }] }, toolResult({ content: [{ type: "image", data: "AAA", mimeType: "image/png" }] })] },
  },
  {
    name: "tool-call-id-normalization",
    context: {
      messages: [
        user("hi"),
        { ...assistantBase, content: [
          { type: "toolCall", id: "call_abcdefghij", name: "read", arguments: {} },
          { type: "toolCall", id: "toolu_01ABCDEF", name: "read", arguments: {} },
          { type: "toolCall", id: "123456789", name: "read", arguments: {} },
        ] },
        toolResult({ toolCallId: "call_abcdefghij" }),
        toolResult({ toolCallId: "toolu_01ABCDEF" }),
        toolResult({ toolCallId: "123456789" }),
      ],
    },
  },

  // --- Event sequences. ---
  { name: "stream-text", body: sse([chunk({ content: "Hel" }), chunk({ content: "lo" }), finish("stop")]) },
  { name: "stream-content-chunks", body: sse([chunk({ content: [{ type: "text", text: "Hel" }] }), chunk({ content: [{ type: "text", text: "lo" }] }), finish("stop")]) },
  { name: "stream-thinking", body: sse([chunk({ content: [{ type: "thinking", thinking: [{ text: "hm" }, { text: "m" }] }] }), chunk({ content: "answer" }), finish("stop")]) },
  { name: "stream-thinking-empty", body: sse([chunk({ content: [{ type: "thinking", thinking: [] }] }), chunk({ content: "answer" }), finish("stop")]) },
  { name: "stream-null-content", body: sse([chunk({ content: null }), chunk({ content: "x" }), finish("stop")]) },
  { name: "stream-usage", body: sse([chunk({ content: "hi" }), { id: "cmpl-1", choices: [], usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 130, prompt_tokens_details: { cached_tokens: 40 } } }, finish("stop")]) },
  { name: "stream-usage-num-cached", body: sse([chunk({ content: "hi" }), { id: "cmpl-1", choices: [], usage: { prompt_tokens: 100, completion_tokens: 20, num_cached_tokens: 30 } }, finish("stop")]) },
  { name: "stream-usage-overflow-cached", body: sse([chunk({ content: "hi" }), { id: "cmpl-1", choices: [], usage: { prompt_tokens: 10, completion_tokens: 2, num_cached_tokens: 99 } }, finish("stop")]) },
  {
    name: "stream-tool-call",
    body: sse([
      chunk({ tool_calls: [{ id: "abcdefghi", index: 0, function: { name: "read", arguments: "" } }] }),
      chunk({ tool_calls: [{ id: "abcdefghi", index: 0, function: { name: "read", arguments: '{"path":' } }] }),
      chunk({ tool_calls: [{ id: "abcdefghi", index: 0, function: { name: "read", arguments: '"a.txt"}' } }] }),
      finish("tool_calls"),
    ]),
  },
  {
    name: "stream-tool-call-object-arguments",
    body: sse([
      chunk({ tool_calls: [{ id: "abcdefghi", index: 0, function: { name: "read", arguments: { path: "a.txt" } } }] }),
      finish("tool_calls"),
    ]),
  },
  {
    name: "stream-tool-call-null-id",
    body: sse([
      chunk({ tool_calls: [{ id: "null", index: 2, function: { name: "read", arguments: "{}" } }] }),
      finish("tool_calls"),
    ]),
  },
  { name: "stream-finish-length", body: sse([chunk({ content: "hi" }), finish("length")]) },
  { name: "stream-finish-model-length", body: sse([chunk({ content: "hi" }), finish("model_length")]) },
  { name: "stream-finish-error", body: sse([chunk({ content: "hi" }), finish("error")]) },
  { name: "stream-finish-unknown", body: sse([chunk({ content: "hi" }), finish("content_filter")]) },
  { name: "stream-no-finish-reason", body: sse([chunk({ content: "hi" })]) },
  { name: "stream-empty", body: sse([]) },
  { name: "stream-crlf-separators", body: "data: " + JSON.stringify(chunk({ content: "hi" })) + "\r\n\r\ndata: " + JSON.stringify(finish("stop")) + "\r\n\r\ndata: [DONE]\r\n\r\n" },
  { name: "stream-trailing-event-without-boundary", body: "data: " + JSON.stringify(chunk({ content: "hi" })) + "\n\ndata: " + JSON.stringify(finish("stop")) },
  { name: "http-error", status: 429, body: JSON.stringify({ message: "slow down" }) },
  { name: "http-error-plain-text", status: 500, body: "gateway exploded" },
];

const out: string[] = [];
for (const testCase of cases) {
  const model: any = { ...baseModel, ...(testCase.model ?? {}) };
  const context = testCase.context ?? { messages: [user("hi")] };
  const status = testCase.status ?? 200;
  const body = testCase.body ?? sse([chunk({ content: "ok" }), finish("stop")]);
  let request: { url: string; headers: Record<string, string>; body: unknown } | undefined;
  const events: unknown[] = [];

  const s = stream(model, context as any, {
    apiKey: "k",
    fetch: (async (url: any, init: any) => {
      request = {
        url: String(url),
        headers: Object.fromEntries([...new Headers(init.headers).entries()]),
        body: JSON.parse(init.body),
      };
      return new Response(body, {
        status,
        headers: { "content-type": status === 200 ? "text/event-stream" : "application/json" },
      });
    }) as any,
    ...(testCase.options ?? {}),
  } as any);

  for await (const event of s) {
    const { partial, ...rest } = event as any;
    events.push(rest);
  }
  out.push(JSON.stringify({ name: testCase.name, model, context, options: testCase.options ?? {}, status, body, request, events }));
}
process.stdout.write(out.join("\n") + "\n");
