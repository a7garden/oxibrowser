//! HTTP client for resource fetching.
//!
//! Provides:
//! - `fetch` — standard HTTP GET with cookies
//! - `intercept` — fetch with an InterceptAction (continue/fail/fulfill)
//! - `fetch_text`, `post`, `post_json` — convenience methods

use crate::config::BrowserConfig;
use crate::error::{CoreError, Result};
use crate::network::cookie::CookieJar;
use crate::network::intercept::{InterceptAction, InterceptedBody, InterceptedResponse};
use crate::network::ip_filter::IpFilter;
use parking_lot::RwLock;
use reqwest::{Client, Response};
use std::sync::Arc;
use url::Url;

/// HTTP client wrapper with cookie support and configurable defaults.
pub struct HttpClient {
    client: Client,
    #[allow(dead_code)]
    config: BrowserConfig,
    cookie_jar: Arc<RwLock<CookieJar>>,
    ip_filter: IpFilter,
}

impl HttpClient {
    /// Build a new HTTP client from browser config.
    pub fn new(config: &BrowserConfig, cookie_jar: Arc<RwLock<CookieJar>>) -> Result<Self> {
        let mut builder = Client::builder()
            .user_agent(&config.user_agent)
            .pool_max_idle_per_host(config.connection_pool_size)
            .timeout(config.default_timeout)
            .redirect(reqwest::redirect::Policy::limited(10));

        if config.accept_invalid_certs {
            builder = builder.danger_accept_invalid_certs(true);
        }

        let client = builder
            .build()
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        Ok(Self {
            client,
            config: config.clone(),
            cookie_jar,
            ip_filter: IpFilter::block_private(),
        })
    }

    /// Check if a URL's resolved IP is allowed by the SSRF filter.
    fn check_ssrf(&self, url: &Url) -> Result<()> {
        if let Some(host) = url.host_str() {
            if !self.ip_filter.is_hostname_allowed(host) {
                return Err(CoreError::NetworkError(format!(
                    "SSRF blocked: hostname {} resolves to a blocked IP address",
                    host
                )));
            }
        }
        Ok(())
    }

    /// Store all Set-Cookie headers from a response.
    fn store_response_cookies(&self, url: &Url, response: &Response) {
        for val in response.headers().get_all("set-cookie").iter() {
            if let Ok(cookie_str) = val.to_str() {
                self.cookie_jar.write().store(url, cookie_str);
            }
        }
    }

    /// Fetch a URL and return the response.
    pub async fn fetch(&self, url: &Url) -> Result<Response> {
        self.check_ssrf(url)?;

        let cookies = self.cookie_jar.read().cookies_for_url(url);

        let mut request = self.client.get(url.as_str());
        if !cookies.is_empty() {
            request = request.header("Cookie", cookies);
        }

        let response = request
            .send()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        // Store response cookies (handle multiple Set-Cookie headers)
        self.store_response_cookies(url, &response);

        Ok(response)
    }

    /// Fetch with an InterceptAction from the Fetch domain.
    ///
    /// - `Continue`: perform the actual HTTP request (with optional modifications)
    /// - `Fail`: return a network error immediately
    /// - `Fulfill`: return a synthetic response via InterceptedResponse
    pub async fn intercept(
        &self,
        url: &Url,
        _method: Option<&str>,
        _headers: &[(String, String)],
        _post_data: Option<&str>,
        action: InterceptAction,
    ) -> Result<Response> {
        use reqwest::header::{HeaderName, HeaderValue};

        match action {
            InterceptAction::Continue { url: url_mod, method: method_mod, headers: headers_mod, post_data: post_data_mod } => {
                let effective_url = url_mod.as_ref().and_then(|u| Url::parse(u).ok()).unwrap_or_else(|| url.clone());
                let effective_method = method_mod.as_deref().unwrap_or("GET");
                let effective_post = post_data_mod.as_deref();

                self.check_ssrf(&effective_url)?;

                let cookies = self.cookie_jar.read().cookies_for_url(&effective_url);

                let mut req_builder = if effective_method == "POST" {
                    let body = effective_post.unwrap_or_default();
                    self.client.post(effective_url.as_str()).body(body.to_string())
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
            InterceptAction::Fail { error_reason } => {
                Err(CoreError::NetworkError(error_reason))
            }
            InterceptAction::Fulfill { status_code, status_text, headers: resp_headers, body } => {
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
    pub async fn fetch_text(&self, url: &Url) -> Result<String> {
        let response = self.fetch(url).await?;
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let bytes = response
            .bytes()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        Ok(crate::encoding::decode_html(
            &bytes,
            content_type.as_deref(),
        ))
    }

    /// Send a POST request with a raw body.
    pub async fn post(&self, url: &Url, body: impl Into<reqwest::Body>) -> Result<Response> {
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

        let url =
            Url::parse("https://httpbin.org/cookies/set?test_cookie=test_value").unwrap();
        let _ = client.fetch(&url).await;

        let cookies =
            jar.read().cookies_for_url(&Url::parse("https://httpbin.org/").unwrap());
        assert!(
            !cookies.is_empty(),
            "cookies should be stored after fetch"
        );
    }
}
