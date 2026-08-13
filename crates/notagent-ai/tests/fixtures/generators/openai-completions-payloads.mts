// Captures the request bodies the TS implementation builds for openai-completions.
// buildParams/convertMessages/convertTools are not exported, so the payloads are taken
// through the `onPayload` hook of `stream`.
import { stream } from "/Users/dev/projects/notagent-main/packages/ai/src/api/openai-completions.ts";

function sseResponse(): Response {
  const body = 'data: {"id":"1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n';
  return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
}

const baseModel: any = {
  id: "gpt-5", name: "gpt-5", api: "openai-completions", provider: "openai",
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
const preferStrictTool = { ...tool, name: "prefer_read", constrainedSampling: { type: "json_schema", strict: "prefer" } };
const grammarTool = {
  name: "grammar_tool", description: "Grammar constrained",
  parameters: { type: "object", properties: { input: { type: "string" } }, required: ["input"] },
  constrainedSampling: { type: "grammar", variants: { openai_lark: "start: /.+/" } },
};

const user = (text: string) => ({ role: "user", content: text, timestamp: 1 });
const assistantWithToolCall = {
  role: "assistant", timestamp: 2, api: "openai-completions", provider: "openai", model: "gpt-5",
  usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
  stopReason: "toolUse",
  content: [
    { type: "text", text: "Let me read that." },
    { type: "toolCall", id: "call_1", name: "read", arguments: { path: "a.txt" } },
  ],
};
// A message from another model: `transformMessages` only rewrites tool-call ids when the
// assistant turn did not come from the model being called.
const foreignAssistant = {
  ...assistantWithToolCall, api: "openai-responses", provider: "github-copilot", model: "gpt-4o",
};
const toolResult = {
  role: "toolResult", timestamp: 3, toolCallId: "call_1", toolName: "read",
  content: [{ type: "text", text: "file body" }], isError: false,
};

interface Case { name: string; model?: any; context: any; options?: any }

const cases: Case[] = [
  // --- Provider detection: one payload per branch of detectCompat. ---
  { name: "openai", context: { messages: [user("hi")] } },
  { name: "openrouter-anthropic", model: { provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1", id: "anthropic/claude-opus-4.5" }, context: { messages: [user("hi")] } },
  { name: "openrouter-other", model: { provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1", id: "meta/llama" }, context: { messages: [user("hi")] } },
  { name: "openrouter-openai", model: { provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1", id: "openai/gpt-5" }, context: { messages: [user("hi")] } },
  { name: "deepseek", model: { provider: "deepseek", baseUrl: "https://api.deepseek.com/v1", id: "deepseek-chat" }, context: { messages: [user("hi")] } },
  { name: "zai", model: { provider: "zai", baseUrl: "https://api.z.ai/v1", id: "glm-4" }, context: { messages: [user("hi")] } },
  { name: "together", model: { provider: "together", baseUrl: "https://api.together.ai/v1", id: "llama" }, context: { messages: [user("hi")] } },
  { name: "moonshot", model: { provider: "moonshotai", baseUrl: "https://api.moonshot.cn/v1", id: "kimi" }, context: { messages: [user("hi")] } },
  { name: "nvidia", model: { provider: "nvidia", baseUrl: "https://integrate.api.nvidia.com/v1", id: "nim" }, context: { messages: [user("hi")] } },
  { name: "ant-ling", model: { provider: "ant-ling", baseUrl: "https://api.ant-ling.com/v1", id: "ling" }, context: { messages: [user("hi")] } },
  { name: "cerebras", model: { provider: "cerebras", baseUrl: "https://api.cerebras.ai/v1", id: "llama" }, context: { messages: [user("hi")] } },
  { name: "xai", model: { provider: "xai", baseUrl: "https://api.x.ai/v1", id: "grok-4" }, context: { messages: [user("hi")] } },
  { name: "cloudflare-workers", model: { provider: "cloudflare-workers-ai", baseUrl: "https://api.cloudflare.com/client/v4", id: "llama" }, context: { messages: [user("hi")] } },
  { name: "cloudflare-gateway", model: { provider: "cloudflare-ai-gateway", baseUrl: "https://gateway.ai.cloudflare.com/v1", id: "llama" }, context: { messages: [user("hi")] } },
  { name: "opencode", model: { provider: "opencode", baseUrl: "https://opencode.ai/zen/v1", id: "zen" }, context: { messages: [user("hi")] } },
  { name: "chutes", model: { provider: "custom", baseUrl: "https://llm.chutes.ai/v1", id: "model" }, context: { messages: [user("hi")] } },
  { name: "unknown-custom", model: { provider: "custom", baseUrl: "https://my-llama.local/v1", id: "local" }, context: { messages: [user("hi")] } },

  // --- Reasoning effort across every thinkingFormat. ---
  { name: "openai-effort", context: { messages: [user("hi")] }, options: { reasoningEffort: "high" } },
  { name: "openai-effort-mapped", model: { thinkingLevelMap: { high: "detailed", off: "none" } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "high" } },
  { name: "openai-off-mapped", model: { thinkingLevelMap: { off: "none" } }, context: { messages: [user("hi")] } },
  { name: "openai-off-null", model: { thinkingLevelMap: { off: null } }, context: { messages: [user("hi")] } },
  { name: "openai-no-reasoning", model: { reasoning: false }, context: { messages: [user("hi")] }, options: { reasoningEffort: "high" } },
  { name: "zai-effort", model: { compat: { thinkingFormat: "zai", supportsReasoningEffort: true } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "medium" } },
  { name: "zai-effort-null", model: { thinkingLevelMap: { medium: null }, compat: { thinkingFormat: "zai", supportsReasoningEffort: true } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "medium" } },
  { name: "zai-off", model: { compat: { thinkingFormat: "zai" } }, context: { messages: [user("hi")] } },
  { name: "zai-tool-stream", model: { compat: { thinkingFormat: "zai", zaiToolStream: true } }, context: { messages: [user("hi")], tools: [tool] } },
  { name: "qwen-effort", model: { compat: { thinkingFormat: "qwen", supportsReasoningEffort: true } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "low" } },
  { name: "qwen-effort-null", model: { thinkingLevelMap: { low: null }, compat: { thinkingFormat: "qwen", supportsReasoningEffort: true } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "low" } },
  { name: "qwen-off", model: { compat: { thinkingFormat: "qwen" } }, context: { messages: [user("hi")] } },
  { name: "qwen-chat-template", model: { compat: { thinkingFormat: "qwen-chat-template" } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "high" } },
  { name: "qwen-chat-template-off", model: { compat: { thinkingFormat: "qwen-chat-template" } }, context: { messages: [user("hi")] } },
  {
    name: "chat-template-vars",
    model: { thinkingLevelMap: { high: "deep", off: "none" }, compat: { thinkingFormat: "chat-template", chatTemplateKwargs: { enable_thinking: { $var: "thinking.enabled" }, effort: { $var: "thinking.effort" }, only_on: { $var: "thinking.effort", omitWhenOff: true }, literal: "keep", num: 3, flag: false, nothing: null } } },
    context: { messages: [user("hi")] }, options: { reasoningEffort: "high" },
  },
  {
    name: "chat-template-vars-off",
    model: { thinkingLevelMap: { off: "none" }, compat: { thinkingFormat: "chat-template", chatTemplateKwargs: { enable_thinking: { $var: "thinking.enabled" }, effort: { $var: "thinking.effort" }, only_on: { $var: "thinking.effort", omitWhenOff: true } } } },
    context: { messages: [user("hi")] },
  },
  {
    name: "chat-template-vars-empty",
    model: { compat: { thinkingFormat: "chat-template", chatTemplateKwargs: { effort: { $var: "thinking.effort" } } } },
    context: { messages: [user("hi")] },
  },
  {
    name: "baseten-effort",
    model: { thinkingLevelMap: { high: "high", off: "none" }, compat: { thinkingFormat: "baseten", supportsReasoningEffort: true, chatTemplateArgs: { thinking: { $var: "thinking.enabled" } } } },
    context: { messages: [user("hi")] }, options: { reasoningEffort: "high" },
  },
  {
    name: "baseten-off",
    model: { thinkingLevelMap: { off: "none" }, compat: { thinkingFormat: "baseten", supportsReasoningEffort: true, chatTemplateArgs: { thinking: { $var: "thinking.enabled" } } } },
    context: { messages: [user("hi")] },
  },
  { name: "deepseek-effort", model: { compat: { thinkingFormat: "deepseek", supportsReasoningEffort: true } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "medium" } },
  { name: "deepseek-off", model: { compat: { thinkingFormat: "deepseek" } }, context: { messages: [user("hi")] } },
  { name: "deepseek-off-null", model: { thinkingLevelMap: { off: null }, compat: { thinkingFormat: "deepseek" } }, context: { messages: [user("hi")] } },
  { name: "openrouter-effort", model: { compat: { thinkingFormat: "openrouter" } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "xhigh" } },
  { name: "openrouter-off-null", model: { thinkingLevelMap: { off: null }, compat: { thinkingFormat: "openrouter" } }, context: { messages: [user("hi")] } },
  { name: "ant-ling-effort", model: { thinkingLevelMap: { high: "high" }, compat: { thinkingFormat: "ant-ling" } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "high" } },
  { name: "ant-ling-unmapped", model: { compat: { thinkingFormat: "ant-ling" } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "high" } },
  { name: "together-effort", model: { compat: { thinkingFormat: "together", supportsReasoningEffort: true } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "low" } },
  { name: "together-off", model: { compat: { thinkingFormat: "together" } }, context: { messages: [user("hi")] } },
  { name: "string-thinking", model: { compat: { thinkingFormat: "string-thinking" } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "high" } },
  { name: "string-thinking-off", model: { compat: { thinkingFormat: "string-thinking" } }, context: { messages: [user("hi")] } },
  { name: "string-thinking-off-null", model: { thinkingLevelMap: { off: null }, compat: { thinkingFormat: "string-thinking" } }, context: { messages: [user("hi")] } },

  // --- thinking_token_budget (vLLM). ---
  { name: "thinking-token-budget", model: { compat: { supportsThinkingTokenBudget: true } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "medium", maxTokens: 20000 } },
  { name: "thinking-token-budget-clamped", model: { compat: { supportsThinkingTokenBudget: true } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "high", maxTokens: 2000 } },
  { name: "thinking-token-budget-zero", model: { compat: { supportsThinkingTokenBudget: true } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "high", maxTokens: 1024 } },
  { name: "thinking-token-budget-custom", model: { compat: { supportsThinkingTokenBudget: true } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "low", maxTokens: 20000, thinkingBudgets: { low: 555 } } },
  { name: "thinking-token-budget-xhigh", model: { compat: { supportsThinkingTokenBudget: true } }, context: { messages: [user("hi")] }, options: { reasoningEffort: "xhigh", maxTokens: 30000 } },

  // --- Messages. ---
  { name: "system-prompt", context: { systemPrompt: "be nice", messages: [user("hi")] } },
  { name: "system-prompt-no-developer", model: { reasoning: false }, context: { systemPrompt: "be nice", messages: [user("hi")] } },
  { name: "image-parts", context: { messages: [{ role: "user", content: [{ type: "text", text: "look" }, { type: "image", data: "AAA", mimeType: "image/png" }], timestamp: 1 }] } },
  { name: "empty-user-blocks", context: { messages: [{ role: "user", content: [], timestamp: 1 }, user("hi")] } },
  { name: "assistant-tool-call", context: { messages: [user("hi"), assistantWithToolCall, toolResult, user("thanks")] } },
  { name: "assistant-tool-call-tools", context: { messages: [user("hi"), assistantWithToolCall, toolResult], tools: [tool] } },
  { name: "tool-history-without-tools", context: { messages: [user("hi"), assistantWithToolCall, toolResult] } },
  {
    name: "assistant-thinking",
    context: { messages: [user("hi"), { ...assistantWithToolCall, content: [{ type: "thinking", thinking: "hmm", thinkingSignature: "reasoning_content" }, { type: "text", text: "answer" }] }] },
  },
  {
    name: "assistant-thinking-as-text",
    model: { compat: { requiresThinkingAsText: true } },
    context: { messages: [user("hi"), { ...assistantWithToolCall, content: [{ type: "thinking", thinking: "hmm" }, { type: "text", text: "answer" }] }] },
  },
  {
    name: "assistant-thinking-opencode-go",
    model: { provider: "opencode-go", baseUrl: "https://opencode.ai/zen/v1" },
    context: { messages: [user("hi"), { ...assistantWithToolCall, content: [{ type: "thinking", thinking: "hmm", thinkingSignature: "reasoning" }, { type: "text", text: "answer" }] }] },
  },
  {
    name: "assistant-empty-skipped",
    context: { messages: [user("hi"), { ...assistantWithToolCall, content: [] }, user("again")] },
  },
  {
    name: "assistant-reasoning-details",
    context: { messages: [user("hi"), { ...assistantWithToolCall, content: [{ type: "toolCall", id: "call_1", name: "read", arguments: {}, thoughtSignature: '{"type":"reasoning.encrypted","id":"call_1","data":"enc"}' }] }] },
  },
  {
    name: "deepseek-reasoning-content",
    model: { provider: "deepseek", baseUrl: "https://api.deepseek.com/v1", id: "deepseek-chat" },
    context: { messages: [user("hi"), { ...assistantWithToolCall, content: [{ type: "text", text: "answer" }] }] },
  },
  {
    name: "requires-assistant-after-tool-result",
    model: { compat: { requiresAssistantAfterToolResult: true } },
    context: { messages: [user("hi"), assistantWithToolCall, toolResult, user("thanks")] },
  },
  {
    name: "tool-result-name",
    model: { compat: { requiresToolResultName: true } },
    context: { messages: [user("hi"), assistantWithToolCall, toolResult] },
  },
  {
    name: "tool-result-images",
    context: { messages: [user("hi"), assistantWithToolCall, { ...toolResult, content: [{ type: "text", text: "shot" }, { type: "image", data: "BBB", mimeType: "image/png" }] }] },
  },
  {
    name: "tool-result-images-text-only-model",
    model: { input: ["text"] },
    context: { messages: [user("hi"), assistantWithToolCall, { ...toolResult, content: [{ type: "image", data: "BBB", mimeType: "image/png" }] }] },
  },
  {
    name: "tool-result-empty",
    context: { messages: [user("hi"), assistantWithToolCall, { ...toolResult, content: [] }] },
  },
  {
    name: "kimi-deferred-tools",
    model: { compat: { deferredToolsMode: "kimi" } },
    context: {
      messages: [user("hi"), assistantWithToolCall, { ...toolResult, addedToolNames: ["write"] }],
      tools: [tool, otherTool],
    },
  },
  {
    name: "normalize-pipe-tool-call-id",
    context: {
      messages: [
        user("hi"),
        { ...foreignAssistant, content: [{ type: "toolCall", id: "call_abc|item_def", name: "read", arguments: {} }] },
        { ...toolResult, toolCallId: "call_abc|item_def" },
      ],
    },
  },
  {
    name: "normalize-long-pipe-tool-call-id",
    context: {
      messages: [
        user("hi"),
        { ...foreignAssistant, content: [{ type: "toolCall", id: `call_abc|${"x".repeat(80)}`, name: "read", arguments: {} }] },
        { ...toolResult, toolCallId: `call_abc|${"x".repeat(80)}` },
      ],
    },
  },
  {
    name: "same-model-tool-call-id-untouched",
    context: {
      messages: [
        user("hi"),
        { ...assistantWithToolCall, content: [{ type: "toolCall", id: "call_abc|item_def", name: "read", arguments: {} }] },
        { ...toolResult, toolCallId: "call_abc|item_def" },
      ],
    },
  },
  {
    name: "normalize-long-openai-tool-call-id",
    context: {
      messages: [
        user("hi"),
        { ...foreignAssistant, content: [{ type: "toolCall", id: `call_${"y".repeat(60)}`, name: "read", arguments: {} }] },
        { ...toolResult, toolCallId: `call_${"y".repeat(60)}` },
      ],
    },
  },

  // --- Tools. ---
  { name: "tools", context: { messages: [user("hi")], tools: [tool] } },
  { name: "tools-strict", context: { messages: [user("hi")], tools: [strictTool] } },
  { name: "tools-no-strict-mode", model: { provider: "moonshotai", baseUrl: "https://api.moonshot.cn/v1", id: "kimi" }, context: { messages: [user("hi")], tools: [preferStrictTool] } },
  { name: "tools-grammar", model: { compat: { supportsOpenAIGrammarTools: true } }, context: { messages: [user("hi")], tools: [grammarTool] } },
  {
    name: "tools-grammar-assistant-call",
    model: { compat: { supportsOpenAIGrammarTools: true } },
    context: {
      messages: [user("hi"), { ...assistantWithToolCall, content: [{ type: "toolCall", id: "call_2", name: "grammar_tool", arguments: { input: "abc" } }] }],
      tools: [grammarTool],
    },
  },
  { name: "tool-choice", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: "required" } },
  { name: "tool-choice-object", context: { messages: [user("hi")], tools: [tool] }, options: { toolChoice: { type: "function", function: { name: "read" } } } },

  // --- cache_control (Anthropic format via OpenRouter). ---
  { name: "cache-control", model: { provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1", id: "anthropic/claude-opus-4.5" }, context: { systemPrompt: "sys", messages: [user("hi"), assistantWithToolCall, toolResult], tools: [tool] } },
  { name: "cache-control-long", model: { provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1", id: "anthropic/claude-opus-4.5" }, context: { systemPrompt: "sys", messages: [user("hi")], tools: [tool] }, options: { cacheRetention: "long" } },
  { name: "cache-control-none", model: { provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1", id: "anthropic/claude-opus-4.5" }, context: { systemPrompt: "sys", messages: [user("hi")] }, options: { cacheRetention: "none" } },
  { name: "cache-control-image-parts", model: { provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1", id: "anthropic/claude-opus-4.5" }, context: { messages: [{ role: "user", content: [{ type: "text", text: "look" }, { type: "image", data: "AAA", mimeType: "image/png" }], timestamp: 1 }] } },

  // --- prompt cache keys. ---
  { name: "prompt-cache-key", context: { messages: [user("hi")] }, options: { sessionId: "session-123" } },
  { name: "prompt-cache-key-clamped", context: { messages: [user("hi")] }, options: { sessionId: "s".repeat(80) } },
  { name: "prompt-cache-key-none", context: { messages: [user("hi")] }, options: { sessionId: "session-123", cacheRetention: "none" } },
  { name: "prompt-cache-retention-long", model: { compat: { supportsLongCacheRetention: true } }, context: { messages: [user("hi")] }, options: { sessionId: "session-123", cacheRetention: "long" } },
  { name: "prompt-cache-long-unsupported", model: { provider: "together", baseUrl: "https://api.together.ai/v1", id: "llama" }, context: { messages: [user("hi")] }, options: { sessionId: "session-123", cacheRetention: "long" } },

  // --- Routing and sampling. ---
  { name: "openrouter-routing", model: { compat: { openRouterRouting: { allow_fallbacks: false, order: ["anthropic"] } } }, context: { messages: [user("hi")] } },
  { name: "openrouter-routing-empty", model: { compat: { openRouterRouting: {} } }, context: { messages: [user("hi")] } },
  { name: "vercel-gateway-routing", model: { compat: { vercelGatewayRouting: { only: ["bedrock"], order: ["bedrock", "anthropic"] } } }, context: { messages: [user("hi")] } },
  { name: "vercel-gateway-routing-partial", model: { compat: { vercelGatewayRouting: { order: ["bedrock"] } } }, context: { messages: [user("hi")] } },
  { name: "vercel-gateway-routing-empty", model: { compat: { vercelGatewayRouting: {} } }, context: { messages: [user("hi")] } },
  { name: "sampling-params", context: { messages: [user("hi")] }, options: { samplingParams: { top_p: 0.5, model: "override" } } },
  { name: "temperature", context: { messages: [user("hi")] }, options: { temperature: 0.25 } },
  { name: "max-tokens", context: { messages: [user("hi")] }, options: { maxTokens: 512 } },
  { name: "max-tokens-field", model: { provider: "deepseek", baseUrl: "https://api.deepseek.com/v1", id: "deepseek-chat" }, context: { messages: [user("hi")] }, options: { maxTokens: 512 } },
  { name: "no-usage-in-streaming", model: { compat: { supportsUsageInStreaming: false } }, context: { messages: [user("hi")] } },
  // No lone-surrogate case: a Rust `String` is UTF-8 and cannot carry one, so
  // `sanitizeSurrogates` has nothing to repair on this side (see utils/sanitize_unicode.rs).
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
