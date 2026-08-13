//! Provider factories.
//!
//! 1:1 port of `packages/ai/src/providers/`. Every `*.models.ts` file is generated from
//! the catalog snapshot in `data/` and therefore has no counterpart here; the factories
//! read the same data through [`crate::model_catalog::get_builtin_models`].

pub mod all;
pub mod amazon_bedrock;
pub mod ant_ling;
pub mod anthropic;
pub mod azure_openai_responses;
pub mod baseten;
pub mod cerebras;
pub mod cloudflare_ai_gateway;
pub mod cloudflare_auth;
pub mod cloudflare_stream;
pub mod cloudflare_workers_ai;
pub mod deepseek;
pub mod faux;
pub mod fireworks;
pub mod github_copilot;
pub mod google;
pub mod google_vertex;
pub mod groq;
pub mod huggingface;
pub mod kimi_coding;
pub mod minimax;
pub mod minimax_cn;
pub mod mistral;
pub mod moonshotai;
pub mod moonshotai_cn;
pub mod nvidia;
pub mod openai;
pub mod openai_codex;
pub mod opencode;
pub mod opencode_go;
pub mod openrouter;
pub mod qwen_token_plan;
pub mod qwen_token_plan_cn;
pub mod qwen_token_plan_individual;
pub mod radius;
pub mod radius_config;
pub mod together;
pub mod vercel_ai_gateway;
pub mod xai;
pub mod xiaomi;
pub mod xiaomi_token_plan_ams;
pub mod xiaomi_token_plan_cn;
pub mod xiaomi_token_plan_sgp;
pub mod zai;
pub mod zai_coding_cn;
