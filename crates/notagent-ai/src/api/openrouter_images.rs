use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::types::{
    AssistantImages, ImageContent, ImagesContext, ImagesInputContent, ImagesModel, ImagesOptions,
    ImagesOutputContent, ImagesStopReason, Modality, ProviderImages, TextContent, Usage, UsageCost,
};
use crate::utils::error_body::{RawProviderError, format_provider_error, normalize_provider_error};
use crate::utils::fetch::{FetchBody, FetchRequest, ReqwestFetch};
use crate::utils::headers::provider_headers_to_record;
use crate::utils::provider_retry::{
    ProviderErrorInfo, ProviderRetryError, ProviderRetryOptions, retry_provider_request,
};
use crate::utils::sanitize_unicode::sanitize_surrogates;

/// `buildParams(model, context)`
pub fn build_request_body(model: &ImagesModel, context: &ImagesContext) -> Value {
    let content: Vec<Value> = context
        .input
        .iter()
        .map(|item| match item {
            ImagesInputContent::Text(text) => json!({
                "type": "text",
                "text": sanitize_surrogates(&text.text),
            }),
            ImagesInputContent::Image(image) => json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{};base64,{}", image.mime_type, image.data),
                },
            }),
        })
        .collect();

    json!({
        "model": model.id,
        "messages": [{ "role": "user", "content": content }],
        "stream": false,
        "modalities": if model.output.contains(&Modality::Text) {
            json!(["image", "text"])
        } else {
            json!(["image"])
        },
    })
}

/// The URL the SDK builds from `baseURL` for `chat.completions.create`.
pub fn build_request_url(model: &ImagesModel) -> String {
    format!("{}/chat/completions", model.base_url.trim_end_matches('/'))
}

/// `defaultHeaders` of the client plus the SDK's own defaults.
pub fn build_request_headers(
    model: &ImagesModel,
    api_key: &str,
    options: Option<&ImagesOptions>,
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "application/json".to_string()),
        ("authorization".to_string(), format!("Bearer {api_key}")),
    ];

    // `{ ...model.headers, ...optionsHeaders }` — option values win per key.
    let mut merged: BTreeMap<String, Option<String>> = model
        .headers
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, Some(value)))
        .collect();
    if let Some(option_headers) = options.and_then(|options| options.base.headers.as_ref()) {
        for (key, value) in option_headers {
            merged.insert(key.clone(), value.clone());
        }
    }
    if let Some(defaults) = provider_headers_to_record(Some(&merged)) {
        for (name, value) in defaults {
            let lowered = name.to_lowercase();
            match headers
                .iter()
                .position(|(existing, _)| existing.to_lowercase() == lowered)
            {
                Some(index) => headers[index] = (name, value),
                None => headers.push((name, value)),
            }
        }
    }

    headers
}

/// `parseUsage(rawUsage, model)`
pub fn parse_usage(raw_usage: &Value, model: &ImagesModel) -> Usage {
    let prompt_tokens = raw_usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let details = raw_usage.get("prompt_tokens_details");
    let reported_cached_tokens = details
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write_tokens = details
        .and_then(|details| details.get("cache_write_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = if cache_write_tokens > 0 {
        reported_cached_tokens.saturating_sub(cache_write_tokens)
    } else {
        reported_cached_tokens
    };
    let input = prompt_tokens
        .saturating_sub(cache_read_tokens)
        .saturating_sub(cache_write_tokens);
    let output = raw_usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let cost_input = (model.cost.input / 1_000_000.0) * input as f64;
    let cost_output = (model.cost.output / 1_000_000.0) * output as f64;
    let cost_cache_read = (model.cost.cache_read / 1_000_000.0) * cache_read_tokens as f64;
    let cost_cache_write = (model.cost.cache_write / 1_000_000.0) * cache_write_tokens as f64;

    Usage {
        input,
        output,
        cache_read: cache_read_tokens,
        cache_write: cache_write_tokens,
        cache_write1h: None,
        reasoning: None,
        total_tokens: Some(input + output + cache_read_tokens + cache_write_tokens),
        cost: UsageCost {
            input: cost_input,
            output: cost_output,
            cache_read: cost_cache_read,
            cache_write: cost_cache_write,
            total: cost_input + cost_output + cost_cache_read + cost_cache_write,
        },
    }
}

/// A [`RawProviderError`] plus the response headers the retry policy inspects.
struct ImagesRequestError {
    raw: RawProviderError,
    headers: Vec<(String, String)>,
}

impl ProviderErrorInfo for ImagesRequestError {
    fn status(&self) -> Option<u16> {
        self.raw.status
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    fn message(&self) -> String {
        self.raw.message.clone()
    }
}

/// `generateImages(model, context, options)`
pub async fn generate_images(
    model: &ImagesModel,
    context: &ImagesContext,
    options: Option<ImagesOptions>,
) -> AssistantImages {
    let timestamp = crate::auth::resolve::now_ms();
    let mut output = AssistantImages {
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        output: Vec::new(),
        response_id: None,
        usage: None,
        stop_reason: ImagesStopReason::Stop,
        error_message: None,
        timestamp,
    };

    match run_request(model, context, options.as_ref(), &mut output).await {
        Ok(()) => output,
        Err(error) => {
            let aborted = options
                .as_ref()
                .and_then(|options| options.base.signal.as_ref())
                .is_some_and(tokio_util::sync::CancellationToken::is_cancelled);
            output.stop_reason = if aborted {
                ImagesStopReason::Aborted
            } else {
                ImagesStopReason::Error
            };
            output.error_message = Some(format_provider_error(
                &normalize_provider_error(&error),
                None,
            ));
            output
        }
    }
}

async fn run_request(
    model: &ImagesModel,
    context: &ImagesContext,
    options: Option<&ImagesOptions>,
    output: &mut AssistantImages,
) -> Result<(), RawProviderError> {
    let plain = |message: String| RawProviderError {
        status: None,
        body_text: None,
        body_json: None,
        message,
    };

    let Some(api_key) = options
        .and_then(|options| options.base.api_key.as_deref())
        .filter(|key| !key.is_empty())
    else {
        return Err(plain(format!(
            "No API key for provider: {}",
            model.provider
        )));
    };

    let mut params = build_request_body(model, context);
    if let Some(on_payload) = options.and_then(|options| options.base.on_payload.as_ref())
        && let Some(next) = on_payload(params.clone(), model).await
    {
        params = next;
    }

    let fetch = options
        .and_then(|options| options.base.fetch.clone())
        .unwrap_or_else(|| std::sync::Arc::new(ReqwestFetch::default()));
    let url = build_request_url(model);
    let headers = build_request_headers(model, api_key, options);
    let payload = serde_json::to_vec(&params).map_err(|error| plain(error.to_string()))?;

    let response = retry_provider_request(
        || {
            let fetch = fetch.clone();
            let url = url.clone();
            let headers = headers.clone();
            let payload = payload.clone();
            async move {
                let response = fetch
                    .fetch(FetchRequest {
                        method: "POST".to_string(),
                        url,
                        headers,
                        body: Some(payload),
                    })
                    .await
                    .map_err(|error| ImagesRequestError {
                        raw: RawProviderError {
                            status: None,
                            body_text: None,
                            body_json: None,
                            message: error.to_string(),
                        },
                        headers: Vec::new(),
                    })?;
                if !(200..300).contains(&response.status) {
                    let status = response.status;
                    let status_text = response.status_text.clone();
                    let headers = response.headers.clone();
                    // On the error path the status is the story; a body that
                    // breaks mid-stream degrades to a note instead of failing.
                    let body = read_body(response.body)
                        .await
                        .unwrap_or_else(|error| format!("(failed to read error body: {error})"));
                    return Err(ImagesRequestError {
                        raw: RawProviderError {
                            status: Some(status),
                            body_json: serde_json::from_str::<Value>(&body).ok(),
                            body_text: Some(body),
                            message: if status_text.is_empty() {
                                format!("{status} status code (no body)")
                            } else {
                                status_text
                            },
                        },
                        headers,
                    });
                }
                Ok(response)
            }
        },
        ProviderRetryOptions {
            max_retries: options.and_then(|options| options.base.max_retries),
            max_retry_delay_ms: options.and_then(|options| options.base.max_retry_delay_ms),
            signal: options.and_then(|options| options.base.signal.clone()),
        },
    )
    .await
    .map_err(|error| match error {
        ProviderRetryError::Request(error) => error.raw,
        ProviderRetryError::RetryDelayTooLong(message) => plain(message),
        ProviderRetryError::Aborted => plain("Request aborted".to_string()),
    })?;

    if let Some(on_response) = options.and_then(|options| options.base.on_response.as_ref()) {
        on_response(
            crate::types::ProviderResponse {
                status: response.status,
                headers: response.headers.iter().cloned().collect(),
            },
            model,
        )
        .await;
    }

    let body = read_body(response.body).await.map_err(plain)?;
    let parsed: Value = serde_json::from_str(&body).map_err(|error| plain(error.to_string()))?;

    output.response_id = parsed.get("id").and_then(Value::as_str).map(str::to_string);
    if let Some(usage) = parsed.get("usage").filter(|usage| !usage.is_null()) {
        output.usage = Some(parse_usage(usage, model));
    }

    let Some(choice) = parsed
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Ok(());
    };

    let message = choice.get("message");
    if let Some(content) = message
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
    {
        output.output.push(ImagesOutputContent::Text(TextContent {
            text: content.to_string(),
            ..TextContent::default()
        }));
    }

    let images = message
        .and_then(|message| message.get("images"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for image in images {
        let image_url = match image.get("image_url") {
            Some(Value::String(url)) => Some(url.clone()),
            Some(Value::Object(object)) => object
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        };
        let Some(image_url) = image_url.filter(|url| url.starts_with("data:")) else {
            continue;
        };
        // `/^data:([^;]+);base64,(.+)$/`
        let Some((mime_type, data)) = parse_data_url(&image_url) else {
            continue;
        };
        output
            .output
            .push(ImagesOutputContent::Image(ImageContent { data, mime_type }));
    }

    Ok(())
}

/// `/^data:([^;]+);base64,(.+)$/`
fn parse_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (mime_type, rest) = rest.split_once(';')?;
    if mime_type.is_empty() || mime_type.contains(';') {
        return None;
    }
    let data = rest.strip_prefix("base64,")?;
    (!data.is_empty()).then(|| (mime_type.to_string(), data.to_string()))
}

async fn read_body(body: FetchBody) -> Result<String, String> {
    match body {
        FetchBody::Bytes(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        FetchBody::Stream(mut receiver) => {
            let mut text = String::new();
            while let Some(chunk) = receiver.recv().await {
                match chunk {
                    Ok(chunk) => text.push_str(&String::from_utf8_lossy(&chunk)),
                    // truncated body must not pass as the response — it would
                    // surface as a misleading JSON parse error.
                    Err(error) => return Err(error.to_string()),
                }
            }
            Ok(text)
        }
    }
}

/// The module as a [`ProviderImages`] implementation.
pub struct OpenRouterImages;

impl ProviderImages for OpenRouterImages {
    fn generate_images(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: Option<ImagesOptions>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AssistantImages> + Send>> {
        let model = model.clone();
        let context = context.clone();
        Box::pin(async move { generate_images(&model, &context, options).await })
    }
}
