# notagent-ai

Unified multi-provider LLM API: one streaming interface over many provider
protocols.

- `api/` — protocol implementations: Anthropic Messages, OpenAI Completions
  and Responses, Google (Generative AI and Vertex), Bedrock, Azure, Mistral,
  Cloudflare, and more, plus SSE plumbing and prompt-cache handling
- `providers/` — provider definitions: baseline model lists, auth wiring,
  dynamic model fetching (e.g. the MTPLX provider asks the server what it
  serves)
- `auth/` — API-key and OAuth flows with credential storage
- `model_catalog.rs` / `models_store.rs` — the model catalog, its persisted
  cache, and `models.json` layering
- `images.rs` — image generation support for providers that offer it

Everything streams: a request returns an event stream of text, thinking,
tool-call, and usage events, normalized across providers.
