//! HTTP client for resource fetching.

use crate::config::BrowserConfig;
use crate::error::{CoreError, Result};
use crate::network::cookie::CookieJar;
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
        })
    }

    /// Fetch a URL and return the response.
    pub async fn fetch(&self, url: &Url) -> Result<Response> {
        let cookies = self.cookie_jar.read().cookies_for_url(url);

        let mut request = self.client.get(url.as_str());
        if !cookies.is_empty() {
            request = request.header("Cookie", cookies);
        }

        let response = request
            .send()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        // Store response cookies
        if let Some(set_cookie) = response.headers().get("set-cookie") {
            if let Ok(val) = set_cookie.to_str() {
                self.cookie_jar.write().store(url, val);
            }
        }

        Ok(response)
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
        let response = self
            .client
            .post(url.as_str())
            .body(body)
            .send()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        // Store response cookies
        if let Some(set_cookie) = response.headers().get("set-cookie") {
            if let Ok(val) = set_cookie.to_str() {
                self.cookie_jar.write().store(url, val);
            }
        }

        Ok(response)
    }

    /// Send a POST request with a JSON body.
    pub async fn post_json(&self, url: &Url, json: &serde_json::Value) -> Result<Response> {
        let response = self
            .client
            .post(url.as_str())
            .json(json)
            .send()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        // Store response cookies
        if let Some(set_cookie) = response.headers().get("set-cookie") {
            if let Ok(val) = set_cookie.to_str() {
                self.cookie_jar.write().store(url, val);
            }
        }

        Ok(response)
    }

    /// Send a POST request with URL-encoded form data.
    pub async fn post_form(&self, url: &Url, form: &[(&str, &str)]) -> Result<Response> {
        let response = self
            .client
            .post(url.as_str())
            .form(form)
            .send()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        // Store response cookies
        if let Some(set_cookie) = response.headers().get("set-cookie") {
            if let Ok(val) = set_cookie.to_str() {
                self.cookie_jar.write().store(url, val);
            }
        }

        Ok(response)
    }

    /// Get the underlying reqwest client.
    pub fn raw_client(&self) -> &Client {
        &self.client
    }
}
