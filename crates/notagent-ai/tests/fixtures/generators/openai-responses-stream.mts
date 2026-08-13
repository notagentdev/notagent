// Captures the event sequence the TS openai-responses stream emits for scripted SSE
// bodies, through a fake `fetch`.
import { stream } from "/Users/dev/projects/notagent-main/packages/ai/src/api/openai-responses.ts";

const baseModel: any = {
  id: "gpt-5", name: "gpt-5", api: "openai-responses", provider: "openai",
  baseUrl: "https://api.openai.com/v1", reasoning: true, input: ["text", "image"],
  cost: { input: 1, output: 2, cacheRead: 0.1, cacheWrite: 0 },
  contextWindow: 128000, maxTokens: 4096,
};

const grammarTool = {
  name: "grammar_tool", description: "Grammar constrained",
  parameters: { type: "object", properties: { input: { type: "string" } }, required: ["input"] },
  constrainedSampling: { type: "grammar", variants: { openai_lark: "start: /.+/" } },
};

function sse(events: unknown[]): string {
  return events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join("") + "data: [DONE]\n\n";
}

const completed = (extra: Record<string, unknown> = {}) => ({
  type: "response.completed",
  response: { id: "resp_1", status: "completed", ...extra },
});

interface Case { name: string; model?: any; context?: any; options?: any; body: string; status?: number }

const cases: Case[] = [
  {
    name: "text",
    body: sse([
      { type: "response.created", response: { id: "resp_created" } },
      { type: "response.output_item.added", output_index: 0, item: { type: "message", id: "msg_1", role: "assistant", content: [], status: "in_progress" } },
      { type: "response.output_text.delta", output_index: 0, delta: "Hel" },
      { type: "response.output_text.delta", output_index: 0, delta: "lo" },
      { type: "response.output_item.done", output_index: 0, item: { type: "message", id: "msg_1", role: "assistant", status: "completed", content: [{ type: "output_text", text: "Hello", annotations: [] }] } },
      completed(),
    ]),
  },
  {
    name: "refusal",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "message", id: "msg_1", role: "assistant", content: [], status: "in_progress" } },
      { type: "response.refusal.delta", output_index: 0, delta: "I cannot" },
      { type: "response.output_item.done", output_index: 0, item: { type: "message", id: "msg_1", role: "assistant", status: "completed", content: [{ type: "refusal", refusal: "I cannot" }] } },
      completed(),
    ]),
  },
  {
    name: "message-phase-final-answer",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "message", id: "msg_1", role: "assistant", phase: "final_answer", content: [], status: "in_progress" } },
      { type: "response.output_text.delta", output_index: 0, delta: "done" },
      { type: "response.output_item.done", output_index: 0, item: { type: "message", id: "msg_1", role: "assistant", phase: "final_answer", status: "completed", content: [{ type: "output_text", text: "done", annotations: [] }] } },
      completed({ status: "incomplete", incomplete_details: { reason: "max_output_tokens" } }),
    ]),
  },
  {
    name: "reasoning-summary",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "reasoning", id: "rs_1", summary: [] } },
      { type: "response.reasoning_summary_text.delta", output_index: 0, delta: "step one" },
      { type: "response.reasoning_summary_part.done", output_index: 0 },
      { type: "response.reasoning_summary_text.delta", output_index: 0, delta: "step two" },
      { type: "response.output_item.done", output_index: 0, item: { type: "reasoning", id: "rs_1", summary: [{ type: "summary_text", text: "step one" }, { type: "summary_text", text: "step two" }], encrypted_content: "enc" } },
      completed(),
    ]),
  },
  {
    name: "reasoning-text",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "reasoning", id: "rs_1", summary: [] } },
      { type: "response.reasoning_text.delta", output_index: 0, delta: "raw thought" },
      { type: "response.output_item.done", output_index: 0, item: { type: "reasoning", id: "rs_1", summary: [], content: [{ type: "reasoning_text", text: "raw thought" }] } },
      completed(),
    ]),
  },
  {
    name: "reasoning-signature-backfill",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "reasoning", id: "rs_1", summary: [] } },
      { type: "response.reasoning_summary_text.delta", output_index: 0, delta: "thought" },
      { type: "response.output_item.done", output_index: 0, item: { type: "reasoning", id: "rs_1", summary: [{ type: "summary_text", text: "thought" }] } },
      completed({ output: [{ type: "reasoning", id: "rs_1", summary: [], encrypted_content: "late-enc" }] }),
    ]),
  },
  {
    name: "reasoning-signature-backfill-skipped",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "reasoning", id: "rs_1", summary: [] } },
      { type: "response.output_item.done", output_index: 0, item: { type: "reasoning", id: "rs_1", summary: [], encrypted_content: "original" } },
      completed({ output: [{ type: "reasoning", id: "rs_1", summary: [], encrypted_content: "late-enc" }] }),
    ]),
  },
  {
    name: "function-call",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: "" } },
      { type: "response.function_call_arguments.delta", output_index: 0, delta: '{"path":' },
      { type: "response.function_call_arguments.delta", output_index: 0, delta: '"a.txt"}' },
      { type: "response.function_call_arguments.done", output_index: 0, arguments: '{"path":"a.txt"}' },
      { type: "response.output_item.done", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: '{"path":"a.txt"}' } },
      completed(),
    ]),
  },
  {
    name: "function-call-arguments-done-adds-a-tail",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: "" } },
      { type: "response.function_call_arguments.delta", output_index: 0, delta: '{"path":"a' },
      { type: "response.function_call_arguments.done", output_index: 0, arguments: '{"path":"a.txt"}' },
      { type: "response.output_item.done", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: '{"path":"a.txt"}' } },
      completed(),
    ]),
  },
  {
    name: "function-call-arguments-done-diverges",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: "" } },
      { type: "response.function_call_arguments.delta", output_index: 0, delta: '{"path":"WRONG' },
      { type: "response.function_call_arguments.done", output_index: 0, arguments: '{"path":"right"}' },
      { type: "response.output_item.done", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: '{"path":"right"}' } },
      completed(),
    ]),
  },
  {
    name: "function-call-namespace",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: "{}", namespace: "files" } },
      { type: "response.output_item.done", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: "{}", namespace: "files" } },
      completed(),
    ]),
  },
  {
    name: "function-call-without-added-event",
    body: sse([
      { type: "response.output_item.done", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: '{"path":"a"}' } },
      completed(),
    ]),
  },
  {
    name: "custom-tool-call",
    model: { compat: { supportsOpenAIGrammarTools: true } },
    context: { messages: [{ role: "user", content: "hi", timestamp: 1 }], tools: [grammarTool] },
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "custom_tool_call", id: "ctc_1", call_id: "call_1", name: "grammar_tool", input: "" } },
      { type: "response.custom_tool_call_input.delta", output_index: 0, delta: "ab" },
      { type: "response.custom_tool_call_input.delta", output_index: 0, delta: "cd" },
      { type: "response.custom_tool_call_input.done", output_index: 0, input: "abcd" },
      { type: "response.output_item.done", output_index: 0, item: { type: "custom_tool_call", id: "ctc_1", call_id: "call_1", name: "grammar_tool", input: "abcd" } },
      completed(),
    ]),
  },
  {
    name: "custom-tool-call-unknown-tool",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "custom_tool_call", id: "ctc_1", call_id: "call_1", name: "made_up", input: "seed" } },
      { type: "response.custom_tool_call_input.delta", output_index: 0, delta: "x" },
      { type: "response.output_item.done", output_index: 0, item: { type: "custom_tool_call", id: "ctc_1", call_id: "call_1", name: "made_up", input: "seedx" } },
      completed(),
    ]),
  },
  {
    name: "parallel-output-items",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "reasoning", id: "rs_1", summary: [] } },
      { type: "response.output_item.added", output_index: 1, item: { type: "message", id: "msg_1", role: "assistant", content: [], status: "in_progress" } },
      { type: "response.output_item.added", output_index: 2, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: "" } },
      { type: "response.reasoning_summary_text.delta", output_index: 0, delta: "thinking" },
      { type: "response.output_text.delta", output_index: 1, delta: "text" },
      { type: "response.function_call_arguments.delta", output_index: 2, delta: "{}" },
      { type: "response.output_item.done", output_index: 0, item: { type: "reasoning", id: "rs_1", summary: [{ type: "summary_text", text: "thinking" }] } },
      { type: "response.output_item.done", output_index: 1, item: { type: "message", id: "msg_1", role: "assistant", status: "completed", content: [{ type: "output_text", text: "text", annotations: [] }] } },
      { type: "response.output_item.done", output_index: 2, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: "{}" } },
      completed(),
    ]),
  },
  {
    name: "deltas-without-a-slot",
    body: sse([
      { type: "response.output_text.delta", output_index: 5, delta: "orphan" },
      { type: "response.function_call_arguments.delta", output_index: 5, delta: "{}" },
      { type: "response.custom_tool_call_input.delta", output_index: 5, delta: "x" },
      { type: "response.output_item.added", output_index: 0, item: { type: "message", id: "msg_1", role: "assistant", content: [], status: "in_progress" } },
      { type: "response.output_text.delta", output_index: 0, delta: "ok" },
      { type: "response.output_item.done", output_index: 0, item: { type: "message", id: "msg_1", role: "assistant", status: "completed", content: [{ type: "output_text", text: "ok", annotations: [] }] } },
      completed(),
    ]),
  },
  {
    name: "usage",
    body: sse([
      completed({
        usage: {
          input_tokens: 100, output_tokens: 20, total_tokens: 120,
          input_tokens_details: { cached_tokens: 30, cache_write_tokens: 10 },
          output_tokens_details: { reasoning_tokens: 7 },
        },
      }),
    ]),
  },
  { name: "usage-flex-tier", options: { serviceTier: "flex" }, body: sse([completed({ service_tier: "flex", usage: { input_tokens: 100, output_tokens: 20, total_tokens: 120 } })]) },
  { name: "usage-priority-tier", options: { serviceTier: "priority" }, body: sse([completed({ service_tier: "priority", usage: { input_tokens: 100, output_tokens: 20, total_tokens: 120 } })]) },
  { name: "incomplete-max-output-tokens", body: sse([{ type: "response.incomplete", response: { id: "r", status: "incomplete", incomplete_details: { reason: "max_output_tokens" } } }]) },
  { name: "incomplete-content-filter", body: sse([{ type: "response.incomplete", response: { id: "r", status: "incomplete", incomplete_details: { reason: "content_filter" } } }]) },
  { name: "incomplete-without-reason", body: sse([{ type: "response.incomplete", response: { id: "r", status: "incomplete" } }]) },
  { name: "status-cancelled", body: sse([{ type: "response.completed", response: { id: "r", status: "cancelled" } }]) },
  { name: "status-in-progress", body: sse([{ type: "response.completed", response: { id: "r", status: "in_progress" } }]) },
  { name: "status-missing", body: sse([{ type: "response.completed", response: { id: "r" } }]) },
  {
    name: "tool-use-overrides-stop",
    body: sse([
      { type: "response.output_item.added", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: "{}" } },
      { type: "response.output_item.done", output_index: 0, item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "read", arguments: "{}" } },
      completed(),
    ]),
  },
  { name: "response-failed-with-error", body: sse([{ type: "response.failed", response: { id: "r", status: "failed", error: { code: "server_error", message: "boom" } } }]) },
  { name: "response-failed-with-details", body: sse([{ type: "response.failed", response: { id: "r", status: "failed", incomplete_details: { reason: "content_filter" } } }]) },
  { name: "response-failed-without-details", body: sse([{ type: "response.failed", response: { id: "r", status: "failed" } }]) },
  { name: "error-event", body: sse([{ type: "error", code: "rate_limit", message: "slow down" }]) },
  { name: "no-terminal-event", body: sse([{ type: "response.created", response: { id: "resp_1" } }]) },
  { name: "http-error", status: 429, body: JSON.stringify({ error: { message: "slow down" } }) },
  { name: "http-error-plain-text", status: 500, body: "gateway exploded" },
];

const out: string[] = [];
for (const testCase of cases) {
  const model: any = { ...baseModel, ...(testCase.model ?? {}) };
  const context = testCase.context ?? { messages: [{ role: "user", content: "hi", timestamp: 1 }] };
  const status = testCase.status ?? 200;
  const events: unknown[] = [];
  const s = stream(model, context as any, {
    apiKey: "k",
    maxRetries: 0,
    fetch: (async () =>
      new Response(testCase.body, {
        status,
        headers: { "content-type": status === 200 ? "text/event-stream" : "application/json" },
      })) as any,
    ...(testCase.options ?? {}),
  } as any);
  for await (const event of s) {
    const { partial, ...rest } = event as any;
    events.push(rest);
  }
  out.push(JSON.stringify({ name: testCase.name, model, context, options: testCase.options ?? {}, body: testCase.body, status, events }));
}
process.stdout.write(out.join("\n") + "\n");
