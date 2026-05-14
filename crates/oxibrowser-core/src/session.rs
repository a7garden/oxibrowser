//! Session — browsing context group with cookie jar, storage, and history.
//!
//! Mirrors Lightpanda's `Session.zig`: owns Pages, navigation history,
//! session storage, and the cookie jar.

use crate::browser::BrowserId;
use std::collections::HashMap;
use crate::config::BrowserConfig;
use crate::error::{CoreError, Result};
use crate::js::dom_snapshot::{DomMutation, DomSnapshot};
use crate::js::runtime::JsRuntimeConfig;
use crate::js::JsRuntime;
use crate::network::cookie::CookieJar;
use crate::network::HttpClient;
use crate::js::runtime::{FetchRequestMsg, FetchResponseMsg};
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

/// Stored HTTP response body for Network.getResponseBody.
#[derive(Debug, Clone)]
pub struct CapturedResponse {
    pub body: String,
    pub base64: bool,
    pub content_type: String,
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
    /// Stored response bodies (requestId -> body) for getResponseBody.
    response_bodies: Arc<parking_lot::RwLock<HashMap<String, CapturedResponse>>>,
    /// JS runtime (per-session).
    js_runtime: JsRuntime,
    /// Fetch handler task handle (for cleanup).
    #[allow(dead_code)]
    fetch_task: Option<std::thread::JoinHandle<()>>,
    /// Whether the session has been closed.
    closed: bool,
}

// ---------------------------------------------------------------------------
// Fetch handler
// ---------------------------------------------------------------------------

/// Handle fetch requests from the JS thread.
/// Spawns a minimal tokio runtime for async HTTP calls.
fn handle_fetch_requests(
    fetch_rx: std::sync::mpsc::Receiver<FetchRequestMsg>,
    http_client: Arc<HttpClient>,
    _cookie_jar: Arc<RwLock<CookieJar>>,
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to create tokio runtime for fetch");

    rt.block_on(async {
        loop {
            // Use try_recv to avoid blocking
            match fetch_rx.try_recv() {
                Ok(request) => {
                    // Process the fetch request
                    let url = match Url::parse(&request.url) {
                        Ok(u) => u,
                        Err(e) => {
                            let _ = request.response_tx.send(FetchResponseMsg {
                                status: 400,
                                status_text: "Invalid URL".to_string(),
                                url: request.url,
                                headers: vec![],
                                body: String::new(),
                                error: Some(format!("invalid URL: {}", e)),
                            });
                            continue;
                        }
                    };

                    let resp = http_client.fetch(&url).await;
                    match resp {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            let status_text = response.status().canonical_reason().unwrap_or("").to_string();
                            let resp_url = response.url().to_string();
                            let headers: Vec<(String, String)> = response
                                .headers()
                                .iter()
                                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                                .collect();
                            let body = match response.bytes().await {
                                Ok(b) => String::from_utf8_lossy(&b).to_string(),
                                Err(e) => {
                                    let _ = request.response_tx.send(FetchResponseMsg {
                                        status,
                                        status_text,
                                        url: resp_url,
                                        headers,
                                        body: String::new(),
                                        error: Some(format!("failed to read body: {}", e)),
                                    });
                                    continue;
                                }
                            };

                            let _ = request.response_tx.send(FetchResponseMsg {
                                status,
                                status_text,
                                url: resp_url,
                                headers,
                                body,
                                error: None,
                            });
                        }
                        Err(e) => {
                            let _ = request.response_tx.send(FetchResponseMsg {
                                status: 0,
                                status_text: "Network Error".to_string(),
                                url: request.url,
                                headers: vec![],
                                body: String::new(),
                                error: Some(e.to_string()),
                            });
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // No request ready — sleep briefly
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // Channel closed — exit
                    break;
                }
            }
        }
    });
}

impl Session {
    /// Create a new session.
    pub async fn new(
        browser_id: BrowserId,
        config: BrowserConfig,
        http_client: Arc<HttpClient>,
        cookie_jar: Arc<RwLock<CookieJar>>,
    ) -> Result<Self> {
        let js_config = JsRuntimeConfig::from(&config);

        // Create fetch channel
        let (fetch_tx, fetch_rx) = std::sync::mpsc::channel();

        // Create JS runtime and wire up fetch channel
        let mut js_runtime = JsRuntime::with_config(js_config);
        js_runtime.set_fetch_channel(fetch_tx);

        // Spawn fetch handler on a blocking thread
        let http_client_clone = http_client.clone();
        let cookie_jar_clone = cookie_jar.clone();
        let fetch_task = Some(std::thread::spawn(move || {
            handle_fetch_requests(fetch_rx, http_client_clone, cookie_jar_clone);
        }));

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
            response_bodies: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            js_runtime,
            fetch_task,
            closed: false,
        })
    }

    /// Navigate to a URL.
    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        if self.closed {
            return Err(CoreError::SessionClosed);
        }

        let parsed = Url::parse(url)?;

        info!(url = %parsed, "navigating");

        // Fetch the document
        let response = self.http_client.fetch(&parsed).await?;
        let status = response.status().as_u16();

        // Check for HTTP errors
        if status >= 400 {
            return Err(CoreError::HttpError {
                status,
                message: format!("HTTP {} for {}", status, parsed),
            });
        }

        let ct_header = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/html")
            .to_string();

        let bytes = response
            .bytes()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        let html = crate::encoding::decode_html(&bytes, Some(&ct_header));

        // Create a new page for this navigation
        let page = Page::from_html(parsed.clone(), &html, status, ct_header).await?;

        // Update history
        if self.history.is_empty() {
            // First navigation — just push
        } else if self.history_index < self.history.len() - 1 {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(parsed);
        self.history_index = self.history.len() - 1;

        self.active_page = Some(page);

        // Inject DOM snapshot into JS runtime
        self.inject_dom_snapshot();

        Ok(())
    }

    /// Navigate to a URL with automatic retries on transient failures.
    ///
    /// Retries DNS errors, connection timeouts, and 5xx errors with
    /// exponential backoff (500ms, 1000ms, 1500ms, ...).
    pub async fn navigate_with_retry(&mut self, url: &str, max_retries: u32) -> Result<()> {
        let mut last_error: Option<CoreError> = None;

        for attempt in 0..=max_retries {
            match self.navigate(url).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let is_retryable = match &e {
                        CoreError::DnsError(_)
                        | CoreError::ConnectionTimeout(_)
                        | CoreError::NetworkError(_) => true,
                        CoreError::HttpError { status, .. } => *status >= 500,
                        _ => false,
                    };

                    if !is_retryable || attempt >= max_retries {
                        return Err(e);
                    }

                    last_error = Some(e);
                    let delay = std::time::Duration::from_millis(500 * (attempt + 1) as u64);
                    info!(
                        attempt = attempt + 1,
                        max_retries,
                        delay_ms = delay.as_millis(),
                        "retrying navigation"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }

        Err(last_error.expect("at least one retry attempt must have occurred"))
    }
    pub async fn go_back(&mut self) -> Result<()> {
        if self.history_index > 0 {
            self.history_index -= 1;
            let url = self.history[self.history_index].clone();

            // Re-fetch without adding to history
            let response = self.http_client.fetch(&url).await?;
            let ct_header = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html")
                .to_string();
            let bytes = response
                .bytes()
                .await
                .map_err(|e| CoreError::NetworkError(e.to_string()))?;
            let html = crate::encoding::decode_html(&bytes, Some(&ct_header));
            self.active_page = Some(Page::from_html(url, &html, 200, ct_header).await?);
            self.inject_dom_snapshot();
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
            let ct_header = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html")
                .to_string();
            let bytes = response
                .bytes()
                .await
                .map_err(|e| CoreError::NetworkError(e.to_string()))?;
            let html = crate::encoding::decode_html(&bytes, Some(&ct_header));
            self.active_page = Some(Page::from_html(url, &html, 200, ct_header).await?);
            self.inject_dom_snapshot();
            Ok(())
        } else {
            Err(CoreError::NavigationFailed("no next page".into()))
        }
    }

    /// Reload the current page.
    pub async fn reload(&mut self) -> Result<()> {
        if let Some(url) = self.current_url() {
            let response = self.http_client.fetch(url).await?;
            let ct_header = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html")
                .to_string();
            let bytes = response
                .bytes()
                .await
                .map_err(|e| CoreError::NetworkError(e.to_string()))?;
            let html = crate::encoding::decode_html(&bytes, Some(&ct_header));
            self.active_page = Some(Page::from_html(url.clone(), &html, 200, ct_header).await?);
            self.inject_dom_snapshot();
            Ok(())
        } else {
            Err(CoreError::NavigationFailed("no current page".into()))
        }
    }

    /// Send a POST request and load the response as a page.
    ///
    /// The `content_type` determines how the body is encoded:
    /// - `"application/json"` — body is parsed as JSON and sent as JSON
    /// - `"application/x-www-form-urlencoded"` — body is parsed as `key=value&key2=value2` form data
    /// - Any other value — body is sent as raw bytes
    pub async fn post(&mut self, url: &str, body: &str, content_type: &str) -> Result<()> {
        let parsed = Url::parse(url)?;

        info!(url = %parsed, content_type, "POST request");

        let response = match content_type {
            "application/json" => {
                let json_value = serde_json::from_str::<serde_json::Value>(body)
                    .unwrap_or(serde_json::Value::Null);
                self.http_client.post_json(&parsed, &json_value).await?
            }
            "application/x-www-form-urlencoded" => {
                let form: Vec<(&str, &str)> = body
                    .split('&')
                    .filter_map(|pair| {
                        let mut parts = pair.splitn(2, '=');
                        Some((parts.next()?, parts.next().unwrap_or("")))
                    })
                    .collect();
                self.http_client.post_form(&parsed, &form).await?
            }
            _ => self.http_client.post(&parsed, body.to_string()).await?,
        };

        let status = response.status().as_u16();
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/html")
            .to_string();

        let bytes = response
            .bytes()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        let html = crate::encoding::decode_html(&bytes, Some(&ct));

        // Create a new page for this navigation
        let page = Page::from_html(parsed.clone(), &html, status, ct).await?;

        // Update history
        if self.history.is_empty() {
            // First navigation
        } else if self.history_index < self.history.len() - 1 {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(parsed);
        self.history_index = self.history.len() - 1;

        self.active_page = Some(page);

        // Inject DOM snapshot into JS runtime
        self.inject_dom_snapshot();

        Ok(())
    }

    /// Evaluate JavaScript.
    ///
    /// Works with or without an active page. Without a page, the DOM bridge
    /// (document.querySelector etc.) will return empty/null results, but
    /// pure JS expressions (arithmetic, JSON, etc.) work fine.
    ///
    /// After evaluation, any DOM mutations recorded by JS (setAttribute,
    /// click, value setter) are applied to the actual DOM and the snapshot
    /// is re-injected into the JS runtime.
    pub async fn evaluate_js(
        &mut self,
        expression: &str,
    ) -> Result<crate::js::runtime::JsEvalResult> {
        if self.closed {
            return Err(CoreError::SessionClosed);
        }
        let result = self.js_runtime.evaluate(expression).await?;

        // Collect and apply DOM mutations
        let mutations = self.js_runtime.drain_mutations();
        if !mutations.is_empty() {
            self.apply_mutations(&mutations);
            self.inject_dom_snapshot();
        }

        Ok(result)
    }

    /// Apply recorded DOM mutations to the active page's DOM tree.
    fn apply_mutations(&mut self, mutations: &[DomMutation]) {
        for m in mutations {
            match m {
                DomMutation::SetAttribute {
                    node_id,
                    name,
                    value,
                } => {
                    if let Some(page) = &mut self.active_page {
                        page.root_frame_mut().set_attribute(
                            oxibrowser_webapi::dom::NodeId(*node_id as usize),
                            name,
                            value,
                        );
                    }
                }
                DomMutation::SetTextContent { node_id, text } => {
                    if let Some(page) = &mut self.active_page {
                        page.root_frame_mut().set_text_content(
                            oxibrowser_webapi::dom::NodeId(*node_id as usize),
                            text,
                        );
                    }
                }
                DomMutation::ClickElement { node_id } => {
                    // Click doesn't modify DOM directly, but could trigger navigation
                    tracing::info!(node_id, "element clicked");
                }
                DomMutation::InputElement { node_id, value } => {
                    if let Some(page) = &mut self.active_page {
                        page.root_frame_mut().set_attribute(
                            oxibrowser_webapi::dom::NodeId(*node_id as usize),
                            "value",
                            value,
                        );
                    }
                }
                // DOM structure mutations (not yet supported)
                DomMutation::CreateElement { .. }
                | DomMutation::CreateTextNode { .. }
                | DomMutation::AppendChild { .. }
                | DomMutation::RemoveChild { .. }
                | DomMutation::SetInnerHtml { .. } => {}
            }
        }
    }

    /// Inject the current page's DOM snapshot into the JS runtime.
    fn inject_dom_snapshot(&mut self) {
        if let Some(page) = &self.active_page {
            let snapshot = DomSnapshot::from_frame(page.root_frame());
            let url = self
                .current_url()
                .map(|u| u.as_str())
                .unwrap_or("")
                .to_string();
            self.js_runtime.set_dom_snapshot(Some(snapshot));
            // Update page URL for window.location
            self.js_runtime.set_page_url(&url);
        }
    }

    /// Wait for a CSS selector to match an element in the current page.
    ///
    /// Polls the active page's DOM every 50ms until the selector matches
    /// or the timeout is exceeded.
    pub async fn wait_for(&mut self, selector: &str, timeout_ms: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let duration = std::time::Duration::from_millis(timeout_ms);

        loop {
            if let Some(page) = &self.active_page {
                if page.root_frame().query_selector(selector).is_some() {
                    return Ok(());
                }
            }

            if start.elapsed() >= duration {
                return Err(CoreError::NavigationFailed(format!(
                    "wait_for('{}') timed out after {}ms",
                    selector, timeout_ms
                )));
            }

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
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

    /// Store a response body for later retrieval (Network.getResponseBody).
    pub fn store_response_body(&self, request_id: &str, body: String, content_type: &str) {
        let mut guard = self.response_bodies.write();
        guard.insert(
            request_id.to_string(),
            CapturedResponse { body, base64: false, content_type: content_type.to_string() },
        );
    }

    /// Get a stored response body by request ID.
    pub fn get_response_body(&self, request_id: &str) -> Option<CapturedResponse> {
        self.response_bodies.read().get(request_id).cloned()
    }

    /// Get the cookie jar for this session.
    pub fn cookie_jar(&self) -> &Arc<RwLock<crate::network::CookieJar>> {
        &self.cookie_jar
    }

    /// Close the session.
    pub async fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        info!(id = %self.id, "session closed");
        self.closed = true;
        self.active_page = None;
        self.history.clear();
        self.local_storage.clear();
        Ok(())
    }

    /// Whether the session has been closed.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Fetch sub-resources (JS, CSS, images) referenced by the current page.
    ///
    /// Extracts resource URLs from the DOM, fetches them over HTTP,
    /// and attaches them as `Resource` objects to the page.
    ///
    /// Returns the number of resources successfully loaded.
    pub async fn load_sub_resources(&mut self) -> usize {
        let resource_urls = match self.active_page.as_ref() {
            Some(page) => page.root_frame().extract_resource_urls(),
            None => return 0,
        };

        if resource_urls.is_empty() {
            return 0;
        }

        let base_url = match self.current_url() {
            Some(u) => u.clone(),
            None => return 0,
        };

        let mut loaded = 0;
        for res in &resource_urls {
            // Resolve relative URLs against the page URL
            let full_url = match base_url.join(&res.url) {
                Ok(u) => u,
                Err(_) => continue,
            };

            let resource_type = match res.kind {
                oxibrowser_webapi::dom::ResourceKind::Script => {
                    crate::network::resource::ResourceType::Script
                }
                oxibrowser_webapi::dom::ResourceKind::Stylesheet => {
                    crate::network::resource::ResourceType::Stylesheet
                }
                oxibrowser_webapi::dom::ResourceKind::Image => {
                    crate::network::resource::ResourceType::Image
                }
                oxibrowser_webapi::dom::ResourceKind::Iframe => {
                    crate::network::resource::ResourceType::Document
                }
            };

            match self.http_client.fetch_text(&full_url).await {
                Ok(body) => {
                    let resource = crate::network::resource::Resource {
                        url: full_url.to_string(),
                        resource_type,
                        status: 200,
                        mime_type: String::new(),
                        body: bytes::Bytes::from(body),
                        loaded_at: std::time::Instant::now(),
                    };
                    if let Some(page) = self.active_page.as_mut() {
                        page.add_resource(resource);
                    }
                    loaded += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        url = %full_url,
                        error = %e,
                        "failed to load sub-resource"
                    );
                }
            }
        }

        tracing::info!(
            loaded = loaded,
            total = resource_urls.len(),
            "sub-resources loaded"
        );
        loaded
    }
}
