// Captures request bodies, request URLs/headers and stream event sequences of the TS
// openai-codex-responses adapter. The SSE transport is exercised through an injected
// `fetch`; the WebSocket transport is not reachable that way and is covered by hand.
import { stream } from "/Users/dev/projects/notagent-main/packages/ai/src/api/openai-codex-responses.ts";

// A JWT whose payload carries the chatgpt_account_id claim the adapter reads.
function makeToken(accountId: string): string {
  const payload = Buffer.from(
    JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: accountId } }),
  ).toString("base64url");
  return `header.${payload}.signature`;
}
const TOKEN = makeToken("acct_123");

const baseModel: any = {
  id: "gpt-5-codex", name: "gpt-5-codex", api: "openai-codex-responses", provider: "openai-codex",
  baseUrl: "https://chatgpt.com/backend-api", reasoning: true, input: ["text", "image"],
  cost: { input: 1, output: 2, cacheRead: 0.1, cacheWrite: 0 },
  contextWindow: 128000, maxTokens: 4096,
};

const tool = {
  name: "read", description: "Reads a file",
  parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
};
const otherTool = { name: "write", description: "Writes a file", parameters: { type: "object", properties: {} } };

const user = (text: string) => ({ role: "user", content: text, timestamp: 1 });
const assistantWithToolCall = {
  role: "assistant", timestamp: 2, api: "openai-codex-responses", provider: "openai-codex", model: "gpt-5-codex",
  usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
  stopReason: "toolUse",
  content: [{ type: "toolCall", id: "call_1|fc_item1", name: "read", arguments: { path: "a.txt" } }],
};
const toolResult = {
  role: "toolResult", timestamp: 3, toolCallId: "call_1|fc_item1", toolName: "read",
  content: [{ type: "text", text: "file body" }], isError: false,
};

function sse(events: unknown[]): string {
  return events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join("") + "data: [DONE]\n\n";
}
const completed = (extra: Record<string, unknown> = {}) => ({
  type: "response.completed",
  response: { id: "resp_1", status: "completed", ...extra },
});

interface Case { name: string; model?: any; context?: any; options?: any; body?: string; status?: number }

const cases: Case[] = [
  // --- Request bodies. ---
  { name: "minimal" },
  { name: "system-prompt", context: { systemPrompt: "be precise", messages: [user("hi")] } },
  { name: "tools", context: { messages: [user("hi")], tools: [tool] } },
  { name: "tools-no-strict-mode", model: { compat: { supportsStrictMode: false } }, context: { messages: [user("hi")], tools: [tool] } },
  { name: "tool-call-replay", context: { messages: [user("hi"), assistantWithToolCall, toolResult] } },
  { name: "tool-choice", options: { toolChoice: "required" } },
  { name: "text-verbosity", options: { textVerbosity: "high" } },
  { name: "temperature", options: { temperature: 0.5 } },
  { name: "service-tier", options: { serviceTier: "priority" } },
  { name: "session-id", options: { sessionId: "session-abc" } },
  { name: "session-id-clamped", options: { sessionId: "s".repeat(80) } },
  { name: "cache-retention-none", options: { sessionId: "session-abc", cacheRetention: "none" } },
  { name: "reasoning-effort", options: { reasoningEffort: "high" } },
  { name: "reasoning-effort-mapped", model: { thinkingLevelMap: { high: "deep" } }, options: { reasoningEffort: "high" } },
  { name: "reasoning-effort-none", model: { thinkingLevelMap: { off: "minimal" } }, options: { reasoningEffort: "none" } },
  { name: "reasoning-effort-none-unmapped", options: { reasoningEffort: "none" } },
  { name: "reasoning-effort-null", model: { thinkingLevelMap: { high: null } }, options: { reasoningEffort: "high" } },
  { name: "reasoning-summary", options: { reasoningEffort: "low", reasoningSummary: "detailed" } },
  { name: "reasoning-absent" },
  {
    name: "deferred-additional-tools",
    model: { compat: { supportsAdditionalTools: true } },
    context: { messages: [user("hi"), assistantWithToolCall, { ...toolResult, addedToolNames: ["write"] }], tools: [tool, otherTool] },
  },
  {
    name: "deferred-tool-search",
    model: { compat: { supportsToolSearch: true } },
    context: { messages: [user("hi"), assistantWithToolCall, { ...toolResult, addedToolNames: ["write"] }], tools: [tool, otherTool] },
  },
  { name: "custom-base-url", model: { baseUrl: "https://proxy.example.com/api" } },
  { name: "custom-base-url-with-codex", model: { baseUrl: "https://proxy.example.com/api/codex" } },
  { name: "custom-base-url-with-responses", model: { baseUrl: "https://proxy.example.com/api/codex/responses" } },
  { name: "headers", model: { headers: { "x-model": "from-model" } }, options: { headers: { "x-request": "from-request", originator: null } } },

  // --- Streams. ---
  {
    name: "stream-text",
    body: sse([
      { type: "response.created", response: { id: "resp_1" } },
      { type: "response.output_item.added", output_index: 0, item: { type: "message", id: "msg_1", role: "assistant", content: [], status: "in_progress" } },
      { type: "response.output_text.delta", output_index: 0, delta: "Hello" },
      { type: "response.output_item.done", output_index: 0, item: { type: "message", id: "msg_1", role: "assistant", status: "completed", content: [{ type: "output_text", text: "Hello", annotations: [] }] } },
      completed({ usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15 } }),
    ]),
  },
  {
    name: "stream-response-done-is-mapped-to-completed",
    body: sse([{ type: "response.done", response: { id: "resp_1", status: "completed", end_turn: true } }]),
  },
  {
    name: "stream-unknown-status-is-dropped",
    body: sse([{ type: "response.done", response: { id: "resp_1", status: "weird" } }]),
  },
  {
    name: "stream-end-turn-false",
    body: sse([{ type: "response.done", response: { id: "resp_1", status: "completed", end_turn: false } }]),
  },
  {
    name: "stream-incomplete",
    body: sse([{ type: "response.incomplete", response: { id: "resp_1", status: "incomplete", incomplete_details: { reason: "max_output_tokens" } } }]),
  },
  { name: "stream-error-event", body: sse([{ type: "error", code: "rate_limit", message: "slow down" }]) },
  { name: "stream-error-event-nested", body: sse([{ type: "error", error: { code: "bad", message: "nested" } }]) },
  { name: "stream-response-failed", body: sse([{ type: "response.failed", response: { id: "r", error: { code: "server_error", message: "boom" } } }]) },
  { name: "stream-response-failed-without-error", body: sse([{ type: "response.failed", response: { id: "r" } }]) },
  { name: "stream-no-terminal-event", body: sse([{ type: "response.created", response: { id: "resp_1" } }]) },
  {
    name: "stream-service-tier",
    options: { serviceTier: "flex" },
    body: sse([completed({ service_tier: "default", usage: { input_tokens: 100, output_tokens: 20, total_tokens: 120 } })]),
  },
  {
    name: "stream-tool-call",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: "" } },
      { type: "response.function_call_arguments.delta", output_index: 0, delta: '{"path":"a"}' },
      { type: "response.output_item.done", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: '{"path":"a"}' } },
      completed(),
    ]),
  },
  { name: "http-error-429-usage-limit", status: 429, body: JSON.stringify({ error: { code: "usage_limit_reached", plan_type: "PLUS", message: "limit" } }) },
  { name: "http-error-429-plain", status: 429, body: JSON.stringify({ error: { code: "rate_limit_exceeded", message: "slow" } }) },
  { name: "http-error-500", status: 500, body: "upstream exploded" },
  { name: "http-error-400-json", status: 400, body: JSON.stringify({ error: { code: "bad_request", message: "nope" } }) },
];

const out: string[] = [];
for (const testCase of cases) {
  const model: any = { ...baseModel, ...(testCase.model ?? {}) };
  const context = testCase.context ?? { messages: [user("hi")] };
  const status = testCase.status ?? 200;
  const body = testCase.body ?? sse([completed()]);
  let payload: any;
  let requestUrl: string | undefined;
  let requestHeaders: Record<string, string> | undefined;
  let requestBodyEncoding: string | undefined;
  const events: unknown[] = [];
  const s = stream(model, context as any, {
    apiKey: TOKEN,
    // Force the SSE path: the WebSocket transport needs a real socket.
    transport: "sse",
    maxRetries: 0,
    fetch: (async (input: any, init: any) => {
      requestUrl = typeof input === "string" ? input : input.url;
      requestHeaders = {};
      new Headers(init?.headers).forEach((value, key) => { requestHeaders![key] = value; });
      requestBodyEncoding = requestHeaders["content-encoding"];
      return new Response(body, {
        status,
        headers: { "content-type": status === 200 ? "text/event-stream" : "application/json" },
      });
    }) as any,
    onPayload: (captured: unknown) => { payload = captured; return undefined; },
    ...(testCase.options ?? {}),
  } as any);
  for await (const event of s) {
    const { partial, ...rest } = event as any;
    events.push(rest);
  }
  out.push(JSON.stringify({
    name: testCase.name, model, context, options: testCase.options ?? {},
    payload, requestUrl, requestHeaders, requestBodyEncoding, status, body, events,
  }));
}
process.stdout.write(out.join("\n") + "\n");
