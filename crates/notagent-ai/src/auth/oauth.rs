//! OAuth login flows.
//!
//! 1:1 port of `packages/ai/src/auth/oauth/`. `load.ts` has no counterpart: it defers a
//! dynamic `import()` so bundlers can split the flows out (deviation class 4 —
//! distribution mechanics; Rust links statically).

pub mod anthropic;
pub mod callback_server;
pub mod device_code;
pub mod github_copilot;
pub mod http;
pub mod kimi_coding;
pub mod oauth_page;
pub mod openai_codex;
pub mod openrouter;
pub mod pkce;
pub mod radius;
pub mod xai;
