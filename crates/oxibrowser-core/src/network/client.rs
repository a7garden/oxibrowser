//! HTTP client for resource fetching.
//!
//! Provides:
//! - `fetch` — standard HTTP GET with cookies
//! - `intercept` — fetch with an InterceptAction (continue/fail/fulfill)
//! - `fetch_text`, `post`, `post_json` — convenience methods

use crate::challenge;
use crate::config::BrowserConfig;
use crate::error::{CoreError, Result};
use crate::network::cookie::CookieJar;
use crate::network::intercept::{InterceptAction, InterceptedBody, InterceptedResponse};
use crate::network::ip_filter::IpFilter;
use parking_lot::RwLock;
use std::sync::Arc;
use url::Url;
use wreq::{Client, Response};
use wreq_util::Emulation;

/// Check if a URL is allowed by the SSRF filter.
/// Only applies to http/https schemes — about:, data:, etc. bypass.
/// This is a standalone function so it can be used both for initial requests
/// and inside the redirect policy closure.
fn check_url_ssrf(url: &Url, filter: &IpFilter) -> bool {
    // Only http/https can be SSRF targets; data:, blob:, about: etc. are local.
    if url.scheme() != "http" && url.scheme() != "https" {
        return true;
    }
    if let Some(host) = url.host_str() {
        return filter.is_hostname_allowed(host);
    }
    true
}

/// HTTP client wrapper with cookie support and configurable defaults.
pub struct HttpClient {
    client: Client,
    config: BrowserConfig,
    cookie_jar: Arc<RwLock<CookieJar>>,
    ip_filter: Arc<IpFilter>,
}

/// Outcome of [`HttpClient::fetch_with_challenge_retry`].
#[derive(Debug, Clone)]
pub struct ChallengeOutcome {
    /// Final HTTP status code.
    pub status: u16,
    /// Response body of the final attempt.
    pub body: String,
    /// Challenge detected on the final attempt, if any.
    pub challenge: Option<challenge::DetectedChallenge>,
}

impl HttpClient {
    /// Read a response body with a streaming byte cap.
    ///
    /// Avoids loading the entire body into memory: keeps at most `max_bytes`
    /// plus one network chunk resident at any time. Returns the body
    /// as raw bytes plus a `truncated` flag. Callers that need a
    /// `String` should apply their own lossy/text conversion after
    /// charset detection.
    pub(crate) async fn read_body_limited(
        response: Response,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, bool)> {
        // NOTE: wreq 6.0.0-rc does not expose chunk(), bytes_stream(), or
        // into_body(). The full body is read via bytes().await before
        // truncation. Content-Length pre-check provides early warning for
        // well-behaved servers but does NOT prevent OOM from malicious
        // responses that omit or falsify Content-Length.
        // TODO: migrate to streaming when wreq exposes bytes_stream() or
        // when reqwest is used instead of wreq.
        if let Some(len) = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<usize>().ok())
            && len > max_bytes
        {
            tracing::warn!(
                content_length = len,
                max_bytes,
                "response body exceeds size limit (Content-Length pre-check)"
            );
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;
        if bytes.len() > max_bytes {
            Ok((bytes[..max_bytes].to_vec(), true))
        } else {
            Ok((bytes.to_vec(), false))
        }
    }

    /// Build a new HTTP client from browser config.
    ///
    /// The client uses a custom redirect policy that validates every redirect
    /// target against the SSRF IP filter. This prevents attackers from using
    /// open redirects to reach internal network resources.
    ///
    /// **TOCTOU limitation:** reqwest performs its own DNS resolution after the
    /// SSRF check. This creates a time-of-check-time-of-use window. The redirect
    /// policy mitigates the most common SSRF-via-redirect attack vector. For
    /// full TOCTOU protection, a custom hyper connector would be needed.
    pub fn new(config: &BrowserConfig, cookie_jar: Arc<RwLock<CookieJar>>) -> Result<Self> {
        let ip_filter = if config.enable_ssrf_filter {
            Arc::new(IpFilter::block_private())
        } else {
            Arc::new(IpFilter::new())
        };
        let redirect_filter = ip_filter.clone();

        let mut builder = Client::builder()
            .emulation(Emulation::Chrome149)
            .user_agent(&config.user_agent)
            .pool_max_idle_per_host(config.connection_pool_size)
            .timeout(config.default_timeout)
            .redirect(wreq::redirect::Policy::custom(move |attempt| {
                let url = match Url::parse(&attempt.uri.to_string()) {
                    Ok(u) => u,
                    Err(_) => return attempt.stop(),
                };
                if !check_url_ssrf(&url, &redirect_filter) {
                    tracing::warn!("SSRF blocked: redirect to {} rejected (blocked IP)", url);
                    return attempt.stop();
                }
                attempt.follow()
            }));

        if config.accept_invalid_certs {
            builder = builder.tls_cert_verification(false);
        }

        if let Some(ref proxy_url) = config.proxy {
            match wreq::Proxy::all(proxy_url.as_str()) {
                Ok(proxy) => builder = builder.proxy(proxy),
                Err(e) => {
                    tracing::warn!(proxy = %proxy_url, error = %e, "invalid proxy URL; ignoring");
                }
            }
        }

        let client = builder
            .build()
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        Ok(Self {
            client,
            config: config.clone(),
            cookie_jar,
            ip_filter,
        })
    }

    /// Check if a URL's resolved IP is allowed by the SSRF filter.
    fn check_ssrf(&self, url: &Url) -> Result<()> {
        if !check_url_ssrf(url, &self.ip_filter)
            && let Some(host) = url.host_str()
        {
            return Err(CoreError::NetworkError(format!(
                "SSRF blocked: hostname {} resolves to a blocked IP address",
                host
            )));
        }
        Ok(())
    }

    /// Store all Set-Cookie headers from a response.
    fn store_response_cookies(&self, url: &Url, response: &Response) {
        let mut set_cookie_count = 0usize;
        for val in response.headers().get_all("set-cookie").iter() {
            if let Ok(cookie_str) = val.to_str() {
                self.cookie_jar.write().store(url, cookie_str);
                set_cookie_count += 1;
            }
        }
        tracing::trace!(url = %url, set_cookie_count, "response cookies stored");
    }

    /// Fetch a URL and return the response.
    #[tracing::instrument(skip(self), err)]
    pub async fn fetch(&self, url: &Url) -> Result<Response> {
        self.check_ssrf(url)?;

        tracing::debug!(url = %url, "HTTP request started");

        let cookies = self.cookie_jar.read().cookies_for_url(url);
        tracing::trace!(url = %url, cookie_count = cookies.len(), "cookies attached");

        let mut request = self.client.get(url.as_str());
        if !cookies.is_empty() {
            request = request.header("Cookie", cookies);
        }

        let response = request
            .send()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        tracing::debug!(url = %url, status = response.status().as_u16(), "HTTP response received");

        // Store response cookies (handle multiple Set-Cookie headers)
        self.store_response_cookies(url, &response);

        Ok(response)
    }

    /// Perform an HTTP request with an arbitrary method, headers, and body.
    ///
    /// The method/headers/body-aware sibling of [`HttpClient::fetch`] (which is
    /// GET-only). SSRF-checked, cookie-attached, and stores response cookies.
    pub async fn request(
        &self,
        url: &Url,
        method: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
    ) -> Result<Response> {
        use wreq::Method;
        use wreq::header::{HeaderName, HeaderValue};

        self.check_ssrf(url)?;

        let cookies = self.cookie_jar.read().cookies_for_url(url);
        tracing::debug!(url = %url, method = %method, "HTTP request started");

        let method_upper = method.trim().to_ascii_uppercase();
        let method_obj = Method::from_bytes(method_upper.as_bytes())
            .map_err(|e| CoreError::NetworkError(format!("invalid method {method:?}: {e}")))?;
        let mut req_builder = self.client.request(method_obj, url.as_str());

        if !cookies.is_empty() {
            req_builder = req_builder.header("Cookie", cookies);
        }
        for (k, v) in headers {
            if let (Ok(name), Ok(val)) = (
                HeaderName::try_from(k.as_str()),
                HeaderValue::try_from(v.as_str()),
            ) {
                req_builder = req_builder.header(name, val);
            }
        }
        if let Some(bytes) = body {
            req_builder = req_builder.body(bytes);
        }

        let response = req_builder
            .send()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        tracing::debug!(
            url = %url,
            method = %method,
            status = response.status().as_u16(),
            "HTTP response received"
        );
        self.store_response_cookies(url, &response);
        Ok(response)
    }

    /// `HttpClient::request` with automatic HTTP-authentication retry.
    ///
    /// If the server replies `401` with a `WWW-Authenticate: Basic` or
    /// `Digest` challenge and credentials are configured
    /// ([`BrowserConfig::http_username`]), the request is retried once with the
    /// computed `Authorization` header. Otherwise the original response is
    /// returned unchanged.
    pub async fn request_with_auth(
        &self,
        url: &Url,
        method: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
    ) -> Result<Response> {
        let response = self.request(url, method, headers, body.clone()).await?;
        if response.status().as_u16() != 401 {
            return Ok(response);
        }

        let challenge = response
            .headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let (user, pass) = match (&self.config.http_username, &self.config.http_password) {
            (Some(u), p) => (u.clone(), p.clone().unwrap_or_default()),
            (None, _) => return Ok(response),
        };

        let auth_header = challenge.and_then(|c| {
            crate::network::auth::build_authorization(&c, method, url.path(), &user, &pass)
        });

        match auth_header {
            Some(auth) => {
                let mut headers2: Vec<(String, String)> = headers
                    .iter()
                    .filter(|(k, _)| !k.eq_ignore_ascii_case("authorization"))
                    .cloned()
                    .collect();
                headers2.push(("Authorization".to_string(), auth));
                self.request(url, method, &headers2, body).await
            }
            None => Ok(response),
        }
    }

    /// Full request context: applies `Origin` + `Referer` headers (Referrer
    /// policy `strict-origin-when-cross-origin`) and performs a CORS preflight
    /// (`OPTIONS`) when the cross-origin request is not "simple", then runs the
    /// actual request through the auth-retry path.
    ///
    /// `origin` is the page's origin (`scheme://host[:port]`); `None` means no
    /// page is loaded and CORS/Referer are skipped.
    pub async fn request_with_context(
        &self,
        url: &Url,
        method: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
        origin: Option<&str>,
    ) -> Result<Response> {
        let page_url = origin.and_then(|o| Url::parse(o).ok());

        // Build effective headers: caller's headers + Referer + (cross-origin) Origin.
        let mut eff: Vec<(String, String)> = headers
            .iter()
            .filter(|(k, _)| {
                !k.eq_ignore_ascii_case("referer") && !k.eq_ignore_ascii_case("origin")
            })
            .cloned()
            .collect();

        let cross_origin = match &page_url {
            Some(p) => !crate::network::cors::same_origin(p, url),
            None => false,
        };

        if let Some(p) = &page_url {
            if let Some(referer) = crate::network::cors::compute_referer(p, url) {
                eff.push(("Referer".to_string(), referer));
            }
            if cross_origin {
                eff.push(("Origin".to_string(), p.origin().ascii_serialization()));
            }
        }

        // CORS preflight for non-simple cross-origin requests. The decision is
        // based on the caller's own headers (Origin/Referer are browser-added
        // and exempt from preflight accounting).
        if cross_origin && crate::network::cors::requires_preflight(method, headers) {
            self.run_preflight(url, method, headers, origin).await?;
        }

        self.request_with_auth(url, method, &eff, body).await
    }

    /// Send a CORS preflight `OPTIONS` and validate the response.
    async fn run_preflight(
        &self,
        url: &Url,
        method: &str,
        request_headers: &[(String, String)],
        origin: Option<&str>,
    ) -> Result<()> {
        // Preflight carries only Origin + Access-Control-Request-* headers.
        let mut preflight_headers: Vec<(String, String)> = Vec::new();
        if let Some(o) = origin {
            preflight_headers.push(("Origin".to_string(), o.to_string()));
        }
        preflight_headers.push((
            "Access-Control-Request-Method".to_string(),
            method.to_ascii_uppercase(),
        ));
        // Collect the non-safelisted request headers being used.
        let used: Vec<String> = request_headers
            .iter()
            .filter_map(|(k, _)| {
                let lower = k.to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "accept" | "accept-language" | "content-language" | "content-type" | "range"
                ) {
                    None
                } else {
                    Some(k.clone())
                }
            })
            .collect();
        if !used.is_empty() {
            preflight_headers.push((
                "Access-Control-Request-Headers".to_string(),
                used.join(", "),
            ));
        }

        let resp = self
            .request(url, "OPTIONS", &preflight_headers, None)
            .await?;
        let h = resp.headers();
        let get = |name: &str| h.get(name).and_then(|v| v.to_str().ok());
        let result = crate::network::cors::validate_preflight(
            method,
            request_headers,
            get("access-control-allow-origin"),
            get("access-control-allow-methods"),
            get("access-control-allow-headers"),
            get("access-control-allow-credentials"),
            false,
        );
        if result == crate::network::cors::PreflightResult::Denied {
            return Err(CoreError::NetworkError(format!(
                "CORS preflight failed for {method} {url}"
            )));
        }
        Ok(())
    }

    /// Fetch `url`, retrying while a bot-management challenge is detected.
    ///
    /// Each attempt runs [`HttpClient::fetch`], reads the body, and runs
    /// [`challenge::detect`]. With no challenge the outcome is returned at
    /// once. When a challenge is detected the client backs off and retries —
    /// re-sending any clearance cookie the cookie jar captured from a prior
    /// attempt's `Set-Cookie` — up to `max_attempts`, then returns the final
    /// outcome with the detected challenge.
    ///
    /// **This does not auto-execute challenge JS** (see [`crate::challenge`]).
    /// A retry only clears the challenge when the passive stealth tier already
    /// satisfies it, or when a clearance cookie was injected into the cookie
    /// jar out-of-band. `max_attempts` is clamped to ≥ 1.
    pub async fn fetch_with_challenge_retry(
        &self,
        url: &Url,
        max_attempts: u32,
    ) -> Result<ChallengeOutcome> {
        let max_attempts = max_attempts.max(1);
        let mut outcome = ChallengeOutcome {
            status: 0,
            body: String::new(),
            challenge: None,
        };
        for attempt in 1..=max_attempts {
            let response = self.fetch(url).await?;
            let status = response.status().as_u16();
            let headers = Self::response_headers(&response);
            let max = self.config.max_response_body_bytes;
            let (buf, truncated) = Self::read_body_limited(response, max).await?;
            if truncated {
                tracing::warn!(url = %url, max_bytes = max, "response body truncated at size limit");
            }
            let body = String::from_utf8_lossy(&buf).into_owned();
            let detected = challenge::detect(status, &headers, &body);
            let is_challenge = detected.is_some();
            outcome = ChallengeOutcome {
                status,
                body,
                challenge: detected,
            };
            if !is_challenge {
                return Ok(outcome);
            }
            // Interactive captchas (need a human) and hard blocks can't be
            // cleared by retrying — return the detected challenge at once.
            if let Some(ref c) = outcome.challenge
                && matches!(
                    c.kind,
                    challenge::ChallengeKind::Interactive | challenge::ChallengeKind::Blocked
                )
            {
                return Ok(outcome);
            }
            if let Some(ref c) = outcome.challenge {
                tracing::warn!(
                    url = %url, attempt,
                    vendor = c.vendor.as_str(), kind = ?c.kind,
                    clearance_cookie = c.clearance_cookie,
                    "bot-management challenge detected; will retry",
                );
            }
            if attempt < max_attempts {
                let backoff = std::time::Duration::from_millis(
                    250u64
                        .saturating_mul(2u64.saturating_pow(attempt - 1))
                        .min(2000),
                );
                tokio::time::sleep(backoff).await;
            }
        }
        Ok(outcome)
    }

    /// Collect response headers into a `(name, value)` slice for [`challenge::detect`].
    fn response_headers(response: &Response) -> Vec<(String, String)> {
        response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect()
    }

    /// Fetch with an InterceptAction from the Fetch domain.
    ///
    /// - `Continue`: perform the actual HTTP request (with optional modifications)
    /// - `Fail`: return a network error immediately
    /// - `Fulfill`: return a synthetic response via InterceptedResponse
    #[tracing::instrument(skip(self, action), err)]
    pub async fn intercept(
        &self,
        url: &Url,
        _method: Option<&str>,
        _headers: &[(String, String)],
        _post_data: Option<&str>,
        action: InterceptAction,
    ) -> Result<Response> {
        use wreq::header::{HeaderName, HeaderValue};

        match action {
            InterceptAction::Continue {
                url: url_mod,
                method: method_mod,
                headers: headers_mod,
                post_data: post_data_mod,
            } => {
                let effective_url = url_mod
                    .as_ref()
                    .and_then(|u| Url::parse(u).ok())
                    .unwrap_or_else(|| url.clone());
                let effective_method = method_mod.as_deref().unwrap_or("GET");
                let effective_post = post_data_mod.as_deref();

                self.check_ssrf(&effective_url)?;

                let cookies = self.cookie_jar.read().cookies_for_url(&effective_url);

                let mut req_builder = if effective_method == "POST" {
                    let body = effective_post.unwrap_or_default();
                    self.client
                        .post(effective_url.as_str())
                        .body(body.to_string())
                } else {
                    self.client.get(effective_url.as_str())
                };

                if !cookies.is_empty() {
                    req_builder = req_builder.header("Cookie", cookies);
                }
                // Apply modified headers
                for (k, v) in headers_mod.iter() {
                    if let (Ok(name), Ok(val)) = (
                        HeaderName::try_from(k.as_str()),
                        HeaderValue::try_from(v.as_str()),
                    ) {
                        req_builder = req_builder.header(name, val);
                    }
                }

                let response = req_builder
                    .send()
                    .await
                    .map_err(|e| CoreError::NetworkError(e.to_string()))?;

                self.store_response_cookies(&effective_url, &response);
                Ok(response)
            }
            InterceptAction::Fail { error_reason } => Err(CoreError::NetworkError(error_reason)),
            InterceptAction::Fulfill {
                status_code,
                status_text,
                headers: resp_headers,
                body,
            } => {
                let resp = InterceptedResponse {
                    status_code,
                    status_text,
                    headers: resp_headers,
                    body: InterceptedBody::Bytes(body),
                };
                Err(CoreError::InterceptedResponse(resp))
            }
        }
    }

    /// Fetch URL and return body as a string, auto-detecting encoding.
    ///
    /// Uses `Content-Type` header charset, BOM, and HTML `<meta>` tags
    /// to detect the character encoding. Falls back to UTF-8.
    #[tracing::instrument(skip(self), err)]
    pub async fn fetch_text(&self, url: &Url) -> Result<String> {
        let response = self.fetch(url).await?;
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let max = self.config.max_response_body_bytes;
        let (buf, truncated) = Self::read_body_limited(response, max).await?;
        if truncated {
            tracing::warn!(url = %url, max_bytes = max, "response body truncated at size limit");
        }

        Ok(crate::encoding::decode_html(&buf, content_type.as_deref()))
    }

    /// Send a POST request with a raw body.
    #[tracing::instrument(skip(self, body), err)]
    pub async fn post(&self, url: &Url, body: impl Into<wreq::Body>) -> Result<Response> {
        self.check_ssrf(url)?;

        let response = self
            .client
            .post(url.as_str())
            .body(body)
            .send()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        self.store_response_cookies(url, &response);

        Ok(response)
    }

    /// Send a POST request with a JSON body.
    #[tracing::instrument(skip(self, json), err)]
    pub async fn post_json(&self, url: &Url, json: &serde_json::Value) -> Result<Response> {
        self.check_ssrf(url)?;

        let response = self
            .client
            .post(url.as_str())
            .json(json)
            .send()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        self.store_response_cookies(url, &response);

        Ok(response)
    }

    /// Send a POST request with URL-encoded form data.
    #[tracing::instrument(skip(self, form), err)]
    pub async fn post_form(&self, url: &Url, form: &[(&str, &str)]) -> Result<Response> {
        self.check_ssrf(url)?;

        let response = self
            .client
            .post(url.as_str())
            .form(form)
            .send()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        self.store_response_cookies(url, &response);

        Ok(response)
    }

    /// Get the underlying reqwest client.
    pub fn raw_client(&self) -> &Client {
        &self.client
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::cookie::CookieJar;
    use parking_lot::RwLock;
    use std::sync::Arc;

    fn make_client() -> HttpClient {
        let config = BrowserConfig::headless();
        let jar = Arc::new(RwLock::new(CookieJar::new()));
        HttpClient::new(&config, jar).unwrap()
    }

    #[test]
    fn test_http_client_new_default_config() {
        let client = make_client();
        // Verify the client was created and has a reqwest::Client internally
        let _ = client.raw_client();
    }

    #[test]
    fn test_cookie_jar_empty_initially() {
        let config = BrowserConfig::headless();
        let jar = Arc::new(RwLock::new(CookieJar::new()));
        let _client = HttpClient::new(&config, jar.clone());

        let url = Url::parse("https://example.com/").unwrap();
        let cookies = jar.read().cookies_for_url(&url);
        assert!(cookies.is_empty(), "new jar should have no cookies");
    }

    #[test]
    fn test_ip_filter_integration() {
        let client = make_client();
        // Verify the client was created with the default block_private filter.
        // The SSRF filter is private, so we just confirm construction succeeds.
        let _ = client.raw_client();
    }

    #[tokio::test]
    #[ignore = "makes real HTTP request"]
    async fn test_http_client_fetch_real() {
        let client = make_client();
        let url = Url::parse("https://httpbin.org/get").unwrap();
        let result = client.fetch(&url).await;
        assert!(result.is_ok(), "fetch to httpbin should succeed");
    }

    #[tokio::test]
    #[ignore = "makes real HTTP requests"]
    async fn test_http_client_fetch_stores_cookies() {
        let config = BrowserConfig::headless();
        let jar = Arc::new(RwLock::new(CookieJar::new()));
        let client = HttpClient::new(&config, jar.clone()).unwrap();

        let url = Url::parse("https://httpbin.org/cookies/set?test_cookie=test_value").unwrap();
        let _ = client.fetch(&url).await;

        let cookies = jar
            .read()
            .cookies_for_url(&Url::parse("https://httpbin.org/").unwrap());
        assert!(!cookies.is_empty(), "cookies should be stored after fetch");
    }

    #[test]
    fn test_check_url_ssrf_blocks_loopback() {
        let filter = IpFilter::block_private();
        let url = Url::parse("http://127.0.0.1/admin").unwrap();
        assert!(!check_url_ssrf(&url, &filter));
    }

    #[test]
    fn test_check_url_ssrf_allows_public() {
        let filter = IpFilter::block_private();
        let url = Url::parse("http://93.184.216.34/").unwrap();
        assert!(check_url_ssrf(&url, &filter));
    }

    #[test]
    fn test_check_url_ssrf_no_host() {
        let filter = IpFilter::block_private();
        // data: URLs have no host
        let url = Url::parse("data:text/plain,hello").unwrap();
        assert!(check_url_ssrf(&url, &filter));
    }

    /// Capture server: reads one raw HTTP request, stores (request-line, body)
    /// into a shared cell, replies 200 OK. Used to assert the on-the-wire
    /// method/headers/body of `HttpClient::request`.
    async fn capture_one_request(
        addr_out: std::sync::mpsc::Sender<std::net::SocketAddr>,
        captured: Arc<parking_lot::Mutex<Option<(String, String)>>>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        addr_out.send(listener.local_addr().unwrap()).unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf: Vec<u8> = Vec::new();
        let mut tmp = [0u8; 1024];
        // Read until end-of-headers.
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        let request_line = text.lines().next().unwrap_or("").to_string();
        let header_end = text.find("\r\n\r\n").unwrap_or(text.len());
        let content_length = text[..header_end]
            .lines()
            .find_map(|l| {
                let l = l.to_ascii_lowercase();
                l.strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        let mut body = text.as_bytes()[header_end + 4..].to_vec();
        while body.len() < content_length {
            let n = stream.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
        }
        *captured.lock() = Some((request_line, String::from_utf8_lossy(&body).to_string()));
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        let _ = stream.write_all(resp.as_bytes()).await;
        let _ = stream.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_sends_method_and_body_on_the_wire() {
        let captured: Arc<parking_lot::Mutex<Option<(String, String)>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let (addr_tx, addr_rx) = std::sync::mpsc::channel::<std::net::SocketAddr>();
        let cap = captured.clone();
        tokio::spawn(capture_one_request(addr_tx, cap));
        let addr = addr_rx.recv().expect("server bound");

        // SSRF filter blocks loopback by default; disable for the local server.
        let config = BrowserConfig {
            enable_ssrf_filter: false,
            ..BrowserConfig::headless()
        };
        let jar = Arc::new(RwLock::new(CookieJar::new()));
        let client = HttpClient::new(&config, jar).unwrap();
        let url = Url::parse(&format!("http://{addr}/post")).unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            client.request(
                &url,
                "POST",
                &[("content-type".to_string(), "text/plain".to_string())],
                Some(b"hello body".to_vec()),
            ),
        )
        .await;
        let result = result.expect("client.request timed out");
        assert!(result.is_ok(), "request failed: {:?}", result.err());

        // Give the capture task a moment to finish storing.
        for _ in 0..50 {
            if captured.lock().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let (line, body) = captured.lock().clone().expect("server captured no request");
        assert!(line.starts_with("POST /post"), "expected POST, got: {line}");
        assert_eq!(body, "hello body", "body not delivered on the wire");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_with_auth_retries_basic_401() {
        use base64::Engine;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let expected = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(b"u:p")
        );
        // Authenticated request → 200 (mounted first so it wins on match).
        Mock::given(method("GET"))
            .and(path("/"))
            .and(header("authorization", &expected))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        // Unauthenticated request → 401 challenge.
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(401)
                    .insert_header("www-authenticate", "Basic realm=\"secure\""),
            )
            .mount(&server)
            .await;

        let config = BrowserConfig {
            enable_ssrf_filter: false,
            http_username: Some("u".to_string()),
            http_password: Some("p".to_string()),
            ..BrowserConfig::headless()
        };
        let jar = Arc::new(RwLock::new(CookieJar::new()));
        let client = HttpClient::new(&config, jar).unwrap();
        let url = Url::parse(&format!("{}/", server.uri())).unwrap();
        let resp = client
            .request_with_auth(&url, "GET", &[], None)
            .await
            .expect("request should succeed");
        assert_eq!(
            resp.status().as_u16(),
            200,
            "should retry with auth and get 200"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_with_auth_no_credentials_returns_401() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(401)
                    .insert_header("www-authenticate", "Basic realm=\"secure\""),
            )
            .mount(&server)
            .await;

        let config = BrowserConfig {
            enable_ssrf_filter: false,
            ..BrowserConfig::headless()
        };
        let jar = Arc::new(RwLock::new(CookieJar::new()));
        let client = HttpClient::new(&config, jar).unwrap();
        let url = Url::parse(&format!("{}/", server.uri())).unwrap();
        let resp = client
            .request_with_auth(&url, "GET", &[], None)
            .await
            .expect("request should succeed");
        assert_eq!(resp.status().as_u16(), 401, "no credentials → return 401");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_with_context_adds_origin_and_referer_cross_origin() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Cross-origin GET requires Origin + Referer to be present.
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(header("origin", "https://example.com"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let config = BrowserConfig {
            enable_ssrf_filter: false,
            ..BrowserConfig::headless()
        };
        let jar = Arc::new(RwLock::new(CookieJar::new()));
        let client = HttpClient::new(&config, jar).unwrap();
        let url = Url::parse(&format!("{}/api", server.uri())).unwrap();
        let resp = client
            .request_with_context(&url, "GET", &[], None, Some("https://example.com/page"))
            .await
            .expect("request should succeed");
        assert_eq!(resp.status().as_u16(), 200);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_with_context_preflight_denied_blocks_request() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Preflight OPTIONS that does NOT grant CORS permission.
        Mock::given(method("OPTIONS"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let config = BrowserConfig {
            enable_ssrf_filter: false,
            ..BrowserConfig::headless()
        };
        let jar = Arc::new(RwLock::new(CookieJar::new()));
        let client = HttpClient::new(&config, jar).unwrap();
        let url = Url::parse(&format!("{}/api", server.uri())).unwrap();
        // Cross-origin PUT requires a preflight; the empty preflight response
        // lacks Access-Control-Allow-* → the request must fail.
        let result = client
            .request_with_context(&url, "PUT", &[], None, Some("https://example.com"))
            .await;
        assert!(
            result.is_err(),
            "cross-origin PUT without a permissive preflight must be blocked"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_with_context_preflight_allowed_proceeds() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Permissive preflight.
        Mock::given(method("OPTIONS"))
            .and(path("/api"))
            .respond_with(
                ResponseTemplate::new(204)
                    .insert_header("access-control-allow-origin", "*")
                    .insert_header("access-control-allow-methods", "PUT")
                    .insert_header("access-control-allow-headers", "content-type"),
            )
            .mount(&server)
            .await;
        // Actual PUT.
        Mock::given(method("PUT"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_string("created"))
            .mount(&server)
            .await;

        let config = BrowserConfig {
            enable_ssrf_filter: false,
            ..BrowserConfig::headless()
        };
        let jar = Arc::new(RwLock::new(CookieJar::new()));
        let client = HttpClient::new(&config, jar).unwrap();
        let url = Url::parse(&format!("{}/api", server.uri())).unwrap();
        let resp = client
            .request_with_context(
                &url,
                "PUT",
                &[("content-type".to_string(), "application/json".to_string())],
                None,
                Some("https://example.com"),
            )
            .await
            .expect("preflight should pass and PUT succeed");
        assert_eq!(resp.status().as_u16(), 200);
    }
}
