use std::time::Duration;

const RETRYABLE_STATUS_CODES: [u16; 7] = [408, 425, 429, 500, 502, 503, 504];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchRetryOptions {
    /// Additional attempts after the first one. Defaults to two.
    pub max_retries: u32,
    /// Retry transient HTTP responses as well as transport failures.
    pub retry_on_status: bool,
    /// Overall time budget shared by all attempts.
    pub timeout: Option<Duration>,
}

impl Default for FetchRetryOptions {
    fn default() -> Self {
        Self {
            max_retries: 2,
            retry_on_status: true,
            timeout: None,
        }
    }
}

/// Send `request` with a bounded immediate retry.
/// Deviation (class 3): `fetch` + `AbortSignal.timeout` become `reqwest` with a
/// deadline; the request is rebuilt per attempt because a `reqwest::Request`
/// cannot be cloned when it carries a streaming body.
pub async fn fetch_with_retry(
    client: &reqwest::Client,
    build_request: impl Fn() -> reqwest::RequestBuilder,
    options: FetchRetryOptions,
) -> Result<reqwest::Response, String> {
    let _ = client;
    let deadline = options
        .timeout
        .map(|timeout| std::time::Instant::now() + timeout);
    let mut attempt = 0u32;
    loop {
        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            return Err("The operation timed out".to_owned());
        }
        let mut request = build_request();
        if let Some(deadline) = deadline {
            request =
                request.timeout(deadline.saturating_duration_since(std::time::Instant::now()));
        }
        match request.send().await {
            Ok(response) => {
                let should_retry = options.retry_on_status
                    && RETRYABLE_STATUS_CODES.contains(&response.status().as_u16())
                    && attempt < options.max_retries;
                if !should_retry {
                    return Ok(response);
                }
            }
            Err(error) => {
                if attempt >= options.max_retries || error.is_timeout() {
                    return Err(error.to_string());
                }
            }
        }
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_public_options() {
        let options = FetchRetryOptions::default();
        assert_eq!(options.max_retries, 2);
        assert!(options.retry_on_status);
        assert_eq!(options.timeout, None);
    }

    #[test]
    fn the_retryable_status_list_matches() {
        assert_eq!(RETRYABLE_STATUS_CODES, [408, 425, 429, 500, 502, 503, 504]);
    }
}
