use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::utils::abort::{AbortError, race_with_abort_signal};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LlamaError(pub String);

impl From<AbortError> for LlamaError {
    fn from(error: AbortError) -> Self {
        LlamaError(error.to_string())
    }
}

/// `LlamaModelStatus = "unloaded" | "loading" | "loaded" | "downloading" | "sleeping"`
/// (`${model.id} is ${model.status.value}`). A Rust enum would have to invent a
/// catch-all variant and could not round-trip an unknown server value unchanged,
/// so the field keeps the string the server sent.
pub const LLAMA_STATUS_LOADED: &str = "loaded";
pub const LLAMA_STATUS_UNLOADED: &str = "unloaded";
pub const LLAMA_STATUS_DOWNLOADING: &str = "downloading";
pub const LLAMA_STATUS_SLEEPING: &str = "sleeping";

/// `LlamaModelInfo["status"]`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LlamaModelStatusInfo {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// `Record<string, { done, total }>`; kept as raw JSON because
    /// [`parse_download_progress`] reads it as `unknown` and tolerates any shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Value>,
}

/// `LlamaModelInfo["architecture"]`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LlamaArchitecture {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_modalities: Option<Vec<String>>,
}

/// `LlamaModelInfo["meta"]`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LlamaModelMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_ctx: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_ctx_train: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ftype: Option<String>,
}

/// `LlamaModelInfo`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlamaModelInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    pub status: LlamaModelStatusInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<LlamaArchitecture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<LlamaModelMeta>,
}

/// `LlamaModelEvent`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlamaModelEvent {
    pub model: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// `LlamaProgress`
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LlamaProgress {
    pub message: String,
    pub ratio: Option<f64>,
    pub detail: Option<String>,
}

/// Progress sink of [`LlamaClient::load_and_wait`] and
/// [`LlamaClient::download_and_wait`]; shared because both the SSE watcher and
/// the polling loop report through it.
pub type ProgressFn = Arc<dyn Fn(LlamaProgress) + Send + Sync>;

/// `errorMessage(payload, fallback)`
fn error_message(payload: Option<&Value>, fallback: &str) -> String {
    let Some(Value::Object(payload)) = payload else {
        return fallback.to_owned();
    };
    let Some(Value::Object(error)) = payload.get("error") else {
        return fallback.to_owned();
    };
    match error.get("message") {
        Some(Value::String(message)) if !message.is_empty() => message.clone(),
        _ => fallback.to_owned(),
    }
}

/// `isModelInfo(value)` — an id and a status string are enough.
fn is_model_info(value: &Value) -> bool {
    let Some(candidate) = value.as_object() else {
        return false;
    };
    candidate.get("id").is_some_and(Value::is_string)
        && candidate
            .get("status")
            .and_then(Value::as_object)
            .and_then(|status| status.get("value"))
            .is_some_and(Value::is_string)
}

/// `sleep(ms, signal)`
async fn sleep(milliseconds: u64, signal: Option<&CancellationToken>) -> Result<(), LlamaError> {
    race_with_abort_signal(
        tokio::time::sleep(Duration::from_millis(milliseconds)),
        signal,
    )
    .await?;
    Ok(())
}

/// `parseLoadProgress(data)`
fn parse_load_progress(data: Option<&Value>) -> Option<LlamaProgress> {
    let progress = data?.as_object()?.get("progress")?.as_object()?;
    let stage = match (progress.get("current"), progress.get("stage")) {
        (Some(Value::String(current)), _) => Some(current.clone()),
        (_, Some(Value::String(stage))) => Some(stage.clone()),
        _ => None,
    };
    let stages: Vec<String> = match progress.get("stages") {
        Some(Value::Array(stages)) => stages
            .iter()
            .filter_map(|entry| entry.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    let stage_ratio = progress
        .get("value")
        .and_then(Value::as_f64)
        .map(|value| value.clamp(0.0, 1.0));
    let mut ratio = stage_ratio;
    if let Some(stage) = &stage
        && !stages.is_empty()
        && let Some(index) = stages.iter().position(|entry| entry == stage)
    {
        #[expect(clippy::cast_precision_loss, reason = "stage counts are tiny")]
        let scale = stages.len() as f64;
        #[expect(clippy::cast_precision_loss, reason = "stage counts are tiny")]
        let position = index as f64;
        ratio = Some((position + stage_ratio.unwrap_or(0.0)) / scale);
    }
    Some(LlamaProgress {
        message: match &stage {
            Some(stage) => format!("Loading {}", stage.replace('_', " ")),
            None => "Loading model".to_owned(),
        },
        ratio,
        detail: None,
    })
}

/// `parseDownloadProgress(data)` — accepts both `{ progress: files }` and a bare
fn parse_download_progress(data: Option<&Value>) -> Option<LlamaProgress> {
    let data = data?.as_object()?;
    let files: &Map<String, Value> = match data.get("progress") {
        Some(Value::Object(nested)) => nested,
        Some(_) | None => data,
    };
    let mut done = 0.0_f64;
    let mut total = 0.0_f64;
    for value in files.values() {
        let Some(entry) = value.as_object() else {
            continue;
        };
        let (Some(entry_done), Some(entry_total)) = (
            entry.get("done").and_then(Value::as_f64),
            entry.get("total").and_then(Value::as_f64),
        ) else {
            continue;
        };
        done += entry_done;
        total += entry_total;
    }
    if total <= 0.0 {
        return None;
    }
    Some(LlamaProgress {
        message: "Downloading model".to_owned(),
        ratio: Some(done / total),
        detail: Some(format!("{} / {}", format_bytes(done), format_bytes(total))),
    })
}

/// `Number.prototype.toFixed(digits)` for the two widths `formatBytes` uses.
/// Deviation (class 3): Rust's `{:.n}` rounds ties to even, `toFixed` rounds them
/// away from zero, so the tie is nudged before formatting.
pub(crate) fn to_fixed(value: f64, digits: u32) -> String {
    let scale = 10_f64.powi(i32::try_from(digits).unwrap_or(0));
    let scaled = value * scale;
    let rounded = if (scaled.fract().abs() - 0.5).abs() < f64::EPSILON {
        scaled.abs().ceil().copysign(scaled)
    } else {
        scaled.round()
    };
    format!("{:.*}", digits as usize, rounded / scale)
}

/// `formatBytes(bytes)`
pub fn format_bytes(bytes: f64) -> String {
    if bytes < 1024.0 {
        return format!("{} B", notagent_ai::utils::js_number::to_js_string(bytes));
    }
    let units = ["KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes / 1024.0;
    let mut unit = units[0];
    let mut index = 1;
    while index < units.len() && value >= 1024.0 {
        value /= 1024.0;
        unit = units[index];
        index += 1;
    }
    if value >= 10.0 {
        format!("{} {unit}", to_fixed(value, 1))
    } else {
        format!("{} {unit}", to_fixed(value, 2))
    }
}

/// `normalizeLlamaServerUrl(value)` — management base URL without `/v1`.
/// Deviation (class 1): an input the WHATWG parser accepts but `url` rejects
/// (for example `127.0.0.1:8080`, where JS reads `127.0.0.1:` as the protocol)
/// reports `Invalid URL` — the message of the `TypeError` JS raises for inputs
/// neither parser accepts — instead of the http/https message.
pub fn normalize_llama_server_url(value: &str) -> Result<String, LlamaError> {
    let mut url =
        url::Url::parse(value.trim()).map_err(|_| LlamaError("Invalid URL".to_owned()))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(LlamaError("Server URL must use http or https".to_owned()));
    }
    url.set_fragment(None);
    url.set_query(None);
    let path = url.path().trim_end_matches('/').to_owned();
    let path = path.strip_suffix("/v1").unwrap_or(&path);
    url.set_path(if path.is_empty() { "/" } else { path });
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

/// `llamaInferenceUrl(serverUrl)` — the OpenAI-compatible `/v1` base.
pub fn llama_inference_url(server_url: &str) -> Result<String, LlamaError> {
    Ok(format!("{}/v1", normalize_llama_server_url(server_url)?))
}

/// `chunk.replaceAll("\r\n", "\n")` on the raw bytes; both are ASCII.
fn replace_crlf(chunk: &[u8]) -> Vec<u8> {
    let mut replaced = Vec::with_capacity(chunk.len());
    let mut index = 0;
    while index < chunk.len() {
        if chunk[index] == b'\r' && chunk.get(index + 1) == Some(&b'\n') {
            replaced.push(b'\n');
            index += 2;
            continue;
        }
        replaced.push(chunk[index]);
        index += 1;
    }
    replaced
}

/// `buffer.indexOf("\n\n")`
fn find_frame_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(2).position(|window| window == b"\n\n")
}

/// `LlamaClient`
/// Cloneable so `loadAndWait`/`downloadAndWait` can hand a client to the
#[derive(Clone)]
pub struct LlamaClient {
    pub server_url: String,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl LlamaClient {
    /// `new LlamaClient(serverUrl, apiKey?)`
    pub fn new(server_url: &str, api_key: Option<&str>) -> Result<Self, LlamaError> {
        Ok(LlamaClient {
            server_url: normalize_llama_server_url(server_url)?,
            api_key: api_key.map(str::to_owned),
            http: reqwest::Client::new(),
        })
    }

    /// `request(path, init)` — 15 s deadline, JSON in and out.
    async fn request(
        &self,
        path: &str,
        body: Option<Value>,
        signal: Option<&CancellationToken>,
    ) -> Result<Option<Value>, LlamaError> {
        let url = format!("{}{path}", self.server_url);
        let mut request = match &body {
            Some(body) => self
                .http
                .post(&url)
                .header("Content-Type", "application/json")
                .body(serde_json::to_string(body).expect("serializable body")),
            None => self.http.get(&url),
        };
        if let Some(api_key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }
        let response =
            race_with_abort_signal(request.timeout(Duration::from_secs(15)).send(), signal)
                .await?
                .map_err(|error| LlamaError(error.to_string()))?;
        let status = response.status().as_u16();
        // non-JSON body — an abort mid-body included — becomes `undefined`.
        let payload = response
            .text()
            .await
            .ok()
            .and_then(|body| serde_json::from_str::<Value>(&body).ok());
        if !(200..300).contains(&status) {
            return Err(LlamaError(error_message(
                payload.as_ref(),
                &format!("llama.cpp returned HTTP {status}"),
            )));
        }
        Ok(payload)
    }

    /// `list({ reload?, signal? })`
    pub async fn list(
        &self,
        reload: bool,
        signal: Option<&CancellationToken>,
    ) -> Result<Vec<LlamaModelInfo>, LlamaError> {
        let path = if reload {
            "/models?reload=1"
        } else {
            "/models"
        };
        let payload = self.request(path, None, signal).await?;
        let data = payload
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|payload| payload.get("data"))
            .and_then(Value::as_array)
            .ok_or_else(|| LlamaError("llama.cpp returned an invalid model catalog".to_owned()))?;
        if !data.iter().all(is_model_info) {
            return Err(LlamaError(
                "Server is not running in llama.cpp router mode".to_owned(),
            ));
        }
        data.iter()
            .map(|entry| {
                serde_json::from_value(entry.clone()).map_err(|error| LlamaError(error.to_string()))
            })
            .collect()
    }

    /// `load(model, signal)`
    pub async fn load(
        &self,
        model: &str,
        signal: Option<&CancellationToken>,
    ) -> Result<(), LlamaError> {
        self.request("/models/load", Some(json!({ "model": model })), signal)
            .await?;
        Ok(())
    }

    /// `unload(model, signal)`
    pub async fn unload(
        &self,
        model: &str,
        signal: Option<&CancellationToken>,
    ) -> Result<(), LlamaError> {
        self.request("/models/unload", Some(json!({ "model": model })), signal)
            .await?;
        Ok(())
    }

    /// `unloadAndWait(model, signal)`
    pub async fn unload_and_wait(
        &self,
        model: &str,
        signal: Option<&CancellationToken>,
    ) -> Result<(), LlamaError> {
        self.unload(model, signal).await?;
        loop {
            let entry = self
                .list(false, signal)
                .await?
                .into_iter()
                .find(|candidate| candidate.id == model);
            match entry {
                None => return Ok(()),
                Some(entry) if entry.status.value == LLAMA_STATUS_UNLOADED => return Ok(()),
                Some(_) => {}
            }
            sleep(100, signal).await?;
        }
    }

    /// `download(model, signal)`
    pub async fn download(
        &self,
        model: &str,
        signal: Option<&CancellationToken>,
    ) -> Result<(), LlamaError> {
        self.request("/models", Some(json!({ "model": model })), signal)
            .await?;
        Ok(())
    }

    /// `watch(onEvent, signal)` — SSE frames of `/models/sse`.
    pub async fn watch(
        &self,
        on_event: impl Fn(LlamaModelEvent) + Send,
        signal: Option<&CancellationToken>,
    ) -> Result<(), LlamaError> {
        let mut request = self.http.get(format!("{}/models/sse", self.server_url));
        if let Some(api_key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }
        let response = race_with_abort_signal(request.send(), signal)
            .await?
            .map_err(|error| LlamaError(error.to_string()))?;
        if !response.status().is_success() {
            return Err(LlamaError(format!(
                "llama.cpp SSE returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let mut stream = response.bytes_stream();
        // Byte buffer, not a String: `TextDecoder(…, { stream: true })` holds an
        // incomplete multi-byte sequence back until the next chunk, and decoding
        // each chunk on its own would turn a character split across two TCP
        // reads into replacement characters. Frames are cut on ASCII bytes, so
        // each complete frame decodes cleanly.
        let mut buffer: Vec<u8> = Vec::new();
        while let Some(chunk) = race_with_abort_signal(stream.next(), signal).await? {
            let chunk = chunk.map_err(|error| LlamaError(error.to_string()))?;
            // there too.
            buffer.extend_from_slice(&replace_crlf(&chunk));
            while let Some(boundary) = find_frame_end(&buffer) {
                let frame = String::from_utf8_lossy(&buffer[..boundary]).into_owned();
                buffer.drain(..boundary + 2);
                let data = frame
                    .split('\n')
                    .filter(|line| line.starts_with("data:"))
                    .map(|line| line[5..].trim_start())
                    .collect::<Vec<_>>()
                    .join("\n");
                if data.is_empty() {
                    continue;
                }
                // Malformed events are ignored; catalog polling stays authoritative.
                if let Ok(event) = serde_json::from_str::<LlamaModelEvent>(&data) {
                    on_event(event);
                }
            }
        }
        Ok(())
    }

    /// `loadAndWait(model, onProgress, signal)`
    pub async fn load_and_wait(
        &self,
        model: &str,
        on_progress: ProgressFn,
        signal: Option<&CancellationToken>,
    ) -> Result<LlamaModelInfo, LlamaError> {
        // A child token is `linkSignal` plus `watcher.abort()` in one: it follows
        // the caller's signal and is cancelled when this call leaves.
        let watcher = signal.map_or_else(CancellationToken::new, CancellationToken::child_token);
        let event_loaded = Arc::new(AtomicBool::new(false));
        let event_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let task = {
            let client = self.clone();
            let model = model.to_owned();
            let watcher = watcher.clone();
            let event_loaded = Arc::clone(&event_loaded);
            let event_error = Arc::clone(&event_error);
            let on_progress = Arc::clone(&on_progress);
            tokio::spawn(async move {
                let _ = client
                    .watch(
                        |event| {
                            if event.model != model {
                                return;
                            }
                            if event.event != "model_status" && event.event != "status_change" {
                                return;
                            }
                            let status = event
                                .data
                                .as_ref()
                                .and_then(Value::as_object)
                                .and_then(|data| data.get("status"))
                                .and_then(Value::as_str);
                            if status == Some(LLAMA_STATUS_LOADED) {
                                event_loaded.store(true, Ordering::SeqCst);
                            }
                            if status == Some(LLAMA_STATUS_UNLOADED) {
                                *event_error.lock().expect("poisoned") =
                                    Some("Model failed to load".to_owned());
                            }
                            if let Some(progress) = parse_load_progress(event.data.as_ref()) {
                                on_progress(progress);
                            }
                        },
                        Some(&watcher),
                    )
                    .await;
            })
        };

        let outcome = self
            .poll_until_loaded(model, &on_progress, signal, &event_loaded, &event_error)
            .await;
        watcher.cancel();
        task.abort();
        outcome
    }

    async fn poll_until_loaded(
        &self,
        model: &str,
        on_progress: &ProgressFn,
        signal: Option<&CancellationToken>,
        event_loaded: &AtomicBool,
        event_error: &Mutex<Option<String>>,
    ) -> Result<LlamaModelInfo, LlamaError> {
        self.load(model, signal).await?;
        on_progress(LlamaProgress {
            message: "Loading model".to_owned(),
            ..LlamaProgress::default()
        });
        loop {
            if signal.is_some_and(CancellationToken::is_cancelled) {
                return Err(AbortError.into());
            }
            let entry = self
                .list(false, signal)
                .await?
                .into_iter()
                .find(|candidate| candidate.id == model);
            if let Some(entry) = &entry
                && entry.status.value == LLAMA_STATUS_LOADED
            {
                return Ok(entry.clone());
            }
            if event_loaded.load(Ordering::SeqCst) && entry.is_none() {
                return Ok(LlamaModelInfo {
                    id: model.to_owned(),
                    aliases: None,
                    status: LlamaModelStatusInfo {
                        value: LLAMA_STATUS_LOADED.to_owned(),
                        ..LlamaModelStatusInfo::default()
                    },
                    architecture: None,
                    source: None,
                    meta: None,
                });
            }
            let failure = event_error.lock().expect("poisoned").clone();
            if entry
                .as_ref()
                .is_some_and(|entry| entry.status.failed == Some(true))
                || failure.is_some()
            {
                let exit_code = entry.as_ref().and_then(|entry| entry.status.exit_code);
                return Err(LlamaError(match exit_code {
                    None => failure.unwrap_or_else(|| "Model failed to load".to_owned()),
                    Some(exit_code) => format!("Model exited with code {exit_code}"),
                }));
            }
            sleep(250, signal).await?;
        }
    }

    /// `downloadAndWait(model, onProgress, signal)`
    pub async fn download_and_wait(
        &self,
        model: &str,
        on_progress: ProgressFn,
        signal: Option<&CancellationToken>,
    ) -> Result<Vec<LlamaModelInfo>, LlamaError> {
        let watcher = signal.map_or_else(CancellationToken::new, CancellationToken::child_token);
        let finished = Arc::new(AtomicBool::new(false));
        let failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let saw_downloading = Arc::new(AtomicBool::new(false));
        let task = {
            let client = self.clone();
            let model = model.to_owned();
            let watcher = watcher.clone();
            let finished = Arc::clone(&finished);
            let failure = Arc::clone(&failure);
            let saw_downloading = Arc::clone(&saw_downloading);
            let on_progress = Arc::clone(&on_progress);
            tokio::spawn(async move {
                let _ = client
                    .watch(
                        |event| {
                            if event.model != model {
                                return;
                            }
                            if event.event == "download_finished" {
                                finished.store(true, Ordering::SeqCst);
                            }
                            if event.event == "download_failed" {
                                *failure.lock().expect("poisoned") =
                                    Some(error_message(event.data.as_ref(), "Download failed"));
                            }
                            if event.event == "download_progress" {
                                saw_downloading.store(true, Ordering::SeqCst);
                                if let Some(progress) = parse_download_progress(event.data.as_ref())
                                {
                                    on_progress(progress);
                                }
                            }
                        },
                        Some(&watcher),
                    )
                    .await;
            })
        };

        let outcome = self
            .poll_until_downloaded(
                model,
                &on_progress,
                signal,
                &finished,
                &failure,
                &saw_downloading,
            )
            .await;
        watcher.cancel();
        task.abort();
        outcome
    }

    async fn poll_until_downloaded(
        &self,
        model: &str,
        on_progress: &ProgressFn,
        signal: Option<&CancellationToken>,
        finished: &AtomicBool,
        failure: &Mutex<Option<String>>,
        saw_downloading: &AtomicBool,
    ) -> Result<Vec<LlamaModelInfo>, LlamaError> {
        self.download(model, signal).await?;
        on_progress(LlamaProgress {
            message: "Downloading model".to_owned(),
            ..LlamaProgress::default()
        });
        let mut polls = 0_u64;
        loop {
            if signal.is_some_and(CancellationToken::is_cancelled) {
                return Err(AbortError.into());
            }
            if let Some(failure) = failure.lock().expect("poisoned").clone() {
                return Err(LlamaError(failure));
            }
            let models = self.list(false, signal).await?;
            polls += 1;
            let entry = models.into_iter().find(|candidate| candidate.id == model);
            if let Some(entry) = &entry
                && entry.status.value == LLAMA_STATUS_DOWNLOADING
            {
                saw_downloading.store(true, Ordering::SeqCst);
                if let Some(progress) = parse_download_progress(entry.status.progress.as_ref()) {
                    on_progress(progress);
                }
            } else if finished.load(Ordering::SeqCst)
                || (entry.is_some() && (saw_downloading.load(Ordering::SeqCst) || polls >= 2))
            {
                return self.list(true, signal).await;
            }
            sleep(500, signal).await?;
        }
    }
}
