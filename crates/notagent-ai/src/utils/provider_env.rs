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
