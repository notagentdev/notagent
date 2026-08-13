// Captures the request URL, headers and body the TS pi-messages adapter sends plus
// the event sequence it emits for scripted SSE bodies. The adapter takes a `fetch`
// option, so a fake fetch records everything without a network call.
import { stream } from "/Users/dev/projects/notagent-main/packages/ai/src/api/pi-messages.ts";

const baseModel: any = {
  id: "pi-model", name: "pi-model", api: "pi-messages", provider: "radius",
  baseUrl: "https://gateway.example.com/v1", reasoning: true, input: ["text", "image"],
  cost: { input: 1, output: 2, cacheRead: 0.1, cacheWrite: 0 },
  contextWindow: 128000, maxTokens: 4096,
};

const user = (text: string) => ({ role: "user", content: text, timestamp: 1 });
const usage = { input: 10, output: 5, cacheRead: 0, cacheWrite: 0, totalTokens: 15, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } };

function sse(events: unknown[], separator = "\n\n"): string {
  return events.map((event) => `data: ${JSON.stringify(event)}${separator}`).join("");
}

interface Case { name: string; model?: any; context?: any; options?: any; body?: string; status?: number; statusText?: string }

const done = { type: "done", reason: "stop", usage };

const cases: Case[] = [
  // --- Request bodies. ---
  { name: "minimal" },
  { name: "system-prompt", context: { systemPrompt: "be nice", messages: [user("hi")] } },
  { name: "all-options", options: { temperature: 0.5, maxTokens: 256, reasoning: "high", cacheRetention: "long", sessionId: "s1", toolChoice: "required" } },
  { name: "tool-choice-function", options: { toolChoice: { type: "function", function: { name: "read" } } } },
  { name: "debug", options: { debug: true } },
  { name: "header-overrides", options: { headers: { "x-request": "1", accept: null } } },
  { name: "base-url-trailing-slash", model: { baseUrl: "https://gateway.example.com/v1///" } },
  { name: "cache-retention-env", options: { env: { PI_CACHE_RETENTION: "long" } } },
  { name: "cache-retention-env-other", options: { env: { PI_CACHE_RETENTION: "short" } } },

  // --- Event sequences. ---
  {
    name: "text",
    body: sse([
      { type: "start" },
      { type: "text_start", contentIndex: 0 },
      { type: "text_delta", contentIndex: 0, delta: "Hel" },
      { type: "text_delta", contentIndex: 0, delta: "lo" },
      { type: "text_end", contentIndex: 0, content: "Hello", contentSignature: "sig" },
      done,
    ]),
  },
  {
    name: "thinking",
    body: sse([
      { type: "start" },
      { type: "thinking_start", contentIndex: 0 },
      { type: "thinking_delta", contentIndex: 0, delta: "hm" },
      { type: "thinking_end", contentIndex: 0, content: "hmm", contentSignature: "tsig", redacted: true },
      done,
    ]),
  },
  {
    name: "tool-call",
    body: sse([
      { type: "start" },
      { type: "toolcall_start", contentIndex: 0, id: "call_1", toolName: "read" },
      { type: "toolcall_delta", contentIndex: 0, delta: '{"path":' },
      { type: "toolcall_delta", contentIndex: 0, delta: '"a.txt"}' },
      { type: "toolcall_end", contentIndex: 0, toolCall: { type: "toolCall", id: "call_1", name: "read", arguments: { path: "a.txt" } } },
      { type: "done", reason: "toolUse", usage },
    ]),
  },
  {
    name: "done-with-response-id-and-rewrite",
    body: sse([
      { type: "start" },
      { type: "done", reason: "stop", usage, responseId: "resp_1", rewrite: { policyId: "p1", policyVersion: 2, changed: true, tokenCountChange: -5, messageCountChange: 0, systemPromptChanged: false } },
    ]),
  },
  {
    name: "error-event",
    body: sse([
      { type: "start" },
      { type: "error", reason: "error", usage, errorMessage: "backend exploded", responseId: "resp_2" },
    ]),
  },
  { name: "crlf-separators", body: sse([{ type: "start" }, done], "\r\n\r\n") },
  { name: "done-marker-is-skipped", body: "data: [DONE]\n\n" + sse([{ type: "start" }, done]) },
  { name: "trailing-event-without-boundary", body: sse([{ type: "start" }]) + `data: ${JSON.stringify(done)}` },
  { name: "stream-without-terminal-event", body: sse([{ type: "start" }]) },
  { name: "http-error-json", status: 400, statusText: "Bad Request", body: JSON.stringify({ error: { message: "bad model", code: "invalid_model" } }) },
  { name: "http-error-plain", status: 502, statusText: "Bad Gateway", body: "upstream down" },
];

const out: string[] = [];
for (const testCase of cases) {
  const model: any = { ...baseModel, ...(testCase.model ?? {}) };
  const context = testCase.context ?? { messages: [user("hi")] };
  const status = testCase.status ?? 200;
  const body = testCase.body ?? sse([{ type: "start" }, done]);
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
        statusText: testCase.statusText ?? "",
        headers: { "content-type": status === 200 ? "text/event-stream" : "application/json" },
      });
    }) as any,
    ...(testCase.options ?? {}),
  } as any);

  for await (const event of s) {
    const { partial, ...rest } = event as any;
    events.push(rest);
  }
  out.push(JSON.stringify({ name: testCase.name, model, context, options: testCase.options ?? {}, status, statusText: testCase.statusText ?? "", body, request, events }));
}
process.stdout.write(out.join("\n") + "\n");
