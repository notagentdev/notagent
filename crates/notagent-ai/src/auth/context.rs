//! Default auth context.
//!
//! 1:1 port of `packages/ai/src/auth/context.ts` (45 LOC). The TS version guards
//! against browsers, where `process.env` and `node:fs` are unavailable; in Rust both
//! always exist (deviation class 1).

use std::sync::Arc;

use crate::auth::types::{AuthContext, BoxFuture};

struct DefaultAuthContext;

impl AuthContext for DefaultAuthContext {
    fn env(&self, name: &str) -> BoxFuture<'_, Option<String>> {
        // TS treats blank values as unset.
        let value = std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty());
        Box::pin(async move { value })
    }

    fn file_exists(&self, path: &str) -> BoxFuture<'_, bool> {
        let path = expand_home(path);
        Box::pin(async move { tokio::fs::metadata(&path).await.is_ok() })
    }
}

/// `path.startsWith("~") ? os.homedir() + path.slice(1) : path`
pub fn expand_home(path: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) => match dirs::home_dir() {
            Some(home) => format!("{}{rest}", home.display()),
            None => path.to_string(),
        },
        None => path.to_string(),
    }
}

/// `defaultProviderAuthContext()`
pub fn default_provider_auth_context() -> Arc<dyn AuthContext> {
    Arc::new(DefaultAuthContext)
}
