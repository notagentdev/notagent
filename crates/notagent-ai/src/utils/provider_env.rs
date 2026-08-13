//! Provider-scoped environment lookup.
//!
//! 1:1 port of `packages/ai/src/utils/provider-env.ts` (52 LOC). The Bun sandbox
//! fallback (`/proc/self/environ`) is a workaround for oven-sh/bun#27802 and has no
//! counterpart in Rust (deviation class 4, distribution mechanics).

use crate::types::ProviderEnv;

/// `getProviderEnvValue(name, env)` — scoped override first, then the process
/// environment. Empty strings are falsy in JS and therefore skipped as well.
pub fn get_provider_env_value(name: &str, env: Option<&ProviderEnv>) -> Option<String> {
    if let Some(env) = env
        && let Some(value) = env.get(name)
        && !value.is_empty()
    {
        return Some(value.clone());
    }
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}
