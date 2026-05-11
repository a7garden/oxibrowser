//! Session — browsing context group with cookie jar, storage, and history.
//!
//! Mirrors Lightpanda's `Session.zig`: owns Pages, navigation history,
//! session storage, and the cookie jar.

use crate::browser::BrowserId;
use crate::config::BrowserConfig;
use crate::error::{CoreError, Result};
use crate::js::JsRuntime;
use crate::network::cookie::CookieJar;
use crate::network::HttpClient;
use crate::page::Page;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tracing::info;
use url::Url;

/// Unique session ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(u32);

impl SessionId {
    fn next() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "session-{}", self.0)
    }
}

/// A browsing session with its own history, storage, and pages.
pub struct Session {
    /// Unique ID.
    id: SessionId,
    /// Parent browser ID.
    #[allow(dead_code)]
    browser_id: BrowserId,
    /// Configuration.
    #[allow(dead_code)]
    config: BrowserConfig,
    /// HTTP client (shared from Browser).
    http_client: Arc<HttpClient>,
    /// Cookie jar (may be shared or isolated).
    #[allow(dead_code)]
    cookie_jar: Arc<RwLock<CookieJar>>,
    /// Active page (current document).
    active_page: Option<Page>,
    /// Navigation history (URLs visited).
    history: Vec<Url>,
    /// Current position in history.
    history_index: usize,
    /// Session-local storage.
    local_storage: std::collections::HashMap<String, String>,
    /// JS runtime (per-session).
    js_runtime: JsRuntime,
}

impl Session {
    /// Create a new session.
    pub async fn new(
        browser_id: BrowserId,
        config: BrowserConfig,
        http_client: Arc<HttpClient>,
        cookie_jar: Arc<RwLock<CookieJar>>,
    ) -> Result<Self> {
        Ok(Self {
            id: SessionId::next(),
            browser_id,
            config,
            http_client,
            cookie_jar,
            active_page: None,
            history: Vec::new(),
            history_index: 0,
            local_storage: std::collections::HashMap::new(),
            js_runtime: JsRuntime::new(),
        })
    }

    /// Navigate to a URL.
    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        let parsed = Url::parse(url)?;

        info!(url = %parsed, "navigating");

        // Fetch the document
        let response = self.http_client.fetch(&parsed).await?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/html")
            .to_string();

        let html = response
            .text()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        // Create a new page for this navigation
        let page = Page::from_html(parsed.clone(), &html, status, content_type).await?;

        // Update history
        if self.history.is_empty() {
            // First navigation — just push
        } else if self.history_index < self.history.len() - 1 {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(parsed);
        self.history_index = self.history.len() - 1;

        self.active_page = Some(page);
        Ok(())
    }

    /// Navigate back in history.
    pub async fn go_back(&mut self) -> Result<()> {
        if self.history_index > 0 {
            self.history_index -= 1;
            let url = self.history[self.history_index].clone();
            
            // Re-fetch without adding to history
            let response = self.http_client.fetch(&url).await?;
            let html = response
                .text()
                .await
                .map_err(|e| CoreError::NetworkError(e.to_string()))?;
            self.active_page = Some(Page::from_html(url, &html, 200, "text/html".into()).await?);
            Ok(())
        } else {
            Err(CoreError::NavigationFailed("no previous page".into()))
        }
    }

    /// Navigate forward in history.
    pub async fn go_forward(&mut self) -> Result<()> {
        if self.history_index < self.history.len() - 1 {
            self.history_index += 1;
            let url = self.history[self.history_index].clone();
            
            let response = self.http_client.fetch(&url).await?;
            let html = response
                .text()
                .await
                .map_err(|e| CoreError::NetworkError(e.to_string()))?;
            self.active_page = Some(Page::from_html(url, &html, 200, "text/html".into()).await?);
            Ok(())
        } else {
            Err(CoreError::NavigationFailed("no next page".into()))
        }
    }

    /// Reload the current page.
    pub async fn reload(&mut self) -> Result<()> {
        if let Some(url) = self.current_url() {
            
            let response = self.http_client.fetch(&url).await?;
            let html = response
                .text()
                .await
                .map_err(|e| CoreError::NetworkError(e.to_string()))?;
            self.active_page = Some(
                Page::from_html(url.clone(), &html, 200, "text/html".into()).await?,
            );
            Ok(())
        } else {
            Err(CoreError::NavigationFailed("no current page".into()))
        }
    }

    /// Evaluate JavaScript in the current page context.
    pub async fn evaluate_js(&mut self, expression: &str) -> Result<crate::js::runtime::JsEvalResult> {
        self.js_runtime.evaluate(expression).await
    }

    /// Get the current page (if any).
    pub fn page(&self) -> Option<&Page> {
        self.active_page.as_ref()
    }

    /// Get the current page mutably.
    pub fn page_mut(&mut self) -> Option<&mut Page> {
        self.active_page.as_mut()
    }

    /// Get the current URL.
    pub fn current_url(&self) -> Option<&Url> {
        self.active_page.as_ref().map(|p| p.url())
    }

    /// Get the session ID.
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// Get navigation history.
    pub fn history(&self) -> &[Url] {
        &self.history
    }

    /// Get history position.
    pub fn history_index(&self) -> usize {
        self.history_index
    }

    /// Set a local storage value.
    pub fn set_local_storage(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.local_storage.insert(key.into(), value.into());
    }

    /// Get a local storage value.
    pub fn get_local_storage(&self, key: &str) -> Option<&str> {
        self.local_storage.get(key).map(|s| s.as_str())
    }

    /// Close the session.
    pub async fn close(&mut self) -> Result<()> {
        info!(id = %self.id, "session closed");
        self.active_page = None;
        self.history.clear();
        self.local_storage.clear();
        Ok(())
    }
}
