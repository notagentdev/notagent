# Faktenbericht: @notagent/ai und @notagent/agent-core

Quelle: `/Users/dev/projects/notagent-main/packages/ai` und `packages/agent`. Alle Aussagen aus dem Quellcode (Stand 2026-08-13).

---

# TEIL A — packages/ai (22 744 LOC src, 34 066 LOC Tests)

## A1. Struktur

### src/ (Root, 3 821 LOC)
- `types.ts` 830 — Alle Kerntypen: Message, AssistantMessage, Usage, StopReason, AssistantMessageEvent, Model, Tool, Context, *Compat-Interfaces, KnownProvider/KnownApi-Unions
- `models.ts` 944 — Provider/Models/MutableModels-Interfaces, ModelsImpl (Auth-Auflösung, Refresh-Generationen, Publikations-Ketten), createModels(), createProvider(), calculateCost(), clampThinkingLevel(), getSupportedThinkingLevels(), hasApi(), modelsAreEqual()
- `image-models.generated.ts` 639 — Bildmodell-Katalog (42 Modelle, nur openrouter)
- `compat.ts` 298 — Legacy-Global-API (als „wird gelöscht" markiert)
- `images-models.ts` 275, `env-api-keys.ts` 188 (Env-Var-Namen, Vertex-ADC-Detektion), `models.generated.ts` 124 (Aggregator über 39 Shards), `cli.ts` 119 (bin notagent-ai: OAuth-Login/Logout in auth.json), `legacy-api-aliases.ts` 108, `images-api-registry.ts` 53, `index.ts` 47, `models-store.ts` 45 (persistierte Kataloge inkl. etag/lastModified/checkedAt), `image-models.ts` 42, `model-catalog.ts` 27 (flattenModelCatalog = Object.assign zur Laufzeit), `session-resources.ts` 24, `images.ts` 21, `bun-oauth.ts` 21, `oauth.ts` 10, `bedrock-provider.ts` 6

### src/api/ (11 381 LOC) — Provider-Protokolle
- `openai-codex-responses.ts` 1662 — ChatGPT-Codex; SSE + WebSocket-Transport mit Fallback, Session-Caching
- `openai-completions.ts` 1577 — OpenAI Chat-Completions (openai SDK), 11 thinkingFormat-Varianten, Anthropic-Style cache_control, Grammar-Tools
- `anthropic-messages.ts` 1352 — Anthropic Messages; **eigener SSE-Decoder** (SDK nur zum Bauen/Signieren via .asResponse()), Claude-Code-OAuth-Toolnamen-Mapping
- `bedrock-converse-stream.ts` 1188 — AWS Bedrock ConverseStream
- `mistral-conversations.ts` 931, `openai-responses-shared.ts` 792, `google-vertex.ts` 596, `google-generative-ai.ts` 521, `pi-messages.ts` 433 (Radius-Gateway: SSE mit vorserialisierten Assistant-Events OHNE partial), `google-shared.ts` 419, `openai-responses.ts` 372, `azure-openai-responses.ts` 330, `constrained-sampling.ts` 277 (makeStrictJsonSchema, Grammar-Tools Lark/Regex, GrammarToolInputJsonBuffer), `transform-messages.ts` 223, `openrouter-images.ts` 196, `cloudflare-gateway-binding.ts` 192, `lazy.ts` 98 (lazyStream/lazyApi), `simple-options.ts` 86 (Thinking-Budgets: minimal 1024 / low 2048 / medium 8192 / high 16384, MIN_ANSWER_TOKENS 1024), `github-copilot-headers.ts` 37, 9 *.lazy.ts-Wrapper

### src/auth/ (3 513 LOC)
- OAuth-Flows: `oauth/openai-codex.ts` 544 (PKCE-Browser + Device-Code), `oauth/github-copilot.ts` 417 (Device-Flow + Copilot-Token-Exchange + Enterprise), `oauth/radius.ts` 403, `oauth/anthropic.ts` 364 (PKCE + lokaler Callback-Server), `oauth/openrouter.ts` 311, `oauth/kimi-coding.ts` 310, `oauth/xai.ts` 239
- `types.ts` 240 (Credential, CredentialStore mit read/list/modify/delete — modify ist einziger Schreibpfad, serialisiertes RMW pro Provider), `resolve.ts` 205 (resolveProviderAuth + Double-Checked-Locking-Refresh: MINIMUM_VALIDITY 5min, REFRESH_TIMEOUT 15s), `oauth-page.ts` 109, `device-code.ts` 98 (RFC 8628, slow_down, +5s Increment), `oauth/load.ts` 68, `credential-store.ts` 67, `helpers.ts` 59, `context.ts` 45, `pkce.ts` 34 (WebCrypto, 32-Byte-Verifier, SHA-256, base64url)

### src/utils/ (1 866 LOC)
validation.ts 350 (TypeBox + JSON-Schema-Coercion), retry.ts 228 (RetryPolicy {enabled,maxRetries,baseDelayMs}, Backoff base·2^(attempt-1), Regex-Klassifikation, Aborts terminal), overflow.ts 180 (25 Kontext-Overflow-Regexe), error-body.ts 149, estimate.ts 143 (4 Zeichen/Token, Bild=4800), json-parse.ts 124 (Partial-JSON), provider-retry.ts 125 (HTTP-Retries: 408/409/429/5xx, retry-after/-ms, x-should-retry, exp. min(0.5·2^i,8)s + 25% Jitter, Cap maxRetryDelayMs 60s), node-http-proxy.ts 112, event-stream.ts 88, uuid.ts 48 (UUIDv7), u.a.

### src/providers/ (2 118 LOC + 39 Shards)
- `faux.ts` 708 — Test-Provider (einziger mit fetchDeferred/cancelDeferred)
- `all.ts` 155 — builtinProviders() (40 Provider), builtinModels(), getBuiltinModelDataGeneratedAt()
- 39 Provider-Factories, `providers/data/*.json` (39 Dateien, ~600 KB, **1 224 Modelle**; größte: openrouter.json 137 KB), Manifest `.manifest.json` mit schemaVersion 3, generatedAt, structureHash, SHA-256 pro Datei

## A2. 40 Built-in-Provider

amazon-bedrock (bedrock-converse-stream; Bearer/AWS-Chain/Profile), ant-ling, anthropic (API-Key + OAuth Claude Pro/Max), azure-openai-responses, baseten, cerebras, cloudflare-ai-gateway (3 APIs), cloudflare-workers-ai, deepseek, fireworks (2 APIs), github-copilot (3 APIs, OAuth Device), google, google-vertex (ADC), groq, huggingface, kimi-coding (nur OAuth), minimax, minimax-cn, mistral, moonshotai, moonshotai-cn, nvidia, openai (openai-responses), openai-codex (nur OAuth, eigene API), opencode, opencode-go, openrouter (OAuth), qwen-token-plan{,-cn,-individual}, radius (pi-messages, OAuth, **einziger dynamischer Katalog**), together, vercel-ai-gateway, xai (OAuth Device), xiaomi{,-token-plan-cn,-ams,-sgp}, zai, zai-coding-cn. Bild-Provider: openrouter.

### Provider-Interface (models.ts:97-149)
id, name, baseUrl?, headers?, auth {apiKey?, oauth?}, getModels() (sync, darf nicht werfen), refreshModels?(), filterModels?(), stream(), streamSimple(), fetchDeferred?(), cancelDeferred?().

**Stream-Vertrag (types.ts:314-319):** Nach Aufruf darf nichts geworfen werden; Fehler werden im Stream als AssistantMessage mit stopReason error|aborted + errorMessage kodiert.

### Stream-Event-Typen (types.ts:523-539)
start | text_start/delta/end | thinking_start/delta/end | toolcall_start/delta/end (alle mit contentIndex + partial) | done {reason: stop|length|toolUse|deferred, message} | error {reason: aborted|error, error: AssistantMessage}.

**`partial` ist dieselbe Objektreferenz über den gesamten Stream — Provider mutieren in-place.** Konsument macht Shallow-Copies. → Rust: eigener Streaming-State oder Snapshot-Klone.

## A3. Streaming-Mechanik
- Anthropic: eigener SSE-Decoder (Zustandsmaschine event/data/raw, CR/LF/CRLF-Handling, 6 Event-Namen, Fehler bei Stream-Ende ohne message_stop)
- OpenAI/Google/Bedrock/Mistral: SDK-Async-Iterator
- pi-messages: eigener SSE-Reader (Split auf Doppel-Newline, [DONE])
- openai-codex: transport auto|sse|websocket|websocket-cached, WebSocket zuerst, Fallback auf SSE vor erstem Event, per Session gemerkt
- **Partial-JSON** (utils/json-parse.ts, parseStreamingJson): 1. JSON.parse → 2. repairJson + parse → 3. partialParse (partial-json) → 4. partialParse(repairJson) → 5. {}. Pro Delta wird der gesamte Buffer neu geparst. Scratch-Felder (partialJson/partialArgs) werden per delete vor Persistenz entfernt.
- `EventStream<T,R>` (event-stream.ts): Push-Queue mit Waiter-Liste, AsyncIterable, result(): Promise<R>; AssistantMessageEventStream: complete bei done/error.
- `lazyStream(model, setup)`: Stream synchron zurückgeben, async Setup dahinter; Setup-Fehler → error-Event.
- Deferred Responses: nur Typinfrastruktur, einzige Implementierung faux.ts → kann im Port entfallen.

## A4. Modellkatalog
- models.generated.ts nur 124 LOC (Aggregator). Daten in providers/data/*.json, Struktur `{api: {model_id: Model}}`, zur Laufzeit flach gemergt.
- **Keine Runtime-Hydration des Builtin-Katalogs.** Dynamik über Models.refresh() (2 Phasen: Restore aus ModelsStore ohne Netz, dann mit Credential + Netz), createProvider({fetchModels}) merged über statische Baseline, Generationszähler + AbortController gegen Races.
- Generierung: scripts/generate-models.ts (~2 700 LOC; Quellen models.dev > OpenRouter > Vercel AI Gateway + NVIDIA; hartkodierte Overrides). Für den Port: JSON-Daten als Snapshot übernehmen (include_str!/build.rs), Manifest-Validierung optional.

## A5. Auth
- Credential = ApiKeyCredential {type, key?, env?} | OAuthCredential {type, refresh, access, expires, ...}
- Auflösung (resolve.ts:63-107): 1. overrides.apiKey → 2. gespeicherte Credential (KEIN stilles Env-Fallback bei vorhandener Credential/fehlgeschlagenem Refresh) → 3. ambient Env (Bedrock: BEARER → PROFILE → Access-Keys → ECS → Web-Identity)
- Token-Refresh: Double-Checked Locking über credentials.modify(), rotierte Credential wird vor Lock-Freigabe persistiert
- 7 OAuth-Flows (siehe A1), alle lazy geladen; Client-IDs teils base64-obfuskiert; PKCE via WebCrypto; Device-Code RFC 8628

## A6. Externe Deps → Rust
- @anthropic-ai/sdk (nur Request-Bau; Stream selbst geparst) → reqwest + eigener SSE
- openai SDK → reqwest + eigener SSE (Chat-Completions + Responses)
- @google/genai → reqwest (Gemini/Vertex REST)
- @aws-sdk/client-bedrock-runtime → aws-sdk-rust (aws-sdk-bedrockruntime) oder eigenes SigV4 + EventStream-Decoder
- partial-json → eigener partieller JSON-Parser
- typebox → schemars/serde_json (JSON Schema) + eigene Validierung/Coercion
- http(s)-proxy-agent → reqwest-Proxy-Support
- Alle SDK-Clients mit maxRetries: 0 — Retry macht provider-retry.ts selbst (SDK-Backoff ignoriert AbortSignal)

## A7. Zentrale Typen (types.ts, Felder exakt)
- TextContent {type:"text", text, textSignature?}, ThinkingContent {type:"thinking", thinking, thinkingSignature?, redacted?}, ImageContent {type:"image", data(base64), mimeType}, ToolCall {type:"toolCall", id, name, arguments, thoughtSignature?, namespace?}
- UserMessage {role:"user", content: string | (Text|Image)[], timestamp}
- AssistantMessage {role:"assistant", content:(Text|Thinking|ToolCall)[], api, provider, model, responseModel?, responseId?, diagnostics?, usage, stopReason, deferred?, errorMessage?, rawStopReason?, endTurn?, timestamp}
- ToolResultMessage {role:"toolResult", toolCallId, toolName, content:(Text|Image)[], details?, usage?, addedToolNames?, isError, timestamp}
- Usage {input, output, cacheRead, cacheWrite, cacheWrite1h?, reasoning?, totalTokens, cost{input,output,cacheRead,cacheWrite,total}}
- StopReason = pending|stop|length|toolUse|error|aborted|deferred
- Tool {name, description, parameters(TSchema), constrainedSampling?}
- Context {systemPrompt?, messages, tools?}
- Model {id, name, api, provider, baseUrl, reasoning, thinkingLevelMap?, input:("text"|"image")[], cost (+tiers mit inputTokensAbove), contextWindow, maxTokens, samplingParams?, headers?, compat?}
- ThinkingLevel = minimal|low|medium|high|xhigh|max; ModelThinkingLevel = off|…
- OpenAICompletionsCompat: 22 Felder inkl. thinkingFormat mit 11 Varianten (openai|openrouter|deepseek|together|baseten|zai|qwen|chat-template|qwen-chat-template|string-thinking|ant-ling); OpenAIResponsesCompat 8; AnthropicMessagesCompat 8; BedrockCompat 1.
- ProviderRequestOptions: signal, telemetryContext, apiKey, fetch, env, onPayload, onResponse, headers (null löscht Default), timeoutMs, maxRetries, maxRetryDelayMs (60s). StreamOptions +temperature, samplingParams, maxTokens, transport, cacheRetention, sessionId, websocketConnectTimeoutMs, metadata. SimpleStreamOptions +reasoning, deferred, thinkingBudgets.

---

# TEIL B — packages/agent (12 616 LOC src)

## B1. Struktur
Kern (2 376 LOC): `agent-loop.ts` 796 (agentLoop, agentLoopContinue, runLoop, streamAssistantResponse, executeToolCalls{Sequential,Parallel}, prepareToolCall, finalizeExecutedToolCall), `agent.ts` 592 (Agent-Klasse: Zustand, Listener, Steering/Follow-up-Queues, Run-Lifecycle), `types.ts` 443, `proxy.ts` 370 (streamProxy — partial-freie Proxy-Events), `index.ts` 145, `stream-fn.ts` 20, `search/scanning.ts` 176.

`src/harness/` (10 240 LOC): **AgentHarness ist ein STUB** — fast alle Methoden werfen HarnessNotImplemented. Funktionsfähig sind: compaction/compaction.ts 848 (compact(), shouldCompact(), findCutPoint(), generateSummary(), SUMMARIZATION_SYSTEM_PROMPT, DEFAULT_COMPACTION_SETTINGS), env/nodejs.ts 695 (NodeExecutionEnv), reducer.ts 667, telemetry.ts 620 (Schemas), tools/ (bash 161, read 144, edit 127, write 39, edit-diff 500 mit Fuzzy-Matching), session/ (jsonl-Storage 277 + Repo 247 + Codec 240 „JSONL v4", memory.ts 192, types.ts 393, conformance.ts 1016), skills.ts 375, prompt-templates.ts 262, compaction/branch-summarization.ts 280, utils/truncate.ts 350 (Defaults 2000 Zeilen / 50 KB), utils/shell-output.ts 195, messages.ts 168 (CustomMessages + convertToLlm), harness/types.ts 315 (Result/ok/err, Skill, PromptTemplate, FileSystem, Shell, ExecutionEnv, Fehlertypen).

## B2. Agent-Loop im Detail

### Events (genau 10 Varianten, types.ts:428-443)
agent_start | agent_end {messages} | turn_start | turn_end {message, toolResults} | message_start {message} | message_update {message, assistantMessageEvent} | message_end {message} | tool_execution_start {toolCallId, toolName, args} | tool_execution_update {+partialResult} | tool_execution_end {toolCallId, toolName, result, isError}.

### Loop-Struktur (agent-loop.ts:155-275)
Äußere Schleife (Follow-ups) → innere Schleife (Turns solange Tool-Calls oder pending Messages): turn_start → pending Messages einreihen → streamAssistantResponse → bei stopReason error|aborted: turn_end + agent_end + return → Tool-Calls: bei stopReason "length" alle Calls fehlschlagen lassen (Truncation-Guard, terminate=false), sonst executeToolCalls → turn_end → prepareNextTurn?() (ersetzt context/model/reasoning) → shouldStopAfterTurn?() → Steering drainen. Follow-ups nur wenn Loop sonst endet. Rückgabe: EventStream<AgentEvent, AgentMessage[]>, Terminator agent_end.

### Assistant-Streaming (281-372)
transformContext? → convertToLlm (AgentMessage[]→Message[]) → getApiKey?(provider) || apiKey **pro Turn neu** (OAuth-Tokens) → streamFunction(model, ctx, config) → Event-Mapping (start: partial in Context pushen; Deltas: letztes Element ersetzen + message_update; done/error: result() awaiten, message_end).

### Tool-Ausführung
- Modus: config.toolExecution === "sequential" ODER ein Tool hat executionMode "sequential" → sequentiell; sonst **parallel** (Default).
- Parallel (489-554), dreiphasig: 1. Preflight sequentiell (tool_execution_start, prepareToolCall: Lookup → prepareArguments → validateToolArguments → beforeToolCall-Hook; Immediate-Outcomes sofort finalisiert; Abbruchcheck nach jedem Call). 2. Promise.all über Thunks; tool_execution_end in Abschlussreihenfolge. 3. Tool-Result-Messages danach in Assistant-Quellreihenfolge (je message_start + message_end).
- Sequentiell (433-487): pro Call prepare → execute → finalize → Events; Abbruchcheck nach jedem Call.
- Abort: signal wird an tool.execute(id, args, signal, onUpdate) durchgereicht; Loop prüft an Schleifengrenzen.
- tool_execution_update: gesammelt in updateEvents[], nach Abschluss abgewartet; acceptingUpdates verhindert Updates nach Settlement.
- Batch-Terminierung: nur wenn JEDES finalisierte Ergebnis terminate === true.

### Steering/Follow-up
PendingMessageQueue mit QueueMode "all" | "one-at-a-time" (Default beide: one-at-a-time). steer(): am Ende jedes Turns gedrained + einmal vor erstem Turn. followUp(): nur wenn Loop sonst endet. continue(): erst Steering (skipInitialSteeringPoll), dann Follow-up; wirft wenn messages leer/letzte nicht assistant. clearSteeringQueue/clearFollowUpQueue/clearAllQueues/hasQueuedMessages.

### State/Listener
Agent.processEvents() ist Reducer (streamingMessage, messages.push, pendingToolCalls als copy-on-write Set, errorMessage); danach **alle Listener sequentiell awaited** in Subscription-Reihenfolge mit AbortSignal — semantisch relevant (Backpressure, Idle-Definition). Run idle erst wenn agent_end-Listener gesettelt + finishRun(). Gleichzeitige Runs werfen. handleRunFailure() baut synthetische AssistantMessage (EMPTY_USAGE, stopReason aborted|error) + 4 Events.

### Retry
Kein Retry im Loop selbst. 1. provider-retry.ts (HTTP), 2. retry.ts retryAssistantCall (Policy), 3. compaction completeSimpleWithRetries; AgentHarness.retryPolicy Default {enabled:false}.

## B3. Zentrale Typen
- AgentState {systemPrompt, model, thinkingLevel (off|…), tools (Set kopiert Top-Level), messages (dito), isStreaming, streamingMessage?, pendingToolCalls: ReadonlySet<string>, errorMessage?}
- AgentTool extends Tool: label, prepareArguments?, execute(toolCallId, params, signal?, onUpdate?) → Promise<AgentToolResult> (wirft bei Fehler), executionMode?
- AgentToolResult {content:(Text|Image)[], details, usage?, addedToolNames?, terminate?}
- AgentMessage = Message | CustomAgentMessages[…] (Declaration Merging; Custom-Messages werden an LLM-Grenze via convertToLlm gefiltert/konvertiert)
- AgentLoopConfig extends SimpleStreamOptions: model, convertToLlm (Pflicht); transformContext, getApiKey, shouldStopAfterTurn, prepareNextTurn, getSteeringMessages, getFollowUpMessages, toolExecution, beforeToolCall, afterToolCall. Alle Callbacks: „must not throw or reject".
- AgentOptions: initialState, convertToLlm (Default: Filter auf user|assistant|toolResult), streamFn, getApiKey, onPayload, onResponse, before/afterToolCall, shouldStopAfterTurn, prepareNextTurn(WithContext), steeringMode, followUpMode, sessionId, thinkingBudgets, transport (Default auto), maxRetryDelayMs, toolExecution (Default parallel)
- Agent-Methoden: subscribe, prompt, continue, steer, followUp, clear*Queue(s), hasQueuedMessages, abort, waitForIdle, reset, state/signal, steeringMode/followUpMode

## B4. ai↔agent-Kopplung
Nur Typen + 4 Runtime-Werte (EventStream, validateToolArguments, parseStreamingJson, uuidv7). AgentMessage ⊃ Message; AgentTool extends Tool; thinkingLevel "off" → reasoning undefined; Referenz-StreamFn = models.streamSimple.

## Port-Hinweise
1. In-place mutiertes partial → in Rust: eigener Streaming-State + Snapshots (proxy.ts/pi-messages.ts zeigen die partial-freie Variante).
2. Scratch-Felder (delete) → separater Streaming-State, nicht in Content-Enums.
3. Katalog: 1 224 Modelle in 39 JSONs + SHA-256-Manifest → include_str!/build.rs.
4. Deferred Responses: nur Typen, kann entfallen (nur faux nutzt es — faux wird aber für Tests gebraucht!).
5. AgentHarness: Stub — nicht portieren; Agent + agent-loop sind der funktionsfähige Kern. Funktionsfähige harness-Teile (compaction, tools, session/jsonl, skills, prompt-templates, truncate, shell-output, messages) werden vom coding-agent GENUTZT — deren tatsächliche Konsumenten prüfen (coding-agent importiert compaction, messages, prompt-templates, skills, truncate, shell-output, tools/edit-diff…).
6. Sequentielle Listener-Awaits sind semantisch relevant — nicht fire-and-forget machen.
