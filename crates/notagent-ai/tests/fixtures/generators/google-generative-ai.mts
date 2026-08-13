// Captures the request URL, headers and body the TS google-generative-ai adapter sends
// through the @google/genai SDK, plus the event sequence for a scripted SSE body.
// The SDK builds the wire body from the `config` object, so the payload the adapter's
// onPayload hook sees is not the request body — both are recorded.
import { stream } from "/Users/dev/projects/notagent-main/packages/ai/src/api/google-generative-ai.ts";

const baseModel: any = {
  id: "gemini-2.5-flash", name: "gemini-2.5-flash", api: "google-generative-ai", provider: "google",
  baseUrl: "", reasoning: true, input: ["text", "image"],
  cost: { input: 1, output: 2, cacheRead: 0.1, cacheWrite: 0 },
  contextWindow: 1000000, maxTokens: 8192,
};

const tool = {
  name: "read", description: "Reads a file",
  parameters: { $schema: "https://json-schema.org/draft/2020-12/schema", type: "object", properties: { path: { type: "string" } }, required: ["path"], $defs: {} },
};
const strictTool = { ...tool, name: "strict_read", constrainedSampling: { type: "json_schema", strict: "require" } };

const user = (text: string) => ({ role: "user", content: text, timestamp: 1 });
const assistantBase = {
  role: "assistant", timestamp: 2, api: "google-generative-ai", provider: "google", model: "gemini-2.5-flash",
  usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
  stopReason: "toolUse", content: [],
};
const toolResult = (extra: Record<string, unknown> = {}) => ({
  role: "toolResult", timestamp: 3, toolCallId: "call_1", toolName: "read",
  content: [{ type: "text", text: "file body" }], isError: false, ...extra,
});

function sse(chunks: unknown[]): string {
  return chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`).join("");
}
const finish = (reason = "STOP", extra: Record<string, unknown> = {}) => ({
  responseId: "resp_1",
  candidates: [{ finishReason: reason }],
  ...extra,
});

interface Case { name: string; model?: any; context?: any; options?: any; body?: string }

const cases: Case[] = [
  // --- Request bodies. ---
  { name: "minimal" },
  { name: "system-prompt", context: { systemPrompt: "be nice", messages: [user("hi")] } },
  { name: "temperature-and-max-tokens", options: { temperature: 0.3, maxTokens: 512 } },
  { name: "image-parts", context: { messages: [{ role: "user", content: [{ type: "text", text: "look" }, { type: "image", data: "AAA", mimeType: "image/png" }], timestamp: 1 }] } },
  { name: "empty-user-blocks", context: { messages: [{ role: "user", content: [], timestamp: 1 }, user("hi")] } },
  { name: "tools", context: { messages: [user("hi")], tools: [tool] } },
  { name: "tools-strict", model: { id: "gemini-3-pro" }, context: { messages: [user("hi")], tools: [strictTool] } },
  { name: "tool-choice-none", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: "none" } },
  { name: "tool-choice-any", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: "any" } },
  { name: "tool-choice-auto", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: "auto" } },
  {
    name: "assistant-replay",
    context: {
      messages: [
        user("hi"),
        { ...assistantBase, content: [
          { type: "thinking", thinking: "hmm", thinkingSignature: "c2ln" },
          { type: "text", text: "answer", textSignature: "dGV4" },
          { type: "toolCall", id: "call_1", name: "read", arguments: { path: "a.txt" }, thoughtSignature: "dG9vbA==" },
        ] },
        toolResult(),
      ],
    },
  },
  {
    name: "assistant-replay-foreign-model",
    context: {
      messages: [
        user("hi"),
        { ...assistantBase, model: "gemini-2.0-flash", content: [
          { type: "thinking", thinking: "hmm", thinkingSignature: "c2ln" },
          { type: "text", text: "answer", textSignature: "dGV4" },
        ] },
      ],
    },
  },
  {
    name: "assistant-empty-text-with-signature",
    context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "text", text: "", textSignature: "c2ln" }] }] },
  },
  {
    name: "assistant-empty-text-without-signature",
    context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "text", text: "  " }, { type: "text", text: "kept" }] }] },
  },
  {
    name: "assistant-invalid-signature",
    context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "text", text: "answer", textSignature: "not base64!" }] }] },
  },
  { name: "tool-result-error", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_1", name: "read", arguments: {} }] }, toolResult({ isError: true })] } },
  { name: "tool-result-images", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_1", name: "read", arguments: {} }] }, toolResult({ content: [{ type: "text", text: "shot" }, { type: "image", data: "BBB", mimeType: "image/png" }] })] } },
  { name: "tool-result-images-gemini-3", model: { id: "gemini-3-pro" }, context: { messages: [user("hi"), { ...assistantBase, model: "gemini-3-pro", content: [{ type: "toolCall", id: "call_1", name: "read", arguments: {} }] }, toolResult({ content: [{ type: "image", data: "BBB", mimeType: "image/png" }] })] } },
  {
    name: "tool-results-merged-into-one-turn",
    context: {
      messages: [
        user("hi"),
        { ...assistantBase, content: [
          { type: "toolCall", id: "call_1", name: "read", arguments: {} },
          { type: "toolCall", id: "call_2", name: "read", arguments: {} },
        ] },
        toolResult(),
        toolResult({ toolCallId: "call_2" }),
      ],
    },
  },
  { name: "tool-call-id-normalized", model: { id: "gemini-3-pro" }, context: { messages: [user("hi"), { ...assistantBase, model: "gemini-2.0-flash", content: [{ type: "toolCall", id: "call|weird/id", name: "read", arguments: {} }] }, toolResult({ toolCallId: "call|weird/id" })] } },
  { name: "thinking-enabled-budget", options: { thinking: { enabled: true, budgetTokens: 4096 } } },
  { name: "thinking-enabled-level", model: { id: "gemini-3-pro" }, options: { thinking: { enabled: true, level: "HIGH" } } },
  { name: "thinking-disabled-25", options: { thinking: { enabled: false } } },
  { name: "thinking-disabled-3-pro", model: { id: "gemini-3-pro" }, options: { thinking: { enabled: false } } },
  { name: "thinking-disabled-3-flash", model: { id: "gemini-3-flash" }, options: { thinking: { enabled: false } } },
  { name: "thinking-disabled-flash-latest", model: { id: "gemini-flash-latest" }, options: { thinking: { enabled: false } } },
  { name: "thinking-disabled-gemma4", model: { id: "gemma-4-27b" }, options: { thinking: { enabled: false } } },
  { name: "thinking-on-non-reasoning-model", model: { reasoning: false }, options: { thinking: { enabled: true, budgetTokens: 1024 } } },
  { name: "custom-base-url", model: { baseUrl: "https://proxy.example.com/v1beta" } },
  { name: "headers", model: { headers: { "x-model": "from-model" } }, options: { headers: { "x-request": "from-request" } } },

  // --- Streams. ---
  {
    name: "stream-text",
    body: sse([
      { responseId: "resp_1", candidates: [{ content: { parts: [{ text: "Hel" }] } }] },
      { candidates: [{ content: { parts: [{ text: "lo" }] } }] },
      finish("STOP", { usageMetadata: { promptTokenCount: 10, candidatesTokenCount: 4, cachedContentTokenCount: 2, thoughtsTokenCount: 3, totalTokenCount: 17 } }),
    ]),
  },
  {
    name: "stream-thinking-then-text",
    body: sse([
      { responseId: "r", candidates: [{ content: { parts: [{ text: "think", thought: true, thoughtSignature: "c2ln" }] } }] },
      { candidates: [{ content: { parts: [{ text: "ing", thought: true }] } }] },
      { candidates: [{ content: { parts: [{ text: "answer", thoughtSignature: "dGV4" }] } }] },
      finish(),
    ]),
  },
  {
    name: "stream-function-call",
    body: sse([
      { responseId: "r", candidates: [{ content: { parts: [{ functionCall: { name: "read", args: { path: "a" }, id: "call_1" }, thoughtSignature: "dG9vbA==" }] } }] },
      finish(),
    ]),
  },
  {
    name: "stream-function-call-without-id",
    body: sse([
      { responseId: "r", candidates: [{ content: { parts: [{ functionCall: { name: "read", args: {} } }] } }] },
      finish(),
    ]),
  },
  {
    name: "stream-function-call-duplicate-id",
    body: sse([
      { responseId: "r", candidates: [{ content: { parts: [{ functionCall: { name: "read", args: {}, id: "dup" } }, { functionCall: { name: "read", args: {}, id: "dup" } }] } }] },
      finish(),
    ]),
  },
  {
    name: "stream-text-then-function-call",
    body: sse([
      { responseId: "r", candidates: [{ content: { parts: [{ text: "calling" }] } }] },
      { candidates: [{ content: { parts: [{ functionCall: { name: "read", args: {}, id: "call_1" } }] } }] },
      finish(),
    ]),
  },
  { name: "stream-max-tokens", body: sse([{ responseId: "r", candidates: [{ content: { parts: [{ text: "cut" }] }, finishReason: "MAX_TOKENS" }] }]) },
  { name: "stream-safety", body: sse([{ responseId: "r", candidates: [{ finishReason: "SAFETY" }] }]) },
  { name: "stream-no-finish-reason", body: sse([{ responseId: "r", candidates: [{ content: { parts: [{ text: "x" }] } }] }]) },
  { name: "stream-empty", body: "" },
];

const out: string[] = [];
for (const testCase of cases) {
  const model: any = { ...baseModel, ...(testCase.model ?? {}) };
  const context = testCase.context ?? { messages: [user("hi")] };
  const body = testCase.body ?? sse([finish()]);
  let payload: any;
  let requestUrl: string | undefined;
  let requestHeaders: Record<string, string> | undefined;
  let requestBody: any;
  const events: unknown[] = [];
  const originalFetch = globalThis.fetch;
  (globalThis as any).fetch = async (input: any, init: any) => {
    requestUrl = typeof input === "string" ? input : input.url;
    requestHeaders = {};
    new Headers(init?.headers).forEach((value, key) => { requestHeaders![key] = value; });
    requestBody = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
    return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
  };
  try {
    const s = stream(model, context as any, {
      apiKey: "k",
      maxRetries: 0,
      onPayload: (captured: unknown) => { payload = captured; return undefined; },
      ...(testCase.options ?? {}),
    } as any);
    for await (const event of s) {
      const { partial, ...rest } = event as any;
      events.push(rest);
    }
  } finally {
    (globalThis as any).fetch = originalFetch;
  }
  out.push(JSON.stringify({
    name: testCase.name, model, context, options: testCase.options ?? {},
    payload, requestUrl, requestHeaders, requestBody, body, events,
  }));
}
process.stdout.write(out.join("\n") + "\n");
