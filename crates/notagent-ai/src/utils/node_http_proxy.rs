//! Proxy resolution for provider requests.
//!
//! 1:1 port of `packages/ai/src/utils/node-http-proxy.ts` (112 LOC). The TS module
//! builds undici proxy agents; in Rust the resolved URL is handed to reqwest
//! (substitution class 3 of the master plan).

use crate::types::ProviderEnv;
use crate::utils::provider_env::get_provider_env_value;

/// Default ports per protocol (`DEFAULT_PROXY_PORTS`).
fn default_proxy_port(protocol: &str) -> u16 {
    match protocol {
        "ftp" => 21,
        "gopher" => 70,
        "http" => 80,
        "https" => 443,
        "ws" => 80,
        "wss" => 443,
        _ => 0,
    }
}

pub const UNSUPPORTED_PROXY_PROTOCOL_MESSAGE: &str = "Unsupported proxy protocol. SOCKS and PAC proxy URLs are not supported; use an HTTP or HTTPS proxy URL.";

/// `getProxyEnv(key, env)` — lower-case first, then upper-case, scoped before ambient.
fn get_proxy_env(key: &str, env: Option<&ProviderEnv>) -> String {
    let lowercase_key = key.to_lowercase();
    let uppercase_key = key.to_uppercase();
    if let Some(env) = env {
        for candidate in [&lowercase_key, &uppercase_key] {
            if let Some(value) = env.get(candidate)
                && !value.is_empty()
            {
                return value.clone();
            }
        }
    }
    for candidate in [&lowercase_key, &uppercase_key] {
        if let Some(value) = get_provider_env_value(candidate, None) {
            return value;
        }
    }
    String::new()
}

/// A parsed target URL reduced to what the proxy rules need.
struct TargetUrl {
    protocol: String,
    hostname: String,
    port: u16,
}

/// Minimal URL split; the TS code uses the WHATWG `URL` parser.
fn parse_proxy_target_url(target_url: &str) -> Option<TargetUrl> {
    let (protocol, rest) = target_url.split_once("://")?;
    if protocol.is_empty() || rest.is_empty() {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.is_empty() {
        return None;
    }
    let (hostname, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
            (host.to_string(), port.parse::<u16>().unwrap_or(0))
        }
        _ => (authority.to_string(), 0),
    };
    let port = if port == 0 {
        default_proxy_port(protocol)
    } else {
        port
    };
    Some(TargetUrl {
        protocol: protocol.to_string(),
        hostname,
        port,
    })
}

/// `shouldProxyHostname(hostname, port, env)` — evaluates `no_proxy`.
fn should_proxy_hostname(hostname: &str, port: u16, env: Option<&ProviderEnv>) -> bool {
    let no_proxy = get_proxy_env("no_proxy", env).to_lowercase();
    if no_proxy.is_empty() {
        return true;
    }
    if no_proxy == "*" {
        return false;
    }

    no_proxy.split([',', ' ', '\t', '\n', '\r']).all(|entry| {
        if entry.is_empty() {
            return true;
        }
        let (mut proxy_hostname, proxy_port) = match entry.rsplit_once(':') {
            Some((host, port_text))
                if port_text.chars().all(|c| c.is_ascii_digit()) && !port_text.is_empty() =>
            {
                (host.to_string(), port_text.parse::<u16>().unwrap_or(0))
            }
            _ => (entry.to_string(), 0),
        };
        if proxy_port != 0 && proxy_port != port {
            return true;
        }
        if !proxy_hostname.starts_with('.') && !proxy_hostname.starts_with('*') {
            return hostname != proxy_hostname;
        }
        if proxy_hostname.starts_with('*') {
            proxy_hostname = proxy_hostname[1..].to_string();
        }
        !hostname.ends_with(&proxy_hostname)
    })
}

/// `getProxyForUrl(targetUrl, env)`
fn get_proxy_for_url(target_url: &str, env: Option<&ProviderEnv>) -> String {
    let Some(parsed) = parse_proxy_target_url(target_url) else {
        return String::new();
    };
    if !should_proxy_hostname(&parsed.hostname, parsed.port, env) {
        return String::new();
    }

    let mut proxy = get_proxy_env(&format!("{}_proxy", parsed.protocol), env);
    if proxy.is_empty() {
        proxy = get_proxy_env("all_proxy", env);
    }
    if !proxy.is_empty() && !proxy.contains("://") {
        proxy = format!("{}://{proxy}", parsed.protocol);
    }
    proxy
}

/// `new URL(proxy).toString()` — the WHATWG parser appends the root path.
fn normalize_proxy_url(proxy: &str) -> String {
    match proxy.split_once("://") {
        Some((_, rest)) if !rest.contains('/') => format!("{proxy}/"),
        _ => proxy.to_string(),
    }
}

/// `resolveHttpProxyUrlForTarget(targetUrl, env)`
pub fn resolve_http_proxy_url_for_target(
    target_url: &str,
    env: Option<&ProviderEnv>,
) -> Result<Option<String>, String> {
    let proxy = get_proxy_for_url(target_url, env);
    if proxy.is_empty() {
        return Ok(None);
    }

    let Some((protocol, _)) = proxy.split_once("://") else {
        return Err(format!(
            "Invalid proxy URL {}: cannot be parsed",
            serde_json::json!(proxy)
        ));
    };
    if protocol != "http" && protocol != "https" {
        return Err(format!(
            "{UNSUPPORTED_PROXY_PROTOCOL_MESSAGE} Got {protocol}:"
        ));
    }
    Ok(Some(normalize_proxy_url(&proxy)))
}
