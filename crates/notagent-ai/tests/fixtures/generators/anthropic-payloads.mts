import { stream } from "/Users/dev/projects/notagent-main/packages/ai/src/api/anthropic-messages.ts";

function sseResponse(): Response {
  const body =
    'event: message_start\ndata: {"type":"message_start","message":{"id":"m","usage":{}}}\n\n' +
    'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}\n\n' +
    'event: message_stop\ndata: {"type":"message_stop"}\n\n';
  return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
}
const fakeClient: any = { messages: { create: () => ({ asResponse: async () => sseResponse() }) } };

const baseModel: any = {
  id: "claude-opus-4-5", name: "Claude Opus 4.5", api: "anthropic-messages", provider: "anthropic",
  baseUrl: "https://api.anthropic.com", reasoning: true, input: ["text", "image"],
  cost: { input: 5, output: 25, cacheRead: 0.5, cacheWrite: 6.25 },
  contextWindow: 200000, maxTokens: 64000,
};

const tool = {
  name: "read", description: "Reads a file",
  parameters: { type: "object", properties: { path: { type: "string" }, offset: { type: "number" } }, required: ["path"] },
};

const cases: Array<{ name: string; model: any; context: any; options: any }> = [
  { name: "plain-text", model: baseModel, context: { systemPrompt: "sys", messages: [{ role: "user", content: "hello", timestamp: 1 }] }, options: {} },
  { name: "no-cache", model: baseModel, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }] }, options: { cacheRetention: "none" } },
  { name: "long-cache", model: baseModel, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }] }, options: { cacheRetention: "long" } },
  { name: "tools", model: baseModel, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }], tools: [tool] }, options: {} },
  { name: "thinking-budget", model: baseModel, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }] }, options: { thinkingEnabled: true, thinkingBudgetTokens: 4096 } },
  { name: "thinking-disabled", model: baseModel, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }] }, options: { thinkingEnabled: false } },
  { name: "adaptive-thinking", model: { ...baseModel, compat: { forceAdaptiveThinking: true } }, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }] }, options: { thinkingEnabled: true, effort: "xhigh" } },
  { name: "temperature", model: baseModel, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }] }, options: { temperature: 0.7 } },
  { name: "temperature-with-thinking", model: baseModel, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }] }, options: { temperature: 0.7, thinkingEnabled: true } },
  { name: "tool-choice", model: baseModel, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }], tools: [tool] }, options: { toolChoice: { type: "tool", name: "read" } } },
  { name: "metadata", model: baseModel, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }] }, options: { metadata: { user_id: "u1", other: "ignored" } } },
  { name: "images", model: baseModel, context: { messages: [{ role: "user", content: [{ type: "text", text: "look" }, { type: "image", data: "AAA", mimeType: "image/png" }], timestamp: 1 }] }, options: {} },
  { name: "image-only", model: baseModel, context: { messages: [{ role: "user", content: [{ type: "image", data: "AAA", mimeType: "image/png" }], timestamp: 1 }] }, options: {} },
  {
    name: "assistant-and-tool-results", model: baseModel,
    context: {
      messages: [
        { role: "user", content: "run", timestamp: 1 },
        { role: "assistant", content: [{ type: "text", text: "sure" }, { type: "toolCall", id: "toolu_1", name: "read", arguments: { path: "a.txt" } }], api: "anthropic-messages", provider: "anthropic", model: "claude-opus-4-5", usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } }, stopReason: "toolUse", timestamp: 2 },
        { role: "toolResult", toolCallId: "toolu_1", toolName: "read", content: [{ type: "text", text: "file body" }], isError: false, timestamp: 3 },
      ],
      tools: [tool],
    },
    options: {},
  },
  {
    name: "thinking-replay", model: baseModel,
    context: {
      messages: [
        { role: "user", content: "q", timestamp: 1 },
        { role: "assistant", content: [{ type: "thinking", thinking: "reasoned", thinkingSignature: "sig" }, { type: "text", text: "answer" }], api: "anthropic-messages", provider: "anthropic", model: "claude-opus-4-5", usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } }, stopReason: "stop", timestamp: 2 },
        { role: "user", content: "next", timestamp: 3 },
      ],
    },
    options: {},
  },
  {
    name: "unsigned-thinking-replay", model: baseModel,
    context: {
      messages: [
        { role: "assistant", content: [{ type: "thinking", thinking: "unsigned" }], api: "anthropic-messages", provider: "anthropic", model: "claude-opus-4-5", usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } }, stopReason: "stop", timestamp: 2 },
        { role: "user", content: "next", timestamp: 3 },
      ],
    },
    options: {},
  },
  { name: "strict-tools", model: { ...baseModel, compat: { supportsStrictTools: true } }, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }], tools: [{ ...tool, constrainedSampling: { type: "json_schema", strict: "prefer" } }] }, options: {} },
  { name: "no-eager-streaming", model: { ...baseModel, compat: { supportsEagerToolInputStreaming: false } }, context: { messages: [{ role: "user", content: "hi", timestamp: 1 }], tools: [tool] }, options: {} },
];

const out: string[] = [];
for (const testCase of cases) {
  let captured: unknown;
  const s = stream(testCase.model, testCase.context as any, {
    ...testCase.options,
    client: fakeClient,
    onPayload: (payload: unknown) => { captured = payload; return undefined; },
  } as any);
  await s.result();
  out.push(JSON.stringify({ name: testCase.name, payload: captured }));
}
process.stdout.write(out.join("\n") + "\n");
