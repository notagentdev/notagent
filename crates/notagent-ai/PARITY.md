# Parity-Ledger: notagent-ai

TS-Quelle: `/Users/dev/projects/notagent-main/packages/ai` (22 744 LOC in src/) — Workstream B.

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

## Lektüre-Protokoll

| Datum | TS-/Referenz-Datei | LOC | gelesen von (Task) |
|---|---|---|---|
| 2026-08-13 | `packages/ai/src/types.ts` | 830 | 1 |
| 2026-08-13 | `packages/ai/src/index.ts` | 47 | 1 |
| 2026-08-13 | `packages/ai/src/utils/diagnostics.ts` | 45 | 1 |
| 2026-08-13 | `packages/ai/src/utils/event-stream.ts` | 88 | 1 |
| 2026-08-13 | `packages/ai/src/utils/uuid.ts` | 48 | 1 |
| 2026-08-13 | `packages/ai/src/utils/json-parse.ts` | 124 | 3 |
| 2026-08-13 | `packages/ai/src/utils/retry.ts` | 228 | 3 |
| 2026-08-13 | `packages/ai/src/utils/provider-retry.ts` | 125 | 3 |
| 2026-08-13 | `packages/ai/src/utils/overflow.ts` | 180 | 3 |
| 2026-08-13 | `packages/ai/src/utils/estimate.ts` | 143 | 3 |
| 2026-08-13 | `packages/ai/src/utils/validation.ts` | 350 | 3 |
| 2026-08-13 | `packages/ai/src/utils/text.ts` | 12 | 3 |
| 2026-08-13 | `packages/ai/src/utils/hash.ts` | 13 | 3 |
| 2026-08-13 | `packages/ai/src/utils/headers.ts` | 18 | 3 |
| 2026-08-13 | `packages/ai/src/utils/sanitize-unicode.ts` | 25 | 3 |
| 2026-08-13 | `packages/ai/src/utils/abort.ts` | 50 | 3 |
| 2026-08-13 | `packages/ai/src/utils/abort-signals.ts` | 41 | 3 |
| 2026-08-13 | `packages/ai/src/utils/error-body.ts` | 149 | 3 |
| 2026-08-13 | `packages/ai/src/utils/deferred-tools.ts` | 39 | 3 |
| 2026-08-13 | `packages/ai/src/utils/provider-env.ts` | 52 | 3 |
| 2026-08-13 | `packages/ai/src/utils/typebox-helpers.ts` | 24 | 3 |
| 2026-08-13 | `packages/ai/src/utils/node-http-proxy.ts` | 112 | 3 |
| 2026-08-13 | `packages/ai/test/validation.test.ts` | 210 | 3 |
| 2026-08-13 | `packages/ai/test/retry.test.ts` | 223 | 3 |
| 2026-08-13 | `packages/ai/test/provider-retry.test.ts` | 81 | 3 |
| 2026-08-13 | `packages/ai/test/overflow.test.ts` | 177 | 3 |
| 2026-08-13 | `packages/ai/test/context-estimate.test.ts` | 81 | 3 |
| 2026-08-13 | `packages/ai/test/text.test.ts` | 33 | 3 |
| 2026-08-13 | `packages/ai/test/uuid.test.ts` | 50 | 3 |
| 2026-08-13 | `packages/ai/test/node-http-proxy.test.ts` | 76 | 3 |
| 2026-08-13 | `node_modules/partial-json/dist/{index,options}.js` (Referenz) | 220 + 60 | 3 |
| 2026-08-13 | `node_modules/typebox/build/value/convert/**` (Referenz) | 285 | 3 |
| 2026-08-13 | `packages/ai/src/models.ts` | 944 | 4 |
| 2026-08-13 | `packages/ai/src/models-store.ts` | 45 | 4 |
| 2026-08-13 | `packages/ai/src/auth/types.ts` | 240 | 6 |
| 2026-08-13 | `packages/ai/src/auth/credential-store.ts` | 67 | 6 |
| 2026-08-13 | `packages/ai/src/auth/resolve.ts` | 205 | 6 |
| 2026-08-13 | `packages/ai/src/auth/context.ts` | 45 | 6 |
| 2026-08-13 | `packages/ai/src/auth/helpers.ts` | 59 | 6 |
| 2026-08-13 | `packages/ai/src/env-api-keys.ts` | 188 | 6 |
| 2026-08-13 | `packages/ai/test/env-api-keys.test.ts` | 116 | 6 |
| 2026-08-13 | `packages/ai/test/oauth-auth.test.ts` | 166 | 6 |
| 2026-08-13 | `packages/ai/src/api/lazy.ts` | 98 | 4 |
| 2026-08-13 | `packages/ai/test/models-runtime.test.ts` | 1159 | 4 |
| 2026-08-13 | `packages/ai/src/model-catalog.ts` | 27 | 5 |
| 2026-08-13 | `packages/ai/src/auth/oauth/pkce.ts` | 34 | 7 |
| 2026-08-13 | `packages/ai/src/auth/oauth/device-code.ts` | 98 | 7 |
| 2026-08-13 | `packages/ai/src/auth/oauth/oauth-page.ts` | 109 | 7 |
| 2026-08-13 | `packages/ai/src/auth/oauth/load.ts` | 68 | 7 |
| 2026-08-13 | `packages/ai/src/auth/oauth/anthropic.ts` | 364 | 7 |
| 2026-08-13 | `packages/ai/src/auth/oauth/openai-codex.ts` | 544 | 7 |
| 2026-08-13 | `packages/ai/src/auth/oauth/github-copilot.ts` | 417 | 7 |
| 2026-08-13 | `packages/ai/src/auth/oauth/openrouter.ts` | 311 | 7 |
| 2026-08-13 | `packages/ai/src/auth/oauth/kimi-coding.ts` | 310 | 7 |
| 2026-08-13 | `packages/ai/src/auth/oauth/xai.ts` | 239 | 7 |
| 2026-08-13 | `packages/ai/src/auth/oauth/radius.ts` | 403 | 7 |
| 2026-08-13 | `packages/ai/test/oauth.ts` | 85 | 7 |
| 2026-08-13 | `packages/ai/test/oauth-device-code.test.ts` | 145 | 7 |
| 2026-08-13 | `packages/ai/test/anthropic-oauth.test.ts` | 144 | 7 |
| 2026-08-13 | `packages/ai/test/openai-codex-oauth.test.ts` | 486 | 7 |
| 2026-08-13 | `packages/ai/test/github-copilot-oauth.test.ts` | 570 | 7 |
| 2026-08-13 | `packages/ai/test/openrouter-oauth.test.ts` | 322 | 7 |
| 2026-08-13 | `packages/ai/test/kimi-coding-oauth.test.ts` | 270 | 7 |
| 2026-08-13 | `packages/ai/test/xai-oauth.test.ts` | 335 | 7 |
| 2026-08-13 | `packages/ai/test/radius-oauth.test.ts` | 129 | 7 |
| 2026-08-13 | `packages/ai/src/api/openai-completions.ts` | 1577 | 9 |
| 2026-08-13 | `packages/ai/src/api/openai-prompt-cache.ts` | 8 | 9 |
| 2026-08-13 | `packages/ai/test/openai-completions-tool-choice.test.ts` | 1843 | 9 |
| 2026-08-13 | `packages/ai/test/openai-completions-empty-tools.test.ts` | 304 | 9 |
| 2026-08-13 | `packages/ai/test/openai-completions-prompt-cache.test.ts` | 274 | 9 |
| 2026-08-13 | `packages/ai/test/openai-completions-cache-control-format.test.ts` | 231 | 9 |
| 2026-08-13 | `packages/ai/test/openai-completions-thinking-as-text.test.ts` | 222 | 9 |
| 2026-08-13 | `packages/ai/test/openai-completions-tool-result-images.test.ts` | 156 | 9 |
| 2026-08-13 | `packages/ai/test/openai-completions-response-model.test.ts` | 140 | 9 |
| 2026-08-13 | `packages/ai/test/openai-completions-retry.test.ts` | 139 | 9 |
| 2026-08-13 | `packages/ai/test/openai-completions-thinking-token-budget.test.ts` | 124 | 9 |
| 2026-08-13 | `packages/ai/test/openai-completions-reasoning-details.test.ts` | 118 | 9 |
| 2026-08-13 | `packages/ai/test/openai-completions-raw-stop-reason.test.ts` | 79 | 9 |
| 2026-08-13 | `packages/ai/src/api/openai-responses-shared.ts` | 792 | 9 |
| 2026-08-13 | `packages/ai/src/api/openai-responses.ts` | 372 | 9 |
| 2026-08-13 | `packages/ai/src/api/azure-openai-responses.ts` | 330 | 9 |
| 2026-08-13 | `packages/ai/test/azure-openai-base-url.test.ts` | 216 | 9 |
| 2026-08-13 | `packages/ai/test/azure-openai-responses-reasoning-replay.test.ts` | 141 | 9 |
| 2026-08-13 | `packages/ai/src/api/openai-codex-responses.ts` | 1662 | 9 |
| 2026-08-13 | `packages/ai/src/session-resources.ts` | 24 | 9 |
| 2026-08-13 | `packages/ai/src/api/google-shared.ts` | 419 | 10 |
| 2026-08-13 | `packages/ai/src/api/google-generative-ai.ts` | 521 | 10 |
| 2026-08-13 | `packages/ai/src/api/google-vertex.ts` | 596 | 10 |
| 2026-08-13 | `packages/ai/src/api/bedrock-converse-stream.ts` | 1188 | 10 |
| 2026-08-13 | `packages/ai/src/api/mistral-conversations.ts` | 931 | 10 |
| 2026-08-13 | `packages/ai/src/api/pi-messages.ts` | 433 | 10 |
| 2026-08-13 | `packages/ai/src/api/cloudflare.ts` | 15 | 10 |
| 2026-08-13 | `packages/ai/src/api/cloudflare-gateway-binding.ts` | 192 | 10 |
| 2026-08-13 | `packages/ai/test/cloudflare-gateway-binding.test.ts` | 332 | 10 |
| 2026-08-13 | `packages/ai/src/api/openrouter-images.ts` | 196 | 10 |
| 2026-08-13 | `packages/ai/src/images.ts` | 21 | 10 |
| 2026-08-13 | `packages/ai/src/images-api-registry.ts` | 53 | 10 |
| 2026-08-13 | `packages/ai/src/images-models.ts` | 275 | 10 |
| 2026-08-13 | `packages/ai/src/image-models.generated.ts` | 639 | 10 |
| 2026-08-13 | `packages/ai/src/providers/images/register-builtins.ts` | 50 | 10 |
| 2026-08-13 | `packages/ai/src/providers/openrouter-images.ts` | 22 | 10 |
| 2026-08-13 | `packages/ai/test/openrouter-images.test.ts` | 140 | 10 |
| 2026-08-13 | `packages/ai/test/images-models.test.ts` | 209 | 10 |
| 2026-08-13 | `packages/ai/src/models.generated.ts` | 124 | 5 |
| 2026-08-13 | `packages/ai/src/providers/all.ts` | 155 | 5 |
| 2026-08-13 | `packages/ai/src/providers/data/` (39 Dateien + Manifest) | ~592 KB | 5 |
| 2026-08-13 | `packages/ai/src/api/anthropic-messages.ts` | 1352 (vollständig) | 8 |
| 2026-08-13 | `packages/ai/src/api/simple-options.ts` | 86 | 8 |
| 2026-08-13 | `packages/ai/src/api/transform-messages.ts` | 223 | 8 |
| 2026-08-13 | `packages/ai/src/providers/faux.ts` | 708 | 11 |
| 2026-08-13 | `packages/ai/src/providers/` (39 Factory-Dateien ohne `*.models.ts`) | 1672 | 11 |
| 2026-08-13 | `packages/ai/src/providers/anthropic.ts` | 59 | 11 |
| 2026-08-13 | `packages/ai/src/providers/amazon-bedrock.ts` | 90 | 11 |
| 2026-08-13 | `packages/ai/src/providers/google-vertex.ts` | 100 | 11 |
| 2026-08-13 | `packages/ai/src/providers/cloudflare-auth.ts` | 103 | 11 |
| 2026-08-13 | `packages/ai/src/providers/cloudflare-stream.ts` | 28 | 11 |
| 2026-08-13 | `packages/ai/src/providers/github-copilot.ts` | 34 | 11 |
| 2026-08-13 | `packages/ai/src/providers/radius.ts` | 82 | 11 |
| 2026-08-13 | `packages/ai/src/providers/radius-config.ts` | 96 | 11 |
| 2026-08-13 | `packages/ai/src/api/*.lazy.ts` (11 Wrapper) | 216 | 11 |
| 2026-08-13 | `packages/ai/src/auth/helpers.ts` (lazyOAuth) + `auth/oauth/load.ts` | 59 + 71 | 11 |
| 2026-08-13 | `packages/ai/src/cli.ts` | 119 | 11 |
| 2026-08-13 | `packages/ai/test/providers.test.ts` | 629 | 11 |
| 2026-08-13 | `packages/ai/test/xiaomi-models.test.ts` | 17 | 13 |
| 2026-08-13 | `packages/ai/test/openrouter-cache-control-models.test.ts` | 15 | 13 |
| 2026-08-13 | `packages/ai/test/together-models.test.ts` | 86 | 13 |
| 2026-08-13 | `packages/ai/test/bedrock-models.test.ts` | 70 | 13 |
| 2026-08-13 | `packages/ai/test/baseten-models.test.ts` | 141 | 13 |
| 2026-08-13 | `packages/ai/test/qwen-token-plan-models.test.ts` | 283 | 13 |
| 2026-08-13 | `packages/ai/test/fireworks-models.test.ts` | 343 | 13 |
| 2026-08-13 | `packages/ai/test/model-catalog-types.test.ts` | 15 | 13 |
| 2026-08-13 | `packages/ai/test/model-data-validation.test.ts` | 177 | 13 |
| 2026-08-13 | `packages/ai/test/reasoning-options.test.ts` | 36 | 13 |
| 2026-08-13 | `packages/ai/test/anthropic-adaptive-thinking-models.test.ts` | 40 | 13 |
| 2026-08-13 | `packages/ai/test/anthropic-temperature-compat.test.ts` | 103 | 13 |
| 2026-08-13 | `packages/ai/test/anthropic-force-adaptive-thinking.test.ts` | 123 | 13 |
| 2026-08-13 | `packages/ai/test/anthropic-empty-thinking-signature-compat.test.ts` | 108 | 13 |
| 2026-08-13 | `packages/ai/test/anthropic-eager-tool-input-compat.test.ts` | 166 | 13 |
| 2026-08-13 | `packages/ai/test/anthropic-cache-write-1h-cost.test.ts` | 86 | 13 |
| 2026-08-13 | `packages/ai/test/anthropic-auth-token.test.ts` | 187 | 13 |
| 2026-08-13 | `packages/ai/test/bedrock-endpoint-resolution.test.ts` | 208 | 13 |
| 2026-08-13 | `packages/ai/test/bedrock-credentials.test.ts` | 115 | 13 |
| 2026-08-13 | `packages/ai/test/bedrock-custom-headers.test.ts` | 202 | 13 |
| 2026-08-13 | `packages/ai/test/bedrock-raw-stop-reason.test.ts` | 82 | 13 |
| 2026-08-13 | `packages/ai/test/google-shared-gemini3-unsigned-tool-call.test.ts` | 167 | 13 |
| 2026-08-13 | `packages/ai/test/google-shared-signed-empty-blocks.test.ts` | 117 | 13 |
| 2026-08-13 | `packages/ai/test/google-shared-image-tool-result-routing.test.ts` | 102 | 13 |
| 2026-08-13 | `packages/ai/test/google-thinking-signature.test.ts` | 38 | 13 |
| 2026-08-13 | `packages/ai/test/google-shared-retry.test.ts` | 40 | 13 |
| 2026-08-13 | `packages/ai/src/index.ts` | 47 | 13 |
| 2026-08-13 | `packages/ai/src/utils/typebox-helpers.ts` | 24 | 13 |
| 2026-08-13 | `packages/ai/src/compat/extension-oauth-types.ts` | 45 | 13 |
| 2026-08-13 | `packages/ai/test/lax-message-content.test.ts` | 67 | 13 |
| 2026-08-13 | `packages/ai/test/fetch-option.test.ts` | 178 | 13 |
| 2026-08-13 | `packages/ai/test/mistral-reasoning-mode.test.ts` | 97 | 13 |
| 2026-08-13 | `packages/ai/test/mistral-tool-schema.test.ts` | 63 | 13 |
| 2026-08-13 | `packages/ai/test/mistral-raw-stop-reason.test.ts` | 61 | 13 |
| 2026-08-13 | `packages/ai/test/mistral-http-transport.test.ts` | 427 | 13 |
| 2026-08-13 | `packages/ai/test/error-body.test.ts` | 226 | 13 |
| 2026-08-13 | `packages/ai/test/max-thinking.test.ts` | 89 | 13 |
| 2026-08-13 | `packages/ai/test/compat-env.test.ts` | 74 | 13 |
| 2026-08-13 | `packages/ai/test/openai-responses-empty-tool-result.test.ts` | 58 | 13 |
| 2026-08-13 | `packages/ai/test/openai-responses-message-id.test.ts` | 48 | 13 |
| 2026-08-13 | `packages/ai/test/openai-responses-foreign-toolcall-id.test.ts` | 66 | 13 |
| 2026-08-13 | `packages/ai/test/provider-error-body-regression.test.ts` | 213 | 13 |
| 2026-08-13 | `packages/ai/test/provider-error-body-passthrough.test.ts` | 78 | 13 |
| 2026-08-13 | `packages/ai/test/github-copilot-anthropic.test.ts` | 127 | 13 |
| 2026-08-13 | `packages/ai/test/google-raw-stop-reason.test.ts` | 106 | 13 |
| 2026-08-13 | `packages/ai/test/cloudflare-stream.test.ts` | 64 | 11 |
| 2026-08-13 | `packages/ai/src/api/openai-completions.ts` (Struktur + Compat-Matrix) | 1577 (Compat 134 vollständig) | 9 |
| 2026-08-13 | `packages/ai/test/anthropic-sse-parsing.test.ts` | 424 | 8 |
| 2026-08-14 | `packages/ai/test/sampling-options.test.ts` | 128 | 13 |
| 2026-08-14 | `packages/ai/test/telemetry-options.test.ts` | 171 | 13 |
| 2026-08-14 | `packages/ai/test/xai-responses.test.ts` | 117 | 13 |
| 2026-08-14 | `packages/ai/test/openai-responses-namespace.test.ts` | 224 | 13 |
| 2026-08-14 | `packages/ai/test/openai-responses-partial-json-cleanup.test.ts` | 106 | 13 |
| 2026-08-14 | `packages/ai/test/transform-messages-copilot-openai-to-anthropic.test.ts` | 191 | 13 |
| 2026-08-14 | `packages/ai/test/bedrock-convert-messages.test.ts` | 410 | 13 |
| 2026-08-14 | `packages/ai/test/bedrock-error-metadata.test.ts` | 212 | 13 |
| 2026-08-14 | `packages/ai/test/deferred-tools.test.ts` | 550 | 13 |
| 2026-08-14 | `packages/ai/test/lazy-module-load.test.ts` | 113 | 13 |
| 2026-08-14 | `packages/ai/test/stream.test.ts` | 1747 | 13 |
| 2026-08-14 | `packages/ai/test/abort.test.ts` | 351 | 13 |
| 2026-08-14 | `packages/ai/test/empty.test.ts` | 822 | 13 |
| 2026-08-14 | `packages/ai/test/tokens.test.ts` | 365 | 13 |
| 2026-08-14 | `packages/ai/test/total-tokens.test.ts` | 882 | 13 |
| 2026-08-14 | `packages/ai/test/unicode-surrogate.test.ts` | 839 | 13 |
| 2026-08-14 | `packages/ai/test/context-overflow.test.ts` | 771 | 13 |
| 2026-08-14 | `packages/ai/test/cross-provider-handoff.test.ts` | 522 | 13 |
| 2026-08-14 | `packages/ai/test/image-tool-result.test.ts` | 550 | 13 |
| 2026-08-14 | `packages/ai/test/tool-call-without-result.test.ts` | 359 | 13 |
| 2026-08-14 | `packages/ai/test/tool-call-id-normalization.test.ts` | 290 | 13 |
| 2026-08-14 | `packages/ai/test/cache-retention.test.ts` | 532 | 13 |
| 2026-08-14 | `packages/ai/test/bedrock-thinking-payload.test.ts` | 266 | 13 |
| 2026-08-14 | `packages/ai/test/anthropic-opus-4-8-smoke.test.ts` | 71 | 13 |
| 2026-08-14 | `packages/ai/test/anthropic-thinking-disable.test.ts` | 172 | 13 |
| 2026-08-14 | `packages/ai/test/anthropic-tool-name-normalization.test.ts` | 204 | 13 |
| 2026-08-14 | `packages/ai/test/anthropic-eager-tool-input-e2e.test.ts` | 155 | 13 |
| 2026-08-14 | `packages/ai/test/anthropic-long-cache-retention-e2e.test.ts` | 127 | 13 |
| 2026-08-14 | `packages/ai/test/interleaved-thinking.test.ts` | 144 | 13 |
| 2026-08-14 | `packages/ai/test/xiaomi-token-plan-ams-anthropic-empty-signature-smoke.test.ts` | 114 | 13 |
| 2026-08-14 | `packages/ai/test/xhigh.test.ts` | 70 | 13 |
| 2026-08-14 | `packages/ai/test/responseid.test.ts` | 120 | 13 |
| 2026-08-14 | `packages/ai/test/zen.test.ts` | 25 | 13 |
| 2026-08-14 | `packages/ai/test/openai-codex-cache-affinity-e2e.test.ts` | 35 | 13 |
| 2026-08-14 | `packages/ai/test/openai-responses-cache-affinity-e2e.test.ts` | 31 | 13 |
| 2026-08-14 | `packages/ai/test/openai-responses-reasoning-replay-e2e.test.ts` | 291 | 13 |
| 2026-08-14 | `packages/ai/test/openai-responses-tool-result-images.test.ts` | 194 | 13 |
| 2026-08-14 | `packages/ai/test/openrouter-cache-write-repro.test.ts` | 77 | 13 |
| 2026-08-14 | `packages/ai/test/google-thinking-disable.test.ts` | 160 | 13 |
| 2026-08-14 | `packages/ai/test/oauth.ts` | 85 | 13 |
| 2026-08-14 | `packages/ai/test/azure-utils.ts` | 28 | 13 |
| 2026-08-14 | `packages/ai/test/bedrock-utils.ts` | 18 | 13 |
| 2026-08-14 | `packages/ai/test/cloudflare-utils.ts` | 9 | 13 |
| 2026-08-14 | `packages/ai/test/scratch.ts` | 57 | 13 |
| 2026-08-14 | `packages/ai/test/codex-websocket-cached-probe.ts` | 299 | 13 |
| 2026-08-14 | `packages/ai/test.sh` (Referenz: Key-Isolation des kanonischen Laufs) | 120 | 13 |
| 2026-08-14 | `packages/ai/src/bedrock-provider.ts` | 6 | 13 |
| 2026-08-14 | `packages/ai/src/bun-oauth.ts` | 21 | 13 |
| 2026-08-14 | `packages/ai/src/oauth.ts` | 10 | 13 |
| 2026-08-14 | `packages/ai/src/image-models.ts` | 42 | 13 |

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| `src/types.ts` | 830 | `types.rs` | verifiziert (Task 1) | Klasse 1: `Api`/`ProviderId`/`ImagesApi`/`ImagesProviderId` sind `String` (TS `KnownX \| (string & {})` ist zur Laufzeit ein String); bekannte Werte als `KNOWN_*`-Konstanten. Klasse 1: `ApiOptionsMap`/`ApiStreamOptions` sind reine Typ-Ebene und entfallen — die Dispatch-Form ist `StreamOptions`/`SimpleStreamOptions`, konkrete Optionstypen liegen bei den API-Modulen. Klasse 1: `Model.compat` wird api-abhängig deserialisiert (`ModelCompat`), da Rust keine bedingten Typen kennt; APIs ohne Zuordnung behalten den Rohwert (`ModelCompat::Other`). Klasse 1: Content-Blöcke tragen `extra: Map` (JS-Objektoffenheit) — erhält Scratch-Felder abgebrochener Streams (`partialJson`) verlustfrei, bug-compat. Klasse 1: `Usage.total_tokens` ist optional (historische TS-Session-Dateien enthalten das Feld nicht; `estimate.ts` behandelt `undefined`/`0` gleich). Klasse 3: `signal: AbortSignal` → `CancellationToken`; `fetch` → `FetchFn`-Trait; TypeBox-`TSchema` → `serde_json::Value`. Klasse 1: `js_number` bildet `JSON.stringify`-Zahlformatierung nach (`0` statt `0.0`). |
| `src/utils/diagnostics.ts` | 45 | `utils/diagnostics.rs` | portiert (Task 3) | Klasse 1: TS unterscheidet `Error`-Instanzen von beliebig geworfenen Werten; in Rust trennen das zwei Funktionen (`extract_diagnostic_error`, `thrown_value_diagnostic`). `stack` gibt es nicht |
| `src/utils/event-stream.ts` | 88 | `utils/event_stream.rs` | verifiziert (Task 3) | Klasse 1: Waiter-Liste als `tokio::sync::Notify`; `result()` klont je Aufruf statt dieselbe Referenz zu liefern |
| `src/utils/uuid.ts` | 48 | `utils/uuid.rs` | verifiziert (Task 3) | Klasse 1: Modulzustand als `Mutex`; Uhr und Zufall sind für den portierten Test injizierbar (TS stubbt `Date.now`/`crypto`) |
| `src/utils/json-parse.ts` | 124 | `utils/json_parse.rs` | verifiziert (Task 3) | Klasse 3: `partial-json` (220 LOC) mitportiert, immer mit `Allow.ALL`. Klasse 1: Indizes zählen Unicode-Skalare statt UTF-16-Einheiten (alle strukturellen Zeichen sind ASCII); `NaN`/`Infinity` werden zu `null` wie bei `JSON.stringify`; Zahlen werden auf JS-Semantik normalisiert (ein f64-Typ) |
| `src/utils/retry.ts` | 228 | `utils/retry.rs` | verifiziert (Task 3) | Klasse 3: `AbortSignal` → `CancellationToken`; Klasse 1: Callbacks als `Arc<dyn Fn>` |
| `src/utils/provider-retry.ts` | 125 | `utils/provider_retry.rs` | verifiziert (Task 3) | Klasse 3: Die SDK-Fehlerform wird zum Trait `ProviderErrorInfo`, das die HTTP-Schicht implementiert |
| `src/utils/overflow.ts` | 180 | `utils/overflow.rs` | verifiziert (Task 3) | alle 25 Overflow- und 3 Nicht-Overflow-Muster in Quellreihenfolge |
| `src/utils/estimate.ts` | 143 | `utils/estimate.rs` | verifiziert (Task 3) | Klasse 1: Zeichenlängen zählen UTF-16-Einheiten wie `String.length` |
| `src/utils/validation.ts` | 350 | `utils/validation.rs` | verifiziert (Task 3) | Klasse 3: TypeBox `Compile().Check/.Errors` und `Value.Convert` mitportiert, inklusive der AJV-Meldungen. Klasse 1: TypeBox markiert seine Schemas mit einem Laufzeit-Symbol ohne JSON-Entsprechung — die Herkunft wird explizit übergeben (`SchemaOrigin`, Default `TypeBox`, weil alle App-Tools so definiert sind); empirisch belegt: `Value.Convert` ist für Plain-Schemas eine No-Op |
| `src/utils/text.ts` | 12 | `utils/text.rs` | verifiziert (Task 3) | Klasse 1: Überladung über `ContentRef` statt einer union-typisierten Signatur |
| `src/utils/hash.ts` | 13 | `utils/hash.rs` | portiert (Task 3) | Klasse 1: `charCodeAt` → `encode_utf16`; `toString(36)` nachgebildet |
| `src/utils/headers.ts` | 18 | `utils/headers.rs` | portiert (Task 3) | |
| `src/utils/sanitize-unicode.ts` | 25 | `utils/sanitize_unicode.rs` | portiert (Task 3) | Klasse 1: Rust-Strings können keine unpaarigen Surrogate enthalten; die Funktion ist die Identität, die UTF-16-Variante bleibt für Provider-Payloads |
| `src/utils/abort.ts` + `abort-signals.ts` | 91 | `utils/abort.rs` | portiert (Task 3) | Klasse 3: `AbortController` → `CancellationToken`; `combineAbortSignals` leitet über eine Task weiter, `cleanup()` bricht sie ab |
| `src/utils/fetch.rs` (Rust-eigen) | — | `utils/fetch.rs` | portiert (Task 8) | Klasse 3: `ReqwestFetch` als Default-Implementierung von `FetchFn`; Antwortkörper als Chunk-Strom für den SSE-Decoder |
| `src/utils/error-body.ts` | 149 | `utils/error_body.rs` | portiert (Task 3) | Klasse 3: Statt SDK-Feldnamen zu erraten, liefert die HTTP-Schicht `RawProviderError`; Tests folgen in Task 13 |
| `src/utils/deferred-tools.ts` | 39 | `utils/deferred_tools.rs` | portiert (Task 3) | Tests folgen in Task 13 |
| `src/utils/provider-env.ts` | 52 | `utils/provider_env.rs` | portiert (Task 3) | Klasse 4: Der Bun-Sandbox-Fallback (`/proc/self/environ`, oven-sh/bun#27802) entfällt |
| `src/utils/node-http-proxy.ts` | 112 | `utils/node_http_proxy.rs` | verifiziert (Task 3) | Klasse 3: liefert die Proxy-URL für reqwest statt eines undici-Agents |
| `src/model-catalog.ts` | 27 | `model_catalog.rs` | verifiziert (Task 5) | Klasse 1: Die TS-Typmagie (`ModelCatalog<TGroups, TProvider>`) ist reine Compile-Zeit-Inferenz; zur Laufzeit bleibt das flache Mergen der API-Gruppen |
| `src/models.generated.ts` | 124 | `model_catalog.rs` | portiert (Task 5) | Klasse 3: Die 39 `*.models.ts`-Aggregatoren entfallen; die JSON-Dateien werden per `include_str!` eingebettet und einmalig geparst |
| `src/providers/data/` | 592 KB | `data/` | verifiziert (Task 5) | Byte-identisch übernommen (SHA-256 je Datei gegen das Manifest getestet, 1 224 Modelle über 39 Provider). `.manifest.json` heißt `manifest.json`. Substitution: `scripts/generate-models.ts` wird laut Plan nicht portiert |
| `src/providers/all.ts` | 155 | `model_catalog.rs` (Katalogteil), `providers/all.rs` | verifiziert (Task 11) | `builtinProviders()`/`builtinModels()` samt aller 39 Factories; Differential-Fixture `tests/fixtures/providers.jsonl` |
| `src/providers/` (33 mechanische Factories: ant-ling, azure-openai-responses, baseten, cerebras, deepseek, fireworks, google, groq, huggingface, kimi-coding, minimax(-cn), mistral, moonshotai(-cn), nvidia, openai, openai-codex, opencode(-go), openrouter, qwen-token-plan(-cn/-individual), together, vercel-ai-gateway, xai, xiaomi(+3 token-plan), zai(-coding-cn)) | 15–28 je Datei | `providers/<name>.rs` | verifiziert (Task 11) | Klasse 1: `Object.values(X_MODELS)` wird `get_builtin_models(id)` — die `*.models.ts` sind generierte Leser desselben `data/`-Snapshots und haben kein eigenes Modul. Klasse 4: `lazyOAuth`/`load.ts` entfallen; die OAuth-Flows werden direkt referenziert (Name, `isSubscription` und `loginLabel` der Flows sind identisch mit den Werten, die `lazyOAuth` überschrieb — durch die Fixture belegt) |
| `src/providers/anthropic.ts` | 59 | `providers/anthropic.rs` | verifiziert (Task 11) | Klasse 1: leere Strings gelten wie in JS als „nicht gesetzt" (`if (credential?.key)`) |
| `src/providers/amazon-bedrock.ts` | 90 | `providers/amazon_bedrock.rs` | verifiziert (Task 11) | Klasse 1: die `env()`-Closure des Originals wird ein async-Closure mit `Result`, weil Abbruch in Rust ein Fehlerwert statt eines Throws ist |
| `src/providers/google-vertex.ts` | 100 | `providers/google_vertex.rs` | verifiziert (Task 11) | wie oben; `??`-Ketten über `ctx.env` werden sequentielle `match`-Ausdrücke, damit die Aufrufreihenfolge (und damit die Abbruchpunkte) erhalten bleibt |
| `src/providers/cloudflare-auth.ts` | 103 | `providers/cloudflare_auth.rs` | verifiziert (Task 11) | Klasse 1: `kind` als Enum statt String-Union |
| `src/providers/cloudflare-stream.ts` | 28 | `providers/cloudflare_stream.rs` | verifiziert (Task 11) | — (der Wrapper reicht `fetchDeferred`/`cancelDeferred` wie im Original nicht durch) |
| `src/providers/github-copilot.ts` | 34 | `providers/github_copilot.rs` | verifiziert (Task 11) | — (`availableModelIds` muss wie in TS vollständig aus Strings bestehen, sonst bleibt der Katalog unverändert) |
| `src/providers/radius-config.ts` | 96 | `providers/radius_config.rs` | verifiziert (Task 11) | Klasse 1: die validierten Gateway-Modelle bleiben JSON-Maps, weil TS das Rohobjekt spreizt; ein Eintrag, dem ein `Model`-Pflichtfeld fehlt, wird verworfen statt als ungültiges Modell durchgereicht (TS verlässt sich hier auf eine Typzusicherung). `normalizeRadiusGatewayUrl` liegt hier statt in `auth/oauth/radius.rs` — der frühere Port dort trimmte Whitespace und akzeptierte jedes Schema, was gegen `/^https?:\/\//iu` verstieß und mit dieser Portierung korrigiert wurde |
| `src/providers/radius.ts` | 82 | `providers/radius.rs` | verifiziert (Task 11) | Klasse 1: eigener `Provider`-Impl statt `createProvider`, weil der Refresh die Cache-Migration selbst mitbringt; Katalog hinter `Mutex` |
| `src/providers/all.ts` | 155 | `providers/all.rs` | verifiziert (Task 11) | Klasse 1: die katalogseitigen Exporte (`getBuiltinModel(s)`, `getBuiltinProviders`, `getBuiltinModelDataGeneratedAt`) liegen bei den Daten in `model_catalog.rs` und werden hier re-exportiert; `builtinImagesProviders`/`builtinImagesModels` liegen bei `images_models.rs` neben `createImagesProvider` |
| `src/providers/openrouter-images.ts` | 22 | `images_models.rs` (`openrouter_images_provider`) | verifiziert (Task 10) | Klasse 1: neben `create_images_provider` abgelegt statt in einer eigenen Datei |
| `src/api/*.lazy.ts` (11 Wrapper) | 216 | `api/streams.rs` | verifiziert (Task 11) | Klasse 4: die Wrapper existieren, damit ein Bundler das dynamisch importierte Modul aus Browser-Builds schneiden kann; Rust linkt statisch, also bleibt je Wrapper eine Unit-Struct, die in ihr Modul weiterleitet. `setBedrockProviderModule`/`registerBundledOAuthFlowLoaders` (Bun-Binary-Registrierung) entfallen aus demselben Grund |
| `test/xiaomi-models.test.ts`, `openrouter-cache-control-models.test.ts`, `together-models.test.ts`, `baseten-models.test.ts`, `qwen-token-plan-models.test.ts`, `fireworks-models.test.ts`, offline-Teil von `bedrock-models.test.ts` | 955 | `tests/model_data.rs` | portiert (Task 13) | Klasse 1: die TS-Suites lesen über das ausgeschlossene `compat.ts` (`getModel`/`getModels`) — der Port liest dieselben Daten über `model_catalog`. Klasse 1: Payload-Zusicherungen erfassen den serialisierten Request-Body über ein injiziertes `fetch` statt über `onPayload` + geworfenen Fehler (dasselbe Objekt). Die key-gated Bedrock-Modellläufe (`BEDROCK_EXTENSIVE_MODEL_TEST`) sind wie in TS nicht Teil des Standardlaufs |
| `test/bedrock-endpoint-resolution.test.ts`, `bedrock-credentials.test.ts`, `bedrock-custom-headers.test.ts`, `bedrock-raw-stop-reason.test.ts` | 607 | `tests/bedrock_converse_stream.rs` (Anbau) | portiert (Task 13) | Klasse 1: die TS-Suites lesen die Konfiguration des gemockten SDK-Clients — hier ist `build_client_config` genau diese Konfiguration. Fälle, die `process.env` pro Test stubben, laufen über das provider-scoped `env` (ein Rust-Testprozess teilt sich eine Prozessumgebung über alle Threads). Die Header-Middleware ist als `applicable_custom_headers` prüfbar; ihre Registrierung im Interceptor-Stack ist ohne echten SDK-Aufruf nicht beobachtbar |
| `test/google-shared-gemini3-unsigned-tool-call.test.ts`, `google-shared-signed-empty-blocks.test.ts`, `google-shared-image-tool-result-routing.test.ts`, `google-thinking-signature.test.ts`, `google-raw-stop-reason.test.ts` | 530 | `tests/google_shared.rs` | portiert (Task 13) | Klasse 1: beide Adapter teilen sich `GoogleStreamState`, also deckt eine Zustandsmaschine den Generative-AI- und den Vertex-Fall ab |
| `test/openai-responses-empty-tool-result.test.ts`, `openai-responses-message-id.test.ts`, `openai-responses-foreign-toolcall-id.test.ts` | 172 | `tests/openai_responses_convert.rs` | portiert (Task 13) | — |
| `test/provider-error-body-regression.test.ts`, `provider-error-body-passthrough.test.ts` | 291 | `tests/provider_error_body.rs` | portiert (Task 13) | Klasse 3: statt eines SDK-Mocks antwortet das injizierte `fetch` mit genau der Nicht-2xx-Antwort, deren Body die SDK-Meldung verdeckt. Der Bedrock-Teil deckt `tests/bedrock_converse_stream.rs` über `format_bedrock_error` ab; der Bild-Teil liegt in `tests/images.rs` |
| `test/github-copilot-anthropic.test.ts` | 127 | `tests/anthropic_compat.rs` (Anbau) | portiert (Task 13) | Dabei aufgedeckt und behoben: der Anthropic-Adapter setzte die dynamischen Copilot-Header (`X-Initiator`, `Openai-Intent`, Vision) nicht — `openai-completions` und `openai-responses` taten es bereits |
| Katalog-Hälfte von `test/max-thinking.test.ts` | 89 | `tests/model_data.rs` (Anbau) | portiert (Task 13) | Der Codex-Payload-Fall (`reasoning: { effort: "max" }`) liegt in `tests/openai_codex_responses.rs` |
| `test/error-body.test.ts` | 226 | `tests/error_body.rs` | portiert (Task 13) | Klasse 3: die vier SDK-Fehlerformen werden zu dem `RawProviderError`, den die HTTP-Schicht daraus bauen würde; die Abwehrfälle gegen serialisierte SDK-Interna (Node-Streams, Wrapper-Klassen) kollabieren zu „die Schicht liefert keinen Body" — genau das, was sie zusichern |
| `test/lax-message-content.test.ts` | 67 | `tests/types_serde.rs` (Anbau) | portiert (Task 13) | Klasse 1 + bug-compat: TS normalisiert `null`/fehlendes `content` erst in `transformMessages`, weil unsauber gebaute Historien und alte Session-Dateien es enthalten (#6259, #6276). In Rust ist dieser Zustand nicht darstellbar, also greift dieselbe Nachsicht beim Deserialisieren (`lax_content`) — das Ergebnis ist derselbe Zustand, den TS vor dem Request erreicht |
| `test/mistral-reasoning-mode.test.ts`, `mistral-tool-schema.test.ts`, `mistral-raw-stop-reason.test.ts`, `mistral-http-transport.test.ts` | 648 | `tests/mistral_conversations.rs` | abgedeckt (Task 13) | Die 46 Fälle der Differential-Fixture enthalten `prompt-mode-reasoning`, `reasoning-effort`, `prompt-cache(-disabled/-explicit-affinity)`, `tools-strict`, `tool-call-id-normalization`, die Finish-Reason-Matrix und beide HTTP-Fehlerformen — dieselben Zusicherungen aus dem TS-Original, gegen aufgezeichnete Request-Bodies und Event-Folgen |
| `test/fetch-option.test.ts` | 178 | `tests/*` (adapterweise) | teilweise abgedeckt (Task 13) | Alle portierten Adapter nehmen ihr `fetch` über `ProviderRequestOptions` entgegen — jede Fixture-Suite fährt ausschließlich darüber, ein ambientes `fetch` gibt es im Port nicht. Klasse 3: die beiden Google-Fälle prüfen, dass der Adapter ein eigenes `fetch` ABLEHNT — das gilt nur, weil das `@google/genai`-SDK keines annimmt; der Port baut die Requests selbst und unterstützt es, weshalb diese zwei Fälle keine Entsprechung haben |
| `test/google-shared-retry.test.ts` | 40 | `tests/event_stream_and_provider_retry.rs` | abgedeckt (Task 13) | Klasse 3: `retryGoogleRequest` existiert in TS nur, um SDK-Fehlern ohne `headers` eine retry-fähige Form zu geben; in Rust liefert `ProviderErrorInfo` den Status direkt, die Google-Adapter rufen `retry_provider_request` unmittelbar auf. Die drei Fälle (Retry bei 429 mit `maxRetries`, kein Retry ohne, kein Retry bei 400) prüft die Retry-Suite |
| `test/anthropic-adaptive-thinking-models.test.ts`, `anthropic-temperature-compat.test.ts`, `anthropic-force-adaptive-thinking.test.ts`, `anthropic-empty-thinking-signature-compat.test.ts`, `anthropic-eager-tool-input-compat.test.ts`, `anthropic-cache-write-1h-cost.test.ts`, `anthropic-auth-token.test.ts` | 813 | `tests/anthropic_compat.rs` | portiert (Task 13) | Klasse 1: statt lokalem HTTP-Server bzw. SDK-Konstruktor-Mock beobachtet ein injiziertes `fetch` denselben Request (Header und Body). Dabei aufgedeckt und behoben: `sessionId` erreichte die Anthropic-Optionen nicht, wodurch der `x-session-affinity`-Header fehlte |
| `src/utils/typebox-helpers.ts` | 24 | `utils/typebox_helpers.rs` | verifiziert (Task 13) | Klasse 3: TypeBox-Schemas sind `serde_json::Value`, also entfällt der `TUnsafe`-Wrapper; das erzeugte JSON-Schema ist identisch |
| `src/compat/extension-oauth-types.ts` | 45 | `compat/extension_oauth_types.rs` | portiert (Task 13) | Klasse 1: `OAuthLoginCallbacks` wird ein Trait; optionale Callbacks sind Default-Methoden, `onSelect` liefert `Option<String>` statt `string \| undefined`. Nur diese Datei aus `compat/` ist portiert — sie steht in `index.ts` und die coding-agent-Extension-API ist dagegen typisiert |
| `src/index.ts` | 47 | `lib.rs` | verifiziert (Task 13) | Klasse 1: Rust-Module sind öffentlich, zusätzlich re-exportiert `lib.rs` die Symbole, die `index.ts` flach exportiert. Klasse 3: `Type`/`Static`/`TSchema` (TypeBox) haben keine Entsprechung — Schemas sind `serde_json::Value` |
| `src/cli.ts` | 119 | `src/bin/notagent-ai.rs` | verifiziert (Task 11) | Klasse 4: der npm-Bin (`dist/cli.js`) wird ein Cargo-Bin-Target; die Usage-Zeile nennt entsprechend `notagent-ai` statt `npx @notagent/ai`. Klasse 1: `readline` wird ein gepufferter Stdin-Reader; die Ausgaben von `list`/`help` sind zeichengleich mit dem TS-Original (verglichen) |
| `src/providers/faux.ts` | 708 | `providers/faux.rs` | verifiziert (Task 11) | Klasse 1: Skript-Schritte als Enum (`Message`/`Factory`) statt einer TS-Union aus Wert und Funktion; Zustand hinter `Arc<Mutex<..>>`. Klasse 1: `structuredClone` der Submission-Options entfällt — beim Auflösen eines Deferred-Handles zählt nur der Kontext, wie in TS nach dem Entfernen von `deferred`/`signal`/`onResponse` |
| `src/models.ts` | 944 | `models.rs` | portiert (Task 4) | Klasse 1: `Provider`/`Models` werden Traits bzw. eine Struktur; `provider.refreshModels !== undefined` ist zur Laufzeit nicht prüfbar und wird zu `Provider::is_dynamic()`. Klasse 1: Publikationsketten und Refresh-Controller nutzen async-Mutex und `CancellationToken` statt Promise-Ketten und `AbortController`. Klasse 1: `getAuth` ist in `get_auth_for_provider`/`get_auth_for_model` geteilt (kein Overloading). `login` persistiert außerhalb des Abbruchpfads, damit ein Abbruch während des Schreibens die Credential nicht verliert |
| `src/models-store.ts` | 45 | `models_store.rs` | portiert (Task 4) | |
| `src/api/anthropic-messages.ts` (SSE-Decoder, Z. 300-430) | 130 | `api/sse.rs` | verifiziert (Task 8) | Laut Plan als generisches Modul herausgezogen, weil alle SSE-APIs es nutzen. Differenziell gegen die TS-Funktionen geprüft (355 Fälle, jede Bruchstelle) |
| `src/api/anthropic-messages.ts` (Antwortseite: Event-Zustandsmaschine, Usage, StopReason) | ~450 von 1352 | `api/anthropic_messages.rs` | verifiziert (Task 8) | Master-Architektur: Scratch-Felder (`index`, `partialJson`) leben im Streaming-State und erreichen die Content-Typen nicht; Snapshots werden je Event geklont. Request-Bau und HTTP-Transport folgen im selben Task |
| `src/api/anthropic-messages.ts` (Transport: HTTP, SSE-Anbindung, Fehlerpfade) | ~300 von 1352 | `api/anthropic_messages.rs` (Transport-Abschnitt) | verifiziert (Task 8) | Klasse 3: reqwest statt SDK-Client; `assertRequestAuth`, Bearer- vs. x-api-key-Auth, `onPayload`/`onResponse`, HTTP-Fehlerkörper und die Terminalprüfungen wie im Original. Der Request läuft wie in TS in `retryProviderRequest` (SDK mit `maxRetries: 0`), sodass `onResponse` nur die endgültige Antwort sieht |
| `src/api/anthropic-messages.ts` (Requestseite: buildParams, convertMessages, convertTools, Header) | ~600 von 1352 | `api/anthropic_params.rs` | verifiziert (Task 8) | Payload-Snapshot-Tests gegen 18 vom TS-Original erzeugte Request-Bodies (onPayload-Hook). Klasse 3: Der SDK-Client entfällt, die Default-Header werden direkt gebaut |
| `src/api/openai-completions.ts` (Compat-Matrix `detectCompat`/`getCompat`) | 134 von 1577 | `api/openai_completions_compat.rs` | verifiziert (Task 9) | Vollständige Auto-Detection nach Provider und baseUrl plus feldweise Overrides |
| `src/api/constrained-sampling.ts` | 277 | `api/constrained_sampling.rs` | verifiziert (Task 8) | Klasse 1: `structuredClone` + In-place-Mutation → Klon plus rekursive Umschreibung; Fehler als `Result` statt `throw` |
| `src/api/github-copilot-headers.ts` | 37 | `api/github_copilot_headers.rs` | portiert (Task 8) | |
| `src/api/simple-options.ts` | 86 | `api/simple_options.rs` | verifiziert (Task 8) | Klasse 1: Token-Breiten vereinheitlicht auf `u64` (JS kennt nur einen Zahlentyp), siehe interface-requests B-3 |
| `src/api/transform-messages.ts` | 223 | `api/transform_messages.rs` | verifiziert (Task 8) | Klasse 1: Die Normalisierung von `content == null` entfällt — die Rust-Typen garantieren das Array bereits. `Date.now()` für synthetische Tool-Ergebnisse wird übergeben |
| `src/api/lazy.ts` | 98 | `api/lazy.rs` | portiert (Task 4) | Klasse 4: `lazyApi` entfällt — es kapselt einen dynamischen `import()` für Bundler; Rust linkt statisch |
| `src/auth/types.ts` | 240 | `auth/types.rs` | portiert (Task 6) | Klasse 1: Interfaces mit Methoden werden Traits (`CredentialStore`, `AuthContext`, `ApiKeyAuth`, `OAuthAuth`, `AuthInteraction`); `modify` nimmt eine `FnOnce`, die leihen darf, damit der OAuth-Refresh unter dem Lock laufen kann. `OAuthCredential` behält Zusatzfelder (`extra`) wie die TS-Index-Signatur |
| `src/auth/credential-store.ts` | 67 | `auth/credential_store.rs` | verifiziert (Task 6) | Klasse 1: Serialisierung je Provider-ID über einen async-Mutex statt einer Promise-Kette |
| `src/auth/resolve.ts` | 205 | `auth/resolve.rs` | verifiziert (Task 6) | Klasse 3: `AbortSignal.any([signal, timeout])` → `child_token` plus Timeout-Task |
| `src/auth/context.ts` | 45 | `auth/context.rs` | portiert (Task 6) | Klasse 1: Die Browser-Guards (dynamischer `import`, fehlendes `process.env`) entfallen |
| `src/api/openai-completions.ts` (Request) | 1577 | `api/openai_completions_params.rs` | verifiziert (Task 9) | `buildParams`, `convertMessages`, `convertTools`, `cache_control`-Platzierung und Chat-Template-Auflösung; 102 Payloads aus dem TS-Original werden byte-identisch reproduziert. Klasse 1: Die Feldreihenfolge von `openRouterRouting`/`vercelGatewayRouting` folgt der Interface-Deklaration statt der Schlüsselreihenfolge der Modelldatei (typisierte Structs statt offener Objekte) |
| `src/api/openai-completions.ts` (Stream) | — | `api/openai_completions.rs` | verifiziert (Task 9) | Klasse 3: Der SDK-Iterator wird zum eigenen SSE-Modul; die SDK-`APIError`-Form (Status, `error`-Feld des Bodys, `makeMessage`) ist nachgebaut, damit `formatProviderError` denselben Text liefert. Klasse 1: Der Text eines nicht parsbaren Chunks stammt von serde statt von V8. Die Scratch-Puffer liegen im Streaming-State statt an den Blöcken (Master-Plan, Architektur) |
| `src/api/openai-codex-responses.ts` | 1662 | `api/openai_codex_responses.rs` | verifiziert (Task 9) | Request-Body, JWT-Account-ID, zstd-komprimierter SSE-Request mit eigener Retry-Schleife, WebSocket-Transport über tokio-tungstenite und der Rückfall auf SSE samt `provider_transport_failure`-Diagnose. Klasse 3: Der WebSocket-Verbindungs-Cache hält nur den Continuation-Zustand (`previous_response_id`), nicht den Socket — `WebSocketStream` ist nicht klonbar; beobachtbar bleibt der Delta-Request, es kostet eine zusätzliche TCP-Verbindung. Klasse 1: Der Idle-Timer wird beim nächsten Zugriff ausgewertet statt per Timer. 41 Request-Bodies, URLs und Event-Sequenzen aus dem TS-Original stimmen exakt |
| `src/session-resources.ts` | 24 | `session_resources.rs` | verifiziert (Task 9) | Klasse 1: Der Deregistrierungs-Callback wird ein Handle; die registrierten Cleanups können nicht fehlschlagen, daher entfällt der `AggregateError` |
| `src/api/google-shared.ts` | 419 | `api/google_shared.rs` | verifiziert (Task 10) | Nachrichten- und Tool-Konvertierung, Thought-Signature-Regeln (nur gleiches Modell, nur gültiges base64), Merge der Function-Responses in einen User-Turn. Klasse 1: `mapStopReason` und `mapStopReasonString` fallen zusammen, weil der REST-Port den Grund als String sieht; `retryGoogleRequest` entfällt, da der Transport `retry_provider_request` direkt aufruft |
| `src/api/google-generative-ai.ts` | 521 | `api/google_generative_ai.rs` | verifiziert (Task 10) | Klasse 3: Statt des `@google/genai`-SDK ein direkter REST-Call auf `models/{id}:streamGenerateContent?alt=sse` — genau die Anfrage, die das SDK stellt. Klasse 1: Die Feldreihenfolge im Request-Body ist die des Adapters; das SDK sortiert Parts nach seinem eigenen Schema um (der Test vergleicht daher strukturell). 40 Request-Bodies, URLs und Event-Sequenzen aus dem TS-Original stimmen |
| `src/api/google-vertex.ts` | 596 | `api/google_vertex.rs` | verifiziert (Task 10) | Endpunkt-Auflösung (API-Key vs. Projekt/Location vs. eigene Base-URL, inkl. `global`, den zwei multiregionalen Hosts und der Unterdrückung des Projektpfads) exakt wie `getBaseUrl`/`constructUrl` des SDK. Klasse 3: ADC liegt nicht in diesem Crate — der Bearer-Token kommt über einen injizierten `VertexTokenProvider`; ohne ihn meldet der Stream denselben Fehlertext wie das SDK. Thinking-Tabellen ohne Gemma-Zweig und ohne Flash-Lite-Eintrag wie im Original |
| `src/api/bedrock-converse-stream.ts` | 1188 | `api/bedrock_converse_stream.rs` | verifiziert (Task 10/11) | Der `ConverseStreamCommand`-Input wird als JSON gebaut — genau das Objekt, das `onPayload` in TS sieht — und anschließend auf die typisierten Builder von `aws-sdk-bedrockruntime` abgebildet (Klasse 3). Cache-Points, Thinking-Felder (Budget vs. adaptiv, GovCloud ohne `display`), Region-/Endpunkt-/Credential-Auflösung und die Fehler-Präfixe wie im Original. 46 Command-Inputs und 46 Event-Sequenzen aus dem TS-Original stimmen exakt. `stream`/`streamSimple` (Task 11) bauen den Client aus der aufgelösten Konfiguration: Profil/Region/Endpunkt/Credentials über `aws-config`, Bearer-Token über `bearer_token` + `auth_scheme_preference`, Caller-Header über einen `modify_before_signing`-Interceptor (entspricht dem TS-`build`-Step vor der Signatur), HTTP-Status für `onResponse` über einen `read_before_deserialization`-Interceptor (das Rust-SDK legt `$metadata.httpStatusCode` nicht auf das Output-Objekt). Klasse 3: die beiden TS-Zweige mit `NodeHttpHandler` (aufgelöster Proxy, `AWS_BEDROCK_FORCE_HTTP1=1`) installieren einen reqwest-basierten `HttpClient` mit `http1_only()` und explizitem Proxy — Node-Agents sprechen in beiden Fällen HTTP/1.1 |
| `src/api/mistral-conversations.ts` | 931 | `api/mistral_conversations.rs` | verifiziert (Task 10) | Klasse 3: `fetch` + eigener Event-Reader statt SSE-Decoder — der TS-Reader akzeptiert acht Trennerformen, die der gemeinsame Decoder nicht kennt. Klasse 1: Der Wire-Body wird direkt in der Schlüsselreihenfolge gebaut, die das TS-`remapMistralProperty` erzeugt (umbenannte Felder wandern ans Objektende). 46 Request-Bodies inkl. URL und Headern sowie 46 Event-Sequenzen aus dem TS-Original stimmen exakt |
| `src/api/pi-messages.ts` | 433 | `api/pi_messages.rs` | verifiziert (Task 10) | Der Backend-Stream liefert bereits serialisierte Assistant-Events; der Konverter baut nur die `partial`-Nachricht mit. Klasse 1: `{ ...event, partial }` schleift im JS auch Felder durch, die der deklarierte `AssistantMessageEvent`-Typ nicht kennt (`contentSignature`, `redacted`, `id`, `toolName`, `usage`, `rewrite`) — kein typisierter Konsument liest sie, die Daten erreichen ihn über `partial.content`; das typisierte Rust-Enum trägt nur die deklarierten Felder. Klasse 1: JS-Fehler tragen einen Stack, Rust-Fehler nicht. 20 Request-Bodies inkl. URL und Headern sowie 20 Event-Sequenzen stimmen exakt |
| `src/api/cloudflare.ts` | 15 | `api/cloudflare.rs` | verifiziert (Task 10) | reine Konstanten |
| `src/api/cloudflare-gateway-binding.ts` | 192 | `api/cloudflare_gateway_binding.rs` | verifiziert (Task 10) | Klasse 3: Das Workers-Binding lebt in der JS-Laufzeit — der Port dreht die Abhängigkeit um und nimmt eine `AiGatewayBinding`-Implementierung des Aufrufers entgegen. Klasse 1: `FetchRequest` trägt immer eine konkrete Methode, URL, Headerliste und einen Byte-Body; die TS-Zweige, die ein `Request`-Objekt mit `RequestInit` abgleichen (Header-Ersetzung, `body: null`, `signal: null`, One-Shot-Streams), haben keine Entsprechung — das beobachtbare Ergebnis ist identisch. Prefix-Prüfung und Provider/Endpoint-Split laufen wie in TS auf der URL-normalisierten Form |
| `src/api/openrouter-images.ts` | 196 | `api/openrouter_images.rs` | verifiziert (Task 10) | Klasse 3: Der `openai`-SDK-Client wird durch einen direkten POST auf `<baseUrl>/chat/completions` ersetzt — genau die Anfrage, die das SDK stellt |
| `src/images.ts` | 21 | `images.rs` | verifiziert (Task 10) | Klasse 4: Der dynamische `import()` des Built-in-Moduls entfällt (Rust linkt statisch) |
| `src/images-api-registry.ts` | 53 | `images_api_registry.rs` | verifiziert (Task 10) | Klasse 1: Registry hinter `Mutex`; der `api`-Abgleich von `wrapGenerateImages` bleibt erhalten |
| `src/images-models.ts` | 275 | `images_models.rs` | verifiziert (Task 10) | Provider-Registry, Auth-Auflösung mit Feld-Merge (explizite Optionen gewinnen, Header/Env pro Schlüssel), Refresh mit `ModelsError("model_source")`, Fehler als `AssistantImages` mit `stopReason: "error"` |
| `src/image-models.generated.ts` | 639 | `data/image-models.json` + `images::built_in_image_models` | verifiziert (Task 10) | Klasse 1: Der Katalog liegt wie die Chat-Modelle als Datei-Snapshot (42 Modelle, aus der TS-Quelle extrahiert) statt als generierter Quelltext |
| `src/providers/images/register-builtins.ts` | 50 | `images.rs::register_built_in_images_api_providers` | verifiziert (Task 10) | Klasse 4: Lazy-Import entfällt |
| `src/providers/openrouter-images.ts` | 22 | `images_models.rs::openrouter_images_provider` | verifiziert (Task 10) | — |
| — | — | `tests/subscription_plans.rs` | verifiziert (Task 10) | Ende-zu-Ende-Nachweis der beiden Abo-Pfade: gespeicherte OAuth-Credential → `resolveProviderAuth` → Adapter → ausgehender Request. Claude-Plan (Bearer statt x-api-key, Claude-Code-Identität, gemappte Tool-Namen) und Codex-Plan (Account-ID aus dem JWT, zstd-Body, `instructions` statt System-Nachricht) |
| `src/api/azure-openai-responses.ts` | 330 | `api/azure_openai_responses.rs` | verifiziert (Task 9) | Deployment-Namen (Option, `AZURE_OPENAI_DEPLOYMENT_NAME_MAP`, Modell-ID), Base-URL-Normalisierung über die `url`-Crate (WHATWG, wie `new URL()`) und `api-key`-Auth. Bug-compat: Eine Base-URL mit Query verschluckt den Pfad in den Query-String, weil das SDK vor dem Parsen konkateniert. 33 Payloads und Request-URLs aus dem TS-Original stimmen exakt |
| `src/api/openai-responses-shared.ts` | 792 | `api/openai_responses_shared.rs` | verifiziert (Task 9) | `convertResponsesMessages`, `convertResponsesTools` und die Slot-Zustandsmaschine (`output_index` → Content-Block). Klasse 1: `normalizeToolCallId` bekommt die Quell-Nachricht; dafür nimmt `NormalizeToolCallId` jetzt `(id, source)` statt nur `id` |
| `src/api/openai-responses.ts` | 372 | `api/openai_responses.rs` | verifiziert (Task 9) | Compat-Auflösung, `buildParams`, Session-Affinity-Header, Service-Tier-Preisfaktor und der Transport. 51 Payloads und 33 Event-Sequenzen aus dem TS-Original werden exakt reproduziert |
| `src/api/openai-prompt-cache.ts` | 8 | `api/openai_prompt_cache.rs` | verifiziert (Task 9) | 64 Code-Punkte, nicht Bytes (`Array.from`) |
| `src/auth/oauth/pkce.ts` | 34 | `auth/oauth/pkce.rs` | verifiziert (Task 7) | Klasse 3: WebCrypto → sha2 plus Zufallsquelle; gegen den RFC-7636-Testvektor geprüft |
| `src/auth/oauth/device-code.ts` | 98 | `auth/oauth/device_code.rs` | verifiziert (Task 7) | RFC 8628 inkl. `slow_down` mit 5-s-Increment, Vorrang eines Server-Intervalls und beider Timeout-Meldungen |
| `src/auth/oauth/oauth-page.ts` | 109 | `auth/oauth/oauth_page.rs` | verifiziert (Task 7) | Markup, Styles und Escaping wörtlich übernommen |
| `src/auth/oauth/anthropic.ts` | 364 | `auth/oauth/anthropic.rs` | verifiziert (Task 7) | Klasse 3: `node:http`-Callback-Server → tokio-Listener (`auth/oauth/callback_server.rs`); Klasse 1: das Rennen zwischen Callback und manueller Eingabe ist ein `tokio::select!` |
| `src/auth/oauth/openai-codex.ts` | 544 | `auth/oauth/openai_codex.rs` | verifiziert (Task 7) | Browser- und Device-Code-Flow; `accountId` aus dem JWT-Claim (Payload base64url-dekodiert, keine Signaturprüfung — wie TS) |
| `src/auth/oauth/github-copilot.ts` | 417 | `auth/oauth/github_copilot.rs` | verifiziert (Task 7) | Device-Flow, Copilot-Token-Exchange, Enterprise-Domains, Modell-Enablement über `availableModelIds` |
| `src/auth/oauth/openrouter.ts` | 311 | `auth/oauth/openrouter.rs` | verifiziert (Task 7) | Permanenter Key: `refresh` gibt die Credential unverändert zurück |
| `src/auth/oauth/kimi-coding.ts` | 310 | `auth/oauth/kimi_coding.rs` | verifiziert (Task 7) | Authentifizierung über den `Authorization`-Header statt eines API-Keys; Refresh-Retries |
| `src/auth/oauth/xai.ts` | 239 | `auth/oauth/xai.rs` | verifiziert (Task 7) | Device-Flow; https-Pflicht für die Verification-URI |
| `src/auth/oauth/radius.ts` | 403 | `auth/oauth/radius.rs` | verifiziert (Task 7) | Discovery nur für den Authorization-Endpunkt; Browser- und Device-Flow |
| — | — | `auth/oauth/callback_server.rs` | portiert (Task 7) | Klasse 3 (Hilfsmodul ohne TS-Pendant): Ein-Routen-HTTP-Server auf tokio als Ersatz für `node:http.createServer` in anthropic/codex/radius |
| — | — | `auth/oauth/http.rs` | portiert (Task 7) | Klasse 1 (Hilfsmodul ohne TS-Pendant): die in allen Flows wiederholten Form-POST- und Feldprüf-Bausteine, Fehlermeldungen wörtlich wie TS |
| `src/auth/helpers.ts` | 59 | `auth/helpers.rs` | portiert (Task 6) | Klasse 4: `lazyOAuth` entfällt — es verzögert einen dynamischen `import()` für Bundler; Rust linkt statisch |
| `src/env-api-keys.ts` | 188 | `env_api_keys.rs` | verifiziert (Task 6) | Klasse 1: Die verzögerte Node-Modul-Ladung entfällt; der ADC-Cache bleibt |
| `src/index.ts` | 47 | `lib.rs` | verifiziert (Task 13) | Klasse 3: `Static`/`TSchema`/`Type` entfallen — Schemas sind `serde_json::Value`, die Helfer liegen in `utils::typebox_helpers`. Klasse 1: `PiMessagesEvent`/`PiMessagesRewriteImpact` sind Wire-Typen des vorserialisierten Streams und im Port `serde_json::Value`, haben also keinen eigenen Namen. Sonst deckungsgleich |
| — | — | `utils/js_number.rs` | verifiziert (Task 1) | Klasse 1 (Hilfsmodul ohne TS-Pendant): ECMAScript-`Number::toString` für byte-identische JSON-Zahlen |
| — | — | `utils/fetch.rs` | verifiziert (Task 8) | Klasse 3: Ersatz für `FetchFunction`; die Form trägt den SSE-Strom (`FetchBody::Stream`) |

Task 13 hat drei Abweichungen aufgedeckt und behoben; die betroffenen Ledger-Zeilen oben
tragen sie jetzt mit:

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| `src/models.ts` (`fetchDeferred`/`cancelDeferred`, 706-731) | 26 | `models.rs` | verifiziert (Task 13) | fehlte im Port und wurde nachgezogen; `telemetry-options.test.ts` verlangt beide Dispatch-Wege |
| `src/models.ts` (`createProvider`, 835-857) | 23 | `models.rs`, `types.rs` | verifiziert (Task 13) | Klasse 1: TS prüft `entry.fetchDeferred !== undefined`; Rust kann eine Default-Methode nicht befragen und der Aufruf hätte Nebenwirkungen, deshalb deklariert `ProviderStreams::supports_fetch_deferred`/`supports_cancel_deferred` die Unterstützung |
| `src/api/openai-responses-shared.ts` (`outputSlots`) | — | `api/openai_responses_shared.rs` | verifiziert (Task 13) | Klasse 1: TS schlüsselt die Slot-Map mit `event.output_index`; ein Payload ohne das Feld schlüsselt auf `undefined` statt verworfen zu werden. Der Slot-Key ist deshalb `Option<i64>` |


## Datei-Abdeckung: `src/`

Jede der 175 TypeScript-Quelldateien mit ihrem Rust-Gegenstück. Wo die Ledger-Tabelle
oben eine Zeile hat, steht dort die ausführliche Abweichungsbegründung; hier steht nur
der Status.

| TS-Datei | LOC | Rust | Status | Anmerkung |
|---|---|---|---|---|
| `src/api/anthropic-messages.lazy.ts` | 4 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/api/anthropic-messages.ts` | 1352 | `api/anthropic_messages.rs`, `api/anthropic_params.rs` | verifiziert (Task 8) | siehe Ledger |
| `src/api/azure-openai-responses.lazy.ts` | 4 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/api/azure-openai-responses.ts` | 330 | `api/azure_openai_responses.rs` | verifiziert (Task 9) | siehe Ledger |
| `src/api/bedrock-converse-stream.lazy.ts` | 30 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/api/bedrock-converse-stream.ts` | 1188 | `api/bedrock_converse_stream.rs` | verifiziert (Task 10/11) | siehe Ledger |
| `src/api/cloudflare-gateway-binding.ts` | 192 | `api/cloudflare_gateway_binding.rs` | verifiziert (Task 10) | siehe Ledger |
| `src/api/cloudflare.ts` | 15 | `api/cloudflare.rs` | verifiziert (Task 10) | siehe Ledger |
| `src/api/constrained-sampling.ts` | 277 | `api/constrained_sampling.rs` | verifiziert (Task 8) | siehe Ledger |
| `src/api/github-copilot-headers.ts` | 37 | `api/github_copilot_headers.rs` | portiert (Task 8) | siehe Ledger |
| `src/api/google-generative-ai.lazy.ts` | 4 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/api/google-generative-ai.ts` | 521 | `api/google_generative_ai.rs` | verifiziert (Task 10) | siehe Ledger |
| `src/api/google-shared.ts` | 419 | `api/google_shared.rs` | verifiziert (Task 10) | siehe Ledger |
| `src/api/google-vertex.lazy.ts` | 4 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/api/google-vertex.ts` | 596 | `api/google_vertex.rs` | verifiziert (Task 10) | siehe Ledger |
| `src/api/lazy.ts` | 98 | `api/lazy.rs` | portiert (Task 4) | siehe Ledger |
| `src/api/mistral-conversations.lazy.ts` | 4 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/api/mistral-conversations.ts` | 931 | `api/mistral_conversations.rs` | verifiziert (Task 10) | siehe Ledger |
| `src/api/openai-codex-responses.lazy.ts` | 4 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/api/openai-codex-responses.ts` | 1662 | `api/openai_codex_responses.rs` | verifiziert (Task 9) | siehe Ledger |
| `src/api/openai-completions.lazy.ts` | 4 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/api/openai-completions.ts` | 1577 | `api/openai_completions{,_params,_compat}.rs` | verifiziert (Task 9) | siehe Ledger |
| `src/api/openai-prompt-cache.ts` | 8 | `api/openai_prompt_cache.rs` | verifiziert (Task 9) | siehe Ledger |
| `src/api/openai-responses-shared.ts` | 792 | `api/openai_responses_shared.rs` | verifiziert (Task 9) | siehe Ledger |
| `src/api/openai-responses.lazy.ts` | 4 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/api/openai-responses.ts` | 372 | `api/openai_responses.rs` | verifiziert (Task 9) | siehe Ledger |
| `src/api/openrouter-images.lazy.ts` | 10 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/api/openrouter-images.ts` | 196 | `api/openrouter_images.rs` | verifiziert (Task 10) | siehe Ledger |
| `src/api/pi-messages.lazy.ts` | 4 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/api/pi-messages.ts` | 433 | `api/pi_messages.rs` | verifiziert (Task 10) | siehe Ledger |
| `src/api/simple-options.ts` | 86 | `api/simple_options.rs` | verifiziert (Task 8) | siehe Ledger |
| `src/api/transform-messages.ts` | 223 | `api/transform_messages.rs` | verifiziert (Task 8) | siehe Ledger |
| `src/auth/context.ts` | 45 | `auth/context.rs` | portiert (Task 6) | siehe Ledger |
| `src/auth/credential-store.ts` | 67 | `auth/credential_store.rs` | verifiziert (Task 6) | siehe Ledger |
| `src/auth/helpers.ts` | 59 | `auth/helpers.rs` | portiert (Task 6) | siehe Ledger |
| `src/auth/oauth/anthropic.ts` | 364 | `auth/oauth/anthropic.rs` | verifiziert (Task 7) | siehe Ledger |
| `src/auth/oauth/device-code.ts` | 98 | `auth/oauth/device_code.rs` | verifiziert (Task 7) | siehe Ledger |
| `src/auth/oauth/github-copilot.ts` | 417 | `auth/oauth/github_copilot.rs` | verifiziert (Task 7) | siehe Ledger |
| `src/auth/oauth/kimi-coding.ts` | 310 | `auth/oauth/kimi_coding.rs` | verifiziert (Task 7) | siehe Ledger |
| `src/auth/oauth/load.ts` | 68 | `providers/*.rs` | ausgeschlossen | Klasse 4 (dynamischer `import()`) |
| `src/auth/oauth/oauth-page.ts` | 109 | `auth/oauth/oauth_page.rs` | verifiziert (Task 7) | siehe Ledger |
| `src/auth/oauth/openai-codex.ts` | 544 | `auth/oauth/openai_codex.rs` | verifiziert (Task 7) | siehe Ledger |
| `src/auth/oauth/openrouter.ts` | 311 | `auth/oauth/openrouter.rs` | verifiziert (Task 7) | siehe Ledger |
| `src/auth/oauth/pkce.ts` | 34 | `auth/oauth/pkce.rs` | verifiziert (Task 7) | siehe Ledger |
| `src/auth/oauth/radius.ts` | 403 | `auth/oauth/radius.rs` | verifiziert (Task 7) | siehe Ledger |
| `src/auth/oauth/xai.ts` | 239 | `auth/oauth/xai.rs` | verifiziert (Task 7) | siehe Ledger |
| `src/auth/resolve.ts` | 205 | `auth/resolve.rs` | verifiziert (Task 6) | siehe Ledger |
| `src/auth/types.ts` | 240 | `auth/types.rs` | portiert (Task 6) | siehe Ledger |
| `src/bedrock-provider.ts` | 6 | `api/streams.rs` | ausgeschlossen | Ausschluss Klasse 4 (Bundler-Wrapper) |
| `src/bun-oauth.ts` | 21 | `providers/*.rs` | ausgeschlossen | Klasse 4 (Bun-Binary-Registrierung) |
| `src/cli.ts` | 119 | `src/bin/notagent-ai.rs` | verifiziert (Task 11) | siehe Ledger |
| `src/compat.ts` | 298 | — | ausgeschlossen | Master-Plan, Scope-Tabelle |
| `src/compat/extension-oauth-types.ts` | 45 | `compat/extension_oauth_types.rs` | portiert (Task 13) | siehe Ledger |
| `src/env-api-keys.ts` | 188 | `env_api_keys.rs` | verifiziert (Task 6) | siehe Ledger |
| `src/image-models.generated.ts` | 639 | `data/image-models.json` | übernommen (Task 10) | Daten-Snapshot |
| `src/image-models.ts` | 42 | `images.rs` | portiert (Task 10) | `built_in_image_models` |
| `src/images-api-registry.ts` | 53 | `images_api_registry.rs` | verifiziert (Task 10) | siehe Ledger |
| `src/images-models.ts` | 275 | `images_models.rs` | verifiziert (Task 10) | siehe Ledger |
| `src/images.ts` | 21 | `images.rs` | verifiziert (Task 10) | siehe Ledger |
| `src/index.ts` | 47 | `lib.rs` | verifiziert (Task 13) | Oberfläche deckungsgleich |
| `src/legacy-api-aliases.ts` | 108 | — | ausgeschlossen | Master-Plan, Scope-Tabelle |
| `src/model-catalog.ts` | 27 | `model_catalog.rs` | verifiziert (Task 5) | siehe Ledger |
| `src/models-store.ts` | 45 | `models_store.rs` | portiert (Task 4) | siehe Ledger |
| `src/models.generated.ts` | 124 | `model_catalog.rs` | portiert (Task 5) | siehe Ledger |
| `src/models.ts` | 944 | `models.rs` | portiert (Task 4) | siehe Ledger |
| `src/oauth.ts` | 10 | `lib.rs` | portiert (Task 13) | reiner Typ-Reexport |
| `src/providers/all.ts` | 155 | `providers/all.rs` | verifiziert (Task 11) | siehe Ledger |
| `src/providers/amazon-bedrock.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/amazon-bedrock.ts` | 90 | `providers/amazon_bedrock.rs` | verifiziert (Task 11) | siehe Ledger |
| `src/providers/ant-ling.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/ant-ling.ts` | 15 | `providers/ant_ling.rs` | portiert |  |
| `src/providers/anthropic.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/anthropic.ts` | 59 | `providers/anthropic.rs` | verifiziert (Task 11) | siehe Ledger |
| `src/providers/azure-openai-responses.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/azure-openai-responses.ts` | 14 | `providers/azure_openai_responses.rs` | portiert |  |
| `src/providers/baseten.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/baseten.ts` | 15 | `providers/baseten.rs` | portiert |  |
| `src/providers/cerebras.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/cerebras.ts` | 15 | `providers/cerebras.rs` | portiert |  |
| `src/providers/cloudflare-ai-gateway.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/cloudflare-ai-gateway.ts` | 23 | `providers/cloudflare_ai_gateway.rs` | portiert |  |
| `src/providers/cloudflare-auth.ts` | 103 | `providers/cloudflare_auth.rs` | verifiziert (Task 11) | siehe Ledger |
| `src/providers/cloudflare-stream.ts` | 28 | `providers/cloudflare_stream.rs` | verifiziert (Task 11) | siehe Ledger |
| `src/providers/cloudflare-workers-ai.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/cloudflare-workers-ai.ts` | 15 | `providers/cloudflare_workers_ai.rs` | portiert |  |
| `src/providers/data-json.d.ts` | 4 | — | ausgeschlossen | reine Typdeklaration |
| `src/providers/deepseek.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/deepseek.ts` | 15 | `providers/deepseek.rs` | portiert |  |
| `src/providers/faux.ts` | 708 | `providers/faux.rs` | verifiziert (Task 11) | siehe Ledger |
| `src/providers/fireworks.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/fireworks.ts` | 19 | `providers/fireworks.rs` | portiert |  |
| `src/providers/github-copilot.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/github-copilot.ts` | 34 | `providers/github_copilot.rs` | verifiziert (Task 11) | siehe Ledger |
| `src/providers/google-vertex.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/google-vertex.ts` | 100 | `providers/google_vertex.rs` | verifiziert (Task 11) | siehe Ledger |
| `src/providers/google.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/google.ts` | 15 | `providers/google.rs` | portiert |  |
| `src/providers/groq.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/groq.ts` | 15 | `providers/groq.rs` | portiert |  |
| `src/providers/huggingface.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/huggingface.ts` | 15 | `providers/huggingface.rs` | portiert |  |
| `src/providers/images/register-builtins.ts` | 50 | `images.rs::register_built_in_images_api_providers` | verifiziert (Task 10) | siehe Ledger |
| `src/providers/kimi-coding.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/kimi-coding.ts` | 24 | `providers/kimi_coding.rs` | portiert |  |
| `src/providers/minimax-cn.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/minimax-cn.ts` | 15 | `providers/minimax_cn.rs` | portiert |  |
| `src/providers/minimax.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/minimax.ts` | 15 | `providers/minimax.rs` | portiert |  |
| `src/providers/mistral.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/mistral.ts` | 15 | `providers/mistral.rs` | portiert |  |
| `src/providers/moonshotai-cn.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/moonshotai-cn.ts` | 15 | `providers/moonshotai_cn.rs` | portiert |  |
| `src/providers/moonshotai.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/moonshotai.ts` | 15 | `providers/moonshotai.rs` | portiert |  |
| `src/providers/nvidia.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/nvidia.ts` | 15 | `providers/nvidia.rs` | portiert |  |
| `src/providers/openai-codex.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/openai-codex.ts` | 22 | `providers/openai_codex.rs` | portiert |  |
| `src/providers/openai.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/openai.ts` | 15 | `providers/openai.rs` | portiert |  |
| `src/providers/opencode-go.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/opencode-go.ts` | 20 | `providers/opencode_go.rs` | portiert |  |
| `src/providers/opencode.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/opencode.ts` | 24 | `providers/opencode.rs` | portiert |  |
| `src/providers/openrouter-images.ts` | 22 | `images_models.rs::openrouter_images_provider` | verifiziert (Task 10) | siehe Ledger |
| `src/providers/openrouter.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/openrouter.ts` | 23 | `providers/openrouter.rs` | portiert |  |
| `src/providers/qwen-token-plan-cn.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/qwen-token-plan-cn.ts` | 15 | `providers/qwen_token_plan_cn.rs` | portiert |  |
| `src/providers/qwen-token-plan-individual.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/qwen-token-plan-individual.ts` | 15 | `providers/qwen_token_plan_individual.rs` | portiert |  |
| `src/providers/qwen-token-plan.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/qwen-token-plan.ts` | 15 | `providers/qwen_token_plan.rs` | portiert |  |
| `src/providers/radius-config.ts` | 96 | `providers/radius_config.rs` | verifiziert (Task 11) | siehe Ledger |
| `src/providers/radius.ts` | 82 | `providers/radius.rs` | verifiziert (Task 11) | siehe Ledger |
| `src/providers/together.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/together.ts` | 15 | `providers/together.rs` | portiert |  |
| `src/providers/vercel-ai-gateway.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/vercel-ai-gateway.ts` | 15 | `providers/vercel_ai_gateway.rs` | portiert |  |
| `src/providers/xai.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/xai.ts` | 28 | `providers/xai.rs` | portiert |  |
| `src/providers/xiaomi-token-plan-ams.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/xiaomi-token-plan-ams.ts` | 15 | `providers/xiaomi_token_plan_ams.rs` | portiert |  |
| `src/providers/xiaomi-token-plan-cn.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/xiaomi-token-plan-cn.ts` | 15 | `providers/xiaomi_token_plan_cn.rs` | portiert |  |
| `src/providers/xiaomi-token-plan-sgp.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/xiaomi-token-plan-sgp.ts` | 15 | `providers/xiaomi_token_plan_sgp.rs` | portiert |  |
| `src/providers/xiaomi.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/xiaomi.ts` | 15 | `providers/xiaomi.rs` | portiert |  |
| `src/providers/zai-coding-cn.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/zai-coding-cn.ts` | 15 | `providers/zai_coding_cn.rs` | portiert |  |
| `src/providers/zai.models.ts` | 8 | `model_catalog.rs` | ausgeschlossen | Ausschluss (generierter Snapshot-Leser) |
| `src/providers/zai.ts` | 15 | `providers/zai.rs` | portiert |  |
| `src/session-resources.ts` | 24 | `session_resources.rs` | verifiziert (Task 9) | siehe Ledger |
| `src/types.ts` | 830 | `types.rs` | verifiziert (Task 1) | siehe Ledger |
| `src/utils/abort-signals.ts` | 41 | `utils/abort.rs` | portiert (Task 3) | mit `abort.ts` zusammengeführt |
| `src/utils/abort.ts` | 50 | `utils/abort.rs` | portiert |  |
| `src/utils/deferred-tools.ts` | 39 | `utils/deferred_tools.rs` | portiert (Task 3) | siehe Ledger |
| `src/utils/diagnostics.ts` | 45 | `utils/diagnostics.rs` | portiert (Task 3) | siehe Ledger |
| `src/utils/error-body.ts` | 149 | `utils/error_body.rs` | portiert (Task 3) | siehe Ledger |
| `src/utils/estimate.ts` | 143 | `utils/estimate.rs` | verifiziert (Task 3) | siehe Ledger |
| `src/utils/event-stream.ts` | 88 | `utils/event_stream.rs` | verifiziert (Task 3) | siehe Ledger |
| `src/utils/hash.ts` | 13 | `utils/hash.rs` | portiert (Task 3) | siehe Ledger |
| `src/utils/headers.ts` | 18 | `utils/headers.rs` | portiert (Task 3) | siehe Ledger |
| `src/utils/json-parse.ts` | 124 | `utils/json_parse.rs` | verifiziert (Task 3) | siehe Ledger |
| `src/utils/node-http-proxy.ts` | 112 | `utils/node_http_proxy.rs` | verifiziert (Task 3) | siehe Ledger |
| `src/utils/overflow.ts` | 180 | `utils/overflow.rs` | verifiziert (Task 3) | siehe Ledger |
| `src/utils/provider-env.ts` | 52 | `utils/provider_env.rs` | portiert (Task 3) | siehe Ledger |
| `src/utils/provider-retry.ts` | 125 | `utils/provider_retry.rs` | verifiziert (Task 3) | siehe Ledger |
| `src/utils/retry.ts` | 228 | `utils/retry.rs` | verifiziert (Task 3) | siehe Ledger |
| `src/utils/sanitize-unicode.ts` | 25 | `utils/sanitize_unicode.rs` | portiert (Task 3) | siehe Ledger |
| `src/utils/text.ts` | 12 | `utils/text.rs` | verifiziert (Task 3) | siehe Ledger |
| `src/utils/typebox-helpers.ts` | 24 | `utils/typebox_helpers.rs` | verifiziert (Task 13) | siehe Ledger |
| `src/utils/uuid.ts` | 48 | `utils/uuid.rs` | verifiziert (Task 3) | siehe Ledger |
| `src/utils/validation.ts` | 350 | `utils/validation.rs` | verifiziert (Task 3) | siehe Ledger |

## Datei-Abdeckung: `test/`

Alle 131 Vitest-Suiten. `portiert (key-gated)` heißt: der Port trägt dieselben Fälle und
dieselben Gates wie TS — ohne Provider-Credential überspringt er sie, genau wie
`describe.skipIf`/`it.skipIf` in TS.

| TS-Suite | LOC | Rust | Status | Anmerkung |
|---|---|---|---|---|
| `test/abort.test.ts` | 351 | `tests/abort_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/anthropic-adaptive-thinking-models.test.ts` | 40 | `tests/anthropic_compat.rs` | portiert |  |
| `test/anthropic-auth-token.test.ts` | 187 | `tests/anthropic_transport.rs` | portiert |  |
| `test/anthropic-cache-write-1h-cost.test.ts` | 86 | `tests/anthropic_compat.rs` | portiert |  |
| `test/anthropic-eager-tool-input-compat.test.ts` | 166 | `tests/anthropic_compat.rs` | portiert |  |
| `test/anthropic-eager-tool-input-e2e.test.ts` | 155 | `tests/anthropic_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/anthropic-empty-thinking-signature-compat.test.ts` | 108 | `tests/anthropic_compat.rs` | portiert |  |
| `test/anthropic-force-adaptive-thinking.test.ts` | 123 | `tests/anthropic_params.rs` | portiert |  |
| `test/anthropic-long-cache-retention-e2e.test.ts` | 127 | `tests/anthropic_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/anthropic-oauth.test.ts` | 144 | `tests/oauth.rs` | portiert |  |
| `test/anthropic-opus-4-8-smoke.test.ts` | 71 | `tests/anthropic_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/anthropic-sse-parsing.test.ts` | 424 | `tests/anthropic_stream.rs` | portiert |  |
| `test/anthropic-temperature-compat.test.ts` | 103 | `tests/anthropic_compat.rs` | portiert |  |
| `test/anthropic-thinking-disable.test.ts` | 172 | `tests/anthropic_params.rs`, `tests/anthropic_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/anthropic-tool-name-normalization.test.ts` | 204 | `tests/anthropic_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/azure-openai-base-url.test.ts` | 203 | `tests/azure_openai_responses.rs` | portiert |  |
| `test/azure-openai-responses-reasoning-replay.test.ts` | 136 | `tests/azure_openai_responses.rs` | portiert |  |
| `test/baseten-models.test.ts` | 141 | `tests/model_data.rs` | portiert |  |
| `test/bedrock-convert-messages.test.ts` | 410 | `tests/bedrock_convert_messages.rs` | portiert |  |
| `test/bedrock-credentials.test.ts` | 115 | `tests/bedrock_converse_stream.rs` | portiert |  |
| `test/bedrock-custom-headers.test.ts` | 202 | `tests/bedrock_converse_stream.rs` | portiert |  |
| `test/bedrock-endpoint-resolution.test.ts` | 208 | `tests/bedrock_converse_stream.rs` | portiert |  |
| `test/bedrock-error-metadata.test.ts` | 212 | `tests/bedrock_error_metadata.rs` | portiert |  |
| `test/bedrock-models.test.ts` | 70 | `tests/model_data.rs` | portiert |  |
| `test/bedrock-raw-stop-reason.test.ts` | 82 | `tests/bedrock_converse_stream.rs` | portiert |  |
| `test/bedrock-thinking-payload.test.ts` | 266 | `tests/bedrock_converse_stream.rs`, `tests/context_and_images_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/cache-retention.test.ts` | 532 | `tests/anthropic_params.rs`, `tests/openai_responses_params.rs`, `tests/context_and_images_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/cloudflare-gateway-binding.test.ts` | 332 | `tests/cloudflare_gateway_binding.rs` | portiert |  |
| `test/cloudflare-stream.test.ts` | 65 | `tests/providers.rs` | portiert |  |
| `test/compat-env.test.ts` | 74 | — | ausgeschlossen | prüft ausschließlich die Legacy-API-Registry aus `compat.ts` |
| `test/constrained-sampling.test.ts` | 305 | `tests/constrained_sampling.rs` | portiert |  |
| `test/context-estimate.test.ts` | 81 | `tests/utils.rs` | portiert |  |
| `test/context-overflow.test.ts` | 771 | `tests/context_and_images_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/cross-provider-handoff.test.ts` | 522 | `tests/context_and_images_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/deferred-tools.test.ts` | 550 | `tests/deferred_tools.rs` | portiert |  |
| `test/empty.test.ts` | 822 | `tests/empty_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/env-api-keys.test.ts` | 116 | `tests/auth.rs` | portiert |  |
| `test/error-body.test.ts` | 226 | `tests/error_body.rs` | portiert |  |
| `test/faux-provider.test.ts` | 616 | `tests/faux.rs` | portiert |  |
| `test/fetch-option.test.ts` | 178 | `tests/*` (adapterweise) | teilweise abgedeckt | siehe Ledger: die zwei Google-Fälle prüfen die Ablehnung eines eigenen `fetch`, die es im Port nicht gibt |
| `test/fireworks-models.test.ts` | 343 | `tests/model_data.rs` | portiert |  |
| `test/generate-models-strict.test.ts` | 85 | — | ausgeschlossen | Test des Generator-Werkzeugs, nicht des Laufzeitverhaltens |
| `test/github-copilot-anthropic.test.ts` | 127 | `tests/anthropic_transport.rs` | portiert |  |
| `test/github-copilot-oauth.test.ts` | 570 | `tests/oauth.rs` | portiert |  |
| `test/google-raw-stop-reason.test.ts` | 106 | `tests/google_shared.rs` | portiert |  |
| `test/google-shared-convert-tools.test.ts` | 203 | `tests/google_generative_ai.rs` | portiert |  |
| `test/google-shared-gemini3-unsigned-tool-call.test.ts` | 167 | `tests/google_shared.rs` | portiert |  |
| `test/google-shared-image-tool-result-routing.test.ts` | 102 | `tests/google_shared.rs` | portiert |  |
| `test/google-shared-retry.test.ts` | 40 | `tests/google_shared.rs` | portiert |  |
| `test/google-shared-signed-empty-blocks.test.ts` | 117 | `tests/google_shared.rs` | portiert |  |
| `test/google-thinking-disable.test.ts` | 160 | `tests/misc_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/google-thinking-signature.test.ts` | 38 | `tests/google_shared.rs` | portiert |  |
| `test/google-vertex-api-key-resolution.test.ts` | 223 | `tests/google_vertex.rs` | portiert |  |
| `test/image-model-data.test.ts` | 47 | — | ausgeschlossen | Test des Generator-Skripts `scripts/generate-image-models.ts` |
| `test/image-tool-result.test.ts` | 550 | `tests/context_and_images_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/images.test.ts` | 90 | `tests/images.rs` | portiert |  |
| `test/images-models.test.ts` | 209 | `tests/images.rs` | portiert |  |
| `test/interleaved-thinking.test.ts` | 144 | `tests/anthropic_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/kimi-coding-oauth.test.ts` | 270 | `tests/oauth.rs` | portiert |  |
| `test/lax-message-content.test.ts` | 67 | `tests/types_serde.rs` | portiert |  |
| `test/lazy-module-load.test.ts` | 113 | — | ausgeschlossen | prüft, dass der Bundler die Provider-SDKs abtrennt; Rust linkt statisch und bindet kein SDK außer aws-sdk |
| `test/max-thinking.test.ts` | 89 | `tests/models.rs` | portiert |  |
| `test/mistral-http-transport.test.ts` | 427 | `tests/mistral_conversations.rs` | portiert |  |
| `test/mistral-raw-stop-reason.test.ts` | 61 | `tests/mistral_conversations.rs` | portiert |  |
| `test/mistral-reasoning-mode.test.ts` | 97 | `tests/mistral_conversations.rs` | portiert |  |
| `test/mistral-tool-schema.test.ts` | 63 | `tests/mistral_conversations.rs` | portiert |  |
| `test/model-catalog-types.test.ts` | 15 | — | ausgeschlossen | `expectTypeOf`-Zusicherungen über generierte Literaltypen |
| `test/model-data-validation.test.ts` | 177 | — | ausgeschlossen | Test des Generator-Werkzeugs `scripts/model-data.ts` |
| `test/models-runtime.test.ts` | 1159 | `tests/models.rs` | portiert |  |
| `test/node-http-proxy.test.ts` | 76 | `tests/utils.rs` | portiert |  |
| `test/oauth-auth.test.ts` | 166 | `tests/auth.rs` | portiert |  |
| `test/oauth-device-code.test.ts` | 145 | `tests/oauth.rs` | portiert |  |
| `test/openai-codex-cache-affinity-e2e.test.ts` | 35 | `tests/misc_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/openai-codex-oauth.test.ts` | 486 | `tests/oauth.rs` | portiert |  |
| `test/openai-codex-stream.test.ts` | 2622 | `tests/openai_codex_responses.rs` | portiert |  |
| `test/openai-completions-cache-control-format.test.ts` | 231 | `tests/openai_compat.rs` | portiert |  |
| `test/openai-completions-empty-tools.test.ts` | 304 | `tests/openai_completions_params.rs` | portiert |  |
| `test/openai-completions-prompt-cache.test.ts` | 274 | `tests/openai_completions_transport.rs` | portiert |  |
| `test/openai-completions-raw-stop-reason.test.ts` | 79 | `tests/openai_completions_stream.rs` | portiert |  |
| `test/openai-completions-reasoning-details.test.ts` | 118 | `tests/openai_completions_stream.rs` | portiert |  |
| `test/openai-completions-response-model.test.ts` | 140 | `tests/openai_completions_stream.rs` | portiert |  |
| `test/openai-completions-retry.test.ts` | 139 | `tests/openai_completions_transport.rs` | portiert |  |
| `test/openai-completions-thinking-as-text.test.ts` | 222 | `tests/openai_compat.rs` | portiert |  |
| `test/openai-completions-thinking-token-budget.test.ts` | 124 | `tests/openai_completions_params.rs` | portiert |  |
| `test/openai-completions-tool-choice.test.ts` | 1843 | `tests/openai_completions_params.rs` | portiert |  |
| `test/openai-completions-tool-result-images.test.ts` | 156 | `tests/openai_completions_params.rs` | portiert |  |
| `test/openai-responses-cache-affinity-e2e.test.ts` | 31 | `tests/misc_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/openai-responses-compat.test.ts` | 524 | `tests/openai_responses_params.rs` | portiert |  |
| `test/openai-responses-empty-tool-result.test.ts` | 58 | `tests/openai_responses_convert.rs` | portiert |  |
| `test/openai-responses-foreign-toolcall-id.test.ts` | 66 | `tests/openai_responses_convert.rs` | portiert |  |
| `test/openai-responses-message-id.test.ts` | 48 | `tests/openai_responses_convert.rs` | portiert |  |
| `test/openai-responses-namespace.test.ts` | 224 | `tests/openai_responses_namespace.rs` | portiert |  |
| `test/openai-responses-partial-json-cleanup.test.ts` | 106 | `tests/openai_responses_namespace.rs` | portiert |  |
| `test/openai-responses-reasoning-replay-e2e.test.ts` | 291 | `tests/context_and_images_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/openai-responses-terminal-event.test.ts` | 356 | `tests/openai_responses_stream.rs` | portiert |  |
| `test/openai-responses-tool-result-images.test.ts` | 194 | `tests/context_and_images_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/openrouter-cache-control-models.test.ts` | 15 | `tests/model_data.rs` | portiert |  |
| `test/openrouter-cache-write-repro.test.ts` | 77 | `tests/misc_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/openrouter-images.test.ts` | 140 | `tests/images.rs` | portiert |  |
| `test/openrouter-oauth.test.ts` | 322 | `tests/oauth.rs` | portiert |  |
| `test/overflow.test.ts` | 177 | `tests/utils.rs` | portiert |  |
| `test/pi-messages.test.ts` | 248 | `tests/pi_messages.rs` | portiert |  |
| `test/provider-error-body-passthrough.test.ts` | 78 | `tests/provider_error_body.rs` | portiert |  |
| `test/provider-error-body-regression.test.ts` | 213 | `tests/provider_error_body.rs` | portiert |  |
| `test/provider-retry.test.ts` | 81 | `tests/event_stream_and_provider_retry.rs` | portiert |  |
| `test/providers.test.ts` | 629 | `tests/providers.rs` | portiert |  |
| `test/qwen-token-plan-models.test.ts` | 283 | `tests/model_data.rs` | portiert |  |
| `test/radius-oauth.test.ts` | 129 | `tests/radius.rs` | portiert |  |
| `test/reasoning-options.test.ts` | 36 | — | ausgeschlossen | Test des Generator-Werkzeugs `scripts/models-dev-reasoning-options.ts` |
| `test/responseid.test.ts` | 120 | `tests/misc_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/retry.test.ts` | 223 | `tests/utils.rs` | portiert |  |
| `test/sampling-options.test.ts` | 128 | `tests/sampling_options.rs` | portiert |  |
| `test/stream.test.ts` | 1747 | `tests/stream_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/supports-xhigh.test.ts` | 157 | `tests/models.rs` | portiert |  |
| `test/telemetry-options.test.ts` | 171 | `tests/telemetry_options.rs` | portiert |  |
| `test/text.test.ts` | 33 | `tests/utils.rs` | portiert |  |
| `test/together-models.test.ts` | 86 | `tests/model_data.rs` | portiert |  |
| `test/tokens.test.ts` | 365 | `tests/usage_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/tool-call-id-normalization.test.ts` | 290 | `tests/tool_call_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/tool-call-without-result.test.ts` | 359 | `tests/tool_call_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/total-tokens.test.ts` | 882 | `tests/usage_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/transform-messages-copilot-openai-to-anthropic.test.ts` | 191 | `tests/transform_messages_copilot.rs` | portiert |  |
| `test/unicode-surrogate.test.ts` | 839 | `tests/unicode_surrogate_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/uuid.test.ts` | 50 | `tests/utils.rs` | portiert |  |
| `test/validation.test.ts` | 210 | `tests/validation.rs` | portiert |  |
| `test/xai-oauth.test.ts` | 335 | `tests/oauth.rs` | portiert |  |
| `test/xai-responses.test.ts` | 117 | `tests/xai_responses.rs` | portiert |  |
| `test/xhigh.test.ts` | 70 | `tests/anthropic_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/xiaomi-models.test.ts` | 17 | `tests/model_data.rs` | portiert |  |
| `test/xiaomi-token-plan-ams-anthropic-empty-signature-smoke.test.ts` | 114 | `tests/anthropic_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |
| `test/zen.test.ts` | 25 | `tests/misc_e2e.rs` | portiert (key-gated) | läuft wie in TS nur mit Provider-Credential |

Die sechs Hilfsdateien unter `test/` (keine Suiten):

| TS-Datei | LOC | Rust | Status | Anmerkung |
|---|---|---|---|---|
| `test/oauth.ts` | 85 | `tests/e2e_support/mod.rs` | portiert (Task 13) | `resolveApiKey` inklusive OAuth-Refresh und Rückschreiben nach `auth.json` |
| `test/azure-utils.ts` | 28 | `tests/e2e_support/mod.rs` | portiert (Task 13) | |
| `test/bedrock-utils.ts` | 18 | `tests/e2e_support/mod.rs` | portiert (Task 13) | |
| `test/cloudflare-utils.ts` | 9 | `tests/e2e_support/mod.rs` | portiert (Task 13) | |
| `test/scratch.ts` | 57 | — | ausgeschlossen | Demo-Skript ohne Zusicherungen (`node test/scratch.ts`), kein Test |
| `test/codex-websocket-cached-probe.ts` | 299 | — | ausgeschlossen | manuelles Live-Probe-Skript für den Codex-`websocket-cached`-Transport, kein Test |

## Key-Gating der e2e-Suiten

TS gated zweistufig: `describe.skipIf`/`it.skipIf` pro Credential, und darüber der
kanonische Lauf `./test.sh`, der die Suite mit `env -i` und isoliertem `HOME` startet —
ohne Provider-Key und ohne `~/.notagent/agent/auth.json`, sodass dort jede e2e-Suite
übersprungen wird.

Der Port bildet beides ab: `skip_unless!`/`skip_unless_some!` pro Credential (Rusts
Harness kennt kein Skip, also gibt der Test dieselbe Begründung aus und kehrt zurück) und
darüber `NOTAGENT_AI_E2E`. `cargo test --workspace` erbt anders als `test.sh` die echte
Umgebung, deshalb liest `e2e_support::env`/`resolve_api_key` ohne diese Variable
grundsätzlich kein Credential. `NOTAGENT_AI_E2E=1 cargo test -p notagent-ai` entspricht
dem TS-Lauf mit Keys in der Umgebung.

## Ausschlüsse


| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| `src/compat.ts` (298) | Master-Plan, Scope-Tabelle: im Code als Legacy markiert („wird mit ModelManager-Migration gelöscht") |
| `src/legacy-api-aliases.ts` (108) | wie oben |
| `scripts/generate-image-models.ts` und `test/image-model-data.test.ts` | Generator-Skript wie `scripts/generate-models.ts` (Workstream-Plan Task 5): der Datei-Snapshot ist die Quelle, Aktualisierungen passieren im TS-Repo; der Test prüft ausschließlich den Generator |
| `src/auth/oauth/load.ts` (71) | Klasse 4: verzögert einen dynamischen `import()`, damit Bundler die Flows abtrennen können; Rust linkt statisch. Die Provider referenzieren die Flows direkt — die von `lazyOAuth` gesetzten Anzeigewerte sind mit denen der Flows identisch (Fixture `tests/fixtures/providers.jsonl`) |
| `src/providers/data-json.d.ts` | reine Typdeklaration für JSON-Importe; ohne Laufzeitanteil |
| `src/providers/*.models.ts` (39 Dateien, je 8 LOC) | generierte Leser des `data/`-Snapshots (`flattenModelCatalog`); die Factories lesen dieselben Daten über `model_catalog::get_builtin_models` |
| `scripts/generate-models.ts` | WS-B-Plan Task 5: der JSON-Snapshot ist die Quelle; Aktualisierung bleibt im TS-Repo |
| `test/compat-env.test.ts` (74) | prüft ausschließlich die Legacy-API-Registry aus `compat.ts`, die laut Master-Plan ausgeschlossen ist |
| `scripts/model-data.ts` + `test/model-data-validation.test.ts` (177), `scripts/models-dev-reasoning-options.ts` + `test/reasoning-options.test.ts` (36), `test/generate-models-strict.test.ts` (85) | Generator-Werkzeuge und ihre Tests: sie prüfen die Erzeugung des Snapshots im TS-Repo, nicht das Laufzeitverhalten |
| `src/api/*.lazy.ts` (11 Dateien) und `src/bedrock-provider.ts` (6) | Klasse 4: Bundler-Wrapper, damit der Bundler die API-Module abtrennen kann; Rust linkt statisch und registriert die Implementierung direkt (`api/streams.rs`) |
| `src/bun-oauth.ts` (21) | Klasse 4: registriert die OAuth-Flows für das gebündelte Bun-Binary; Rust linkt sie statisch |
| `test/lazy-module-load.test.ts` (113) | prüft, dass der Bundler beim Import der Barrel-Datei keine Provider-SDKs lädt. Der Port bindet außer `aws-sdk` (Master-Substitution) kein Provider-SDK ein, die Aussage ist also strukturell erfüllt und nicht beobachtbar |
| `test/scratch.ts` (57) | Demo-Skript ohne Zusicherungen, kein Test |
| `test/codex-websocket-cached-probe.ts` (299) | manuelles Live-Probe-Skript für den Codex-`websocket-cached`-Transport, kein Test |
| `test/model-catalog-types.test.ts` (15) | `expectTypeOf`-Zusicherungen über die generierten Literaltypen; die eine Laufzeitzusicherung (Copilot `grok-4.5` → `openai-responses`) deckt `tests/model_catalog.rs` über den Snapshot ab |
