// Captures request bodies and request URLs of the TS azure-openai-responses adapter.
// The payload comes from the `onPayload` hook, the URL and headers from the injected
// `fetch` the AzureOpenAI client is constructed with.
import { stream } from "/Users/dev/projects/notagent-main/packages/ai/src/api/azure-openai-responses.ts";

function sseBody(): string {
  return 'data: {"type":"response.completed","response":{"id":"resp_1","status":"completed"}}\n\ndata: [DONE]\n\n';
}

const baseModel: any = {
  id: "gpt-4o-mini", name: "gpt-4o-mini", api: "azure-openai-responses", provider: "azure-openai-responses",
  baseUrl: "", reasoning: true, input: ["text", "image"],
  cost: { input: 1, output: 2, cacheRead: 0.1, cacheWrite: 0 },
  contextWindow: 128000, maxTokens: 4096,
};

const tool = {
  name: "read", description: "Reads a file",
  parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
};

const user = (text: string) => ({ role: "user", content: text, timestamp: 1 });
const assistantWithToolCall = {
  role: "assistant", timestamp: 2, api: "azure-openai-responses", provider: "azure-openai-responses", model: "gpt-4o-mini",
  usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
  stopReason: "toolUse",
  content: [{ type: "toolCall", id: "call_1|fc_item1", name: "read", arguments: { path: "a.txt" } }],
};

interface Case { name: string; model?: any; context?: any; options: any }

const cases: Case[] = [
  { name: "minimal", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com" } },
  { name: "system-prompt", context: { systemPrompt: "be nice", messages: [user("hi")] }, options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com" } },
  { name: "deployment-name-option", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", azureDeploymentName: "my-deployment" } },
  { name: "deployment-name-map", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", env: { AZURE_OPENAI_DEPLOYMENT_NAME_MAP: "other=nope, gpt-4o-mini=mapped-deployment" } } },
  { name: "deployment-name-map-miss", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", env: { AZURE_OPENAI_DEPLOYMENT_NAME_MAP: "other=nope" } } },
  { name: "prompt-cache-key", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", sessionId: "x".repeat(80) } },
  { name: "max-tokens", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", maxTokens: 4 } },
  { name: "temperature", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", temperature: 0.2 } },
  { name: "tools", context: { messages: [user("hi")], tools: [tool] }, options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com" } },
  { name: "tools-strict-off", model: { compat: { supportsStrictMode: false } }, context: { messages: [user("hi")], tools: [tool] }, options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com" } },
  { name: "tool-call-replay", context: { messages: [user("hi"), assistantWithToolCall, { role: "toolResult", timestamp: 3, toolCallId: "call_1|fc_item1", toolName: "read", content: [{ type: "text", text: "body" }], isError: false }] }, options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com" } },
  { name: "reasoning-effort", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", reasoningEffort: "high" } },
  { name: "reasoning-summary", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", reasoningSummary: "concise" } },
  { name: "reasoning-off", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com" } },
  { name: "reasoning-off-null", model: { thinkingLevelMap: { off: null } }, options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com" } },
  { name: "reasoning-disabled-model", model: { reasoning: false }, options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", reasoningEffort: "high" } },
  { name: "sampling-params", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", samplingParams: { top_p: 0.9, store: true } } },
  { name: "api-version", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", azureApiVersion: "2024-12-01" } },
  { name: "api-version-env", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", env: { AZURE_OPENAI_API_VERSION: "2025-01-01" } } },
  { name: "resource-name", options: { apiKey: "k", azureResourceName: "my-resource" } },
  { name: "resource-name-env", options: { apiKey: "k", env: { AZURE_OPENAI_RESOURCE_NAME: "env-resource" } } },
  { name: "base-url-env", options: { apiKey: "k", env: { AZURE_OPENAI_BASE_URL: "https://env-resource.openai.azure.com" } } },
  { name: "model-base-url", model: { baseUrl: "https://model-resource.openai.azure.com" }, options: { apiKey: "k" } },
  { name: "url-cognitive-root", options: { apiKey: "k", azureBaseUrl: "https://marc-quicktests-resource.cognitiveservices.azure.com" } },
  { name: "url-foundry-root", options: { apiKey: "k", azureBaseUrl: "https://marc-quicktests-resource.ai.azure.com" } },
  { name: "url-openai-path", options: { apiKey: "k", azureBaseUrl: "https://my-resource.cognitiveservices.azure.com/openai" } },
  { name: "url-openai-v1-path", options: { apiKey: "k", azureBaseUrl: "https://my-resource.cognitiveservices.azure.com/openai/v1" } },
  { name: "url-openai-v1-responses", options: { apiKey: "k", azureBaseUrl: "https://my-resource.services.ai.azure.com/openai/v1/responses" } },
  { name: "url-non-azure-proxy", options: { apiKey: "k", azureBaseUrl: "https://my-proxy.example.com/v1" } },
  { name: "url-azure-query-stripped", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com/openai?api-version=2024-12-01" } },
  { name: "url-non-azure-query-kept", options: { apiKey: "k", azureBaseUrl: "https://my-proxy.example.com/v1?custom=true" } },
  { name: "url-trailing-slashes", options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com///" } },
  { name: "headers", model: { headers: { "x-model": "from-model" } }, options: { apiKey: "k", azureBaseUrl: "https://my-resource.openai.azure.com", headers: { "x-request": "from-request" } } },
];

const out: string[] = [];
for (const testCase of cases) {
  const model: any = { ...baseModel, ...(testCase.model ?? {}) };
  const context = testCase.context ?? { messages: [user("hi")] };
  let payload: any;
  let requestUrl: string | undefined;
  let requestHeaders: Record<string, string> | undefined;
  const s = stream(model, context as any, {
    maxRetries: 0,
    fetch: (async (input: any, init: any) => {
      requestUrl = typeof input === "string" ? input : input.url;
      requestHeaders = {};
      new Headers(init?.headers).forEach((value, key) => { requestHeaders![key] = value; });
      return new Response(sseBody(), { status: 200, headers: { "content-type": "text/event-stream" } });
    }) as any,
    onPayload: (captured: unknown) => { payload = captured; return undefined; },
    ...testCase.options,
  } as any);
  const result = await s.result();
  out.push(JSON.stringify({
    name: testCase.name, model, context, options: testCase.options,
    payload, requestUrl, requestHeaders,
    errorMessage: result.errorMessage,
  }));
}
process.stdout.write(out.join("\n") + "\n");
