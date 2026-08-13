// Captures the ConverseStream command input the TS bedrock adapter builds, through its
// `onPayload` hook, plus the event sequence for a scripted stream. The AWS SDK client is
// mocked so no credentials or network are involved.
import { vi } from "vitest";

const sdkState = vi.hoisted(() => ({ items: [] as unknown[] }));

vi.mock("@aws-sdk/client-bedrock-runtime", async (importOriginal) => {
  const actual = (await importOriginal()) as Record<string, unknown>;
  class BedrockRuntimeClient {
    middlewareStack = { add: () => {} };
    async send() {
      return {
        $metadata: { httpStatusCode: 200, requestId: "req-1" },
        stream: (async function* () {
          for (const item of sdkState.items) yield item;
        })(),
      };
    }
  }
  class ConverseStreamCommand {
    constructor(public input: unknown) {}
  }
  return { ...actual, BedrockRuntimeClient, ConverseStreamCommand };
});

const { stream } = await import(
  "/Users/dev/projects/notagent-main/packages/ai/src/api/bedrock-converse-stream.ts"
);

const baseModel: any = {
  id: "anthropic.claude-sonnet-4-5-20250929-v1:0", name: "Claude Sonnet 4.5",
  api: "bedrock-converse-stream", provider: "amazon-bedrock",
  baseUrl: "https://bedrock-runtime.us-east-1.amazonaws.com", reasoning: true, input: ["text", "image"],
  cost: { input: 3, output: 15, cacheRead: 0.3, cacheWrite: 3.75 },
  contextWindow: 200000, maxTokens: 64000,
};

const tool = {
  name: "read", description: "Reads a file",
  parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
};
const strictTool = { ...tool, name: "strict_read", constrainedSampling: { type: "json_schema", strict: "require" } };

const user = (text: string) => ({ role: "user", content: text, timestamp: 1 });
const assistantBase = {
  role: "assistant", timestamp: 2, api: "bedrock-converse-stream", provider: "amazon-bedrock",
  model: "anthropic.claude-sonnet-4-5-20250929-v1:0",
  usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
  stopReason: "toolUse", content: [],
};
const toolResult = (extra: Record<string, unknown> = {}) => ({
  role: "toolResult", timestamp: 3, toolCallId: "call_1", toolName: "read",
  content: [{ type: "text", text: "file body" }], isError: false, ...extra,
});

interface Case { name: string; model?: any; context?: any; options?: any; items?: unknown[] }

const cases: Case[] = [
  { name: "minimal" },
  { name: "system-prompt", context: { systemPrompt: "be nice", messages: [user("hi")] } },
  { name: "system-prompt-long-cache", context: { systemPrompt: "be nice", messages: [user("hi")] }, options: { cacheRetention: "long" } },
  { name: "system-prompt-no-cache", context: { systemPrompt: "be nice", messages: [user("hi")] }, options: { cacheRetention: "none" } },
  { name: "system-prompt-non-claude", model: { id: "amazon.nova-pro-v1:0", name: "Nova Pro" }, context: { systemPrompt: "be nice", messages: [user("hi")] } },
  { name: "temperature-and-max-tokens", options: { temperature: 0.4, maxTokens: 1024 } },
  { name: "non-claude-has-no-default-max-tokens", model: { id: "amazon.nova-pro-v1:0", name: "Nova Pro" } },
  { name: "image-parts", context: { messages: [{ role: "user", content: [{ type: "text", text: "look" }, { type: "image", data: "AAAA", mimeType: "image/png" }], timestamp: 1 }] } },
  { name: "empty-user-text", context: { messages: [{ role: "user", content: [{ type: "text", text: "   " }], timestamp: 1 }] } },
  { name: "tools", context: { messages: [user("hi")], tools: [tool] } },
  { name: "tools-strict", model: { compat: { supportsStrictMode: true } }, context: { messages: [user("hi")], tools: [strictTool] } },
  { name: "tool-choice-auto", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: "auto" } },
  { name: "tool-choice-any", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: "any" } },
  { name: "tool-choice-none", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: "none" } },
  { name: "tool-choice-tool", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: { type: "tool", name: "read" } } },
  {
    name: "assistant-replay",
    context: {
      messages: [
        user("hi"),
        { ...assistantBase, content: [
          { type: "thinking", thinking: "hmm", thinkingSignature: "sig" },
          { type: "text", text: "answer" },
          { type: "toolCall", id: "call_1", name: "read", arguments: { path: "a.txt" } },
        ] },
        toolResult(),
      ],
    },
  },
  {
    name: "assistant-thinking-without-signature",
    context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "thinking", thinking: "hmm" }, { type: "text", text: "answer" }] }] },
  },
  {
    name: "assistant-thinking-non-claude",
    model: { id: "amazon.nova-pro-v1:0", name: "Nova Pro" },
    context: { messages: [user("hi"), { ...assistantBase, model: "amazon.nova-pro-v1:0", content: [{ type: "thinking", thinking: "hmm", thinkingSignature: "sig" }] }] },
  },
  { name: "assistant-empty-skipped", context: { messages: [user("hi"), { ...assistantBase, content: [] }, user("again")] } },
  { name: "assistant-only-blank-text", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "text", text: "  " }] }, user("again")] } },
  {
    name: "consecutive-tool-results-merge",
    context: {
      messages: [
        user("hi"),
        { ...assistantBase, content: [
          { type: "toolCall", id: "call_1", name: "read", arguments: {} },
          { type: "toolCall", id: "call_2", name: "read", arguments: {} },
        ] },
        toolResult(),
        toolResult({ toolCallId: "call_2", isError: true }),
      ],
    },
  },
  { name: "tool-result-images", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_1", name: "read", arguments: {} }] }, toolResult({ content: [{ type: "image", data: "AAAA", mimeType: "image/jpeg" }] })] } },
  { name: "tool-result-empty", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_1", name: "read", arguments: {} }] }, toolResult({ content: [] })] } },
  { name: "tool-call-id-normalized", context: { messages: [user("hi"), { ...assistantBase, model: "other", content: [{ type: "toolCall", id: "call|weird/id", name: "read", arguments: {} }] }, toolResult({ toolCallId: "call|weird/id" })] } },
  { name: "reasoning-budget", options: { reasoning: "medium" } },
  { name: "reasoning-budget-custom", options: { reasoning: "low", thinkingBudgets: { low: 777 } } },
  { name: "reasoning-xhigh-clamped", options: { reasoning: "xhigh" } },
  { name: "reasoning-no-interleaved", options: { reasoning: "high", interleavedThinking: false } },
  { name: "reasoning-adaptive", model: { id: "anthropic.claude-opus-4-6-v1:0", name: "Claude Opus 4.6" }, options: { reasoning: "high" } },
  { name: "reasoning-adaptive-xhigh", model: { id: "anthropic.claude-opus-4-8-v1:0", name: "Claude Opus 4.8" }, options: { reasoning: "xhigh" } },
  { name: "reasoning-adaptive-omitted", model: { id: "anthropic.claude-opus-4-6-v1:0", name: "Claude Opus 4.6" }, options: { reasoning: "high", thinkingDisplay: "omitted" } },
  { name: "reasoning-govcloud", model: { id: "us-gov.anthropic.claude-sonnet-4-5-v1:0", name: "Claude Sonnet 4.5" }, options: { reasoning: "high" } },
  { name: "reasoning-non-claude", model: { id: "amazon.nova-pro-v1:0", name: "Nova Pro" }, options: { reasoning: "high" } },
  { name: "reasoning-on-non-reasoning-model", model: { reasoning: false }, options: { reasoning: "high" } },
  { name: "request-metadata", options: { requestMetadata: { team: "core" } } },
  { name: "cache-point-on-last-user-message", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "text", text: "answer" }] }, user("again")] } },

  // --- Streams. ---
  {
    name: "stream-text",
    items: [
      { messageStart: { role: "assistant" } },
      { contentBlockDelta: { contentBlockIndex: 0, delta: { text: "Hel" } } },
      { contentBlockDelta: { contentBlockIndex: 0, delta: { text: "lo" } } },
      { contentBlockStop: { contentBlockIndex: 0 } },
      { messageStop: { stopReason: "end_turn" } },
      { metadata: { usage: { inputTokens: 10, outputTokens: 4, cacheReadInputTokens: 2, cacheWriteInputTokens: 1, totalTokens: 17 } } },
    ],
  },
  {
    name: "stream-thinking",
    items: [
      { messageStart: { role: "assistant" } },
      { contentBlockDelta: { contentBlockIndex: 0, delta: { reasoningContent: { text: "think" } } } },
      { contentBlockDelta: { contentBlockIndex: 0, delta: { reasoningContent: { signature: "sig-a" } } } },
      { contentBlockDelta: { contentBlockIndex: 0, delta: { reasoningContent: { signature: "sig-b" } } } },
      { contentBlockStop: { contentBlockIndex: 0 } },
      { contentBlockDelta: { contentBlockIndex: 1, delta: { text: "answer" } } },
      { contentBlockStop: { contentBlockIndex: 1 } },
      { messageStop: { stopReason: "end_turn" } },
    ],
  },
  {
    name: "stream-tool-use",
    items: [
      { messageStart: { role: "assistant" } },
      { contentBlockStart: { contentBlockIndex: 0, start: { toolUse: { toolUseId: "call_1", name: "read" } } } },
      { contentBlockDelta: { contentBlockIndex: 0, delta: { toolUse: { input: '{"path":' } } } },
      { contentBlockDelta: { contentBlockIndex: 0, delta: { toolUse: { input: '"a.txt"}' } } } },
      { contentBlockStop: { contentBlockIndex: 0 } },
      { messageStop: { stopReason: "tool_use" } },
    ],
  },
  {
    name: "stream-truncated-tool-arguments",
    items: [
      { messageStart: { role: "assistant" } },
      { contentBlockStart: { contentBlockIndex: 0, start: { toolUse: { toolUseId: "call_1", name: "read" } } } },
      { contentBlockDelta: { contentBlockIndex: 0, delta: { toolUse: { input: '{"path":"a' } } } },
      { contentBlockStop: { contentBlockIndex: 0 } },
      { messageStop: { stopReason: "tool_use" } },
    ],
  },
  { name: "stream-max-tokens", items: [{ messageStart: { role: "assistant" } }, { messageStop: { stopReason: "max_tokens" } }] },
  { name: "stream-context-window-exceeded", items: [{ messageStart: { role: "assistant" } }, { messageStop: { stopReason: "model_context_window_exceeded" } }] },
  { name: "stream-stop-sequence", items: [{ messageStart: { role: "assistant" } }, { messageStop: { stopReason: "stop_sequence" } }] },
  { name: "stream-unknown-stop-reason", items: [{ messageStart: { role: "assistant" } }, { messageStop: { stopReason: "guardrail_intervened" } }] },
  { name: "stream-no-stop-reason", items: [{ messageStart: { role: "assistant" } }] },
  { name: "stream-user-message-start", items: [{ messageStart: { role: "user" } }] },
];

const out: string[] = [];
for (const testCase of cases) {
  const model: any = { ...baseModel, ...(testCase.model ?? {}) };
  const context = testCase.context ?? { messages: [user("hi")] };
  sdkState.items = testCase.items ?? [
    { messageStart: { role: "assistant" } },
    { messageStop: { stopReason: "end_turn" } },
  ];
  let payload: any;
  const events: unknown[] = [];
  const s = stream(model, context as any, {
    env: { AWS_REGION: "us-east-1", AWS_ACCESS_KEY_ID: "x", AWS_SECRET_ACCESS_KEY: "y" },
    onPayload: (captured: unknown) => { payload = captured; return undefined; },
    ...(testCase.options ?? {}),
  } as any);
  for await (const event of s) {
    const { partial, ...rest } = event as any;
    events.push(rest);
  }
  // `image.source.bytes` is a Uint8Array; JSON.stringify turns it into an index map, so
  // encode it back to base64 for the fixture.
  const normalized = JSON.parse(JSON.stringify(payload ?? null, (_key, value) => {
    if (value && typeof value === "object" && value instanceof Uint8Array) {
      return { __bytes__: Buffer.from(value).toString("base64") };
    }
    return value;
  }));
  out.push(JSON.stringify({ name: testCase.name, model, context, options: testCase.options ?? {}, payload: normalized, events }));
}
process.stdout.write(out.join("\n") + "\n");
