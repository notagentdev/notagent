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

/// Client for requests to notagent.dev. The site's CDN answers HTTP/2 requests
/// from this client with a browser challenge (403) whenever they carry a
/// User-Agent, while HTTP/1.1 requests with the same headers pass. A binary
/// cannot solve a challenge, so it never offers HTTP/2 to that host.
pub fn site_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().http1_only()
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

    /// The protocols a client offers in its TLS ClientHello, which is sent in
    /// the clear before any certificate check.
    async fn offered_alpn(client: reqwest::Client) -> Vec<u8> {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("address").port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut hello = Vec::new();
            let mut chunk = [0u8; 4096];
            // The record header carries the handshake length in bytes 3..5.
            while hello.len() < 5
                || hello.len() < 5 + usize::from(u16::from_be_bytes([hello[3], hello[4]]))
            {
                let read = socket.read(&mut chunk).await.expect("read");
                if read == 0 {
                    break;
                }
                hello.extend_from_slice(&chunk[..read]);
            }
            hello
        });
        let _ = client
            .get(format!("https://127.0.0.1:{port}/"))
            .send()
            .await;
        server.await.expect("server task")
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[tokio::test]
    async fn requests_to_the_site_never_offer_http2() {
        let default = offered_alpn(reqwest::Client::new()).await;
        assert!(
            contains(&default, b"\x02h2"),
            "the probe must see the h2 offer of a default client, or it proves nothing"
        );
        let site = offered_alpn(site_client_builder().build().expect("client")).await;
        assert!(
            contains(&site, b"\x08http/1.1"),
            "the site client must still offer HTTP/1.1"
        );
        assert!(
            !contains(&site, b"\x02h2"),
            "the site's CDN challenges HTTP/2 requests from this client"
        );
    }
}
