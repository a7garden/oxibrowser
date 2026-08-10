//! Session — browsing context group with cookie jar, storage, and history.
//!

//! Session — browsing context group with cookie jar, storage, and history.

use crate::browser::BrowserId;
use crate::config::BrowserConfig;
use crate::error::{CoreError, Result};
use crate::frame::Frame;
use crate::js::JsRuntime;
use crate::js::dom_snapshot::DomMutation;
use crate::js::runtime::JsRuntimeConfig;
use crate::js::runtime::{FetchRequestMsg, FetchResponseMsg, LocalStorageMsg, WsReqMsg};
use crate::network::HttpClient;
use crate::network::cookie::CookieJar;
use crate::network::ws::{WsCmd, WsEvent, run_ws_connection};
use crate::page::Page;
use parking_lot::RwLock;
use percent_encoding::percent_decode_str;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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
    /// Session-local storage (shared with localStorage sync handler thread).
    local_storage: Arc<parking_lot::RwLock<std::collections::HashMap<String, String>>>,
    /// Stored response bodies (requestId -> body) for getResponseBody.
    response_bodies: Arc<parking_lot::RwLock<HashMap<String, CapturedResponse>>>,
    /// JS runtime (per-session).
    js_runtime: JsRuntime,
    /// Fetch handler task handle (for cleanup).
    #[allow(dead_code)]
    fetch_task: Option<std::thread::JoinHandle<()>>,
    /// LocalStorage sync handler task handle (for cleanup).
    #[allow(dead_code)]
    local_storage_task: Option<std::thread::JoinHandle<()>>,
    /// WebSocket bridge task handle (for cleanup).
    #[allow(dead_code)]
    ws_task: Option<std::thread::JoinHandle<()>>,
    /// Whether the session has been closed.
    closed: AtomicBool,
    /// In-flight HTTP request counter shared with the fetch handler thread.
    ///
    /// Incremented when a request is dispatched (navigate / go_back /
    /// go_forward / reload / post / load_sub_resources / JS-issued fetch)
    /// and decremented when its response (or terminal error) is observed.
    /// `wait_for_condition(NetworkIdle)` polls this counter on the Tab side.
    /// Stored as `Arc<AtomicU64>` so the background `handle_fetch_requests`
    /// thread can share the same counter without holding `&Session` — matches
    /// the existing pattern for `local_storage` and `response_bodies`.
    in_flight: Arc<AtomicU64>,
    /// Shared dialog-resolution gate for blocking `alert`/`confirm`/`prompt`.
    /// Written by the CDP layer (`Page.handleJavaScriptDialog`), polled by the
    /// JS thread's dialog closures.
    dialog_gate: crate::js::DialogGate,
    /// Optional CoreEvent sender (clone of the one given to the JS runtime)
    /// so the navigate path + the fetch bridge can emit download / interception
    /// events from their async threads. Shared (Arc) so the background fetch
    /// bridge thread can read it once `set_event_sink` populates it.
    event_tx:
        std::sync::Arc<parking_lot::RwLock<Option<std::sync::mpsc::Sender<crate::js::CoreEvent>>>>,
    /// Frame-id → execution-context-id mapping for per-frame JS evaluation
    /// (Phase 8). The main frame is always context_id=1 and is NOT stored
    /// here; only child iframe contexts (≥ 2) appear.
    frame_contexts: parking_lot::RwLock<HashMap<String /* "frame-N" */, u32 /* context_id */>>,
    /// Next child execution-context id to assign (starts at 2; main=1).
    next_context_id: std::sync::atomic::AtomicU32,
}

/// Configurable download directory for `Content-Disposition: attachment`
/// responses. Set via [`set_download_behavior`] (CDP `Page.setDownloadBehavior`).
static DOWNLOAD_DIR: std::sync::LazyLock<parking_lot::RwLock<Option<std::path::PathBuf>>> =
    std::sync::LazyLock::new(|| parking_lot::RwLock::new(None));

/// Set the download directory (`None` = downloads disabled / discarded).
pub fn set_download_behavior(path: Option<std::path::PathBuf>) {
    *DOWNLOAD_DIR.write() = path;
}

/// Emulated viewport override `(width, height)` set via
/// `Emulation.setDeviceMetricsOverride`. When set, navigations lay out at this
/// size instead of `BrowserConfig`'s viewport.
static VIEWPORT_OVERRIDE: std::sync::LazyLock<parking_lot::RwLock<Option<(u32, u32)>>> =
    std::sync::LazyLock::new(|| parking_lot::RwLock::new(None));

/// Install a viewport override consumed by navigation layout.
pub fn set_viewport_override(width: u32, height: u32) {
    *VIEWPORT_OVERRIDE.write() = Some((width.max(1), height.max(1)));
}

/// Clear the viewport override.
pub fn clear_viewport_override() {
    *VIEWPORT_OVERRIDE.write() = None;
}

/// Read the viewport override, if any.
pub(crate) fn current_viewport_override() -> Option<(u32, u32)> {
    *VIEWPORT_OVERRIDE.read()
}

// ---------------------------------------------------------------------------
// Fetch interception (JS fetch/XHR path)
// ---------------------------------------------------------------------------

/// Active Fetch-domain interception patterns (raw `urlPattern` strings), set by
/// CDP `Fetch.enable`. Read by the fetch bridge to decide whether a JS-originated
/// request should be paused.
static FETCH_PATTERNS: std::sync::LazyLock<parking_lot::RwLock<Vec<String>>> =
    std::sync::LazyLock::new(|| parking_lot::RwLock::new(Vec::new()));

/// Set the active interception patterns (CDP `Fetch.enable`).
pub fn set_fetch_patterns(patterns: Vec<String>) {
    *FETCH_PATTERNS.write() = patterns;
}

/// Read the active interception patterns.
pub(crate) fn fetch_patterns() -> Vec<String> {
    FETCH_PATTERNS.read().clone()
}

/// Whether a URL matches any interception pattern. A pattern is a simple glob:
/// `*` matches any substring; otherwise the URL must contain the pattern as a
/// substring (covers `http://example.com/*` and bare domain fragments).
pub(crate) fn url_matches_patterns(url: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    patterns.iter().any(|p| {
        if p.is_empty() {
            return false;
        }
        if p == "*" {
            return true;
        }
        // Glob: split on '*'; every non-empty segment must appear in order.
        if p.contains('*') {
            let segments: Vec<&str> = p.split('*').filter(|s| !s.is_empty()).collect();
            if segments.is_empty() {
                return true;
            }
            let mut pos = 0;
            for seg in segments {
                let Some(found) = url[pos..].find(seg) else {
                    return false;
                };
                pos += found + seg.len();
            }
            return true;
        }
        url.contains(p)
    })
}

/// Outcome of the Fetch-domain interception check for a JS-originated request.
#[derive(Debug)]
enum InterceptDecision {
    /// Proceed with the (possibly modified) request.
    Proceed {
        url: Url,
        method: String,
        headers: Vec<(String, String)>,
    },
    /// Respond directly (fail / fulfill) without a network request.
    Respond(FetchResponseMsg),
}

/// If Fetch interception is enabled and `url` matches a pattern, pause the
/// request (insert a `PausedRequest` + emit `CoreEvent::RequestPaused`), await
/// the client's decision, and return it. Otherwise (or on no decision) proceed
/// unchanged. The empty-pattern fast path returns immediately — no behavior
/// change when interception is not enabled.
async fn maybe_intercept(
    event_tx: &std::sync::Arc<
        parking_lot::RwLock<Option<std::sync::mpsc::Sender<crate::js::CoreEvent>>>,
    >,
    request_id: u64,
    url: &str,
    method: &str,
    headers: &[(String, String)],
) -> InterceptDecision {
    use crate::js::CoreEvent;
    use crate::network::intercept::{InterceptAction, PausedRequest, shared_registry};
    use tokio::sync::oneshot;

    let patterns = fetch_patterns();
    if !url_matches_patterns(url, &patterns) {
        return InterceptDecision::Proceed {
            url: Url::parse(url).unwrap_or_else(|_| Url::parse("about:blank").unwrap()),
            method: method.to_string(),
            headers: headers.to_vec(),
        };
    }

    let pause_id = format!("oxi-int-{}", uuid::Uuid::new_v4().as_simple());
    let (tx, rx) = oneshot::channel::<InterceptAction>();
    shared_registry().insert(
        pause_id.clone(),
        PausedRequest {
            url: url.to_string(),
            method: method.to_string(),
            headers: headers.to_vec(),
            resource_type: "XHR".to_string(),
            tx,
        },
    );

    if let Some(sender) = event_tx.read().as_ref() {
        let _ = sender.send(CoreEvent::RequestPaused {
            request_id: pause_id.clone(),
            url: url.to_string(),
            method: method.to_string(),
            headers: headers.to_vec(),
            resource_type: "XHR".to_string(),
            timestamp: current_time_ms(),
        });
    }

    match rx.await {
        Ok(InterceptAction::Continue {
            url: Some(u),
            method: Some(m),
            headers: h,
            ..
        }) => InterceptDecision::Proceed {
            url: Url::parse(&u).unwrap_or_else(|_| Url::parse(url).unwrap()),
            method: m,
            headers: h,
        },
        Ok(InterceptAction::Continue { .. }) => InterceptDecision::Proceed {
            url: Url::parse(url).unwrap(),
            method: method.to_string(),
            headers: headers.to_vec(),
        },
        Ok(InterceptAction::Fail { error_reason }) => {
            InterceptDecision::Respond(FetchResponseMsg {
                id: request_id,
                status: 0,
                status_text: "Network Error".to_string(),
                url: url.to_string(),
                headers: vec![],
                body: String::new(),
                error: Some(error_reason),
            })
        }
        Ok(InterceptAction::Fulfill {
            status_code,
            status_text,
            headers,
            body,
        }) => InterceptDecision::Respond(FetchResponseMsg {
            id: request_id,
            status: status_code,
            status_text,
            url: url.to_string(),
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
            error: None,
        }),
        Err(_) => {
            // No decision (client never responded) — proceed with the request.
            InterceptDecision::Proceed {
                url: Url::parse(url).unwrap(),
                method: method.to_string(),
                headers: headers.to_vec(),
            }
        }
    }
}

fn current_time_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// Fetch handler
// ---------------------------------------------------------------------------

/// Dispatch fetch requests from the JS thread to real HTTP I/O.
///
/// Runs a minimal tokio runtime and **spawns an independent task per request**
/// (Phase 3), so concurrent in-flight fetches run in parallel rather than
/// serially. Each task awaits its own `http_client.fetch`, then pushes a single
/// `FetchResponseMsg { id, .. }` onto the shared `response_tx` (routed back to
/// the JS thread's `PENDING_FETCH` registry by `id`) and decrements `in_flight`.
///
/// `in_flight` is incremented before spawn and decremented exactly once per
/// task on every terminal branch — including the error/early-return paths — so
/// the counter never leaks and `wait_for_condition(NetworkIdle)` observes real
/// parallelism.
fn handle_fetch_requests(
    fetch_rx: std::sync::mpsc::Receiver<FetchRequestMsg>,
    response_tx: std::sync::mpsc::Sender<FetchResponseMsg>,
    http_client: Arc<HttpClient>,
    _cookie_jar: Arc<RwLock<CookieJar>>,
    max_body_bytes: usize,
    in_flight: Arc<AtomicU64>,
    event_tx: std::sync::Arc<
        parking_lot::RwLock<Option<std::sync::mpsc::Sender<crate::js::CoreEvent>>>,
    >,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("failed to create tokio runtime for fetch: {}", e);
            return;
        }
    };

    rt.block_on(async {
        loop {
            // try_recv + sleep().await: must yield to the current-thread
            // runtime so spawned tasks get polled. A blocking recv() would
            // park the OS thread and starve the runtime (deadlock).
            match fetch_rx.try_recv() {
                Ok(request) => {
                    // Mark in-flight before spawning — a NetworkIdle observer
                    // may read the counter at any moment.
                    in_flight.fetch_add(1, Ordering::Relaxed);

                    let http_client = http_client.clone();
                    let response_tx = response_tx.clone();
                    let in_flight = in_flight.clone();
                    let event_tx = event_tx.clone();

                    // Spawn an independent task per request so concurrent
                    // fetches run in parallel (Phase 3), not one-at-a-time.
                    tokio::spawn(async move {
                        let id = request.id;
                        let method = request.method;
                        let headers = request.headers;
                        let body = request.body;
                        let origin = request.origin;
                        let url_str = request.url;

                        if Url::parse(&url_str).is_err() {
                            let _ = response_tx.send(FetchResponseMsg {
                                id,
                                status: 400,
                                status_text: "Invalid URL".to_string(),
                                url: url_str,
                                headers: vec![],
                                body: String::new(),
                                error: Some("invalid URL".to_string()),
                            });
                            in_flight.fetch_sub(1, Ordering::Relaxed);
                            return;
                        }

                        // Fetch-domain interception (JS fetch/XHR path).
                        let decision =
                            maybe_intercept(&event_tx, id, &url_str, &method, &headers).await;
                        let resp = match decision {
                            InterceptDecision::Respond(msg) => {
                                let _ = response_tx.send(msg);
                                in_flight.fetch_sub(1, Ordering::Relaxed);
                                return;
                            }
                            InterceptDecision::Proceed {
                                url,
                                method,
                                headers,
                            } => {
                                http_client
                                    .request_with_context(
                                        &url,
                                        &method,
                                        &headers,
                                        body,
                                        origin.as_deref(),
                                    )
                                    .await
                            }
                        };
                        match resp {
                            Ok(response) => {
                                let status = response.status().as_u16();
                                let status_text = response
                                    .status()
                                    .canonical_reason()
                                    .unwrap_or("")
                                    .to_string();
                                let resp_url = response.uri().to_string();
                                let headers: Vec<(String, String)> = response
                                    .headers()
                                    .iter()
                                    .map(|(k, v)| {
                                        (k.to_string(), v.to_str().unwrap_or("").to_string())
                                    })
                                    .collect();
                                let body =
                                    match HttpClient::read_body_limited(response, max_body_bytes)
                                        .await
                                    {
                                        Ok((buf, truncated)) => {
                                            if truncated {
                                                tracing::warn!(
                                                    url = %resp_url,
                                                    max_bytes = max_body_bytes,
                                                    "fetch body truncated"
                                                );
                                            }
                                            String::from_utf8_lossy(&buf).into_owned()
                                        }
                                        Err(e) => {
                                            let _ = response_tx.send(FetchResponseMsg {
                                                id,
                                                status,
                                                status_text,
                                                url: resp_url,
                                                headers,
                                                body: String::new(),
                                                error: Some(format!("failed to read body: {}", e)),
                                            });
                                            in_flight.fetch_sub(1, Ordering::Relaxed);
                                            return;
                                        }
                                    };

                                let _ = response_tx.send(FetchResponseMsg {
                                    id,
                                    status,
                                    status_text,
                                    url: resp_url,
                                    headers,
                                    body,
                                    error: None,
                                });
                                in_flight.fetch_sub(1, Ordering::Relaxed);
                            }
                            Err(e) => {
                                let _ = response_tx.send(FetchResponseMsg {
                                    id,
                                    status: 0,
                                    status_text: "Network Error".to_string(),
                                    url: url_str,
                                    headers: vec![],
                                    body: String::new(),
                                    error: Some(e.to_string()),
                                });
                                in_flight.fetch_sub(1, Ordering::Relaxed);
                            }
                        }
                    });
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
    });
}
/// Background WebSocket bridge: routes Connect/Send/Close from the JS thread
/// to per-socket tokio tasks. Events flow straight to the JS-thread
/// `WS_EVENT_RX` via the shared event channel (id-routed). Mirrors
/// `handle_fetch_requests` (try_recv + sleep polling — never a blocking recv
/// inside the current-thread runtime, or spawned socket tasks stall).
pub(crate) fn handle_ws_requests(
    ws_req_rx: std::sync::mpsc::Receiver<WsReqMsg>,
    ws_event_tx: std::sync::mpsc::Sender<WsEvent>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("failed to create tokio runtime for ws: {}", e);
            return;
        }
    };
    let mut sockets: std::collections::HashMap<u64, tokio::sync::mpsc::Sender<WsCmd>> =
        std::collections::HashMap::new();
    rt.block_on(async move {
        loop {
            while let Ok(req) = ws_req_rx.try_recv() {
                match req {
                    WsReqMsg::Connect { id, url, protocols } => {
                        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<WsCmd>(16);
                        let event_tx = ws_event_tx.clone();
                        sockets.insert(id, cmd_tx);
                        tokio::spawn(async move {
                            run_ws_connection(id, url, protocols, cmd_rx, event_tx).await;
                        });
                    }
                    WsReqMsg::Send { id, data } => {
                        if let Some(tx) = sockets.get(&id) {
                            let _ = tx.try_send(WsCmd::Send(data));
                        }
                    }
                    WsReqMsg::Close { id, code, reason } => {
                        if let Some(tx) = sockets.get(&id) {
                            let _ = tx.try_send(WsCmd::Close { code, reason });
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });
}

// ---------------------------------------------------------------------------
// LocalStorage sync handler
// ---------------------------------------------------------------------------
/// Handle localStorage sync messages from the JS thread.
///
/// Updates the Session's shared `local_storage` HashMap in response to
/// JS localStorage.setItem/removeItem/clear calls.
fn handle_local_storage_sync(
    ls_rx: std::sync::mpsc::Receiver<LocalStorageMsg>,
    local_storage: Arc<parking_lot::RwLock<std::collections::HashMap<String, String>>>,
) {
    while let Ok(msg) = ls_rx.recv() {
        match msg {
            LocalStorageMsg::SetItem(key, value) => {
                local_storage.write().insert(key, value);
            }
            LocalStorageMsg::RemoveItem(key) => {
                local_storage.write().remove(&key);
            }
            LocalStorageMsg::Clear => {
                local_storage.write().clear();
            }
        }
    }
}

/// RAII guard for the Session in-flight request counter.
///
/// Increments on construction; decrements on drop. Using a guard instead of
/// manual `fetch_add` / `fetch_sub` pairs ensures the counter always returns
/// to its correct value even when an awaited HTTP call returns `Err` and
/// the caller early-returns via `?` — the guard's `Drop` runs regardless.
struct InFlightGuard {
    counter: Arc<AtomicU64>,
}

impl InFlightGuard {
    fn new(counter: Arc<AtomicU64>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Session {
    /// Create a new session.
    #[tracing::instrument(skip(config, http_client, cookie_jar), err)]
    pub async fn new(
        browser_id: BrowserId,
        config: BrowserConfig,
        http_client: Arc<HttpClient>,
        cookie_jar: Arc<RwLock<CookieJar>>,
    ) -> Result<Self> {
        let js_config = JsRuntimeConfig::from(&config);

        // Fetch channels: request sender (JS→background) + shared response
        // receiver (background→JS, id-routed). Phase 3 async fetch.
        let (fetch_tx, fetch_rx) = std::sync::mpsc::channel();
        let (fetch_resp_tx, fetch_resp_rx) = std::sync::mpsc::channel::<FetchResponseMsg>();

        // Create localStorage sync channel
        let (ls_tx, ls_rx) = std::sync::mpsc::channel::<LocalStorageMsg>();

        // Create JS runtime and wire up fetch channels
        let mut js_runtime = JsRuntime::with_config(js_config);
        js_runtime.set_fetch_channel(fetch_tx, fetch_resp_rx);
        js_runtime.set_local_storage_channel(ls_tx);
        // Dialog gate: shared cell for blocking alert/confirm/prompt, resolved
        // by the CDP layer via Page.handleJavaScriptDialog.
        let dialog_gate: crate::js::DialogGate = Arc::new(parking_lot::Mutex::new(None));
        js_runtime.set_dialog_gate(dialog_gate.clone());
        // WebSocket channels: request sender (JS→bridge) + shared event
        // receiver (bridge→JS, id-routed). Phase 4 WebSocket.
        let (ws_req_tx, ws_req_rx) = std::sync::mpsc::channel::<WsReqMsg>();
        let (ws_event_tx, ws_event_rx) = std::sync::mpsc::channel::<WsEvent>();
        js_runtime.set_ws_channel(ws_req_tx, ws_event_rx);

        // Spawn fetch handler on a blocking thread
        let http_client_clone = http_client.clone();
        let cookie_jar_clone = cookie_jar.clone();
        let in_flight = Arc::new(AtomicU64::new(0));
        let in_flight_clone = in_flight.clone();
        let max_body_bytes = config.max_response_body_bytes;
        let event_tx = std::sync::Arc::new(parking_lot::RwLock::new(None));
        let event_tx_clone = event_tx.clone();
        let fetch_task = Some(std::thread::spawn(move || {
            handle_fetch_requests(
                fetch_rx,
                fetch_resp_tx,
                http_client_clone,
                cookie_jar_clone,
                max_body_bytes,
                in_flight_clone,
                event_tx_clone,
            );
        }));
        // Spawn WebSocket bridge handler thread (Phase 4)
        let ws_task = Some(std::thread::spawn(move || {
            handle_ws_requests(ws_req_rx, ws_event_tx);
        }));

        // Spawn localStorage sync handler thread
        let local_storage_arc =
            Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new()));
        let ls_arc_clone = local_storage_arc.clone();
        let local_storage_task = Some(std::thread::spawn(move || {
            handle_local_storage_sync(ls_rx, ls_arc_clone);
        }));

        if let Err(e) = js_runtime.set_cookie_jar(cookie_jar.clone()) {
            tracing::warn!("failed to set cookie jar: {}", e);
        }

        Ok(Self {
            id: SessionId::next(),
            browser_id,
            config,
            http_client,
            cookie_jar,
            active_page: None,
            history: Vec::new(),
            history_index: 0,
            local_storage: local_storage_arc,
            response_bodies: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            js_runtime,
            fetch_task,
            local_storage_task,
            ws_task,
            closed: AtomicBool::new(false),
            dialog_gate,
            event_tx,
            frame_contexts: parking_lot::RwLock::new(HashMap::new()),
            next_context_id: std::sync::atomic::AtomicU32::new(2),
            in_flight,
        })
    }

    /// Navigate to a URL.
    #[tracing::instrument(skip(self), fields(session = %self.id), err)]
    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(CoreError::SessionClosed);
        }

        let parsed = Url::parse(url)?;

        // `data:` URLs are resolved locally (no HTTP fetch) so the stealth
        // surface can be exercised fully offline.
        // `about:` URLs create an empty local page (no HTTP fetch).
        // `about:blank` is the canonical case, but we accept any about:<path>
        // and render it identically to about:blank for now.
        if parsed.scheme() == "about" {
            return self.navigate_about().await;
        }

        if parsed.scheme() == "data" {
            return self.navigate_data_url(&parsed).await;
        }

        info!(url = %parsed, "navigating");

        // Fetch the document
        let start = std::time::Instant::now();
        let _in_flight = InFlightGuard::new(self.in_flight.clone());
        let response = self.http_client.fetch(&parsed).await?;
        let status = response.status().as_u16();
        let final_url = Url::parse(&response.uri().to_string()).unwrap_or_else(|_| parsed.clone());

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
        let content_disposition = response
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let max = self.config.max_response_body_bytes;
        let (bytes, truncated) = HttpClient::read_body_limited(response, max).await?;
        if truncated {
            tracing::warn!(final_url = %final_url, max_bytes = max, "navigate body truncated");
        }

        // Download handling: a `Content-Disposition: attachment` response (or
        // a non-HTML content type with a download dir configured) is saved to
        // the download directory instead of being rendered.
        if content_disposition
            .to_ascii_lowercase()
            .contains("attachment")
        {
            if let Err(e) = self.handle_download(&final_url, &content_disposition, &bytes) {
                tracing::warn!(error = %e, "download handling failed; falling back to render");
            } else {
                return Ok(());
            }
        }

        let html = crate::encoding::decode_html(&bytes, Some(&ct_header));

        tracing::debug!(status, final_url = %final_url, elapsed_ms = start.elapsed().as_millis() as u64, "page fetched");

        // Store the response body for Network.getResponseBody
        if !html.is_empty() {
            let request_id = format!("REQ-{}", uuid::Uuid::new_v4().as_simple());
            self.store_response_body(&request_id, html.clone(), &ct_header);
            tracing::trace!(request_id, body_len = html.len(), "response body stored");
        }

        tracing::debug!(html_bytes = html.len(), "response decoded");

        // Create a new page for this navigation (use final URL after redirects)
        let mut page = Page::from_html(final_url.clone(), &html, status, ct_header).await?;

        // Phase 8: populate child <iframe> frames by fetching each
        // src (resolved against the page URL) and parsing it into a child Frame.
        self.populate_iframes(&mut page, &final_url).await;

        // Update history
        if self.history.is_empty() {
            // First navigation — just push
        } else if self.history_index < self.history.len() - 1 {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(final_url);
        self.history_index = self.history.len() - 1;

        self.active_page = Some(page);

        // Clear previous navigation's child-frame contexts (Phase 8).
        self.js_runtime.clear_child_contexts();
        self.frame_contexts.write().clear();
        self.next_context_id
            .store(2, std::sync::atomic::Ordering::Relaxed);

        // Inject DOM snapshot into JS runtime
        self.inject_dom_snapshot().await;

        // Phase 8: build per-frame execution contexts for child iframes.
        self.inject_child_frames().await;

        Ok(())
    }

    /// Save a downloaded attachment to the configured download directory and
    /// emit a [`CoreEvent::Download`]. Returns `Err` if no download directory
    /// is configured (so the caller falls back to rendering the body).
    fn handle_download(&self, url: &Url, disposition: &str, bytes: &[u8]) -> Result<()> {
        let dir = DOWNLOAD_DIR
            .read()
            .clone()
            .ok_or_else(|| CoreError::NetworkError("no download directory configured".into()))?;
        std::fs::create_dir_all(&dir).map_err(|e| CoreError::NetworkError(e.to_string()))?;

        let filename = filename_from_disposition(disposition)
            .or_else(|| {
                url.path_segments()
                    .and_then(|mut s| s.next_back())
                    .filter(|n| !n.is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "download.bin".to_string());
        // Sanitize: keep only the basename (no path traversal).
        let safe = std::path::Path::new(&filename)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| "download.bin".to_string());
        let save_path = dir.join(&safe);

        std::fs::write(&save_path, bytes)
            .map_err(|e| CoreError::NetworkError(format!("failed to write download: {e}")))?;

        let guid = format!("dl-{}", uuid::Uuid::new_v4().as_simple());
        if let Some(tx) = self.event_tx.read().as_ref() {
            let _ = tx.send(crate::js::CoreEvent::Download {
                guid: guid.clone(),
                url: url.to_string(),
                filename: safe.clone(),
                save_path: save_path.to_string_lossy().into_owned(),
                total_bytes: bytes.len(),
            });
        }
        tracing::info!(url = %url, filename = %safe, bytes = bytes.len(), "download saved");
        Ok(())
    }

    /// Fetch each `<iframe src>` in the page's root frame and attach the
    /// fetched document as a child [`Frame`] (Phase 8 population step).
    /// Failures (bad URL, network error) are logged and skipped so a single
    /// broken iframe can't abort navigation.
    async fn populate_iframes(&self, page: &mut Page, base_url: &Url) {
        let iframes = page.root_frame().document().extract_iframes();
        for iframe in iframes {
            // 1. srcdoc → inline content, no fetch (W3a).
            if let Some(srcdoc) = iframe.srcdoc {
                let child_url =
                    Url::parse("about:srcdoc").unwrap_or_else(|_| base_url.clone());
                match Frame::from_html(child_url, &srcdoc).await {
                    Ok(child) => page.root_frame_mut().add_child(child),
                    Err(e) => tracing::warn!(error = %e, "failed to parse srcdoc iframe"),
                }
                continue;
            }
            // 2. src present — resolve against the page URL.
            let Some(src) = iframe.src else { continue };
            let Ok(full) = base_url.join(&src) else { continue };
            // 2a. non-http(s) (about:blank, javascript:, etc.) → empty child (W3a).
            if full.scheme() != "http" && full.scheme() != "https" {
                let child_url =
                    Url::parse("about:blank").unwrap_or_else(|_| base_url.clone());
                let empty = "<!DOCTYPE html><html><head></head><body></body></html>";
                match Frame::from_html(child_url, empty).await {
                    Ok(child) => page.root_frame_mut().add_child(child),
                    Err(e) => tracing::warn!(src = %src, error = %e, "failed to parse about:blank iframe"),
                }
                continue;
            }
            // 2b. http(s) → fetch + parse (original behavior).
            match self.http_client.fetch_text(&full).await {
                Ok(child_html) => match Frame::from_html(full.clone(), &child_html).await {
                    Ok(child) => page.root_frame_mut().add_child(child),
                    Err(e) => tracing::warn!(src = %src, error = %e, "failed to parse iframe document"),
                },
                Err(e) => tracing::warn!(src = %src, error = %e, "failed to fetch iframe"),
            }
        }
    }

    /// Navigate to a URL with automatic retries on transient failures.
    ///
    /// Retries DNS errors, connection timeouts, and 5xx errors with
    /// exponential backoff (500ms, 1000ms, 1500ms, ...).
    async fn navigate_data_url(&mut self, url: &Url) -> Result<()> {
        let data_str = url.as_str();
        let data_part = data_str.strip_prefix("data:").unwrap_or("");
        let (mime, encoded_body) = if let Some(comma_idx) = data_part.find(',') {
            let mime = data_part[..comma_idx].trim().to_string();
            let body = &data_part[comma_idx + 1..];
            (mime, body)
        } else {
            ("text/plain".to_string(), data_part)
        };

        // Percent-decode the body (the url crate encodes special chars)
        let body = percent_decode_str(encoded_body)
            .decode_utf8()
            .unwrap_or_else(|_| encoded_body.into());

        let page = Page::from_html(url.clone(), &body, 200, mime.clone()).await?;
        if self.history.is_empty() {
        } else if self.history_index < self.history.len() - 1 {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(url.clone());
        self.history_index = self.history.len() - 1;
        self.active_page = Some(page);
        self.inject_dom_snapshot().await;
        Ok(())
    }

    /// Navigate to an `about:` URL — creates an empty page without network fetch.
    /// `about:blank` is the canonical case; `about:srcdoc`, `about:config`, etc.
    /// all render as a blank HTML5 document for simplicity.
    async fn navigate_about(&mut self) -> Result<()> {
        const ABOUT_HTML: &str = r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>about:blank</title></head><body></body></html>"#;
        let about_url = Url::parse("about:blank").unwrap();
        let page = Page::from_html(about_url.clone(), ABOUT_HTML, 200, "text/html".into()).await?;
        if self.history.is_empty() {
        } else if self.history_index < self.history.len() - 1 {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(about_url.clone());
        self.history_index = self.history.len() - 1;
        self.active_page = Some(page);
        self.inject_dom_snapshot().await;
        Ok(())
    }
    #[tracing::instrument(skip(self), fields(session = %self.id), err)]
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

        Err(last_error
            .unwrap_or_else(|| CoreError::NavigationFailed("no retry attempts were made".into())))
    }
    pub async fn go_back(&mut self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(CoreError::SessionClosed);
        }
        if self.history_index > 0 {
            self.history_index -= 1;
            let url = self.history[self.history_index].clone();

            // Re-fetch without adding to history
            let _in_flight = InFlightGuard::new(self.in_flight.clone());
            let response = self.http_client.fetch(&url).await?;
            let ct_header = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html")
                .to_string();
            let max = self.config.max_response_body_bytes;
            let (bytes, truncated) = HttpClient::read_body_limited(response, max).await?;
            if truncated {
                tracing::warn!(url = %url, max_bytes = max, "history body truncated");
            }
            let html = crate::encoding::decode_html(&bytes, Some(&ct_header));
            self.active_page = Some(Page::from_html(url, &html, 200, ct_header).await?);
            self.inject_dom_snapshot().await;
            Ok(())
        } else {
            Err(CoreError::NavigationFailed("no previous page".into()))
        }
    }

    /// Navigate forward in history.
    pub async fn go_forward(&mut self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(CoreError::SessionClosed);
        }
        if self.history_index < self.history.len() - 1 {
            self.history_index += 1;
            let url = self.history[self.history_index].clone();

            let _in_flight = InFlightGuard::new(self.in_flight.clone());
            let response = self.http_client.fetch(&url).await?;
            let ct_header = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html")
                .to_string();
            let max = self.config.max_response_body_bytes;
            let (bytes, truncated) = HttpClient::read_body_limited(response, max).await?;
            if truncated {
                tracing::warn!(url = %url, max_bytes = max, "history body truncated");
            }
            let html = crate::encoding::decode_html(&bytes, Some(&ct_header));
            self.active_page = Some(Page::from_html(url, &html, 200, ct_header).await?);
            self.inject_dom_snapshot().await;
            Ok(())
        } else {
            Err(CoreError::NavigationFailed("no next page".into()))
        }
    }

    /// Reload the current page.
    pub async fn reload(&mut self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(CoreError::SessionClosed);
        }
        if let Some(url) = self.current_url() {
            let _in_flight = InFlightGuard::new(self.in_flight.clone());
            let response = self.http_client.fetch(url).await?;
            let ct_header = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html")
                .to_string();
            let max = self.config.max_response_body_bytes;
            let (bytes, truncated) = HttpClient::read_body_limited(response, max).await?;
            if truncated {
                tracing::warn!(url = %url, max_bytes = max, "reload body truncated");
            }
            let html = crate::encoding::decode_html(&bytes, Some(&ct_header));
            self.active_page = Some(Page::from_html(url.clone(), &html, 200, ct_header).await?);
            self.inject_dom_snapshot().await;
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
    #[tracing::instrument(skip(self, body), fields(session = %self.id), err)]
    pub async fn post(&mut self, url: &str, body: &str, content_type: &str) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(CoreError::SessionClosed);
        }
        let parsed = Url::parse(url)?;

        info!(url = %parsed, content_type, "POST request");

        let _in_flight = InFlightGuard::new(self.in_flight.clone());
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

        let final_url = Url::parse(&response.uri().to_string()).unwrap_or_else(|_| parsed.clone());

        let bytes = response
            .bytes()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;

        let html = crate::encoding::decode_html(&bytes, Some(&ct));

        // Create a new page for this navigation (use final URL after redirects)
        let page = Page::from_html(final_url.clone(), &html, status, ct).await?;

        // Update history
        if self.history.is_empty() {
            // First navigation
        } else if self.history_index < self.history.len() - 1 {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(final_url);
        self.history_index = self.history.len() - 1;

        self.active_page = Some(page);

        // Inject DOM snapshot into JS runtime
        self.inject_dom_snapshot().await;

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
        self.evaluate_js_with_await(expression, false).await
    }

    /// Evaluate a JS expression, optionally awaiting Promise resolution.
    #[tracing::instrument(skip(self), fields(session = %self.id), err)]
    pub async fn evaluate_js_with_await(
        &mut self,
        expression: &str,
        await_promise: bool,
    ) -> Result<crate::js::runtime::JsEvalResult> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(CoreError::SessionClosed);
        }
        tracing::debug!(expr_len = expression.len(), await = await_promise, "evaluating JS");
        let result = self
            .js_runtime
            .evaluate_with_await(expression, await_promise)
            .await?;

        // DOM edits are now applied live to the RenderDocument by the JS
        // bindings themselves — no mutation log to drain/apply. Only
        // JS-triggered navigation (location.href / assign / reload) is still
        // signalled via the mutation channel, because it needs async network I/O.
        for m in self.js_runtime.drain_mutations() {
            match m {
                DomMutation::Navigate { url } => {
                    tracing::debug!(url = %url, "JS-triggered navigation");
                    self.navigate(&url).await?;
                }
                DomMutation::Reload => {
                    tracing::debug!("JS-triggered reload");
                    self.reload().await?;
                }
                _ => {} // DOM edits handled directly on the RenderDocument.
            }
        }

        Ok(result)
    }

    /// Install the CoreEvent sink so console / exception / fetch / WebSocket /
    /// dialog events flow to an observer (typically the CDP layer). Called by
    /// the CDP session once it has created its event drainer.
    pub fn set_event_sink(&mut self, tx: std::sync::mpsc::Sender<crate::js::CoreEvent>) {
        *self.event_tx.write() = Some(tx.clone());
        self.js_runtime.set_event_sink(tx);
    }

    /// Resolve a pending `alert`/`confirm`/`prompt` dialog. Called by the CDP
    /// `Page.handleJavaScriptDialog` handler; wakes the blocked JS thread.
    pub fn resolve_dialog(&self, accept: bool, prompt_text: Option<String>) {
        *self.dialog_gate.lock() = Some(crate::js::DialogResult {
            accept,
            prompt_text,
        });
    }

    /// Clone of the shared dialog-resolution gate. Lets the CDP layer resolve a
    /// pending dialog WITHOUT acquiring the session lock (which a blocking
    /// `alert()` holds via `evaluate_js`).
    pub fn dialog_gate(&self) -> crate::js::DialogGate {
        self.dialog_gate.clone()
    }

    /// Capture a full-page PNG screenshot of the live (post-JS) document.
    ///
    /// Renders the current `RenderDocument` — which JS mutates directly — via
    /// the JS thread. This is a consistent snapshot between JS ticks, with no
    /// serialize/reparse round-trip (the legacy `DomSnapshot` bridge is gone).
    /// The document is laid out at the session's configured viewport.
    pub async fn capture_screenshot_png(&mut self, _viewport_width: u32) -> Result<Vec<u8>> {
        let opts = oxibrowser_render::CaptureOpts {
            viewport: None,
            full_page: true,
        };
        self.js_runtime.capture_png(opts).await
    }

    /// Inject the current page into the JS runtime.
    ///
    /// Builds the `RenderDocument` (the single DOM source of truth that JS
    /// mutates directly) from the page HTML, then also seeds the legacy
    /// `DomSnapshot` (still used by `document.title`/`document.cookie`/window
    /// globals until the webapi DOM is retired) and the page URL.
    async fn inject_dom_snapshot(&mut self) {
        let (html, url, mut scripts) = match &self.active_page {
            Some(page) => {
                let html = page.content().to_string();
                let url = self
                    .current_url()
                    .map(|u| u.as_str().to_string())
                    .unwrap_or_default();
                let scripts = page.root_frame().extract_scripts();
                (html, url, scripts)
            }
            None => return,
        };

        // Fetch external (<script src>) bodies in document order, filling each
        // entry's `source`. Inline scripts already carry their source. Fetch is
        // sequential + in-order for Phase 1 (parallel fetch is Phase 3).
        if !scripts.is_empty() {
            let base = Url::parse(&url).ok();
            for s in scripts.iter_mut() {
                let Some(src) = s.src_url.clone() else {
                    continue;
                };
                let Some(full_url) = base.as_ref().and_then(|b| b.join(&src).ok()) else {
                    continue;
                };
                let _in_flight = InFlightGuard::new(self.in_flight.clone());
                match self.http_client.fetch_text(&full_url).await {
                    Ok(body) => s.source = body,
                    Err(e) => {
                        tracing::warn!(src = %src, error = %e, "failed to fetch external script")
                    }
                }
            }
        }

        // Set window.location BEFORE running page scripts: `set_page_url`
        // re-registers the whole `window` global, so it must precede script
        // execution — otherwise any `window.*` properties a script sets
        // (window.onload handlers, framework globals, etc.) would be wiped.
        self.js_runtime.set_page_url(&url);
        // Build/replace the render document AND execute the page's `<script>`
        // tags (Phase 1 keystone).
        let viewport = current_viewport_override()
            .unwrap_or((self.config.viewport_width, self.config.viewport_height));
        // Load @font-face webfonts declared in inline <style> and stage them for
        // the document build (public-API path via DocumentConfig.font_ctx; no fork).
        let font_urls = crate::fonts::extract_font_face_urls(&html);
        if !font_urls.is_empty() {
            let base = Url::parse(&url).ok();
            let mut fonts = Vec::new();
            for furl in font_urls {
                let Some(full) = base.as_ref().and_then(|b| b.join(&furl).ok()) else {
                    continue;
                };
                if full.scheme() != "http" && full.scheme() != "https" {
                    continue;
                }
                match self.http_client.fetch_bytes(&full).await {
                    Ok(bytes) => fonts.push(bytes),
                    Err(e) => tracing::warn!(url = %full, error = %e, "@font-face fetch failed"),
                }
            }
            if !fonts.is_empty() {
                self.js_runtime.set_pending_fonts(fonts);
            }
        }
        if let Err(e) = self
            .js_runtime
            .set_document_with_scripts(&html, Some(&url), viewport, scripts)
            .await
        {
            tracing::warn!(error = %e, "failed to build render document; falling back");
        }
        // Derive the DomSnapshot from the (now-current) RenderDocument so every
        // reader — JS metadata bindings, CDP DOM/OXI, extract — reflects JS
        // mutations, not a stale navigate-time copy.
        let snapshot = match self.js_runtime.dom_snapshot(&url).await {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(error = %e, "failed to derive DOM snapshot");
                None
            }
        };
        tracing::debug!(
            node_count = snapshot.as_ref().map(|s| s.nodes.len()).unwrap_or(0),
            "DOM snapshot injected"
        );
        self.js_runtime.set_dom_snapshot(snapshot);
    }

    /// Build per-frame execution contexts for each child iframe (Phase 8).
    ///
    /// For each child `Frame` in the page's root frame, assigns a unique
    /// `context_id` (≥ 2), fetches external `<script src>` bodies, and sends a
    /// `SetFrameDocument` command to the JS thread which creates a dedicated
    /// `Context` + `RenderDocument` and runs the frame's scripts. The
    /// frame-id → context-id mapping is stored in `frame_contexts` for CDP
    /// routing (`Runtime.evaluate` with `contextId`).
    async fn inject_child_frames(&mut self) {
        let Some(page) = &self.active_page else {
            return;
        };
        // Collect child frame data upfront to avoid holding a borrow across await.
        let base_url = match self.current_url() {
            Some(u) => u.clone(),
            None => return,
        };
        let children: Vec<(String, String, Vec<crate::js::dom_snapshot::ScriptSource>)> = page
            .root_frame()
            .children()
            .iter()
            .map(|child| {
                (
                    child.id().to_string(),
                    child.url().to_string(),
                    child.extract_scripts(),
                )
            })
            .collect();
        let viewport = current_viewport_override()
            .unwrap_or((self.config.viewport_width, self.config.viewport_height));

        for (frame_id_str, url_str, mut scripts) in children {
            // Fetch external <script src> bodies for this child frame.
            if !scripts.is_empty() {
                let base = Url::parse(&url_str).ok().or_else(|| base_url.join("").ok());
                for s in scripts.iter_mut() {
                    let Some(src) = s.src_url.clone() else {
                        continue;
                    };
                    let Some(full_url) = base.as_ref().and_then(|b| b.join(&src).ok()) else {
                        continue;
                    };
                    let _in_flight = InFlightGuard::new(self.in_flight.clone());
                    match self.http_client.fetch_text(&full_url).await {
                        Ok(body) => s.source = body,
                        Err(e) => {
                            tracing::warn!(src = %src, error = %e, "failed to fetch child frame script")
                        }
                    }
                }
            }

            // Get the child frame's HTML.
            let Some(child_frame) = self.active_page.as_ref().and_then(|p| {
                p.root_frame()
                    .children()
                    .iter()
                    .find(|c| c.id().to_string() == frame_id_str)
            }) else {
                continue;
            };
            let html = child_frame.html().to_string();

            let context_id = self
                .next_context_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            if let Err(e) = self
                .js_runtime
                .set_frame_document(context_id, &html, &url_str, viewport, scripts)
                .await
            {
                tracing::warn!(frame_id = %frame_id_str, url = %url_str, error = %e, "failed to build child frame context");
                continue;
            }
            tracing::debug!(frame_id = %frame_id_str, context_id, url = %url_str, "child frame context built");
            self.frame_contexts.write().insert(frame_id_str, context_id);
        }
    }

    /// Evaluate a JS expression in a specific frame's execution context
    /// (Phase 8). `context_id` must correspond to a known frame context.
    pub async fn evaluate_js_in_context(
        &mut self,
        expression: &str,
        context_id: u32,
        await_promise: bool,
    ) -> Result<crate::js::runtime::JsEvalResult> {
        self.js_runtime
            .evaluate_in_context(expression, context_id, await_promise)
            .await
    }

    /// Return the frame-id → context-id map for CDP execution-context routing
    /// (Phase 8). Includes only child frames; the main frame is always
    /// context_id=1.
    pub fn frame_context_map(&self) -> &parking_lot::RwLock<HashMap<String, u32>> {
        &self.frame_contexts
    }

    /// Serialize the live (post-JS) document to a [`DomSnapshot`].
    ///
    /// For CDP DOM/OXI and `extract` readers — reflects JS mutations because it
    /// is derived from the `RenderDocument` on the JS thread.
    pub async fn dom_snapshot(&mut self) -> Result<Option<crate::js::dom_snapshot::DomSnapshot>> {
        let url = self
            .current_url()
            .map(|u| u.as_str().to_string())
            .unwrap_or_default();
        match self.js_runtime.dom_snapshot(&url).await {
            Ok(s) => Ok(Some(s)),
            Err(CoreError::ScreenshotError(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Wait for a CSS selector to match an element in the current page.
    ///
    /// Polls the active page's DOM every 50ms until the selector matches
    /// or the timeout is exceeded.
    pub async fn wait_for(&mut self, selector: &str, timeout_ms: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let duration = std::time::Duration::from_millis(timeout_ms);

        let expr = format!(
            "document.querySelector({}) !== null",
            serde_json::to_string(selector).unwrap_or_else(|_| "null".into())
        );
        loop {
            // Check the LIVE (post-JS) DOM. Each evaluate drains microtasks +
            // due timers, advancing the event loop so delayed renders surface.
            if let Ok(r) = self.evaluate_js(&expr).await
                && r.value == Some(serde_json::Value::Bool(true))
            {
                return Ok(());
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

    /// Get the parent browser ID.
    pub fn browser_id(&self) -> BrowserId {
        self.browser_id
    }

    /// Get the HTTP client.
    pub fn http_client(&self) -> Arc<HttpClient> {
        self.http_client.clone()
    }

    /// Snapshot of currently in-flight HTTP requests (navigates + JS fetches).
    ///
    /// Returns the count of dispatched requests whose response (or terminal
    /// error) has not yet been observed. `wait_for_condition(NetworkIdle)`
    /// polls this value via the Tab layer; it is also useful for tests and
    /// for surfacing load progress in higher layers. The counter is shared
    /// with the background fetch handler thread via `Arc<AtomicU64>` and
    /// updated under `Relaxed` ordering — fast to read, may briefly
    /// straddle a request start/complete.
    pub fn in_flight_requests(&self) -> u64 {
        self.in_flight.load(Ordering::Relaxed)
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
    pub fn set_local_storage(&self, key: impl Into<String>, value: impl Into<String>) {
        self.local_storage.write().insert(key.into(), value.into());
    }

    /// Get a local storage value.
    pub fn get_local_storage(&self, key: &str) -> Option<String> {
        self.local_storage.read().get(key).cloned()
    }

    /// Store a response body for later retrieval (Network.getResponseBody).
    pub fn store_response_body(&self, request_id: &str, body: String, content_type: &str) {
        let mut guard = self.response_bodies.write();
        guard.insert(
            request_id.to_string(),
            CapturedResponse {
                body,
                base64: false,
                content_type: content_type.to_string(),
            },
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
    #[tracing::instrument(skip(self), fields(session = %self.id), err)]
    pub async fn close(&mut self) -> Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        info!(id = %self.id, "session closed");
        self.active_page = None;
        self.history.clear();
        self.local_storage.write().clear();
        Ok(())
    }

    /// Whether the session has been closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Replace the active page and inject DOM snapshot (for testing).
    #[cfg(test)]
    pub async fn inject_dom_snapshot_for_test(&mut self, page: Page) {
        self.active_page = Some(page);
        self.inject_dom_snapshot().await;
    }

    /// Test-only: clone the in-flight counter's `Arc` so tests can
    /// simulate request starts/completions without driving a real
    /// `Session::navigate` / `handle_fetch_requests` round-trip.
    #[cfg(test)]
    pub fn in_flight_counter_handle_for_test(&self) -> Arc<AtomicU64> {
        self.in_flight.clone()
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
                crate::js::dom_snapshot::ResourceKind::Script => {
                    crate::network::resource::ResourceType::Script
                }
                crate::js::dom_snapshot::ResourceKind::Stylesheet => {
                    crate::network::resource::ResourceType::Stylesheet
                }
                crate::js::dom_snapshot::ResourceKind::Image => {
                    crate::network::resource::ResourceType::Image
                }
                crate::js::dom_snapshot::ResourceKind::Iframe => {
                    crate::network::resource::ResourceType::Document
                }
            };

            let _in_flight = InFlightGuard::new(self.in_flight.clone());
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

/// Extract the `filename` from a `Content-Disposition` header value.
/// Handles both `filename="name"` and `filename=name` (case-insensitive).
fn filename_from_disposition(disposition: &str) -> Option<String> {
    let lower = disposition.to_ascii_lowercase();
    let idx = lower.find("filename=")?;
    let rest = &disposition[idx + "filename=".len()..];
    let rest = rest.trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        // Quoted: take until closing quote.
        stripped.split('"').next().map(|s| s.to_string())
    } else {
        // Unquoted: take until ';' or end.
        rest.split(';').next().map(|s| s.trim().to_string())
    }
    .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::BrowserId;
    use crate::config::BrowserConfig;
    use crate::network::HttpClient;
    use crate::network::cookie::CookieJar;
    use crate::page::Page;

    /// Build a Session with SSRF disabled (so tests can reach loopback mocks)
    /// and the default (high) nav-script limits.
    async fn make_session() -> Session {
        let mut config = BrowserConfig::headless();
        config.enable_ssrf_filter = false;
        let cookie_jar = Arc::new(RwLock::new(CookieJar::new()));
        let http_client = Arc::new(HttpClient::new(&config, cookie_jar.clone()).unwrap());
        Session::new(BrowserId::next(), config, http_client, cookie_jar)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_inject_dom_snapshot_runs_inline_scripts() {
        // Phase 1 keystone end-to-end: a page's inline <script> must execute
        // during document injection, mutating the live DOM that a later
        // evaluate observes — exactly like headless Chrome.
        let mut session = make_session().await;
        let html = r#"<html><head></head><body>
            <div id="app">placeholder</div>
            <script>document.getElementById('app').textContent = 'rendered';</script>
            </body></html>"#;
        let url = Url::parse("https://test.local/").unwrap();
        let page = Page::from_html(url, html, 200, "text/html".into())
            .await
            .unwrap();
        session.inject_dom_snapshot_for_test(page).await;

        let r = session
            .evaluate_js("document.getElementById('app').textContent")
            .await
            .expect("evaluate");
        assert_eq!(
            r.value,
            Some(serde_json::json!("rendered")),
            "inline script ran during injection"
        );
    }

    #[tokio::test]
    async fn test_ready_state_is_complete_after_inject() {
        let mut session = make_session().await;
        let html = r#"<html><body><script>window.__any = 1;</script></body></html>"#;
        let url = Url::parse("https://test.local/").unwrap();
        let page = Page::from_html(url, html, 200, "text/html".into())
            .await
            .unwrap();
        session.inject_dom_snapshot_for_test(page).await;

        let r = session
            .evaluate_js("document.readyState")
            .await
            .expect("evaluate");
        assert_eq!(r.value, Some(serde_json::json!("complete")));
    }

    #[test]
    fn test_filename_from_disposition() {
        assert_eq!(
            filename_from_disposition("attachment; filename=\"report.pdf\""),
            Some("report.pdf".into())
        );
        assert_eq!(
            filename_from_disposition("attachment; filename=data.csv"),
            Some("data.csv".into())
        );
        assert_eq!(filename_from_disposition("inline"), None);
    }

    #[tokio::test]
    async fn test_navigate_to_attachment_downloads_file() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-disposition", "attachment; filename=\"hello.txt\"")
                    .set_body_string("downloaded-body"),
            )
            .mount(&server)
            .await;

        let dir = std::env::temp_dir().join(format!("oxi-dl-{}", uuid::Uuid::new_v4()));
        set_download_behavior(Some(dir.clone()));

        let mut session = make_session().await;
        let url = format!("{}/file", server.uri());
        session.navigate(&url).await.expect("navigate");

        let saved = dir.join("hello.txt");
        assert!(saved.exists(), "download file should exist at {saved:?}");
        assert_eq!(
            std::fs::read_to_string(&saved).unwrap(),
            "downloaded-body",
            "saved content should match the response body"
        );
        set_download_behavior(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_iframe_population() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    "<html><body><iframe src=\"/child.html\"></iframe></body></html>",
                ),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/child.html"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string("<html><body><p>inside-iframe</p></body></html>"),
            )
            .mount(&server)
            .await;

        let mut session = make_session().await;
        session
            .navigate(&format!("{}/", server.uri()))
            .await
            .expect("navigate");

        let children = session.page().expect("page").root_frame().children();
        assert_eq!(children.len(), 1, "iframe should populate one child frame");
        let has_text = children[0]
            .document()
            .nodes
            .values()
            .any(|n| n.text_content.contains("inside-iframe"));
        assert!(
            has_text,
            "child frame should contain the fetched iframe content"
        );
    }

    #[tokio::test]
    async fn test_viewport_override_applies_to_layout() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "<html><body><div style=\"width:100%;height:10px\"></div></body></html>",
            ))
            .mount(&server)
            .await;

        set_viewport_override(500, 400);
        let mut session = make_session().await;
        session
            .navigate(&format!("{}/", server.uri()))
            .await
            .expect("navigate");
        let png = session.capture_screenshot_png(500).await.expect("capture");
        clear_viewport_override();

        let img = image::load_from_memory(&png).expect("decode");
        // Full-page capture reflects the laid-out content width (≥ override width).
        assert!(
            img.width() >= 500,
            "viewport override should drive layout width, got {}",
            img.width()
        );
    }

    // multi_thread: the test blocks on a std mpsc recv_timeout while a
    // spawned task must run — a current-thread runtime would starve it.
    // Single test fn: the two scenarios share the process-wide FETCH_PATTERNS
    // static, so they MUST run serially (parallel tests would race on it).
    #[tokio::test(flavor = "multi_thread")]
    async fn test_maybe_intercept_js_fetch_interception() {
        use crate::js::CoreEvent;
        use crate::network::intercept::{InterceptAction, shared_registry};

        // --- Fulfill scenario: matching pattern pauses, decision resolves. ---
        set_fetch_patterns(vec!["http://example.com".to_string()]);
        let (tx, rx) = std::sync::mpsc::channel::<CoreEvent>();
        let event_tx = std::sync::Arc::new(parking_lot::RwLock::new(Some(tx)));

        let task_tx = event_tx.clone();
        let task = tokio::spawn(async move {
            maybe_intercept(&task_tx, 7, "http://example.com/api", "GET", &[]).await
        });

        // The bridge must emit a RequestPaused event carrying the pause id.
        let ev = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("RequestPaused event");
        let pause_id = match ev {
            CoreEvent::RequestPaused { request_id, .. } => request_id,
            other => panic!("expected RequestPaused, got {other:?}"),
        };

        // Resolve the paused request with a Fulfill.
        let paused = shared_registry()
            .take(&pause_id)
            .expect("paused request in registry");
        paused
            .tx
            .send(InterceptAction::Fulfill {
                status_code: 200,
                status_text: "OK".to_string(),
                headers: vec![],
                body: b"mock-body".to_vec(),
            })
            .unwrap();

        let decision = task.await.expect("task");
        match decision {
            InterceptDecision::Respond(msg) => {
                assert_eq!(msg.id, 7);
                assert_eq!(msg.status, 200);
                assert_eq!(msg.body, "mock-body");
            }
            other => panic!("expected Respond, got {other:?}"),
        }

        // --- Empty-pattern fast path: no pause, proceeds unchanged. ---
        set_fetch_patterns(vec![]);
        let decision = maybe_intercept(&event_tx, 3, "http://example.com/api", "GET", &[]).await;
        match decision {
            InterceptDecision::Proceed { url, .. } => {
                assert_eq!(url.as_str(), "http://example.com/api");
            }
            other => panic!("expected Proceed with empty patterns, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_iframe_creates_child_execution_context() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<html><body>
                        <h1 id="main-title">main-page</h1>
                        <iframe src="/child.html"></iframe>
                       </body></html>"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/child.html"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(
                        r#"<html><body>
                            <p id="iframe-text">inside-iframe-content</p>
                            <script>window.__iframeVar = 42;</script>
                           </body></html>"#,
                    ),
            )
            .mount(&server)
            .await;

        let mut session = make_session().await;
        session
            .navigate(&format!("{}/", server.uri()))
            .await
            .expect("navigate");

        // The frame_contexts map should contain one child entry.
        let frame_map = session.frame_context_map().read().clone();
        assert_eq!(
            frame_map.len(),
            1,
            "one child iframe should have a context: {frame_map:?}"
        );
        let (_child_frame_id, &child_context_id) = frame_map.iter().next().unwrap();
        assert!(
            child_context_id >= 2,
            "child context_id should be ≥ 2, got {child_context_id}"
        );

        // Evaluate in the child frame context: query the iframe's DOM.
        let r = session
            .evaluate_js_in_context(
                "document.getElementById('iframe-text').textContent",
                child_context_id,
                false,
            )
            .await
            .expect("evaluate in child context");
        assert_eq!(
            r.value,
            Some(serde_json::json!("inside-iframe-content")),
            "child context eval should return the iframe's DOM text"
        );

        // The iframe's script should have executed (window.__iframeVar = 42).
        let r2 = session
            .evaluate_js_in_context("window.__iframeVar", child_context_id, false)
            .await
            .expect("evaluate iframe var");
        assert_eq!(
            r2.value,
            Some(serde_json::json!(42)),
            "child frame scripts should execute in their own context"
        );

        // Main frame should NOT see the iframe's global (isolation).
        let r3 = session
            .evaluate_js("typeof window.__iframeVar")
            .await
            .expect("evaluate in main context");
        assert_eq!(
            r3.value,
            Some(serde_json::json!("undefined")),
            "main frame should be isolated from child frame globals"
        );
    }
}
