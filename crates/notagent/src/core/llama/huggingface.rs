//! Port of `packages/coding-agent/src/extensions/llama/huggingface.ts` (158 LOC).
//!
//! Model search on huggingface.co: GGUF repositories, their quantizations and
//! whether access is gated, plus the token lookup the CLI shares with `hf`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::client::LlamaError;
use crate::utils::abort::race_with_abort_signal;

const DEFAULT_HUGGING_FACE_URL: &str = "https://huggingface.co";

/// `QUANTIZATION_PATTERN` — the trailing quantization tag of a GGUF file stem.
fn quantization_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?i)(?:^|[-_.])((?:UD-)?(?:IQ\d(?:_[A-Z0-9]+)+|Q\d(?:_[A-Z0-9]+)+|BF16|F16|F32|MXFP\d(?:_[A-Z0-9]+)*))$",
        )
        .expect("static pattern")
    })
}

/// `SHARD_SUFFIX_PATTERN` — `-00001-of-00002`.
fn shard_suffix_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"-\d{5}-of-\d{5}$").expect("static pattern"))
}

/// `RATE_LIMIT` — the `t=<seconds>` field of the `ratelimit` header.
fn rate_limit_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?:^|;)t=(\d+)").expect("static pattern"))
}

/// `HuggingFaceModel`
#[derive(Debug, Clone, PartialEq)]
pub struct HuggingFaceModel {
    pub id: String,
    pub downloads: f64,
}

/// `HuggingFaceQuantization`
#[derive(Debug, Clone, PartialEq)]
pub struct HuggingFaceQuantization {
    pub name: String,
    pub size: Option<f64>,
}

/// `HuggingFaceModelDetails["gated"] = false | "auto" | "manual"`
///
/// No serde derive on purpose: the union mixes a boolean with two strings and
/// TS never writes it back out, so there is no wire format to reproduce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HuggingFaceGated {
    /// TS: `false`.
    #[default]
    No,
    Auto,
    Manual,
}

/// `HuggingFaceModelDetails`
#[derive(Debug, Clone, PartialEq)]
pub struct HuggingFaceModelDetails {
    pub id: String,
    pub gated: HuggingFaceGated,
    pub quantizations: Vec<HuggingFaceQuantization>,
}

/// `payloadError(payload, fallback)` — Hugging Face reports `error` as a string.
fn payload_error(payload: Option<&Value>, fallback: &str) -> String {
    match payload
        .and_then(Value::as_object)
        .and_then(|payload| payload.get("error"))
    {
        Some(Value::String(error)) if !error.is_empty() => error.clone(),
        _ => fallback.to_owned(),
    }
}

/// `parseRateLimitDelay(value)`
fn parse_rate_limit_delay(value: Option<&str>) -> Option<f64> {
    let captures = rate_limit_pattern().captures(value?)?;
    captures.get(1)?.as_str().parse().ok()
}

/// `readToken(path)`
async fn read_token(path: &PathBuf) -> Option<String> {
    let token = tokio::fs::read_to_string(path).await.ok()?;
    let token = token.trim().to_owned();
    (!token.is_empty()).then_some(token)
}

/// `findHuggingFaceToken(env)` — `HF_TOKEN` first, then the token files `hf` writes.
///
/// The environment is passed in, as in TS, so the ported suite can drive it.
pub async fn find_hugging_face_token(env: &BTreeMap<String, String>) -> Option<String> {
    let from_environment = env.get("HF_TOKEN").map(|value| value.trim()).unwrap_or("");
    if !from_environment.is_empty() {
        return Some(from_environment.to_owned());
    }

    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(path) = env.get("HF_TOKEN_PATH") {
        paths.push(PathBuf::from(path));
    }
    if let Some(home) = env.get("HF_HOME") {
        paths.push(PathBuf::from(home).join("token"));
    }
    if let Some(cache) = env.get("XDG_CACHE_HOME") {
        paths.push(PathBuf::from(cache).join("huggingface").join("token"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".cache").join("huggingface").join("token"));
    }
    let mut seen: Vec<&PathBuf> = Vec::new();
    for path in &paths {
        // TS deduplicates through `new Set(paths)`, which keeps insertion order.
        if seen.contains(&path) {
            continue;
        }
        seen.push(path);
        if let Some(token) = read_token(path).await {
            return Some(token);
        }
    }
    None
}

/// `findHuggingFaceToken()` with TS's default argument, `process.env`.
pub async fn find_hugging_face_token_from_process_env() -> Option<String> {
    find_hugging_face_token(&std::env::vars().collect()).await
}

/// `HuggingFaceClient`
pub struct HuggingFaceClient {
    token: Option<String>,
    base_url: String,
    http: reqwest::Client,
}

impl HuggingFaceClient {
    /// `new HuggingFaceClient(token?, baseUrl?)`
    pub fn new(token: Option<&str>, base_url: Option<&str>) -> Self {
        HuggingFaceClient {
            token: token.map(str::to_owned),
            base_url: base_url
                .unwrap_or(DEFAULT_HUGGING_FACE_URL)
                .trim_end_matches('/')
                .to_owned(),
            http: reqwest::Client::new(),
        }
    }

    /// `request(path, signal)` — 15 s deadline; 429 gets the rate-limit message.
    async fn request(
        &self,
        path: &str,
        signal: Option<&CancellationToken>,
    ) -> Result<Option<Value>, LlamaError> {
        let mut request = self.http.get(format!("{}{path}", self.base_url));
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response =
            race_with_abort_signal(request.timeout(Duration::from_secs(15)).send(), signal)
                .await?
                .map_err(|error| LlamaError(error.to_string()))?;
        let status = response.status().as_u16();
        let retry_after = header_value(&response, "retry-after");
        let rate_limit = header_value(&response, "ratelimit");
        let payload = response
            .text()
            .await
            .ok()
            .and_then(|body| serde_json::from_str::<Value>(&body).ok());
        if !(200..300).contains(&status) {
            let fallback = format!("Hugging Face returned HTTP {status}");
            if status == 429 {
                // `Number(header) || parse(ratelimit)`: 0, NaN and a missing
                // header all fall through to the `ratelimit` field.
                let delay = retry_after
                    .as_deref()
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|delay| *delay != 0.0)
                    .or_else(|| parse_rate_limit_delay(rate_limit.as_deref()));
                return Err(LlamaError(match delay {
                    Some(delay) => format!(
                        "Hugging Face rate limit reached; retry in {}s",
                        notagent_ai::utils::js_number::to_js_string(delay)
                    ),
                    None => "Hugging Face rate limit reached".to_owned(),
                }));
            }
            return Err(LlamaError(payload_error(payload.as_ref(), &fallback)));
        }
        Ok(payload)
    }

    /// `search(query, signal)` — the 20 most downloaded GGUF repositories.
    pub async fn search(
        &self,
        query: &str,
        signal: Option<&CancellationToken>,
    ) -> Result<Vec<HuggingFaceModel>, LlamaError> {
        let params = [
            ("search", query),
            ("filter", "gguf"),
            ("sort", "downloads"),
            ("direction", "-1"),
            ("limit", "20"),
        ]
        .iter()
        .map(|(name, value)| format!("{name}={}", urlencode_query(value)))
        .collect::<Vec<_>>()
        .join("&");
        let payload = self
            .request(&format!("/api/models?{params}"), signal)
            .await?;
        let Some(Value::Array(entries)) = payload else {
            return Err(LlamaError(
                "Hugging Face returned invalid search results".to_owned(),
            ));
        };
        Ok(entries
            .iter()
            .filter_map(|value| {
                let model = value.as_object()?;
                let id = model.get("id")?.as_str()?.to_owned();
                Some(HuggingFaceModel {
                    id,
                    downloads: model
                        .get("downloads")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
                })
            })
            .collect())
    }

    /// `details(id, signal)` — quantizations with summed shard sizes.
    pub async fn details(
        &self,
        id: &str,
        signal: Option<&CancellationToken>,
    ) -> Result<HuggingFaceModelDetails, LlamaError> {
        let encoded_id = id
            .split('/')
            .map(urlencode_component)
            .collect::<Vec<_>>()
            .join("/");
        let payload = self
            .request(&format!("/api/models/{encoded_id}?blobs=true"), signal)
            .await?;
        let Some(Value::Object(model)) = payload else {
            return Err(LlamaError(
                "Hugging Face returned invalid model details".to_owned(),
            ));
        };
        // Insertion-ordered like the JS `Map`, so the sort below stays stable.
        let mut sizes: Vec<(String, f64, bool)> = Vec::new();
        if let Some(Value::Array(siblings)) = model.get("siblings") {
            for value in siblings {
                let Some(file) = value.as_object() else {
                    continue;
                };
                let Some(rfilename) = file.get("rfilename").and_then(Value::as_str) else {
                    continue;
                };
                if !rfilename.to_lowercase().ends_with(".gguf") {
                    continue;
                }
                let filename = rfilename.rsplit('/').next().unwrap_or(rfilename);
                if filename.to_lowercase().starts_with("mmproj") {
                    continue;
                }
                let stem = &filename[..filename.len() - 5];
                let stem = shard_suffix_pattern().replace(stem, "");
                let Some(quantization) = quantization_pattern()
                    .captures(&stem)
                    .and_then(|captures| captures.get(1))
                    .map(|found| found.as_str().to_uppercase())
                else {
                    continue;
                };
                let index = match sizes.iter().position(|(name, _, _)| *name == quantization) {
                    Some(index) => index,
                    None => {
                        sizes.push((quantization, 0.0, true));
                        sizes.len() - 1
                    }
                };
                match file.get("size").and_then(Value::as_f64) {
                    Some(size) => sizes[index].1 += size,
                    None => sizes[index].2 = false,
                }
            }
        }
        let quantizations: Vec<HuggingFaceQuantization> = sizes
            .into_iter()
            .map(|(name, total, complete)| HuggingFaceQuantization {
                name,
                size: complete.then_some(total),
            })
            .collect();
        // TS answers `-1`/`1` whenever `Q4_K_M` is involved, so it beats every
        // other name; the rest sorts by size, ties by name. Expressed as a
        // partition plus a proper total order, because Rust's sort rejects a
        // comparator that is not one (TS would call `Q4_K_M < Q4_K_M`).
        let mut quantizations: Vec<HuggingFaceQuantization> = quantizations;
        quantizations.sort_by(|left, right| {
            let left_size = left.size.unwrap_or(MAX_SAFE_INTEGER);
            let right_size = right.size.unwrap_or(MAX_SAFE_INTEGER);
            left_size
                .total_cmp(&right_size)
                .then_with(|| compare_locale(&left.name, &right.name))
        });
        if let Some(index) = quantizations
            .iter()
            .position(|entry| entry.name == "Q4_K_M")
        {
            let preferred = quantizations.remove(index);
            quantizations.insert(0, preferred);
        }
        Ok(HuggingFaceModelDetails {
            id: model
                .get("id")
                .and_then(Value::as_str)
                .map_or_else(|| id.to_owned(), str::to_owned),
            gated: match model.get("gated").and_then(Value::as_str) {
                Some("auto") => HuggingFaceGated::Auto,
                Some("manual") => HuggingFaceGated::Manual,
                _ => HuggingFaceGated::No,
            },
            quantizations,
        })
    }
}

/// `Number.MAX_SAFE_INTEGER`
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// `String.prototype.localeCompare` for the ASCII names quantizations use.
fn compare_locale(left: &str, right: &str) -> std::cmp::Ordering {
    left.cmp(right)
}

fn header_value(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// `URLSearchParams` — like `encodeURIComponent`, but the space becomes `+`.
fn urlencode_query(value: &str) -> String {
    urlencode(value, true)
}

/// `encodeURIComponent(value)`
fn urlencode_component(value: &str) -> String {
    urlencode(value, false)
}

fn urlencode(value: &str, space_as_plus: bool) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => encoded.push(byte as char),
            b' ' if space_as_plus => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}
