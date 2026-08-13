// Captures the request bodies the TS implementation builds for openai-responses,
// through the `onPayload` hook of `stream`.
import { stream } from "/Users/dev/projects/notagent-main/packages/ai/src/api/openai-responses.ts";

function sseResponse(): Response {
  const body =
    'data: {"type":"response.completed","response":{"id":"resp_1","status":"completed"}}\n\ndata: [DONE]\n\n';
  return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
}

const baseModel: any = {
  id: "gpt-5", name: "gpt-5", api: "openai-responses", provider: "openai",
  baseUrl: "https://api.openai.com/v1", reasoning: true, input: ["text", "image"],
  cost: { input: 1, output: 2, cacheRead: 0.1, cacheWrite: 0 },
  contextWindow: 128000, maxTokens: 4096,
};

const tool = {
  name: "read", description: "Reads a file",
  parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
};
const otherTool = { name: "write", description: "Writes a file", parameters: { type: "object", properties: {} } };
const strictTool = { ...tool, name: "strict_read", constrainedSampling: { type: "json_schema", strict: "require" } };
const grammarTool = {
  name: "grammar_tool", description: "Grammar constrained",
  parameters: { type: "object", properties: { input: { type: "string" } }, required: ["input"] },
  constrainedSampling: { type: "grammar", variants: { openai_lark: "start: /.+/" } },
};

const user = (text: string) => ({ role: "user", content: text, timestamp: 1 });
const assistantBase = {
  role: "assistant", timestamp: 2, api: "openai-responses", provider: "openai", model: "gpt-5",
  usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
  stopReason: "toolUse", content: [],
};
const assistantWithToolCall = {
  ...assistantBase,
  content: [
    { type: "text", text: "Reading.", textSignature: JSON.stringify({ v: 1, id: "msg_abc" }) },
    { type: "toolCall", id: "call_1|fc_item1", name: "read", arguments: { path: "a.txt" } },
  ],
};
const toolResult = {
  role: "toolResult", timestamp: 3, toolCallId: "call_1|fc_item1", toolName: "read",
  content: [{ type: "text", text: "file body" }], isError: false,
};
const reasoningItem = {
  type: "reasoning", id: "rs_1", summary: [{ type: "summary_text", text: "thinking" }],
  encrypted_content: "enc",
};

interface Case { name: string; model?: any; context: any; options?: any }

const cases: Case[] = [
  { name: "minimal", context: { messages: [user("hi")] } },
  { name: "system-prompt", context: { systemPrompt: "be nice", messages: [user("hi")] } },
  { name: "system-prompt-no-developer", model: { compat: { supportsDeveloperRole: false } }, context: { systemPrompt: "be nice", messages: [user("hi")] } },
  { name: "system-prompt-no-reasoning", model: { reasoning: false }, context: { systemPrompt: "be nice", messages: [user("hi")] } },
  { name: "image-parts", context: { messages: [{ role: "user", content: [{ type: "text", text: "look" }, { type: "image", data: "AAA", mimeType: "image/png" }], timestamp: 1 }] } },
  { name: "empty-user-blocks", context: { messages: [{ role: "user", content: [], timestamp: 1 }, user("hi")] } },

  // Assistant replay.
  { name: "assistant-tool-call", context: { messages: [user("hi"), assistantWithToolCall, toolResult] } },
  { name: "assistant-text-signature-legacy", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "text", text: "answer", textSignature: "msg_legacy" }] }] } },
  { name: "assistant-text-signature-phase", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "text", text: "answer", textSignature: JSON.stringify({ v: 1, id: "msg_x", phase: "final_answer" }) }] }] } },
  { name: "assistant-text-signature-missing", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "text", text: "one" }, { type: "text", text: "two" }] }] } },
  { name: "assistant-text-signature-too-long", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "text", text: "answer", textSignature: JSON.stringify({ v: 1, id: `msg_${"z".repeat(80)}` }) }] }] } },
  { name: "assistant-reasoning-replay", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "thinking", thinking: "thinking", thinkingSignature: JSON.stringify(reasoningItem) }, { type: "text", text: "answer", textSignature: JSON.stringify({ v: 1, id: "msg_1" }) }] }] } },
  { name: "assistant-different-model", model: { id: "gpt-5.1" }, context: { messages: [user("hi"), assistantWithToolCall, { ...toolResult }] } },
  { name: "assistant-foreign-provider", context: { messages: [user("hi"), { ...assistantWithToolCall, provider: "github-copilot", api: "openai-completions" }, toolResult] } },
  { name: "assistant-namespace", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_2|fc_item2", name: "read", arguments: {}, namespace: "files" }] }] } },
  { name: "assistant-non-fc-item-id", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_3|ctc_item3", name: "read", arguments: {} }] }] } },
  { name: "assistant-tool-call-without-item-id", context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_4", name: "read", arguments: {} }] }] } },
  { name: "assistant-empty-skipped", context: { messages: [user("hi"), { ...assistantBase, content: [] }, user("again")] } },

  // Tool results.
  { name: "tool-result-images", context: { messages: [user("hi"), assistantWithToolCall, { ...toolResult, content: [{ type: "text", text: "shot" }, { type: "image", data: "BBB", mimeType: "image/png" }] }] } },
  { name: "tool-result-images-text-only", model: { input: ["text"] }, context: { messages: [user("hi"), assistantWithToolCall, { ...toolResult, content: [{ type: "image", data: "BBB", mimeType: "image/png" }] }] } },
  { name: "tool-result-empty", context: { messages: [user("hi"), assistantWithToolCall, { ...toolResult, content: [] }] } },

  // Tools.
  { name: "tools", context: { messages: [user("hi")], tools: [tool] } },
  { name: "tools-strict-mode", model: { compat: { supportsStrictMode: true } }, context: { messages: [user("hi")], tools: [tool] } },
  { name: "tools-strict-constrained", model: { compat: { supportsStrictMode: true } }, context: { messages: [user("hi")], tools: [strictTool] } },
  { name: "tools-grammar", model: { compat: { supportsOpenAIGrammarTools: true } }, context: { messages: [user("hi")], tools: [grammarTool] } },
  { name: "tools-grammar-replay", model: { compat: { supportsOpenAIGrammarTools: true } }, context: { messages: [user("hi"), { ...assistantBase, content: [{ type: "toolCall", id: "call_5|fc_item5", name: "grammar_tool", arguments: { input: "abc" } }] }, { role: "toolResult", timestamp: 3, toolCallId: "call_5|fc_item5", toolName: "grammar_tool", content: [{ type: "text", text: "ok" }], isError: false }], tools: [grammarTool] } },
  { name: "tool-choice", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: "required" } },

  // Deferred tools.
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
  {
    name: "deferred-tool-search-twice",
    model: { compat: { supportsToolSearch: true } },
    context: {
      messages: [
        user("hi"), assistantWithToolCall, { ...toolResult, addedToolNames: ["write"] },
        { ...assistantBase, content: [{ type: "toolCall", id: "call_6|fc_item6", name: "write", arguments: {} }] },
        { role: "toolResult", timestamp: 5, toolCallId: "call_6|fc_item6", toolName: "write", content: [{ type: "text", text: "ok" }], isError: false, addedToolNames: ["write"] },
      ],
      tools: [tool, otherTool],
    },
  },

  // Reasoning.
  { name: "reasoning-effort", context: { messages: [user("hi")] }, options: { reasoningEffort: "high" } },
  { name: "reasoning-effort-mapped", model: { thinkingLevelMap: { high: "detailed" } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "high" } },
  { name: "reasoning-summary-only", context: { messages: [user("hi")] }, options: { reasoningSummary: "detailed" } },
  { name: "reasoning-summary-null", context: { messages: [user("hi")] }, options: { reasoningEffort: "low", reasoningSummary: null } },
  { name: "reasoning-off", context: { messages: [user("hi")] } },
  { name: "reasoning-off-mapped", model: { thinkingLevelMap: { off: "minimal" } }, context: { messages: [user("hi")] } },
  { name: "reasoning-off-null", model: { thinkingLevelMap: { off: null } }, context: { messages: [user("hi")] } },
  { name: "reasoning-copilot", model: { provider: "github-copilot", baseUrl: "https://api.githubcopilot.com" }, context: { messages: [user("hi")] } },
  { name: "reasoning-xai", model: { provider: "xai", baseUrl: "https://api.x.ai/v1" }, context: { messages: [user("hi")] } },
  { name: "reasoning-disabled-model", model: { reasoning: false }, context: { messages: [user("hi")] }, options: { reasoningEffort: "high" } },

  // Cache and misc.
  { name: "prompt-cache-key", context: { messages: [user("hi")] }, options: { sessionId: "session-1" } },
  { name: "prompt-cache-key-clamped", context: { messages: [user("hi")] }, options: { sessionId: "s".repeat(80) } },
  { name: "prompt-cache-none", context: { messages: [user("hi")] }, options: { sessionId: "session-1", cacheRetention: "none" } },
  { name: "prompt-cache-explicit-mode", model: { compat: { supportsExplicitPromptCacheMode: true } }, context: { messages: [user("hi")] }, options: { sessionId: "session-1", cacheRetention: "none" } },
  { name: "prompt-cache-long", context: { messages: [user("hi")] }, options: { sessionId: "session-1", cacheRetention: "long" } },
  { name: "prompt-cache-long-unsupported", model: { compat: { supportsLongCacheRetention: false } }, context: { messages: [user("hi")] }, options: { sessionId: "session-1", cacheRetention: "long" } },
  { name: "max-tokens", context: { messages: [user("hi")] }, options: { maxTokens: 512 } },
  { name: "max-tokens-below-minimum", context: { messages: [user("hi")] }, options: { maxTokens: 4 } },
  { name: "temperature", context: { messages: [user("hi")] }, options: { temperature: 0.3 } },
  { name: "service-tier", context: { messages: [user("hi")] }, options: { serviceTier: "flex" } },
  { name: "sampling-params", context: { messages: [user("hi")] }, options: { samplingParams: { top_p: 0.4, store: true } } },
];

const out: string[] = [];
for (const testCase of cases) {
  const model: any = { ...baseModel, ...(testCase.model ?? {}) };
  let payload: any;
  const s = stream(model, testCase.context as any, {
    apiKey: "k",
    fetch: (async () => sseResponse()) as any,
    onPayload: (captured: unknown) => { payload = captured; return undefined; },
    ...(testCase.options ?? {}),
  } as any);
  await s.result();
  if (payload === undefined) throw new Error(`no payload for ${testCase.name}`);
  out.push(JSON.stringify({ name: testCase.name, model, context: testCase.context, options: testCase.options ?? {}, payload }));
}
process.stdout.write(out.join("\n") + "\n");
