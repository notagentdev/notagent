// Captures the resolved compat matrix of the TS implementation for every catalog model
// that uses openai-completions, plus synthetic provider/baseUrl combinations.
import { readFileSync, readdirSync } from "node:fs";

// detectCompat/getCompat are not exported, so drive them through a payload capture.
import { stream } from "/Users/dev/projects/notagent-main/packages/ai/src/api/openai-completions.ts";

function sseResponse(): Response {
  const body = 'data: {"id":"1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n';
  return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
}

const cases: Array<{ name: string; provider: string; baseUrl: string; id: string }> = [
  { name: "openai", provider: "openai", baseUrl: "https://api.openai.com/v1", id: "gpt-5" },
  { name: "openrouter-anthropic", provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1", id: "anthropic/claude-opus-4.5" },
  { name: "openrouter-other", provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1", id: "meta/llama" },
  { name: "openrouter-openai", provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1", id: "openai/gpt-5" },
  { name: "deepseek", provider: "deepseek", baseUrl: "https://api.deepseek.com/v1", id: "deepseek-chat" },
  { name: "zai", provider: "zai", baseUrl: "https://api.z.ai/v1", id: "glm-4" },
  { name: "together", provider: "together", baseUrl: "https://api.together.ai/v1", id: "llama" },
  { name: "moonshot", provider: "moonshotai", baseUrl: "https://api.moonshot.cn/v1", id: "kimi" },
  { name: "nvidia", provider: "nvidia", baseUrl: "https://integrate.api.nvidia.com/v1", id: "nim" },
  { name: "ant-ling", provider: "ant-ling", baseUrl: "https://api.ant-ling.com/v1", id: "ling" },
  { name: "cerebras", provider: "cerebras", baseUrl: "https://api.cerebras.ai/v1", id: "llama" },
  { name: "xai", provider: "xai", baseUrl: "https://api.x.ai/v1", id: "grok-4" },
  { name: "cloudflare-workers", provider: "cloudflare-workers-ai", baseUrl: "https://api.cloudflare.com/client/v4", id: "llama" },
  { name: "cloudflare-gateway", provider: "cloudflare-ai-gateway", baseUrl: "https://gateway.ai.cloudflare.com/v1", id: "llama" },
  { name: "opencode", provider: "opencode", baseUrl: "https://opencode.ai/zen/v1", id: "zen" },
  { name: "chutes", provider: "custom", baseUrl: "https://llm.chutes.ai/v1", id: "model" },
  { name: "unknown-custom", provider: "custom", baseUrl: "https://my-llama.local/v1", id: "local" },
];

const out: string[] = [];
for (const testCase of cases) {
  const model: any = {
    id: testCase.id, name: testCase.id, api: "openai-completions", provider: testCase.provider,
    baseUrl: testCase.baseUrl, reasoning: true, input: ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 128000, maxTokens: 4096,
  };
  let payload: any;
  const s = stream(model, { messages: [{ role: "user", content: "hi", timestamp: 1 }] } as any, {
    apiKey: "k",
    reasoning: "medium",
    fetch: (async () => sseResponse()) as any,
    onPayload: (captured: unknown) => { payload = captured; return undefined; },
  } as any);
  await s.result();
  out.push(JSON.stringify({ name: testCase.name, provider: testCase.provider, baseUrl: testCase.baseUrl, id: testCase.id, payload }));
}
process.stdout.write(out.join("\n") + "\n");
