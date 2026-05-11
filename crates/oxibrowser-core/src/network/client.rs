//! HTTP client for resource fetching.

use crate::config::BrowserConfig;
use crate::error::{CoreError, Result};
use crate::network::cookie::CookieJar;
use parking_lot::RwLock;
use reqwest::{Client, Response};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// HTTP client wrapper with cookie support and configurable defaults.
pub struct HttpClient {
    client: Client,
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

    /// Fetch URL and return body as string.
    pub async fn fetch_text(&self, url: &Url) -> Result<String> {
        let response = self.fetch(url).await?;
        let text = response
            .text()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;
        Ok(text)
    }

    /// Get the underlying reqwest client.
    pub fn raw_client(&self) -> &Client {
        &self.client
    }
}
