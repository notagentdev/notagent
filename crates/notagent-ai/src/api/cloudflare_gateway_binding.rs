//! AI Gateway transport over the Workers AI binding.
//!
//! 1:1 port of `packages/ai/src/api/cloudflare-gateway-binding.ts` (192 LOC).
//! [`create_gateway_binding_fetch`] returns a [`FetchFunction`] that translates requests
//! under a gateway HTTPS prefix into calls to the binding's universal endpoint,
//! `env.AI.gateway(id).run({provider, endpoint, headers, query})`.
//!
//! Deviation class 3: the concrete Workers binding lives in the JavaScript runtime, so the
//! port inverts the dependency — the caller supplies an [`AiGatewayBinding`] implementation.
//! Deviation class 1: [`FetchRequest`] always carries a concrete method, URL, header list
//! and byte body, so the TS branches that reconcile a `Request` input with `RequestInit`
//! (`init.headers` replacing request headers, `body: null` / `signal: null` clearing them,
//! one-shot stream bodies) have no counterpart; their observable outcome is the same.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::utils::fetch::{FetchError, FetchFn, FetchFuture, FetchRequest};

/// Placeholder value for auth headers on binding-routed requests.
pub const CLOUDFLARE_GATEWAY_BINDING_AUTH_SENTINEL: &str = "cloudflare-gateway-binding";

/// Never forwarded to the binding: hop-by-hop/derived headers and gateway auth.
const STRIP_HEADERS: [&str; 3] = ["content-length", "host", "cf-aig-authorization"];

/// One universal-endpoint request entry, as accepted by `AiGateway.run()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiGatewayUniversalRequest {
    pub provider: String,
    pub endpoint: String,
    pub headers: BTreeMap<String, String>,
    pub query: Value,
}

/// The Workers AI binding's gateway surface (`env.AI`).
pub trait AiGatewayBinding: Send + Sync {
    /// `binding.gateway(id).run(data, options)`
    fn run(&self, gateway: &str, data: AiGatewayUniversalRequest) -> FetchFuture;
}

/// Options of [`create_gateway_binding_fetch`].
pub struct GatewayBindingFetchOptions {
    /// The Workers AI binding (e.g. `env.AI`).
    pub binding: Arc<dyn AiGatewayBinding>,
    /// Gateway HTTPS prefix every request must fall under, without a trailing slash.
    pub base_url: String,
    /// Gateway name passed to `binding.gateway()`.
    pub gateway: String,
}

/// A `fetch` that routes AI Gateway requests through the Workers AI binding.
pub struct GatewayBindingFetch {
    binding: Arc<dyn AiGatewayBinding>,
    gateway: String,
    origin: String,
    base_path: String,
}

/// `createGatewayBindingFetch(options)`
pub fn create_gateway_binding_fetch(options: GatewayBindingFetchOptions) -> Arc<dyn FetchFn> {
    // Prefix matching runs on URL-normalized components (origin + pathname), not raw
    // strings, so a lexical variant cannot split provider/endpoint differently.
    let (origin, path) = split_url(&options.base_url).unwrap_or_else(|| {
        // `new URL(baseUrl)` throws for an invalid base; the port keeps the raw value so
        // every request reports the prefix mismatch instead.
        (options.base_url.clone(), String::new())
    });
    let base_path = if path.ends_with('/') {
        path
    } else {
        format!("{path}/")
    };
    Arc::new(GatewayBindingFetch {
        binding: options.binding,
        gateway: options.gateway,
        origin,
        base_path,
    })
}

impl FetchFn for GatewayBindingFetch {
    fn fetch(&self, request: FetchRequest) -> FetchFuture {
        let method = request.method.to_uppercase();
        let url = request.url.clone();

        let parsed = split_url(&url);
        let out_of_prefix = match &parsed {
            Some((origin, path)) => {
                *origin != self.origin || !normalize_path(path).starts_with(&self.base_path)
            }
            None => true,
        };
        if out_of_prefix {
            return error(format!(
                "createGatewayBindingFetch: {method} {url} is outside the configured gateway \
                 prefix ({}{}); this fetch only serves its gateway-bound client",
                self.origin, self.base_path
            ));
        }

        // In-prefix requests the universal endpoint cannot express always reject.
        let unexpressible = |reason: &str| {
            error(format!(
                "createGatewayBindingFetch: cannot express {method} {url} as a universal \
                 gateway request ({reason}); route it over HTTPS with gateway auth instead"
            ))
        };
        if method != "POST" {
            return unexpressible("only POST is supported");
        }

        let (_, path) = parsed.expect("prefix matched");
        let (path, search) = match path.split_once('?') {
            Some((path, search)) => (normalize_path(path), format!("?{search}")),
            None => (normalize_path(&path), String::new()),
        };
        let rest = &path[self.base_path.len()..];
        let Some(slash) = rest.find('/').filter(|slash| *slash > 0) else {
            return unexpressible("missing provider/endpoint path");
        };
        let provider = rest[..slash].to_string();
        // Keep the query string on the endpoint — it is part of what HTTPS would send.
        let endpoint = format!("{}{search}", &rest[slash + 1..]);

        let Some(body) = request.body.as_deref() else {
            return unexpressible("missing body");
        };
        let Ok(query) = serde_json::from_slice::<Value>(body) else {
            return unexpressible("non-JSON body");
        };

        let headers = collect_headers(&request.headers);
        self.binding.run(
            &self.gateway,
            AiGatewayUniversalRequest {
                provider,
                endpoint,
                headers,
                query,
            },
        )
    }
}

fn error(message: String) -> FetchFuture {
    Box::pin(async move { Err(FetchError { message }) })
}

/// `collectHeaders(request, init)` — names are lowercased so case-variant duplicates
/// collapse and stripping is uniform.
fn collect_headers(headers: &[(String, String)]) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for (key, value) in headers {
        let name = key.to_lowercase();
        if STRIP_HEADERS.contains(&name.as_str()) {
            continue;
        }
        result.insert(name, value.clone());
    }
    result
}

/// `new URL(url)` reduced to `(origin, pathname + search)`.
fn split_url(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() || rest.is_empty() {
        return None;
    }
    let rest = rest.split('#').next().unwrap_or(rest);
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return None;
    }
    Some((
        format!("{}://{authority}", scheme.to_lowercase()),
        path.to_string(),
    ))
}

/// Resolve `.` and `..` segments, as `new URL()` does.
fn normalize_path(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "." => {}
            ".." => {
                segments.pop();
            }
            segment => segments.push(segment),
        }
    }
    let mut normalized = segments.join("/");
    if !normalized.starts_with('/') {
        normalized.insert(0, '/');
    }
    // A trailing `.`/`..` segment leaves a trailing slash, as in the URL parser.
    if (path.ends_with("/.") || path.ends_with("/..")) && !normalized.ends_with('/') {
        normalized.push('/');
    }
    normalized
}
