// Differential oracle for the small utility and header modules of `packages/ai`
// that carry no dedicated TS suite of their own: hash, headers, sanitize-unicode,
// provider-env, deferred-tools, diagnostics, github-copilot-headers, auth/context
// and image-models. Every value below comes out of the TypeScript original.
const AI = "/Users/dev/projects/notagent-main/packages/ai/src";

const { shortHash } = await import(`${AI}/utils/hash.ts`);
const { headersToRecord, providerHeadersToRecord } = await import(`${AI}/utils/headers.ts`);
const { sanitizeSurrogates } = await import(`${AI}/utils/sanitize-unicode.ts`);
const { getProviderEnvValue } = await import(`${AI}/utils/provider-env.ts`);
const { splitDeferredTools } = await import(`${AI}/utils/deferred-tools.ts`);
const { formatThrownValue, extractDiagnosticError } = await import(`${AI}/utils/diagnostics.ts`);
const { inferCopilotInitiator, hasCopilotVisionInput, buildCopilotDynamicHeaders } = await import(
  `${AI}/api/github-copilot-headers.ts`
);
const { defaultProviderAuthContext } = await import(`${AI}/auth/context.ts`);
const { getImageModel, getImageProviders, getImageModels } = await import(`${AI}/image-models.ts`);

const lines: string[] = [];
const emit = (value: unknown) => lines.push(JSON.stringify(value));

// --- hash.ts ---------------------------------------------------------------
for (const input of [
  "",
  "a",
  "hello world",
  "https://api.openai.com/v1/responses",
  "gruesse-aus-utf16",
  "x".repeat(1000),
  "0",
  " ",
]) {
  emit({ kind: "shortHash", input, output: shortHash(input) });
}
// Non-ASCII inputs go in as code units so the fixture stays plain ASCII.
for (const codeUnits of [
  [0x00e4, 0x1f600 >= 0x10000 ? 0xd83d : 0, 0xde00, 0x6f22],
  [0x4e2d, 0x6587],
  [0xd83d, 0xde00],
]) {
  const input = String.fromCharCode(...codeUnits);
  emit({ kind: "shortHashUnits", input: codeUnits, output: shortHash(input) });
}

// --- sanitize-unicode.ts ---------------------------------------------------
// Emitted as UTF-16 code units so lone surrogates survive the JSON round trip.
const codeUnitsOf = (text: string) =>
  Array.from({ length: text.length }, (_, index) => text.charCodeAt(index));
for (const input of [
  `Hello ${String.fromCharCode(0xd83d, 0xde48)} World`,
  `Text ${String.fromCharCode(0xd83d)} here`,
  `Text ${String.fromCharCode(0xdc4b)} here`,
  String.fromCharCode(0xd83d, 0xde00),
  `${String.fromCharCode(0xd83d)}${String.fromCharCode(0xd83d, 0xde00)}`,
  "plain",
]) {
  emit({
    kind: "sanitizeSurrogates",
    input: codeUnitsOf(input),
    output: codeUnitsOf(sanitizeSurrogates(input)),
  });
}

// --- headers.ts ------------------------------------------------------------
emit({
  kind: "headersToRecord",
  input: [
    ["content-type", "application/json"],
    ["x-api-key", "secret"],
  ],
  output: headersToRecord(
    new Headers([
      ["content-type", "application/json"],
      ["x-api-key", "secret"],
    ]),
  ),
});
emit({ kind: "headersToRecord", input: [], output: headersToRecord(new Headers()) });

for (const input of [
  null,
  {},
  { "x-one": "1", "x-two": null },
  { "x-only-null": null },
  { "x-a": "a", "x-b": "b" },
]) {
  emit({
    kind: "providerHeadersToRecord",
    input,
    output:
      providerHeadersToRecord(
        input === null ? undefined : (input as Record<string, string | null>),
      ) ?? null,
  });
}

// --- provider-env.ts -------------------------------------------------------
process.env.NOTAGENT_ORACLE_SET = "from-process";
process.env.NOTAGENT_ORACLE_EMPTY = "";
for (const [name, env] of [
  ["NOTAGENT_ORACLE_SET", undefined],
  ["NOTAGENT_ORACLE_SET", { NOTAGENT_ORACLE_SET: "from-scope" }],
  ["NOTAGENT_ORACLE_SET", { NOTAGENT_ORACLE_SET: "" }],
  ["NOTAGENT_ORACLE_EMPTY", undefined],
  ["NOTAGENT_ORACLE_MISSING", undefined],
  ["NOTAGENT_ORACLE_MISSING", { NOTAGENT_ORACLE_MISSING: "only-scoped" }],
] as [string, Record<string, string> | undefined][]) {
  emit({
    kind: "getProviderEnvValue",
    name,
    env: env ?? null,
    output: getProviderEnvValue(name, env) ?? null,
  });
}

// --- deferred-tools.ts -----------------------------------------------------
const tool = (name: string) => ({ name, description: name, parameters: { type: "object" } });
const deferredCases = [
  {
    label: "disabled keeps every tool immediate",
    enabled: false,
    tools: [tool("read"), tool("bash")],
    messages: [] as unknown[],
  },
  {
    label: "no transcript hints",
    enabled: true,
    tools: [tool("read"), tool("bash")],
    messages: [] as unknown[],
  },
  {
    label: "added tool name that was never called is deferred",
    enabled: true,
    tools: [tool("read"), tool("bash")],
    messages: [{ role: "toolResult", addedToolNames: ["bash"] }] as unknown[],
  },
  {
    label: "a used tool stays immediate",
    enabled: true,
    tools: [tool("read"), tool("bash")],
    messages: [
      { role: "assistant", content: [{ type: "toolCall", name: "bash" }] },
      { role: "toolResult", addedToolNames: ["bash"] },
    ] as unknown[],
  },
  {
    label: "duplicates collapse to the last definition",
    enabled: true,
    tools: [tool("read"), tool("read"), tool("bash")],
    messages: [{ role: "toolResult", addedToolNames: ["read"] }] as unknown[],
  },
];
for (const testCase of deferredCases) {
  const result = splitDeferredTools(
    { messages: testCase.messages, tools: testCase.tools } as never,
    testCase.enabled,
  );
  emit({
    kind: "splitDeferredTools",
    label: testCase.label,
    enabled: testCase.enabled,
    tools: testCase.tools.map((entry) => entry.name),
    messages: testCase.messages,
    immediate: result.immediate.map((entry: { name: string }) => entry.name),
    deferred: [...result.deferred.keys()],
  });
}

// --- diagnostics.ts --------------------------------------------------------
const describeError = (value: unknown) => {
  const info = extractDiagnosticError(value);
  return {
    name: info.name ?? null,
    message: info.message,
    code: info.code ?? null,
    // `stack` is environment specific and stays out of the comparison.
    hasStack: info.stack !== undefined,
  };
};
const thrown: [string, unknown][] = [
  ["error with message", new Error("boom")],
  ["error without message", Object.assign(new Error(""), { name: "EmptyError" })],
  ["string", "just a string"],
  ["number", 42],
  ["null", null],
  ["undefined", undefined],
  ["object with custom toString", { toString: () => "custom" }],
  ["error with a string code", Object.assign(new Error("no entry"), { code: "ENOENT" })],
  ["error with a numeric code", Object.assign(new Error("errno"), { code: 13 })],
  ["error with a non-scalar code", Object.assign(new Error("weird"), { code: { nested: true } })],
];
for (const [label, value] of thrown) {
  emit({
    kind: "diagnostics",
    label,
    formatted: formatThrownValue(value),
    error: describeError(value),
  });
}

// --- github-copilot-headers.ts ---------------------------------------------
const copilotCases: [string, unknown[]][] = [
  ["empty", []],
  ["last is user", [{ role: "user", content: "hi" }]],
  [
    "last is assistant",
    [
      { role: "user", content: "hi" },
      { role: "assistant", content: [] },
    ],
  ],
  ["last is toolResult", [{ role: "toolResult", content: [] }]],
  ["user image", [{ role: "user", content: [{ type: "image", data: "", mimeType: "image/png" }] }]],
  [
    "toolResult image",
    [{ role: "toolResult", content: [{ type: "image", data: "", mimeType: "image/png" }] }],
  ],
  ["user text only", [{ role: "user", content: [{ type: "text", text: "hi" }] }]],
  ["string content is not an image", [{ role: "user", content: "hi" }]],
];
for (const [label, messages] of copilotCases) {
  const hasImages = hasCopilotVisionInput(messages as never);
  emit({
    kind: "copilotHeaders",
    label,
    messages,
    initiator: inferCopilotInitiator(messages as never),
    hasVisionInput: hasImages,
    headers: buildCopilotDynamicHeaders({ messages: messages as never, hasImages }),
  });
}

// --- auth/context.ts -------------------------------------------------------
process.env.NOTAGENT_ORACLE_BLANK = "   ";
process.env.NOTAGENT_ORACLE_PADDED = "  value  ";
const authContext = defaultProviderAuthContext();
for (const name of [
  "NOTAGENT_ORACLE_SET",
  "NOTAGENT_ORACLE_EMPTY",
  "NOTAGENT_ORACLE_BLANK",
  "NOTAGENT_ORACLE_PADDED",
  "NOTAGENT_ORACLE_MISSING",
]) {
  emit({ kind: "authContextEnv", name, output: (await authContext.env(name)) ?? null });
}
emit({
  kind: "authContextFileExists",
  path: "/definitely/not/here",
  output: await authContext.fileExists("/definitely/not/here"),
});

// --- image-models.ts -------------------------------------------------------
const providers = getImageProviders();
emit({ kind: "imageProviders", output: providers });
for (const provider of providers) {
  emit({
    kind: "imageModels",
    provider,
    ids: getImageModels(provider as never).map((model: { id: string }) => model.id),
  });
}
const firstProvider = providers[0];
const firstModelId = getImageModels(firstProvider as never)[0]?.id;
if (firstProvider && firstModelId) {
  const model = getImageModel(firstProvider as never, firstModelId as never);
  emit({
    kind: "imageModel",
    provider: firstProvider,
    id: firstModelId,
    api: model.api,
    name: model.name,
  });
}
emit({
  kind: "imageModelMissing",
  provider: firstProvider,
  id: "does-not-exist",
  output: getImageModel(firstProvider as never, "does-not-exist" as never) ?? null,
});

process.stdout.write(`${lines.join("\n")}\n`);
