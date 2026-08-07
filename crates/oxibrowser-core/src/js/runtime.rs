#![allow(clippy::arc_with_non_send_sync)]
//! JavaScript runtime using boa_engine with a persistent context.
//!
//! boa_engine is a pure Rust JavaScript engine (ES2024+), no C dependencies.
//!
//! ## Architecture
//!
//! `boa_engine::Context` is `!Send` (internal GC pointers use `NonNull`).
//! To keep `JsRuntime: Send + Sync` for tokio, we run the `Context` on a
//! dedicated **std::thread** and communicate via `mpsc` channels.
//!
//! ```text
//! main thread (async)          JS thread (sync, std::thread)
//! ┌─────────────────┐          ┌──────────────────┐
//! │ JsRuntime        │──send──→│ Context (영구)    │
//! │  evaluate()     │          │  console.log 등록 │
//! │  set_global()   │          │  eval(script)     │
//! │  set_dom()      │          │  document 객체    │
//! │  console_output  │←─recv──│  json_value 반환  │
//! └─────────────────┘          └──────────────────┘
//! ```
//!
//! This means JS state (variables, functions, closures) **persists across
//! evaluate() calls** — exactly like a real browser.

use parking_lot::RwLock;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use base64::Engine;
use boa_engine::object::builtins::JsArray;
use boa_engine::object::{FunctionObjectBuilder, JsObject};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsString, JsValue, NativeFunction, Source, js_string};
use serde_json::Value;

use crate::css::LayoutEngine;
use crate::error::{CoreError, Result};
use crate::js::dom_snapshot::{DomMutation, DomNode, DomSnapshot};
use crate::js::job_queue::TokioJobQueue;
use crate::network::cookie::CookieJar;
use oxibrowser_render::{CaptureOpts, RenderDocument, Viewport};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Global counter for unique node IDs, avoids collisions in tight loops.
/// Starts at 1_000_000 to stay above any parsed DOM snapshot IDs.
static NEXT_NODE_ID: AtomicU64 = AtomicU64::new(1_000_000);

// ── Thread-local listener registry ─────────────────────────────────────────
// Event listeners keyed by node_id → event_type → callbacks.
// Thread-local because boa `Context` is !Send — every closure runs on the
// same JS thread. This lets the bubbling walk find listeners registered via
// any element object, regardless of object identity (each DOM query mints a
// fresh JS object, so `__listeners` on one instance is invisible to another).
thread_local! {
    static LISTENER_REGISTRY: RefCell<HashMap<u32, HashMap<String, Vec<JsObject>>>> =
        RefCell::new(HashMap::new());
}

/// Register a listener callback for a node in the thread-local registry.
fn registry_add(node_id: u32, event_type: &str, callback: JsObject) {
    LISTENER_REGISTRY.with(|r| {
        r.borrow_mut()
            .entry(node_id)
            .or_default()
            .entry(event_type.to_string())
            .or_default()
            .push(callback);
    });
}

/// Get all callbacks for a node + event type (cloned out to release the borrow
/// before calling them — callbacks may themselves call addEventListener).
fn registry_get(node_id: u32, event_type: &str) -> Vec<JsObject> {
    LISTENER_REGISTRY.with(|r| {
        r.borrow()
            .get(&node_id)
            .and_then(|m| m.get(event_type))
            .cloned()
            .unwrap_or_default()
    })
}

/// Remove all callbacks for a node + event type.
fn registry_remove(node_id: u32, event_type: &str) {
    LISTENER_REGISTRY.with(|r| {
        if let Some(m) = r.borrow_mut().get_mut(&node_id) {
            m.remove(event_type);
        }
    });
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of a JavaScript evaluation.
#[derive(Debug, Clone)]
pub struct JsEvalResult {
    /// The return value as a JSON value (if any).
    pub value: Option<Value>,
    /// Exception message (if an error occurred).
    pub exception: Option<String>,
    /// Console output captured during execution.
    pub console_output: Vec<String>,
    /// Whether the evaluation was aborted due to a timeout.
    /// When true, the JS context was reset and previous state (variables, etc.) is lost.
    pub timed_out: bool,
}

impl JsEvalResult {
    /// Create a successful result with a value.
    pub fn ok(value: Value) -> Self {
        Self {
            value: Some(value),
            exception: None,
            console_output: Vec::new(),
            timed_out: false,
        }
    }

    /// Create a result with no return value (void/undefined).
    pub fn void() -> Self {
        Self {
            value: None,
            exception: None,
            console_output: Vec::new(),
            timed_out: false,
        }
    }

    /// Create an error result.
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            value: None,
            exception: Some(msg.into()),
            console_output: Vec::new(),
            timed_out: false,
        }
    }

    /// Create a timeout result (context was reset).
    pub fn timeout(timeout_ms: u64) -> Self {
        Self {
            value: None,
            exception: Some(format!(
                "JS execution timed out after {timeout_ms}ms — context was reset, previous state lost"
            )),
            console_output: Vec::new(),
            timed_out: true,
        }
    }

    /// Whether the evaluation succeeded (no exception).
    pub fn is_ok(&self) -> bool {
        self.exception.is_none()
    }
}

// ---------------------------------------------------------------------------
// Command / Response types (channel messages)
// ---------------------------------------------------------------------------
/// Serializable info about a node in the [`RenderDocument`], returned by the
/// async query façades. `id` is the opaque `NodeId` valid on the JS thread.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NodeInfo {
    /// Opaque node id (valid only on the JS thread's `RenderDocument`).
    pub id: usize,
    /// Lowercased tag name, or `None` for non-element nodes.
    pub tag: Option<String>,
    /// Recursive text content of the node.
    pub text: String,
    /// `(name, value)` attribute pairs (empty for non-elements / text nodes).
    pub attributes: Vec<(String, String)>,
}


/// Commands sent from the async main thread to the JS thread.
enum JsCommand {
    /// Evaluate a JS expression.
    Eval {
        expression: String,
        timeout_ms: Option<u64>,
        max_loop_iterations: Option<u64>,
        max_recursion: Option<usize>,
        max_stack_size: Option<usize>,
        await_promise: bool,
        response_tx: Sender<JsResponse>,
    },
    /// Set a global variable in the persistent Context.
    SetGlobal {
        name: String,
        value: Value,
        response_tx: Sender<JsResponse>,
    },
    /// Update the DOM snapshot available to `document` object.
    SetDom {
        snapshot: Box<Option<DomSnapshot>>,
        response_tx: Sender<JsResponse>,
    },
    /// Update the page URL (for window.location).
    SetPageUrl {
        url: String,
        response_tx: Sender<JsResponse>,
    },
    /// Set the fetch channel so JS can make real HTTP requests.
    SetFetchChannel {
        tx: std::sync::mpsc::Sender<FetchRequestMsg>,
        response_tx: Sender<JsResponse>,
    },
    /// Set the localStorage sync channel so JS operations propagate to Session.
    SetLocalStorageChannel {
        tx: std::sync::mpsc::Sender<LocalStorageMsg>,
        response_tx: Sender<JsResponse>,
    },
    /// Set the CookieJar so document.cookie can read/write real cookies.
    SetCookieJar {
        jar: Arc<RwLock<CookieJar>>,
        response_tx: Sender<JsResponse>,
    },
    /// Build (or replace) the `RenderDocument` on the JS thread from HTML.
    SetDocument {
        html: String,
        base_url: Option<String>,
        viewport: (u32, u32),
        response_tx: Sender<JsResponse>,
    },
    /// Capture a PNG of the current `RenderDocument`.
    Capture {
        opts: CaptureOpts,
        response_tx: Sender<JsResponse>,
    },
    /// Query all nodes matching a CSS selector against the `RenderDocument`.
    Query {
        selector: String,
        response_tx: Sender<JsResponse>,
    },
    /// Shut down the JS thread.
    Shutdown,
}

/// Responses sent from the JS thread back to the main thread.
enum JsResponse {
    /// Result of an Eval command.
    EvalResult {
        value: Option<Value>,
        exception: Option<String>,
        console_output: Vec<String>,
        timed_out: bool,
    },
    /// Ack for SetGlobal / SetDom / Shutdown.
    Done,
    /// PNG bytes returned by a `Capture` command.
    CaptureResult { png: Vec<u8> },
    /// Nodes returned by a `Query` command.
    QueryResult { nodes: Vec<NodeInfo> },
    /// Error from a `SetDocument` / `Capture` / `Query` command.
    Error { message: String },
}

// ---------------------------------------------------------------------------
// Fetch message types
// ---------------------------------------------------------------------------

/// A fetch request from JS.
pub struct FetchRequestMsg {
    /// URL to fetch.
    pub url: String,
    /// HTTP method.
    pub method: String,
    /// Request headers (name, value pairs).
    pub headers: Vec<(String, String)>,
    /// Request body (if any).
    pub body: Option<String>,
    /// Channel to send the response back.
    pub response_tx: std::sync::mpsc::Sender<FetchResponseMsg>,
}

/// HTTP response sent back to the JS thread.
pub struct FetchResponseMsg {
    /// HTTP status code.
    pub status: u16,
    /// HTTP status text.
    pub status_text: String,
    /// Final URL (after redirects).
    pub url: String,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    /// Response body text.
    pub body: String,
    /// Error message if request failed.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// LocalStorage sync messages
// ---------------------------------------------------------------------------

/// Messages sent from JS localStorage operations to Session for sync.
#[derive(Debug)]
pub enum LocalStorageMsg {
    /// localStorage.setItem(key, value)
    SetItem(String, String),
    /// localStorage.removeItem(key)
    RemoveItem(String),
    /// localStorage.clear()
    Clear,
}

// ---------------------------------------------------------------------------
// JsRuntime
// ---------------------------------------------------------------------------

/// Configuration for JS runtime limits and timeouts.
#[derive(Debug, Clone)]
pub struct JsRuntimeConfig {
    /// Default timeout in ms for each evaluate() call.
    pub timeout_ms: u64,
    /// Max recursion depth.
    pub max_recursion: usize,
    /// Max loop iterations.
    pub max_loop_iterations: u64,
    /// Max operand stack size.
    pub max_stack_size: usize,
    /// Viewport width (pixels, 0 = headless).
    pub viewport_width: u32,
    /// Viewport height (pixels, 0 = headless).
    pub viewport_height: u32,
    /// User-Agent exposed to JS (navigator.userAgent). Drives the stealth
    /// fingerprint profile — must match the UA sent over the wire.
    pub user_agent: String,
}

impl Default for JsRuntimeConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 5000,
            max_recursion: 100,
            max_loop_iterations: 100_000,
            max_stack_size: 1024,
            viewport_width: 1280,
            viewport_height: 720,
            user_agent: "Mozilla/5.0 (OxiBrowser/0.1.0; +https://github.com/oxios/oxibrowser)"
                .to_string(),
        }
    }
}

impl From<&crate::config::BrowserConfig> for JsRuntimeConfig {
    fn from(config: &crate::config::BrowserConfig) -> Self {
        Self {
            timeout_ms: config.js_timeout_ms,
            max_recursion: config.js_max_recursion,
            max_loop_iterations: config.js_max_loop_iterations,
            max_stack_size: config.js_max_stack_size,
            viewport_width: config.viewport_width,
            viewport_height: config.viewport_height,
            user_agent: config.user_agent.clone(),
        }
    }
}

/// A JavaScript runtime backed by boa_engine with a persistent context.
///
/// The `boa_engine::Context` lives on a dedicated OS thread and persists
/// across `evaluate()` calls, so JS variables, functions, and closures
/// survive between invocations.
///
/// Thread-safe: `Send + Sync` via channel communication.
pub struct JsRuntime {
    /// Channel to send commands to the JS thread.
    cmd_tx: Sender<JsCommand>,
    /// Shared console output buffer (also shared with JS thread closures).
    console_output: Arc<RwLock<Vec<String>>>,
    /// Shared mutation buffer — JS thread pushes, main thread drains.
    mutations: Arc<RwLock<Vec<DomMutation>>>,
    /// Global variables tracked on the Rust side.
    globals: RwLock<HashMap<String, Value>>,
    /// Runtime configuration (limits, timeouts).
    config: JsRuntimeConfig,
    /// Channel to send fetch requests (set via set_fetch_channel()).
    fetch_tx: Option<std::sync::mpsc::Sender<FetchRequestMsg>>,
}

impl JsRuntime {
    /// Create a new JS runtime with default configuration.
    pub fn new() -> Self {
        Self::with_config(JsRuntimeConfig::default())
    }

    /// Create a new JS runtime with the given configuration.
    pub fn with_config(config: JsRuntimeConfig) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<JsCommand>();
        let console_output = Arc::new(RwLock::new(Vec::<String>::new()));
        let mutations = Arc::new(RwLock::new(Vec::<DomMutation>::new()));

        // Spawn JS thread
        let console_output_clone = console_output.clone();
        let mutations_clone = mutations.clone();
        let viewport = (config.viewport_width, config.viewport_height);
        let user_agent = config.user_agent.clone();
        let _local_storage = Arc::new(RwLock::new(HashMap::<String, String>::new()));
        std::thread::Builder::new()
            .name("oxibrowser-js".into())
            .spawn(move || {
                js_thread_loop(
                    cmd_rx,
                    console_output_clone,
                    mutations_clone,
                    viewport,
                    None,
                    user_agent,
                );
            })
            .expect("failed to spawn JS thread");

        Self {
            cmd_tx,
            console_output,
            mutations,
            globals: RwLock::new(HashMap::new()),
            config,
            fetch_tx: None,
        }
    }

    /// Set the channel for fetch requests. Must be called before JS can use fetch().
    pub fn set_fetch_channel(&mut self, tx: std::sync::mpsc::Sender<FetchRequestMsg>) {
        self.fetch_tx = Some(tx.clone());
        let (response_tx, response_rx) = mpsc::channel::<JsResponse>();
        if let Err(e) = self
            .cmd_tx
            .send(JsCommand::SetFetchChannel { tx, response_tx })
        {
            tracing::error!(error = %e, "failed to send SetFetchChannel: JS thread has died");
            return;
        }
        let _ = response_rx.recv();
    }

    /// Set the channel for localStorage sync.
    pub fn set_local_storage_channel(&mut self, tx: std::sync::mpsc::Sender<LocalStorageMsg>) {
        let (response_tx, response_rx) = mpsc::channel::<JsResponse>();
        if let Err(e) = self
            .cmd_tx
            .send(JsCommand::SetLocalStorageChannel { tx, response_tx })
        {
            tracing::error!(error = %e, "failed to send SetLocalStorageChannel: JS thread has died");
            return;
        }
        let _ = response_rx.recv();
    }

    /// Evaluate a JavaScript expression and return the result.
    pub async fn evaluate(&mut self, expression: &str) -> Result<JsEvalResult> {
        self.evaluate_with_timeout(expression, None).await
    }

    /// Evaluate a JavaScript expression, optionally awaiting Promise resolution.
    pub async fn evaluate_with_await(
        &mut self,
        expression: &str,
        await_promise: bool,
    ) -> Result<JsEvalResult> {
        self.evaluate_with_timeout_and_await(expression, None, await_promise)
            .await
    }

    /// Evaluate a JavaScript expression with an explicit timeout override.
    pub async fn evaluate_with_timeout(
        &mut self,
        expression: &str,
        timeout_ms: Option<u64>,
    ) -> Result<JsEvalResult> {
        self.evaluate_with_timeout_and_await(expression, timeout_ms, false)
            .await
    }

    /// Evaluate a JavaScript expression with timeout and optional Promise awaiting.
    pub async fn evaluate_with_timeout_and_await(
        &mut self,
        expression: &str,
        timeout_ms: Option<u64>,
        await_promise: bool,
    ) -> Result<JsEvalResult> {
        self.console_output.write().clear();
        tracing::debug!(
            expr_len = expression.len(),
            timeout_ms = timeout_ms.unwrap_or(self.config.timeout_ms),
            "JS evaluation started"
        );
        let (response_tx, response_rx) = mpsc::channel::<JsResponse>();
        self.cmd_tx
            .send(JsCommand::Eval {
                expression: expression.to_string(),
                timeout_ms: Some(timeout_ms.unwrap_or(self.config.timeout_ms)),
                max_loop_iterations: Some(self.config.max_loop_iterations),
                max_recursion: Some(self.config.max_recursion),
                max_stack_size: Some(self.config.max_stack_size),
                await_promise,
                response_tx,
            })
            .map_err(|_| CoreError::JsError("JS thread has died".into()))?;
        let resp = response_rx
            .recv()
            .map_err(|_| CoreError::JsError("JS thread has died".into()))?;
        match resp {
            JsResponse::EvalResult {
                value,
                exception,
                console_output,
                timed_out,
            } => {
                if timed_out {
                    tracing::warn!(
                        timeout_ms = timeout_ms.unwrap_or(self.config.timeout_ms),
                        "JS evaluation timed out — context reset"
                    );
                    return Err(CoreError::JsTimeout(
                        timeout_ms.unwrap_or(self.config.timeout_ms),
                    ));
                }
                Ok(JsEvalResult {
                    value,
                    exception,
                    console_output,
                    timed_out: false,
                })
            }
            _ => Err(CoreError::JsError(
                "unexpected response from JS thread".into(),
            )),
        }
    }

    /// Evaluate a script (multiple statements, no return value needed).
    pub async fn execute(&mut self, script: &str) -> Result<JsEvalResult> {
        self.evaluate(script).await
    }

    /// Get captured console output from the last eval.
    pub fn console_output(&self) -> Vec<String> {
        self.console_output.read().clone()
    }

    /// Clear captured console output.
    pub fn clear_console(&mut self) {
        self.console_output.write().clear();
    }

    /// Drain all pending DOM mutations collected by JS execution.
    pub fn drain_mutations(&self) -> Vec<DomMutation> {
        let mut guard = self.mutations.write();
        std::mem::take(&mut *guard)
    }

    /// Set a global variable — injected into the persistent JS Context.
    pub fn set_global(&mut self, name: impl Into<String>, value: Value) {
        let name = name.into();
        self.globals.write().insert(name.clone(), value.clone());
        let (response_tx, response_rx) = mpsc::channel::<JsResponse>();
        if let Err(e) = self.cmd_tx.send(JsCommand::SetGlobal {
            name,
            value,
            response_tx,
        }) {
            tracing::error!(error = %e, "failed to send SetGlobal: JS thread has died");
            return;
        }
        let _ = response_rx.recv();
    }

    /// Set the DOM snapshot (called after navigate).
    pub fn set_dom_snapshot(&mut self, snapshot: Option<DomSnapshot>) {
        self.mutations.write().clear();
        let (response_tx, response_rx) = mpsc::channel::<JsResponse>();
        if let Err(e) = self.cmd_tx.send(JsCommand::SetDom {
            snapshot: Box::new(snapshot),
            response_tx,
        }) {
            tracing::error!(error = %e, "failed to send SetDom: JS thread has died");
            return;
        }
        let _ = response_rx.recv();
    }

    /// Set the CookieJar so document.cookie reads/writes real cookies.
    pub fn set_cookie_jar(&mut self, jar: Arc<RwLock<CookieJar>>) -> Result<()> {
        let (response_tx, response_rx) = mpsc::channel::<JsResponse>();
        self.cmd_tx
            .send(JsCommand::SetCookieJar { jar, response_tx })
            .map_err(|_| CoreError::JsError("JS thread has died".into()))?;
        let resp = response_rx
            .recv()
            .map_err(|_| CoreError::JsError("JS thread has died".into()))?;
        match resp {
            JsResponse::Done => Ok(()),
            _ => Err(CoreError::JsError("unexpected response".into())),
        }
    }

    /// Update the page URL (used for window.location).
    pub fn set_page_url(&mut self, url: &str) {
        let (response_tx, response_rx) = mpsc::channel::<JsResponse>();
        if let Err(e) = self.cmd_tx.send(JsCommand::SetPageUrl {
            url: url.to_string(),
            response_tx,
        }) {
            tracing::error!(error = %e, "failed to send SetPageUrl: JS thread has died");
            return;
        }
        let _ = response_rx.recv();
    }

    // ── Render façades (ship a command to the JS thread, await a response) ────
    //
    // These reach the `!Send` `RenderDocument` that lives on the JS thread —
    // the single source of truth for the DOM after unification. The JS thread
    // builds/captures/queries it synchronously between JS ticks.

    /// Build (or replace) the renderable document on the JS thread from HTML.
    ///
    /// The `RenderDocument` is constructed on the JS thread (it is `!Send`),
    /// then mutated by JS bindings and captured/queried via the façades below.
    pub async fn set_document(
        &mut self,
        html: &str,
        base_url: Option<&str>,
        viewport: (u32, u32),
    ) -> Result<()> {
        let (response_tx, response_rx) = mpsc::channel::<JsResponse>();
        self.cmd_tx
            .send(JsCommand::SetDocument {
                html: html.to_string(),
                base_url: base_url.map(|s| s.to_string()),
                viewport,
                response_tx,
            })
            .map_err(|_| CoreError::JsError("JS thread has died".into()))?;
        let resp = response_rx
            .recv()
            .map_err(|_| CoreError::JsError("JS thread has died".into()))?;
        match resp {
            JsResponse::Done => Ok(()),
            JsResponse::Error { message } => {
                Err(CoreError::ScreenshotError(message))
            }
            _ => Err(CoreError::JsError("unexpected response".into())),
        }
    }

    /// Capture a full-page PNG screenshot of the current render document.
    ///
    /// The render runs synchronously on the JS thread, so the captured frame
    /// is a consistent snapshot (no half-applied JS mutations).
    pub async fn capture_png(&mut self, opts: CaptureOpts) -> Result<Vec<u8>> {
        let (response_tx, response_rx) = mpsc::channel::<JsResponse>();
        self.cmd_tx
            .send(JsCommand::Capture { opts, response_tx })
            .map_err(|_| CoreError::JsError("JS thread has died".into()))?;
        let resp = response_rx
            .recv()
            .map_err(|_| CoreError::JsError("JS thread has died".into()))?;
        match resp {
            JsResponse::CaptureResult { png } => Ok(png),
            JsResponse::Error { message } => Err(CoreError::ScreenshotError(message)),
            _ => Err(CoreError::JsError("unexpected response".into())),
        }
    }

    /// Query all nodes matching a CSS selector against the render document.
    ///
    /// Returns serializable [`NodeInfo`] (the async side never touches the
    /// `!Send` `RenderDocument` directly).
    pub async fn query_selector_all(&mut self, selector: &str) -> Result<Vec<NodeInfo>> {
        let (response_tx, response_rx) = mpsc::channel::<JsResponse>();
        self.cmd_tx
            .send(JsCommand::Query {
                selector: selector.to_string(),
                response_tx,
            })
            .map_err(|_| CoreError::JsError("JS thread has died".into()))?;
        let resp = response_rx
            .recv()
            .map_err(|_| CoreError::JsError("JS thread has died".into()))?;
        match resp {
            JsResponse::QueryResult { nodes } => Ok(nodes),
            JsResponse::Error { message } => Err(CoreError::ScreenshotError(message)),
            _ => Err(CoreError::JsError("unexpected response".into())),
        }
    }
}

impl Default for JsRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for JsRuntime {
    fn drop(&mut self) {
        // Signal the JS thread to shut down — no response needed for Shutdown
        let _ = self.cmd_tx.send(JsCommand::Shutdown);
    }
}

// ---------------------------------------------------------------------------
// JS thread
// ---------------------------------------------------------------------------

/// Main loop for the JS thread.
///
/// Creates a single `Context`, registers globals, and processes commands
/// until a `Shutdown` is received.
fn js_thread_loop(
    cmd_rx: Receiver<JsCommand>,
    console_output: Arc<RwLock<Vec<String>>>,
    mutations: Arc<RwLock<Vec<DomMutation>>>,
    viewport: (u32, u32),
    _fetch_tx: Option<std::sync::mpsc::Sender<FetchRequestMsg>>,
    user_agent: String,
) {
    let fetch_tx_arc: Arc<RwLock<Option<std::sync::mpsc::Sender<FetchRequestMsg>>>> =
        Arc::new(RwLock::new(None));
    let local_storage_tx_arc: Arc<RwLock<Option<std::sync::mpsc::Sender<LocalStorageMsg>>>> =
        Arc::new(RwLock::new(None));
    let cookie_jar_arc: Arc<RwLock<Option<Arc<RwLock<CookieJar>>>>> = Arc::new(RwLock::new(None));
    let dom_snapshot: Arc<RwLock<Option<DomSnapshot>>> = Arc::new(RwLock::new(None));
    let (mut ctx, mut job_queue) = create_context(
        &console_output,
        &dom_snapshot,
        &mutations,
        viewport,
        "",
        &user_agent,
        &fetch_tx_arc,
        &cookie_jar_arc,
    );

    // The Blitz-backed render document. `BaseDocument` is effectively `!Send`,
    // so this lives here on the JS thread (co-located with boa's `Context`),
    // mirroring a real browser's main thread. Set via `SetDocument`, mutated by
    // JS bindings (Task 2), captured/queried via `Capture`/`Query`.
    let mut render_doc: Option<RenderDocument> = None;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            JsCommand::Eval {
                expression,
                timeout_ms,
                max_loop_iterations,
                max_recursion,
                max_stack_size,
                await_promise,
                response_tx,
            } => {
                // Clear console buffer before eval
                console_output.write().clear();
                // Clear mutation buffer before eval
                mutations.write().clear();

                // Apply runtime limits to context
                let loop_limit = max_loop_iterations.unwrap_or(100_000);
                let recursion_limit = max_recursion.unwrap_or(100);
                let stack_limit = max_stack_size.unwrap_or(1024);

                {
                    let limits = ctx.runtime_limits_mut();
                    limits.set_loop_iteration_limit(loop_limit);
                    limits.set_recursion_limit(recursion_limit);
                    limits.set_stack_size_limit(stack_limit);
                }

                let timeout = timeout_ms.unwrap_or(5000);
                let start = std::time::Instant::now();

                let source = Source::from_bytes(&expression);
                let result = ctx.eval(source);

                // Drain Promise microtasks queued during eval
                ctx.run_jobs();

                // Drain due timers and fire their callbacks
                drain_timers(&job_queue, &mut ctx);

                let elapsed = start.elapsed();
                let console = console_output.read().clone();

                // Check if we timed out
                if elapsed.as_millis() > timeout as u128 {
                    // Context may be in a bad state — recreate it
                    let (new_ctx, new_queue) = create_context(
                        &console_output,
                        &dom_snapshot,
                        &mutations,
                        viewport,
                        "",
                        &user_agent,
                        &fetch_tx_arc,
                        &cookie_jar_arc,
                    );
                    ctx = new_ctx;
                    job_queue = new_queue;
                    let _ = response_tx.send(JsResponse::EvalResult {
                        value: None,
                        exception: Some(format!(
                            "JS execution timed out after {}ms — context was reset, previous state lost",
                            elapsed.as_millis()
                        )),
                        console_output: console,
                        timed_out: true,
                    });
                    continue;
                }

                match result {
                    Ok(value) => {
                        // If awaitPromise is requested, check if the result is a Promise
                        // and drain microtasks until it resolves.
                        let final_value = if await_promise {
                            await_promise_value(value, &mut ctx, &job_queue)
                        } else {
                            value
                        };
                        let json_value = js_value_to_json(&final_value, &mut ctx);
                        let _ = response_tx.send(JsResponse::EvalResult {
                            value: Some(json_value),
                            exception: None,
                            console_output: console,
                            timed_out: false,
                        });
                    }
                    Err(err) => {
                        let msg = format_js_error(&err, &mut ctx);
                        // Check if it was a runtime limit error (loop/recursion/stack)
                        let is_runtime_limit = msg.contains("Maximum loop iteration limit")
                            || msg.contains("exceeded the maximum call stack size")
                            || msg.contains("recursion limit");

                        if is_runtime_limit {
                            // Runtime limit hit — context is still valid,
                            // but the partial execution may have left state.
                            // We don't reset the context here since boa throws
                            // a catchable error (the context is fine).
                        }

                        let _ = response_tx.send(JsResponse::EvalResult {
                            value: None,
                            exception: Some(msg),
                            console_output: console,
                            timed_out: false,
                        });
                    }
                }
            }
            JsCommand::SetGlobal {
                name,
                value,
                response_tx,
            } => {
                let js_val = json_to_js_value(&value, &mut ctx);
                let _ = ctx.register_global_property(
                    JsString::from(name.as_str()),
                    js_val,
                    Attribute::all(),
                );
                let _ = response_tx.send(JsResponse::Done);
            }
            JsCommand::SetDom {
                snapshot,
                response_tx,
            } => {
                *dom_snapshot.write() = *snapshot;
                // Update document title/URL in the JS context
                let snap = dom_snapshot.read();
                if let Some(ref s) = *snap {
                    let _ = ctx.register_global_property(
                        js_string!("__domTitle"),
                        JsValue::from(JsString::from(s.title.as_str())),
                        Attribute::all(),
                    );
                    let _ = ctx.register_global_property(
                        js_string!("__domUrl"),
                        JsValue::from(JsString::from(s.url.as_str())),
                        Attribute::all(),
                    );
                }
                let _ = response_tx.send(JsResponse::Done);
            }
            JsCommand::SetPageUrl { url, response_tx } => {
                // Re-register window.location with the new URL
                let snap = dom_snapshot.read();
                let dom_snapshot_ref = dom_snapshot.clone();
                drop(snap);
                register_window_globals(
                    &mut ctx,
                    &dom_snapshot_ref,
                    &mutations,
                    viewport,
                    &url,
                    &user_agent,
                    &fetch_tx_arc,
                );
                // Preserve localStorage across URL changes.
                // TODO(#sop): Check same-origin before preserving localStorage.
                // Currently preserves across all navigations, including cross-origin.
                // In a production browser, localStorage should be scoped per-origin.
                //
                // Only re-register localStorage if it hasn't been registered yet;
                // otherwise the existing JS-side storage object persists across navigations
                // (same-origin policy would be checked in a full implementation).
                // Previously this always re-registered with an empty HashMap, wiping storage.
                let existing_ls = ctx
                    .global_object()
                    .get(js_string!("localStorage"), &mut ctx)
                    .ok();
                if existing_ls
                    .as_ref()
                    .is_none_or(|v| v.is_undefined() || v.is_null())
                {
                    // First time — register fresh
                    let empty = std::collections::HashMap::new();
                    register_local_storage(
                        &mut ctx,
                        empty,
                        &dom_snapshot_ref,
                        local_storage_tx_arc.clone(),
                    );
                }
                // else: localStorage already exists, preserve it across navigation
                let _ = response_tx.send(JsResponse::Done);
            }
            JsCommand::SetLocalStorageChannel { tx, response_tx } => {
                *local_storage_tx_arc.write() = Some(tx);
                let _ = response_tx.send(JsResponse::Done);
            }
            JsCommand::SetFetchChannel { tx, response_tx } => {
                *fetch_tx_arc.write() = Some(tx);
                let _ = response_tx.send(JsResponse::Done);
            }
            JsCommand::SetCookieJar { jar, response_tx } => {
                *cookie_jar_arc.write() = Some(jar);
                let _ = response_tx.send(JsResponse::Done);
            }
            JsCommand::SetDocument {
                html,
                base_url,
                viewport,
                response_tx,
            } => {
                let vp = Viewport {
                    width: viewport.0.max(64),
                    height: viewport.1.max(64),
                    scale: 1.0,
                };
                match RenderDocument::from_html(&html, base_url.as_deref(), vp) {
                    Ok(doc) => {
                        render_doc = Some(doc);
                        let _ = response_tx.send(JsResponse::Done);
                    }
                    Err(e) => {
                        let _ = response_tx.send(JsResponse::Error {
                            message: e.to_string(),
                        });
                    }
                }
            }
            JsCommand::Capture { opts, response_tx } => match render_doc.as_mut() {
                Some(doc) => match doc.capture_png(&opts) {
                    Ok(png) => {
                        let _ = response_tx.send(JsResponse::CaptureResult { png });
                    }
                    Err(e) => {
                        let _ = response_tx.send(JsResponse::Error {
                            message: e.to_string(),
                        });
                    }
                },
                None => {
                    let _ = response_tx.send(JsResponse::Error {
                        message: "no render document set".into(),
                    });
                }
            },
            JsCommand::Query {
                selector,
                response_tx,
            } => {
                let nodes = match render_doc.as_ref() {
                    Some(doc) => doc
                        .query_selector_all(&selector)
                        .into_iter()
                        .map(|id| NodeInfo {
                            id,
                            tag: doc.tag_name(id),
                            text: doc.node_text(id),
                            attributes: doc.node_attributes(id),
                        })
                        .collect(),
                    None => Vec::new(),
                };
                let _ = response_tx.send(JsResponse::QueryResult { nodes });
            }
            JsCommand::Shutdown => {
                break;
            }
        }
    }
}

// Create a fresh boa_engine Context with console.log/warn/error/info
// and `document` object registered.
// ---------------------------------------------------------------------------
// Timer drain
// ---------------------------------------------------------------------------

/// Drain all due timers from the job queue and execute their callbacks.
///
/// For interval timers, the callback is re-scheduled with the original interval.
/// After each batch of timer callbacks, we also drain any microtasks they
/// enqueued. Repeats until no more due timers remain (up to a safety limit).
/// When `Runtime.evaluate` is called with `awaitPromise: true`, this function:
/// 1. Checks if the value is a Promise (has a `.then` method)
/// 2. Attaches `.then()`/`.catch()` handlers to capture the settled value
/// 3. Drains the microtask queue repeatedly until the Promise settles
/// 4. Returns the resolved value (or the rejection error as a string)
///
/// If the value is not a Promise, returns it unchanged.
fn await_promise_value(
    value: JsValue,
    ctx: &mut Context,
    job_queue: &Rc<TokioJobQueue>,
) -> JsValue {
    // Check if the value is thenable (has a .then method)
    let is_thenable = value.as_object().is_some_and(|obj| {
        obj.get(js_string!("then"), ctx)
            .ok()
            .and_then(|v| v.as_object().map(|o| o.is_callable()))
            .unwrap_or(false)
    });

    if !is_thenable {
        return value;
    }

    // Set up __promiseResult / __promiseSettled globals
    // then attach .then() and .catch() handlers
    let setup_result = ctx.eval(Source::from_bytes(
        "globalThis.__promiseResult = undefined; globalThis.__promiseSettled = false; globalThis.__promiseError = null;"
    ));
    if setup_result.is_err() {
        return value; // fallback: return the Promise object
    }

    // Store the promise and attach handlers via eval
    let _ = ctx.register_global_property(
        js_string!("__pendingPromise"),
        value.clone(),
        boa_engine::property::Attribute::all(),
    );

    let handler_code = r#"
        (function() {
            var p = globalThis.__pendingPromise;
            p.then(
                function(v) { globalThis.__promiseResult = v; globalThis.__promiseSettled = true; },
                function(e) { globalThis.__promiseError = e instanceof Error ? e.message : String(e); globalThis.__promiseSettled = true; }
            );
        })()
    "#;
    let _ = ctx.eval(Source::from_bytes(handler_code));

    // Drain microtasks repeatedly until the Promise settles
    // (up to 50 iterations to prevent infinite loops)
    for _ in 0..50 {
        ctx.run_jobs();
        drain_timers(job_queue, ctx);

        let settled = ctx
            .global_object()
            .get(js_string!("__promiseSettled"), ctx)
            .ok()
            .and_then(|v| v.as_boolean())
            .unwrap_or(false);

        if settled {
            break;
        }
    }

    // Read the settled value
    let error = ctx
        .global_object()
        .get(js_string!("__promiseError"), ctx)
        .ok();
    let has_error = error
        .as_ref()
        .and_then(|v| v.as_string())
        .map(|s| !s.to_std_string_escaped().is_empty())
        .unwrap_or(false);

    if has_error {
        // Return the error as a string value — the CDP handler will detect
        // this as an exception-like result
        error.unwrap_or(JsValue::undefined())
    } else {
        ctx.global_object()
            .get(js_string!("__promiseResult"), ctx)
            .unwrap_or(value)
    }
}

fn drain_timers(queue: &Rc<TokioJobQueue>, ctx: &mut Context) {
    let mut iterations = 0u32;
    loop {
        let due = queue.pop_due_timers();
        if due.is_empty() {
            break;
        }

        for timer in due {
            let _ = timer.callback.call(&JsValue::undefined(), &timer.args, ctx);

            // Re-schedule interval timers
            if timer.is_interval {
                let interval_ms = timer.interval_ms.unwrap_or(0).max(1);
                let deadline = Instant::now() + Duration::from_millis(interval_ms);
                queue.schedule_timer(
                    deadline,
                    timer.callback,
                    timer.args,
                    true,
                    Some(interval_ms),
                );
            }
        }

        // Timer callbacks may have queued microtasks — drain those too
        ctx.run_jobs();

        iterations += 1;
        if iterations > 100 {
            // Safety limit to prevent infinite timer loops
            break;
        }
    }
}

/// Push a mutation record to all active MutationObservers.
fn notify_mutation_observers(ctx: &mut Context, mutation_type: &str, target_id: u32) {
    let registry = ctx.global_object().get(js_string!("__moRegistry"), ctx);
    if let Ok(reg_val) = registry
        && let Some(reg_obj) = reg_val.as_object()
        && let Ok(reg_arr) = JsArray::from_object(reg_obj.clone())
        && let Ok(len) = reg_arr.length(ctx)
    {
        for i in 0..len {
            if let Ok(observer_val) = reg_arr.at(i as i64, ctx)
                && let Some(obs_obj) = observer_val.as_object()
            {
                let observing = obs_obj
                    .get(js_string!("__observing"), ctx)
                    .ok()
                    .and_then(|v| v.as_boolean())
                    .unwrap_or(false);
                if observing {
                    // Create MutationRecord
                    let record = boa_engine::object::ObjectInitializer::new(ctx)
                        .property(
                            js_string!("type"),
                            JsValue::from(JsString::from(mutation_type)),
                            Attribute::all(),
                        )
                        .property(
                            js_string!("target"),
                            JsValue::from(target_id),
                            Attribute::all(),
                        )
                        .build();
                    // Push to __records
                    let records_val = obs_obj
                        .get(js_string!("__records"), ctx)
                        .unwrap_or(JsValue::Null);
                    if let Some(rec_obj) = records_val.as_object()
                        && let Ok(rec_arr) = JsArray::from_object(rec_obj.clone())
                    {
                        let _ = rec_arr.push(JsValue::from(record), ctx);
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn create_context(
    output: &Arc<RwLock<Vec<String>>>,
    dom_snapshot: &Arc<RwLock<Option<DomSnapshot>>>,
    mutations: &Arc<RwLock<Vec<DomMutation>>>,
    viewport: (u32, u32),
    page_url: &str,
    user_agent: &str,
    fetch_tx_arc: &Arc<RwLock<Option<std::sync::mpsc::Sender<FetchRequestMsg>>>>,
    cookie_jar_arc: &Arc<RwLock<Option<Arc<RwLock<CookieJar>>>>>,
) -> (Context, Rc<TokioJobQueue>) {
    let job_queue = Rc::new(TokioJobQueue::new());
    let mut context = Context::builder()
        .job_queue(job_queue.clone())
        .build()
        .expect("failed to build boa Engine context");

    // --- Console functions ---

    macro_rules! console_fn {
        ($out:expr) => {
            unsafe {
                NativeFunction::from_closure(
                    move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                        let mut line = String::new();
                        for (i, arg) in args.iter().enumerate() {
                            if i > 0 {
                                line.push(' ');
                            }
                            let s = arg
                                .to_string(ctx)
                                .map(|s| s.to_std_string_escaped())
                                .unwrap_or_else(|_| "undefined".to_string());
                            line.push_str(&s);
                        }
                        {
                            let mut guard = $out.write();
                            guard.push(line);
                        }
                        Ok(JsValue::undefined())
                    },
                )
            }
        };
    }

    let out_log = output.clone();
    let out_warn = output.clone();
    let out_error = output.clone();
    let out_info = output.clone();

    let log_fn = console_fn!(out_log);

    // Register standalone `log(...)` function
    let _ = context.register_global_callable(js_string!("log"), 1, log_fn.clone());

    // Build console object
    let console = boa_engine::object::ObjectInitializer::new(&mut context)
        .function(log_fn, js_string!("log"), 1)
        .function(console_fn!(out_warn), js_string!("warn"), 1)
        .function(console_fn!(out_error), js_string!("error"), 1)
        .function(console_fn!(out_info), js_string!("info"), 1)
        .build();

    let _ = context.register_global_property(js_string!("console"), console, Attribute::all());

    // --- Timer functions (scheduled via TokioJobQueue) ---
    //
    // setTimeout(fn, delay, ...args) — schedules callback via schedule_timer().
    //   The callback fires on the next timer drain (after eval returns).
    // setInterval(fn, delay)        — same, but re-schedules after each firing.
    // clearTimeout / clearInterval   — cancels the timer by ID.

    let timer_queue_st = job_queue.clone();
    let set_timeout_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            if args.is_empty() {
                return Ok(JsValue::undefined());
            }
            let callback = args[0].clone();
            let delay_ms = args.get(1).and_then(|v| v.as_number()).unwrap_or(0.0) as u64;
            let cb_args: Vec<JsValue> = args[2..].to_vec();

            if let Some(func) = callback.as_object().cloned()
                && func.is_callable()
            {
                let deadline = Instant::now() + Duration::from_millis(delay_ms);
                let id = timer_queue_st.schedule_timer(deadline, func, cb_args, false, None);
                return Ok(JsValue::from(id as f64));
            }
            Ok(JsValue::undefined())
        })
    };

    let timer_queue_si = job_queue.clone();
    let set_interval_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            if args.is_empty() {
                return Ok(JsValue::undefined());
            }
            let callback = args[0].clone();
            let delay_ms = args.get(1).and_then(|v| v.as_number()).unwrap_or(0.0) as u64;
            let cb_args: Vec<JsValue> = args[2..].to_vec();

            if let Some(func) = callback.as_object().cloned()
                && func.is_callable()
            {
                let deadline = Instant::now() + Duration::from_millis(delay_ms);
                let id =
                    timer_queue_si.schedule_timer(deadline, func, cb_args, true, Some(delay_ms));
                return Ok(JsValue::from(id as f64));
            }
            Ok(JsValue::undefined())
        })
    };

    let timer_queue_ct = job_queue.clone();
    let clear_timer_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            if let Some(id) = args.first().and_then(|v| v.as_number()) {
                timer_queue_ct.cancel_timer(id as u64);
            }
            Ok(JsValue::undefined())
        })
    };

    let _ = context.register_global_callable(js_string!("setTimeout"), 2, set_timeout_fn);
    let _ = context.register_global_callable(js_string!("setInterval"), 2, set_interval_fn);
    let _ = context.register_global_callable(js_string!("clearTimeout"), 1, clear_timer_fn.clone());
    let _ = context.register_global_callable(js_string!("clearInterval"), 1, clear_timer_fn);

    // --- fetch() implementation ---
    //
    // Makes real HTTP requests via the HttpClient in the main session.
    // The JS thread sends FetchRequestMsg to the main thread via channel,
    // then blocks waiting for the response.
    //
    // Returns a JS Promise that resolves with the Response object.

    let fetch_tx_inner = fetch_tx_arc.clone();

    let fetch_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            // Get URL and options from arguments
            let url = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            // Extract method and options from second argument
            let mut method = String::from("GET");
            let mut headers: Vec<(String, String)> = Vec::new();
            if let Some(opts) = args.get(1).and_then(|v| v.as_object())
                && let Ok(hdrs) = opts.get(js_string!("headers"), ctx)
                && let Some(hdr_obj) = hdrs.as_object()
            {
                for &key in &[
                    "content-type",
                    "accept",
                    "authorization",
                    "user-agent",
                    "cookie",
                ] {
                    if let Ok(val) = hdr_obj.get(js_string!(key), ctx)
                        && !val.is_undefined()
                        && !val.is_null()
                        && let Some(s) = val.as_string()
                    {
                        headers.push((key.to_string(), s.to_std_string_escaped()));
                    }
                }
            }

            let mut body: Option<String> = None;
            let mut _timeout_ms: Option<u64> = None;

            if args.len() > 1
                && let Some(opts) = args[1].as_object()
            {
                // method
                if let Ok(m) = opts.get(js_string!("method"), ctx)
                    && let Some(s) = m.as_string()
                {
                    method = s.to_std_string_escaped().to_uppercase();
                }
                // headers (simplified — just extract common ones)
                // Full header iteration via enumerate() skipped for simplicity
                // since boa 0.20's JsIterator API requires careful handling
                // body
                if let Ok(b) = opts.get(js_string!("body"), ctx)
                    && !b.is_undefined()
                    && !b.is_null()
                    && let Some(s) = b.as_string()
                {
                    body = Some(s.to_std_string_escaped());
                }
                // timeout
                if let Ok(t) = opts.get(js_string!("timeout"), ctx)
                    && let Some(n) = t.as_number()
                {
                    _timeout_ms = Some(n as u64);
                }
            }

            // Send fetch request to main thread
            let tx = fetch_tx_inner.read();
            let tx = match tx.as_ref() {
                Some(t) => t.clone(),
                None => {
                    // No fetch channel — return rejected Promise
                    let reject_code = r#"
                        Promise.reject(new Error('fetch() is not available — channel not set'))
                    "#;
                    let result = ctx.eval(Source::from_bytes(reject_code.trim()));
                    return result;
                }
            };
            drop(tx);

            // Recreate tx after dropping the read guard (to avoid deadlock)
            let tx = {
                let guard = fetch_tx_inner.read();
                guard.as_ref().cloned().unwrap()
            };

            let (response_tx, response_rx) = std::sync::mpsc::channel::<FetchResponseMsg>();
            let request = FetchRequestMsg {
                url: url.clone(),
                method: method.clone(),
                headers,
                body,
                response_tx,
            };

            if let Err(e) = tx.send(request) {
                // Channel error — return rejected Promise
                let err_json = serde_json::to_string(&e.to_string())
                    .unwrap_or_else(|_| "\"fetch channel error\"".to_string());
                let reject_code = format!("Promise.reject(new Error({}))", err_json);
                let result = ctx.eval(Source::from_bytes(reject_code.trim()));
                return result;
            }

            // Wait for response (blocks JS thread)
            // TODO(#async-fetch): This blocking recv() holds the JS thread while waiting
            // for the HTTP response, which prevents other JS from running and blocks
            // the dedicated std::thread. A proper fix would use an async-aware approach:
            // 1. Return a Pending Promise from this closure
            // 2. Use a non-blocking channel check or integrate with boa's job queue
            // 3. Resolve/reject the Promise when the HTTP response arrives
            // This requires architectural changes to how the JS thread processes events.
            let response = response_rx.recv();
            let resp_error: Option<String>;

            match response {
                Ok(resp) => {
                    if let Some(err) = resp.error {
                        resp_error = Some(err);
                    } else {
                        // text() / json() — stash __body as a global prop and let the
                        // eval'd IIFE grab it by reference. Avoids re-stringifying the
                        // body on every call. The body value itself is a GC-tracked
                        // JsValue (JsString), not a serialized copy.
                        let text_fn = {
                            NativeFunction::from_closure(move |this, _args, ctx| {
                                let body = this
                                    .as_object()
                                    .and_then(|o| o.get(js_string!("__body"), ctx).ok())
                                    .unwrap_or(JsValue::undefined());
                                let _ = ctx.register_global_property(
                                    js_string!("__text_body"),
                                    body,
                                    Attribute::all(),
                                );
                                ctx.eval(Source::from_bytes(
                                    "(() => { const v = __text_body; delete globalThis.__text_body; return Promise.resolve(v); })()",
                                ))
                            })
                        };

                        let json_fn = {
                            NativeFunction::from_closure(move |this, _args, ctx| {
                                let body = this
                                    .as_object()
                                    .and_then(|o| o.get(js_string!("__body"), ctx).ok())
                                    .unwrap_or(JsValue::undefined());
                                let _ = ctx.register_global_property(
                                    js_string!("__json_body"),
                                    body,
                                    Attribute::all(),
                                );
                                ctx.eval(Source::from_bytes(
                                    "(() => { const v = __json_body; delete globalThis.__json_body; return Promise.resolve(JSON.parse(v)); })()",
                                ))
                            })
                        };

                        // arrayBuffer() — reads body from this.__body, returns Uint8Array
                        let array_buffer_fn = {
                            NativeFunction::from_closure(move |this, _args, ctx| {
                                let body_owned = {
                                    if let Some(obj) = this.as_object()
                                        && let Ok(v) = obj.get(js_string!("__body"), ctx)
                                        && let Some(s) = v.as_string()
                                    {
                                        s.to_std_string_escaped()
                                    } else {
                                        String::new()
                                    }
                                };
                                let bytes_json = serde_json::to_string(body_owned.as_bytes())
                                    .unwrap_or_else(|_| String::from("[]"));
                                let code =
                                    format!("Promise.resolve(new Uint8Array({}))", bytes_json);
                                ctx.eval(Source::from_bytes(code.trim()))
                            })
                        };

                        let headers_obj = boa_engine::object::ObjectInitializer::new(ctx).build();
                        for (k, v) in &resp.headers {
                            let _ = headers_obj.set(
                                JsString::from(k.as_str()),
                                JsValue::from(JsString::from(v.as_str())),
                                true,
                                ctx,
                            );
                        }

                        let response_obj = boa_engine::object::ObjectInitializer::new(ctx)
                            .property(
                                js_string!("status"),
                                JsValue::from(resp.status),
                                Attribute::all(),
                            )
                            .property(
                                js_string!("statusText"),
                                JsValue::from(JsString::from(resp.status_text.as_str())),
                                Attribute::all(),
                            )
                            .property(
                                js_string!("ok"),
                                JsValue::from(resp.status < 400),
                                Attribute::all(),
                            )
                            .property(
                                js_string!("url"),
                                JsValue::from(JsString::from(resp.url.as_str())),
                                Attribute::all(),
                            )
                            .property(
                                js_string!("bodyUsed"),
                                JsValue::from(false),
                                Attribute::all(),
                            )
                            .property(
                                js_string!("type"),
                                JsValue::from(JsString::from("basic")),
                                Attribute::all(),
                            )
                            .property(
                                js_string!("__body"),
                                JsValue::from(JsString::from(resp.body.as_str())),
                                Attribute::all(),
                            )
                            .property(
                                js_string!("headers"),
                                JsValue::from(headers_obj),
                                Attribute::all(),
                            )
                            .function(text_fn, js_string!("text"), 0)
                            .function(json_fn, js_string!("json"), 0)
                            .function(array_buffer_fn, js_string!("arrayBuffer"), 0)
                            .build();

                        let _ = ctx.register_global_property(
                            js_string!("__fetch_response"),
                            JsValue::from(response_obj),
                            Attribute::all(),
                        );
                        let result = ctx.eval(Source::from_bytes(
                            "(() => { const r = __fetch_response; delete globalThis.__fetch_response; return Promise.resolve(r); })()"
                        ));
                        return result;
                    }
                }
                Err(_) => {
                    resp_error = Some("fetch channel closed".to_string());
                }
            }

            // Return rejected Promise on error
            let err_msg = resp_error.unwrap_or_else(|| "fetch failed".to_string());
            let err_json =
                serde_json::to_string(&err_msg).unwrap_or_else(|_| "\"fetch failed\"".to_string());
            let reject_code = format!("Promise.reject(new Error({}))", err_json);

            ctx.eval(Source::from_bytes(reject_code.trim()))
        })
    };

    let _ = context.register_global_callable(js_string!("fetch"), 2, fetch_fn);

    // --- XMLHttpRequest ---
    let xhr_fetch_tx = fetch_tx_arc.clone();
    let xhr_ctor = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let open_method: Arc<RwLock<String>> = Arc::new(RwLock::new("GET".to_string()));
            let open_url: Arc<RwLock<String>> = Arc::new(RwLock::new(String::new()));
            let open_async: Arc<RwLock<bool>> = Arc::new(RwLock::new(true));
            let ready_state: Arc<RwLock<f64>> = Arc::new(RwLock::new(0.0)); // UNSENT
            let status_val: Arc<RwLock<f64>> = Arc::new(RwLock::new(0.0));
            let response_text: Arc<RwLock<String>> = Arc::new(RwLock::new(String::new()));
            let response_headers: Arc<RwLock<String>> = Arc::new(RwLock::new(String::new()));

            // Event handler callbacks
            let onload_cb: Arc<RwLock<Option<JsValue>>> = Arc::new(RwLock::new(None));
            let onerror_cb: Arc<RwLock<Option<JsValue>>> = Arc::new(RwLock::new(None));
            let onreadystatechange_cb: Arc<RwLock<Option<JsValue>>> = Arc::new(RwLock::new(None));

            // onload setter
            let onload_set = onload_cb.clone();
            let onload_setter = {
                NativeFunction::from_closure(move |_this, args, _ctx| {
                    if let Some(v) = args.first() {
                        *onload_set.write() = Some(v.clone());
                    }
                    Ok(JsValue::undefined())
                })
            };
            let onload_setter_fn = FunctionObjectBuilder::new(ctx.realm(), onload_setter)
                .name(js_string!("set onload"))
                .build();

            // onerror setter
            let onerror_set = onerror_cb.clone();
            let onerror_setter = {
                NativeFunction::from_closure(move |_this, args, _ctx| {
                    if let Some(v) = args.first() {
                        *onerror_set.write() = Some(v.clone());
                    }
                    Ok(JsValue::undefined())
                })
            };
            let onerror_setter_fn = FunctionObjectBuilder::new(ctx.realm(), onerror_setter)
                .name(js_string!("set onerror"))
                .build();

            // onreadystatechange setter
            let onrsc_set = onreadystatechange_cb.clone();
            let onrsc_setter = {
                NativeFunction::from_closure(move |_this, args, _ctx| {
                    if let Some(v) = args.first() {
                        *onrsc_set.write() = Some(v.clone());
                    }
                    Ok(JsValue::undefined())
                })
            };
            let onrsc_setter_fn = FunctionObjectBuilder::new(ctx.realm(), onrsc_setter)
                .name(js_string!("set onreadystatechange"))
                .build();

            // .open(method, url, async)
            let om = open_method.clone();
            let ou = open_url.clone();
            let oa = open_async.clone();
            let rs = ready_state.clone();
            let open_fn = {
                NativeFunction::from_closure(move |_this, args, ctx| {
                    let method = args
                        .first()
                        .and_then(|v| v.to_string(ctx).ok())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let url = args
                        .get(1)
                        .and_then(|v| v.to_string(ctx).ok())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let async_flag = args.get(2).and_then(|v| v.as_boolean()).unwrap_or(true);
                    *om.write() = method;
                    *ou.write() = url;
                    *oa.write() = async_flag;
                    *rs.write() = 1.0; // OPENED
                    Ok(JsValue::undefined())
                })
            };

            // .send(body?)
            let send_method = open_method.clone();
            let send_url = open_url.clone();
            let send_async = open_async.clone();
            let send_rs = ready_state.clone();
            let send_status = status_val.clone();
            let send_resp = response_text.clone();
            let send_hdrs = response_headers.clone();
            let send_onload = onload_cb.clone();
            let send_onerror = onerror_cb.clone();
            let send_onrsc = onreadystatechange_cb.clone();
            let send_tx = xhr_fetch_tx.clone();
            let send_fn = {
                NativeFunction::from_closure(move |_this, args, ctx| {
                    let body = args
                        .first()
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped());
                    let method = send_method.read().clone();
                    let url = send_url.read().clone();
                    let _is_async = *send_async.read();

                    *send_rs.write() = 2.0; // HEADERS_RECEIVED

                    let tx_guard = send_tx.read();
                    if let Some(ref tx) = *tx_guard {
                        let (response_tx, response_rx) =
                            std::sync::mpsc::channel::<FetchResponseMsg>();
                        let request = FetchRequestMsg {
                            url: url.clone(),
                            method: method.clone(),
                            headers: Vec::new(),
                            body,
                            response_tx,
                        };
                        if tx.send(request).is_ok() {
                            match response_rx.recv() {
                                Ok(resp) => {
                                    *send_rs.write() = 3.0; // LOADING
                                    *send_status.write() = resp.status as f64;
                                    *send_resp.write() = resp.body.clone();
                                    // Parse response headers
                                    let mut hdr_str = String::new();
                                    for (k, v) in &resp.headers {
                                        hdr_str.push_str(&format!("{}: {}\r\n", k, v));
                                    }
                                    *send_hdrs.write() = hdr_str;
                                    *send_rs.write() = 4.0; // DONE

                                    if resp.error.is_none() {
                                        // Fire onload
                                        if let Some(ref cb) = *send_onload.read()
                                            && let Some(cb_obj) = cb.as_object()
                                            && cb_obj.is_callable()
                                        {
                                            let _ = cb_obj.call(&JsValue::undefined(), &[], ctx);
                                        }
                                    } else {
                                        if let Some(ref cb) = *send_onerror.read()
                                            && let Some(cb_obj) = cb.as_object()
                                            && cb_obj.is_callable()
                                        {
                                            let _ = cb_obj.call(&JsValue::undefined(), &[], ctx);
                                        }
                                    }
                                    // Fire onreadystatechange
                                    if let Some(ref cb) = *send_onrsc.read()
                                        && let Some(cb_obj) = cb.as_object()
                                        && cb_obj.is_callable()
                                    {
                                        let _ = cb_obj.call(&JsValue::undefined(), &[], ctx);
                                    }
                                }
                                Err(_) => {
                                    *send_rs.write() = 4.0;
                                    *send_status.write() = 0.0;
                                }
                            }
                        }
                    }

                    Ok(JsValue::undefined())
                })
            };

            // .setRequestHeader(name, value) — noop for now
            let set_req_header_fn = {
                NativeFunction::from_closure(move |_this, _args, _ctx| Ok(JsValue::undefined()))
            };

            // .getResponseHeader(name)
            let get_hdr_rs = response_headers.clone();
            let get_header_fn = {
                NativeFunction::from_closure(move |_this, args, ctx| {
                    let name = args
                        .first()
                        .and_then(|v| v.to_string(ctx).ok())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let hdrs = get_hdr_rs.read();
                    for line in hdrs.lines() {
                        if let Some(eq) = line.find(':') {
                            let key = line[..eq].trim();
                            if key.eq_ignore_ascii_case(&name) {
                                return Ok(JsValue::from(JsString::from(line[eq + 1..].trim())));
                            }
                        }
                    }
                    Ok(JsValue::null())
                })
            };

            // .abort() — reset state
            let abort_rs = ready_state.clone();
            let abort_fn = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    *abort_rs.write() = 0.0;
                    Ok(JsValue::undefined())
                })
            };

            // Build object
            let rs_clone = ready_state.clone();
            let rs_getter = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    Ok(JsValue::from(*rs_clone.read()))
                })
            };
            let rs_getter_fn = FunctionObjectBuilder::new(ctx.realm(), rs_getter)
                .name(js_string!("get readyState"))
                .build();

            let st_clone = status_val.clone();
            let st_getter = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    Ok(JsValue::from(*st_clone.read()))
                })
            };
            let st_getter_fn = FunctionObjectBuilder::new(ctx.realm(), st_getter)
                .name(js_string!("get status"))
                .build();

            let rt_clone = response_text.clone();
            let rt_getter = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    Ok(JsValue::from(JsString::from(rt_clone.read().as_str())))
                })
            };
            let rt_getter_fn = FunctionObjectBuilder::new(ctx.realm(), rt_getter)
                .name(js_string!("get responseText"))
                .build();

            let ol_clone = onload_cb.clone();
            let ol_getter = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    Ok(ol_clone.read().clone().unwrap_or(JsValue::null()))
                })
            };
            let ol_getter_fn = FunctionObjectBuilder::new(ctx.realm(), ol_getter)
                .name(js_string!("get onload"))
                .build();

            let oe_clone = onerror_cb.clone();
            let oe_getter = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    Ok(oe_clone.read().clone().unwrap_or(JsValue::null()))
                })
            };
            let oe_getter_fn = FunctionObjectBuilder::new(ctx.realm(), oe_getter)
                .name(js_string!("get onerror"))
                .build();

            let obj = boa_engine::object::ObjectInitializer::new(ctx)
                .accessor(
                    js_string!("readyState"),
                    Some(rs_getter_fn),
                    None,
                    Attribute::all(),
                )
                .accessor(
                    js_string!("status"),
                    Some(st_getter_fn),
                    None,
                    Attribute::all(),
                )
                .accessor(
                    js_string!("responseText"),
                    Some(rt_getter_fn),
                    None,
                    Attribute::all(),
                )
                .accessor(
                    js_string!("onload"),
                    Some(ol_getter_fn),
                    Some(onload_setter_fn),
                    Attribute::all(),
                )
                .accessor(
                    js_string!("onerror"),
                    Some(oe_getter_fn),
                    Some(onerror_setter_fn),
                    Attribute::all(),
                )
                .accessor(
                    js_string!("onreadystatechange"),
                    None,
                    Some(onrsc_setter_fn),
                    Attribute::all(),
                )
                .property(
                    js_string!("responseType"),
                    JsValue::from(JsString::from("")),
                    Attribute::all(),
                )
                .property(js_string!("timeout"), JsValue::from(0), Attribute::all())
                .property(
                    js_string!("withCredentials"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .function(open_fn, js_string!("open"), 3)
                .function(send_fn, js_string!("send"), 1)
                .function(set_req_header_fn, js_string!("setRequestHeader"), 2)
                .function(get_header_fn, js_string!("getResponseHeader"), 1)
                .function(abort_fn, js_string!("abort"), 0)
                .build();

            Ok(JsValue::from(obj))
        })
    };
    let _ = context.register_global_callable(js_string!("XMLHttpRequest"), 0, xhr_ctor);

    // --- MutationObserver ---
    let mo_ctor = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let callback = args.first().cloned().unwrap_or(JsValue::undefined());

            // Observation state stored in JS object properties
            // __callback: the MutationCallback
            // __observing: boolean flag
            // __records: array of MutationRecord objects

            let disconnect_fn = {
                NativeFunction::from_closure(move |_this, _args, ctx| {
                    if let Some(obj) = _this.as_object() {
                        let _ = obj.set(js_string!("__observing"), JsValue::from(false), true, ctx);
                        let empty_arr = JsArray::new(ctx);
                        let _ =
                            obj.set(js_string!("__records"), JsValue::from(empty_arr), true, ctx);
                    }
                    Ok(JsValue::undefined())
                })
            };

            let observe_fn = {
                NativeFunction::from_closure(move |_this, args, ctx| {
                    let _target = args.first();
                    let _options = args.get(1);

                    if let Some(obj) = _this.as_object() {
                        let _ = obj.set(js_string!("__observing"), JsValue::from(true), true, ctx);

                        // Register in global __moRegistry
                        let registry = ctx
                            .global_object()
                            .get(js_string!("__moRegistry"), ctx)
                            .unwrap_or(JsValue::Null);
                        if let Some(reg_obj) = registry.as_object()
                            && let Ok(reg_arr) = JsArray::from_object(reg_obj.clone())
                        {
                            let _ = reg_arr.push(JsValue::from(obj.clone()), ctx);
                        }
                    }
                    Ok(JsValue::undefined())
                })
            };

            let take_records_fn = {
                NativeFunction::from_closure(move |_this, _args, ctx| {
                    if let Some(obj) = _this.as_object() {
                        let records = obj
                            .get(js_string!("__records"), ctx)
                            .unwrap_or(JsValue::Null);
                        // Clear records
                        let empty_arr = JsArray::new(ctx);
                        let _ =
                            obj.set(js_string!("__records"), JsValue::from(empty_arr), true, ctx);
                        return Ok(records);
                    }
                    let arr = JsArray::new(ctx);
                    Ok(JsValue::from(arr))
                })
            };

            let empty_arr = JsArray::new(ctx);
            let obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(js_string!("__callback"), callback, Attribute::all())
                .property(
                    js_string!("__observing"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(
                    js_string!("__records"),
                    JsValue::from(empty_arr),
                    Attribute::all(),
                )
                .function(observe_fn, js_string!("observe"), 2)
                .function(disconnect_fn, js_string!("disconnect"), 0)
                .function(take_records_fn, js_string!("takeRecords"), 0)
                .build();

            Ok(JsValue::from(obj))
        })
    };
    let _ = context.register_global_callable(js_string!("MutationObserver"), 1, mo_ctor);

    // Global MutationObserver registry — tracks all active observers
    let mo_registry = JsArray::new(&mut context);
    let _ = context.register_global_property(
        js_string!("__moRegistry"),
        JsValue::from(mo_registry),
        Attribute::all(),
    );

    // --- Document object ---

    register_document_object(&mut context, dom_snapshot, mutations, cookie_jar_arc);

    // --- Window global ---

    register_window_globals(
        &mut context,
        dom_snapshot,
        mutations,
        viewport,
        page_url,
        user_agent,
        fetch_tx_arc,
    );

    // --- atob / btoa (Base64) ---
    let atob_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let encoded = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            // Decode base64
            let decoded = base64::engine::general_purpose::STANDARD.decode(&encoded);
            match decoded {
                Ok(bytes) => {
                    let s = String::from_utf8_lossy(&bytes).to_string();
                    Ok(JsValue::from(JsString::from(s.as_str())))
                }
                Err(_) => {
                    // Try URL-safe base64
                    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&encoded);
                    match decoded {
                        Ok(bytes) => {
                            let s = String::from_utf8_lossy(&bytes).to_string();
                            Ok(JsValue::from(JsString::from(s.as_str())))
                        }
                        Err(_) => Ok(JsValue::undefined()),
                    }
                }
            }
        })
    };
    let _ = context.register_global_callable(js_string!("atob"), 1, atob_fn);

    let btoa_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let decoded = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            // Encode base64
            let encoded = base64::engine::general_purpose::STANDARD.encode(decoded.as_bytes());
            Ok(JsValue::from(JsString::from(encoded.as_str())))
        })
    };
    let _ = context.register_global_callable(js_string!("btoa"), 1, btoa_fn);

    // --- URLSearchParams (minimal) ---
    // URLSearchParams is typically used as: new URLSearchParams("foo=bar&baz=1")
    // We create a class-like constructor that returns an object with
    // get, set, append, delete, has, keys, values, entries, forEach methods.
    let search_params_ctor = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let query_string = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            // Parse query string into HashMap
            let map: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            let storage = std::cell::RefCell::new(map);

            for pair in query_string.split('&') {
                if let Some(eq) = pair.find('=') {
                    let key = pair[..eq].to_string();
                    let val = pair[eq + 1..].to_string();
                    let mut s = storage.borrow_mut();
                    s.entry(key).or_default().push(val);
                } else if !pair.is_empty() {
                    let mut s = storage.borrow_mut();
                    s.entry(pair.to_string()).or_default();
                }
            }

            let storage_arc = std::sync::Arc::new(storage);
            let _sp_storage = storage_arc.clone();

            // --- get ---
            let get_sp = storage_arc.clone();
            let get_fn = {
                NativeFunction::from_closure(move |_this, _args, ctx| {
                    let key = _args
                        .first()
                        .and_then(|v| v.to_string(ctx).ok())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let val = get_sp
                        .borrow()
                        .get(&key)
                        .and_then(|v| v.first())
                        .cloned()
                        .unwrap_or_default();
                    Ok(JsValue::from(JsString::from(val.as_str())))
                })
            };

            // --- set ---
            let set_sp = storage_arc.clone();
            let set_fn = {
                NativeFunction::from_closure(move |_this, _args, ctx| {
                    let key = _args
                        .first()
                        .and_then(|v| v.to_string(ctx).ok())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let val = _args
                        .get(1)
                        .and_then(|v| v.to_string(ctx).ok())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    set_sp.borrow_mut().insert(key, vec![val]);
                    Ok(JsValue::undefined())
                })
            };

            // --- append ---
            let app_sp = storage_arc.clone();
            let app_fn = {
                NativeFunction::from_closure(move |_this, _args, ctx| {
                    let key = _args
                        .first()
                        .and_then(|v| v.to_string(ctx).ok())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let val = _args
                        .get(1)
                        .and_then(|v| v.to_string(ctx).ok())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    app_sp.borrow_mut().entry(key).or_default().push(val);
                    Ok(JsValue::undefined())
                })
            };

            // --- delete ---
            let del_sp = storage_arc.clone();
            let del_fn = {
                NativeFunction::from_closure(move |_this, _args, ctx| {
                    let key = _args
                        .first()
                        .and_then(|v| v.to_string(ctx).ok())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    del_sp.borrow_mut().remove(&key);
                    Ok(JsValue::undefined())
                })
            };

            // --- has ---
            let has_sp = storage_arc.clone();
            let has_fn = {
                NativeFunction::from_closure(move |_this, _args, ctx| {
                    let key = _args
                        .first()
                        .and_then(|v| v.to_string(ctx).ok())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let result = has_sp.borrow().contains_key(&key);
                    Ok(JsValue::from(result))
                })
            };

            // --- forEach ---
            let foreach_sp = storage_arc.clone();
            let foreach_fn = {
                NativeFunction::from_closure(move |_this, _args, ctx| {
                    if let Some(callback) = _args.first()
                        && let Some(cb_obj) = callback.as_object()
                        && cb_obj.is_callable()
                    {
                        for (key, values) in foreach_sp.borrow().iter() {
                            for val in values {
                                let cb_args = &[
                                    JsValue::from(JsString::from(val.as_str())),
                                    JsValue::from(JsString::from(key.as_str())),
                                    JsValue::undefined(),
                                ];
                                let _ = cb_obj.call(&JsValue::undefined(), cb_args, ctx);
                            }
                        }
                    }
                    Ok(JsValue::undefined())
                })
            };

            // --- toString ---
            let str_sp = storage_arc.clone();
            let str_fn = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    let mut parts = Vec::new();
                    for (key, values) in str_sp.borrow().iter() {
                        for val in values {
                            parts.push(format!("{}={}", key, val));
                        }
                    }
                    Ok(JsValue::from(JsString::from(parts.join("&").as_str())))
                })
            };

            // Build the URLSearchParams object
            let sp_obj = boa_engine::object::ObjectInitializer::new(ctx)
                .function(get_fn, js_string!("get"), 1)
                .function(set_fn, js_string!("set"), 2)
                .function(app_fn, js_string!("append"), 2)
                .function(del_fn, js_string!("delete"), 1)
                .function(has_fn, js_string!("has"), 1)
                .function(foreach_fn, js_string!("forEach"), 1)
                .function(str_fn, js_string!("toString"), 0)
                .build();

            Ok(JsValue::from(sp_obj))
        })
    };
    let _ = context.register_global_callable(js_string!("URLSearchParams"), 1, search_params_ctor);

    // --- URL class (stub) ---
    // new URL(url) — basic URL parsing with protocol, host, pathname, search
    let url_ctor = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let url_str = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let parsed = url::Url::parse(&url_str);

            // Storage object for URL properties
            let url_storage =
                std::sync::Arc::new(std::cell::RefCell::new(std::collections::HashMap::new()));

            // url_storage holds parsed values as strings
            {
                let mut s = url_storage.borrow_mut();
                match &parsed {
                    Ok(u) => {
                        s.insert("href".to_string(), u.to_string());
                        s.insert("origin".to_string(), u.origin().ascii_serialization());
                        s.insert("protocol".to_string(), format!("{}:", u.scheme()));
                        s.insert(
                            "host".to_string(),
                            u.host().map(|h| h.to_string()).unwrap_or_default(),
                        );
                        s.insert(
                            "hostname".to_string(),
                            u.host().map(|h| h.to_string()).unwrap_or_default(),
                        );
                        s.insert("pathname".to_string(), u.path().to_string());
                        s.insert(
                            "search".to_string(),
                            u.query().map(|q| format!("?{}", q)).unwrap_or_default(),
                        );
                        s.insert(
                            "hash".to_string(),
                            u.fragment().map(|f| format!("#{}", f)).unwrap_or_default(),
                        );
                        s.insert(
                            "port".to_string(),
                            u.port().map(|p| p.to_string()).unwrap_or_default(),
                        );
                        s.insert("username".to_string(), u.username().to_string());
                        s.insert(
                            "password".to_string(),
                            u.password().map(|p| p.to_string()).unwrap_or_default(),
                        );
                        s.insert("searchParams".to_string(), "URLSearchParams".to_string());
                        // marker
                    }
                    Err(_) => {
                        s.insert("href".to_string(), url_str.clone());
                        s.insert("origin".to_string(), url_str);
                        s.insert("protocol".to_string(), String::new());
                        s.insert("host".to_string(), String::new());
                        s.insert("hostname".to_string(), String::new());
                        s.insert("pathname".to_string(), String::new());
                        s.insert("search".to_string(), String::new());
                        s.insert("hash".to_string(), String::new());
                        s.insert("port".to_string(), String::new());
                        s.insert("username".to_string(), String::new());
                        s.insert("password".to_string(), String::new());
                    }
                }
            }

            let us_storage = url_storage.clone();
            let href_storage = url_storage.clone();
            let us_storage2 = url_storage.clone();
            let us_storage3 = url_storage.clone();
            let us_storage4 = url_storage.clone();
            let us_storage5 = url_storage.clone();
            let us_storage6 = url_storage.clone();
            let us_storage7 = url_storage.clone();
            let _us_storage8 = url_storage.clone();

            // href getter
            let href_fn = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    let href = href_storage
                        .borrow()
                        .get("href")
                        .cloned()
                        .unwrap_or_default();
                    Ok(JsValue::from(JsString::from(href.as_str())))
                })
            };

            // origin getter
            let origin_fn = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    let origin = us_storage
                        .borrow()
                        .get("origin")
                        .cloned()
                        .unwrap_or_default();
                    Ok(JsValue::from(JsString::from(origin.as_str())))
                })
            };

            // protocol getter
            let proto_fn = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    let proto = us_storage2
                        .borrow()
                        .get("protocol")
                        .cloned()
                        .unwrap_or_default();
                    Ok(JsValue::from(JsString::from(proto.as_str())))
                })
            };

            // host getter
            let host_fn = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    let host = us_storage3
                        .borrow()
                        .get("host")
                        .cloned()
                        .unwrap_or_default();
                    Ok(JsValue::from(JsString::from(host.as_str())))
                })
            };

            // pathname getter
            let path_fn = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    let path = us_storage4
                        .borrow()
                        .get("pathname")
                        .cloned()
                        .unwrap_or_default();
                    Ok(JsValue::from(JsString::from(path.as_str())))
                })
            };

            // search getter
            let search_fn = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    let search = us_storage5
                        .borrow()
                        .get("search")
                        .cloned()
                        .unwrap_or_default();
                    Ok(JsValue::from(JsString::from(search.as_str())))
                })
            };

            // hash getter
            let hash_fn = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    let hash = us_storage6
                        .borrow()
                        .get("hash")
                        .cloned()
                        .unwrap_or_default();
                    Ok(JsValue::from(JsString::from(hash.as_str())))
                })
            };

            // searchParams getter (returns URLSearchParams-like object)
            let sp_storage = us_storage7.clone();
            let sp_fn = {
                NativeFunction::from_closure(move |_this, _args, ctx| {
                    let search = sp_storage
                        .borrow()
                        .get("search")
                        .cloned()
                        .unwrap_or_default();
                    let query = search.trim_start_matches('?').to_string();

                    // Parse query string into key-value pairs
                    let params: Vec<(String, String)> = if query.is_empty() {
                        Vec::new()
                    } else {
                        query
                            .split('&')
                            .filter_map(|pair| {
                                let mut kv = pair.splitn(2, '=');
                                let key = kv.next().unwrap_or("").to_string();
                                let val = kv.next().unwrap_or("").to_string();
                                if !key.is_empty() {
                                    Some((key, val))
                                } else {
                                    None
                                }
                            })
                            .collect()
                    };

                    // Build a JS object that acts like URLSearchParams
                    let _sp_get_fn = {
                        NativeFunction::from_closure(move |_this, _args, _ctx| {
                            Ok(JsValue::undefined())
                        })
                    };

                    // Store params in an array for methods to use
                    let params_arr = JsArray::new(ctx);
                    for (k, v) in &params {
                        let entry = boa_engine::object::ObjectInitializer::new(ctx)
                            .property(
                                js_string!("0"),
                                JsValue::from(JsString::from(k.as_str())),
                                Attribute::all(),
                            )
                            .property(
                                js_string!("1"),
                                JsValue::from(JsString::from(v.as_str())),
                                Attribute::all(),
                            )
                            .build();
                        let _ = params_arr.push(JsValue::from(entry), ctx);
                    }

                    // get(name) — returns first value for the key
                    let get_params = params.clone();
                    let sp_get = {
                        NativeFunction::from_closure(move |_this, args, _ctx| {
                            let key = args
                                .first()
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_std_string_escaped())
                                .unwrap_or_default();
                            for (k, v) in &get_params {
                                if k == &key {
                                    return Ok(JsValue::from(JsString::from(v.as_str())));
                                }
                            }
                            Ok(JsValue::null())
                        })
                    };

                    // has(name)
                    let has_params = params.clone();
                    let sp_has = {
                        NativeFunction::from_closure(move |_this, args, _ctx| {
                            let key = args
                                .first()
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_std_string_escaped())
                                .unwrap_or_default();
                            Ok(JsValue::from(has_params.iter().any(|(k, _)| k == &key)))
                        })
                    };

                    // toString()
                    let to_str_query = query.clone();
                    let sp_to_string = {
                        NativeFunction::from_closure(move |_this, _args, _ctx| {
                            Ok(JsValue::from(JsString::from(to_str_query.as_str())))
                        })
                    };

                    // getAll(name)
                    let getall_params = params.clone();
                    let sp_get_all = {
                        NativeFunction::from_closure(move |_this, args, ctx2| {
                            let key = args
                                .first()
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_std_string_escaped())
                                .unwrap_or_default();
                            let vals: Vec<JsValue> = getall_params
                                .iter()
                                .filter(|(k, _)| k == &key)
                                .map(|(_, v)| JsValue::from(JsString::from(v.as_str())))
                                .collect();
                            Ok(JsValue::from(JsArray::from_iter(vals, ctx2)))
                        })
                    };

                    let sp_obj = boa_engine::object::ObjectInitializer::new(ctx)
                        .function(sp_get, js_string!("get"), 1)
                        .function(sp_has, js_string!("has"), 1)
                        .function(sp_get_all, js_string!("getAll"), 1)
                        .function(sp_to_string, js_string!("toString"), 0)
                        .build();
                    Ok(JsValue::from(sp_obj))
                })
            };

            // Build URL object — convert NativeFunction to JsFunction via FunctionObjectBuilder
            let href_getter = FunctionObjectBuilder::new(ctx.realm(), href_fn)
                .name("get href")
                .build();
            let origin_getter = FunctionObjectBuilder::new(ctx.realm(), origin_fn)
                .name("get origin")
                .build();
            let proto_getter = FunctionObjectBuilder::new(ctx.realm(), proto_fn)
                .name("get protocol")
                .build();
            let host_getter = FunctionObjectBuilder::new(ctx.realm(), host_fn)
                .name("get host")
                .build();
            let path_getter = FunctionObjectBuilder::new(ctx.realm(), path_fn)
                .name("get pathname")
                .build();
            let search_getter = FunctionObjectBuilder::new(ctx.realm(), search_fn)
                .name("get search")
                .build();
            let hash_getter = FunctionObjectBuilder::new(ctx.realm(), hash_fn)
                .name("get hash")
                .build();
            let sp_getter = FunctionObjectBuilder::new(ctx.realm(), sp_fn)
                .name("get searchParams")
                .build();

            let url_obj = boa_engine::object::ObjectInitializer::new(ctx)
                .accessor(
                    js_string!("href"),
                    Some(href_getter),
                    None,
                    Attribute::all(),
                )
                .accessor(
                    js_string!("origin"),
                    Some(origin_getter),
                    None,
                    Attribute::all(),
                )
                .accessor(
                    js_string!("protocol"),
                    Some(proto_getter),
                    None,
                    Attribute::all(),
                )
                .accessor(
                    js_string!("host"),
                    Some(host_getter),
                    None,
                    Attribute::all(),
                )
                .accessor(
                    js_string!("pathname"),
                    Some(path_getter),
                    None,
                    Attribute::all(),
                )
                .accessor(
                    js_string!("search"),
                    Some(search_getter),
                    None,
                    Attribute::all(),
                )
                .accessor(
                    js_string!("hash"),
                    Some(hash_getter),
                    None,
                    Attribute::all(),
                )
                .accessor(
                    js_string!("searchParams"),
                    Some(sp_getter),
                    None,
                    Attribute::all(),
                )
                .build();

            Ok(JsValue::from(url_obj))
        })
    };
    let _ = context.register_global_callable(js_string!("URL"), 1, url_ctor);

    // --- crypto.getRandomValues (CSPRNG) ---
    // Supports both JsArray and TypedArray (Uint8Array, Int32Array, etc.)
    // by using object.get("length") + object.set(index, value) directly.
    let get_random_values_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let arr = args.first().cloned().unwrap_or(JsValue::undefined());
            if let Some(arr_obj) = arr.as_object() {
                // Try to get length from the object (works for both JsArray and TypedArray)
                if let Ok(len_val) = arr_obj.get(js_string!("length"), ctx)
                    && let Some(len) = len_val.as_number()
                {
                    let arr_len = (len as usize).min(65536);
                    let mut buf = vec![0u8; arr_len];
                    // Use real CSPRNG instead of predictable time-based PRNG
                    let _ = getrandom::fill(&mut buf);
                    for (i, val) in buf.iter().enumerate().take(arr_len) {
                        let _ = arr_obj.set(i as u32, JsValue::from(*val as i32), true, ctx);
                    }
                }
            }
            Ok(arr)
        })
    };
    let crypto_obj = boa_engine::object::ObjectInitializer::new(&mut context)
        .function(get_random_values_fn, js_string!("getRandomValues"), 1)
        .build();
    let _ = context.register_global_property(
        js_string!("crypto"),
        JsValue::from(crypto_obj),
        Attribute::all(),
    );

    // --- TextEncoder ---
    let _te_encode_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let input = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let bytes = input.as_bytes();
            let arr = JsArray::new(ctx);
            for &b in bytes {
                let _ = arr.push(JsValue::from(b), ctx);
            }
            let _obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(
                    js_string!("encoding"),
                    JsValue::from(JsString::from("utf-8")),
                    Attribute::all(),
                )
                .build();
            // Return Uint8Array-like object
            let result = boa_engine::object::ObjectInitializer::new(ctx)
                .property(
                    js_string!("buffer"),
                    JsValue::from(arr.clone()),
                    Attribute::all(),
                )
                .build();
            Ok(JsValue::from(result))
        })
    };
    // Avoid recursive closure — use a simpler approach
    let te_ctor = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let encode_fn = {
                NativeFunction::from_closure(move |_this2, args2, ctx2| {
                    let input = args2
                        .first()
                        .and_then(|v| v.to_string(ctx2).ok())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let bytes = input.as_bytes();
                    let arr = JsArray::new(ctx2);
                    for &b in bytes {
                        let _ = arr.push(JsValue::from(b), ctx2);
                    }
                    Ok(JsValue::from(arr))
                })
            };
            let obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(
                    js_string!("encoding"),
                    JsValue::from(JsString::from("utf-8")),
                    Attribute::all(),
                )
                .function(encode_fn, js_string!("encode"), 1)
                .build();
            Ok(JsValue::from(obj))
        })
    };
    let _ = context.register_global_callable(js_string!("TextEncoder"), 0, te_ctor);

    // --- TextDecoder ---
    let td_ctor = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let decode_fn = {
                NativeFunction::from_closure(move |_this2, args2, ctx2| {
                    // Decode buffer/array back to string
                    let input = args2.first().cloned().unwrap_or(JsValue::undefined());
                    if let Some(arr_obj) = input.as_object()
                        && let Ok(arr) = JsArray::from_object(arr_obj.clone())
                        && let Ok(len) = arr.length(ctx2)
                    {
                        let mut bytes = Vec::with_capacity(len as usize);
                        for i in 0..len {
                            if let Ok(v) = arr.at(i as i64, ctx2)
                                && let Some(n) = v.as_number()
                            {
                                bytes.push(n as u8);
                            }
                        }
                        let s = String::from_utf8_lossy(&bytes).to_string();
                        return Ok(JsValue::from(JsString::from(s.as_str())));
                    }
                    Ok(JsValue::from(JsString::from("")))
                })
            };
            let encoding = args
                .first()
                .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
                .unwrap_or_else(|| "utf-8".to_string());
            let obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(
                    js_string!("encoding"),
                    JsValue::from(JsString::from(encoding.as_str())),
                    Attribute::all(),
                )
                .function(decode_fn, js_string!("decode"), 1)
                .build();
            Ok(JsValue::from(obj))
        })
    };
    let _ = context.register_global_callable(js_string!("TextDecoder"), 0, td_ctor);

    // --- Array.from() polyfill ---
    // boa_engine doesn't expose Array.from yet, so we inject it.
    let array_from_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let source = args.first().cloned().unwrap_or(JsValue::undefined());

            // Case 1: Already an array → shallow copy
            if let Some(obj) = source.as_object() {
                if let Ok(arr) = JsArray::from_object(obj.clone())
                    && let Ok(len) = arr.length(ctx)
                {
                    let items: Vec<JsValue> = (0..len)
                        .filter_map(|i| arr.at(i as i64, ctx).ok())
                        .collect();
                    return Ok(JsArray::from_iter(items, ctx).into());
                }

                // Case 2: Array-like object (has .length + indexed props)
                if let Ok(len_val) = obj.get(js_string!("length"), ctx)
                    && let Some(len) = len_val.as_number()
                {
                    let items: Vec<JsValue> = (0..len as u32)
                        .filter_map(|i| obj.get(i, ctx).ok())
                        .collect();
                    return Ok(JsArray::from_iter(items, ctx).into());
                }
            }

            // Case 3: Single value → wrap in array
            if !source.is_undefined() {
                return Ok(JsArray::from_iter([source], ctx).into());
            }

            Ok(JsArray::new(ctx).into())
        })
    };
    let _ = context.register_global_callable(js_string!("ArrayFrom"), 1, array_from_fn);
    let _ = context.eval(Source::from_bytes(
        "if (typeof Array.from === 'undefined') { Array.from = ArrayFrom; delete globalThis.ArrayFrom; }"
    ));

    // --- requestAnimationFrame ---
    //
    // Mirrors setTimeout's schedule_timer pattern: clone the job_queue into the closure,
    // pick the callback from args[0], fire after ~16ms (~60fps), and pass a
    // DOMHighResTimeStamp (ms since Unix epoch) as the callback's argument.
    let raf_queue = job_queue.clone();
    let raf_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let Some(callback) = args.first().cloned() else {
                return Ok(JsValue::undefined());
            };
            if let Some(func) = callback.as_object().cloned()
                && func.is_callable()
            {
                let deadline = Instant::now() + Duration::from_millis(16);
                let timestamp_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64() * 1000.0)
                    .unwrap_or(0.0);
                let cb_args: Vec<JsValue> = vec![JsValue::from(timestamp_ms)];
                let id = raf_queue.schedule_timer(deadline, func, cb_args, false, None);
                return Ok(JsValue::from(id as f64));
            }
            Ok(JsValue::undefined())
        })
    };
    let _ = context.register_global_callable(js_string!("requestAnimationFrame"), 1, raf_fn);

    // --- cancelAnimationFrame ---
    //
    // Cancels a previously scheduled rAF by its handle ID — same shape as clearTimeout.
    let cancel_raf_queue = job_queue.clone();
    let cancel_raf_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            if let Some(id) = args.first().and_then(|v| v.as_number()) {
                cancel_raf_queue.cancel_timer(id as u64);
            }
            Ok(JsValue::undefined())
        })
    };
    let _ = context.register_global_callable(js_string!("cancelAnimationFrame"), 1, cancel_raf_fn);

    // --- Event constructor ---

    // ── Event init-dict helpers ──────────────────────────────────────────────
    /// Copy known properties from args[1] (init dict) onto a freshly-built event object.
    /// Keys that exist in the init dict override the default; keys missing from the
    /// dict keep the default. This is used by every built-in event constructor
    /// because boa_engine's ObjectInitializer doesn't support dynamic property
    /// injection at build time.
    fn apply_init_dict(args: &[JsValue], obj: &JsObject, ctx: &mut Context, keys: &[&str]) {
        let Some(init) = args.get(1).and_then(|v| v.as_object()) else {
            return;
        };
        for &key in keys {
            if let Ok(val) = init.get(js_string!(key), ctx)
                && !val.is_undefined()
            {
                let _ = obj.set(js_string!(key), val, true, ctx);
            }
        }
    }

    /// Add Event.prototype methods as own properties on every event object.
    /// This is needed because boa_engine's register_global_callable does not
    /// create a .prototype property on the constructor, making prototype-based
    /// inheritance unavailable.
    fn setup_event_object(obj: &JsObject, ctx: &mut Context) {
        // preventDefault
        let prevent_fn = unsafe {
            NativeFunction::from_closure(move |_this, _args, ctx| {
                if let Some(o) = _this.as_object() {
                    let _ = o.set(
                        js_string!("defaultPrevented"),
                        JsValue::from(true),
                        true,
                        ctx,
                    );
                }
                Ok(JsValue::undefined())
            })
        };
        let _ = obj.set(
            js_string!("preventDefault"),
            FunctionObjectBuilder::new(ctx.realm(), prevent_fn)
                .name(js_string!("preventDefault"))
                .build(),
            true,
            ctx,
        );

        // stopPropagation
        let stop_fn = unsafe {
            NativeFunction::from_closure(move |_this, _args, ctx| {
                if let Some(o) = _this.as_object() {
                    let _ = o.set(
                        js_string!("__stopPropagation"),
                        JsValue::from(true),
                        true,
                        ctx,
                    );
                }
                Ok(JsValue::undefined())
            })
        };
        let _ = obj.set(
            js_string!("stopPropagation"),
            FunctionObjectBuilder::new(ctx.realm(), stop_fn)
                .name(js_string!("stopPropagation"))
                .build(),
            true,
            ctx,
        );

        let stop_imm_fn = unsafe {
            NativeFunction::from_closure(move |_this, _args, ctx| {
                if let Some(o) = _this.as_object() {
                    let _ = o.set(
                        js_string!("__stopPropagation"),
                        JsValue::from(true),
                        true,
                        ctx,
                    );
                    let _ = o.set(
                        js_string!("__stopImmediatePropagation"),
                        JsValue::from(true),
                        true,
                        ctx,
                    );
                }
                Ok(JsValue::undefined())
            })
        };
        let _ = obj.set(
            js_string!("stopImmediatePropagation"),
            FunctionObjectBuilder::new(ctx.realm(), stop_imm_fn)
                .name(js_string!("stopImmediatePropagation"))
                .build(),
            true,
            ctx,
        );
    }
    const EVENT_INIT_KEYS: &[&str] = &["bubbles", "cancelable"];
    const MOUSE_INIT_KEYS: &[&str] = &[
        "bubbles",
        "cancelable",
        "clientX",
        "clientY",
        "button",
        "buttons",
        "screenX",
        "screenY",
        "ctrlKey",
        "shiftKey",
        "altKey",
        "metaKey",
        "relatedTarget",
        "view",
        "detail",
    ];
    const KEYBOARD_INIT_KEYS: &[&str] = &[
        "key",
        "code",
        "keyCode",
        "charCode",
        "which",
        "location",
        "ctrlKey",
        "shiftKey",
        "altKey",
        "metaKey",
        "repeat",
        "isComposing",
    ];
    const FOCUS_INIT_KEYS: &[&str] = &["bubbles", "cancelable", "relatedTarget"];
    let event_ctor = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(
                    js_string!("type"),
                    JsValue::from(JsString::from(event_type.as_str())),
                    Attribute::all(),
                )
                .property(
                    js_string!("bubbles"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(
                    js_string!("cancelable"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(
                    js_string!("defaultPrevented"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(js_string!("target"), JsValue::null(), Attribute::all())
                .property(
                    js_string!("currentTarget"),
                    JsValue::null(),
                    Attribute::all(),
                )
                .property(js_string!("eventPhase"), JsValue::from(0), Attribute::all())
                .build();
            apply_init_dict(args, &obj, ctx, EVENT_INIT_KEYS);
            setup_event_object(&obj, ctx);
            Ok(JsValue::from(obj))
        })
    };
    let _ = context.register_global_callable(js_string!("Event"), 1, event_ctor);

    // Event.prototype methods — needed by dispatchEvent logic
    let _ = context.eval(Source::from_bytes(
        r#"
        Event.prototype.preventDefault = function() {
            this.defaultPrevented = true;
        };
        Event.prototype.stopPropagation = function() {
            this.__stopPropagation = true;
        };
        Event.prototype.stopImmediatePropagation = function() {
            this.__stopImmediatePropagation = true;
            this.__stopPropagation = true;
        };
    "#,
    ));

    // --- MouseEvent constructor ---
    let mouse_event_ctor = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(
                    js_string!("type"),
                    JsValue::from(JsString::from(event_type.as_str())),
                    Attribute::all(),
                )
                .property(js_string!("bubbles"), JsValue::from(true), Attribute::all())
                .property(
                    js_string!("cancelable"),
                    JsValue::from(true),
                    Attribute::all(),
                )
                .property(js_string!("clientX"), JsValue::from(0), Attribute::all())
                .property(js_string!("clientY"), JsValue::from(0), Attribute::all())
                .property(js_string!("button"), JsValue::from(0), Attribute::all())
                .property(js_string!("buttons"), JsValue::from(0), Attribute::all())
                .property(js_string!("screenX"), JsValue::from(0), Attribute::all())
                .property(js_string!("screenY"), JsValue::from(0), Attribute::all())
                .property(
                    js_string!("ctrlKey"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(
                    js_string!("shiftKey"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(js_string!("altKey"), JsValue::from(false), Attribute::all())
                .property(
                    js_string!("metaKey"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(
                    js_string!("relatedTarget"),
                    JsValue::null(),
                    Attribute::all(),
                )
                .property(js_string!("view"), JsValue::null(), Attribute::all())
                .property(js_string!("detail"), JsValue::from(0), Attribute::all())
                .build();
            apply_init_dict(args, &obj, ctx, MOUSE_INIT_KEYS);
            setup_event_object(&obj, ctx);
            Ok(JsValue::from(obj))
        })
    };
    let _ = context.register_global_callable(js_string!("MouseEvent"), 1, mouse_event_ctor);

    // --- KeyboardEvent constructor ---
    let keyboard_event_ctor = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(
                    js_string!("type"),
                    JsValue::from(JsString::from(event_type.as_str())),
                    Attribute::all(),
                )
                .property(
                    js_string!("key"),
                    JsValue::from(JsString::from("")),
                    Attribute::all(),
                )
                .property(
                    js_string!("code"),
                    JsValue::from(JsString::from("")),
                    Attribute::all(),
                )
                .property(js_string!("keyCode"), JsValue::from(0), Attribute::all())
                .property(js_string!("charCode"), JsValue::from(0), Attribute::all())
                .property(js_string!("which"), JsValue::from(0), Attribute::all())
                .property(js_string!("location"), JsValue::from(0), Attribute::all())
                .property(
                    js_string!("ctrlKey"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(
                    js_string!("shiftKey"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(js_string!("altKey"), JsValue::from(false), Attribute::all())
                .property(
                    js_string!("metaKey"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(js_string!("repeat"), JsValue::from(false), Attribute::all())
                .property(
                    js_string!("isComposing"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .build();
            apply_init_dict(args, &obj, ctx, KEYBOARD_INIT_KEYS);
            setup_event_object(&obj, ctx);
            Ok(JsValue::from(obj))
        })
    };
    let _ = context.register_global_callable(js_string!("KeyboardEvent"), 1, keyboard_event_ctor);
    let focus_event_ctor = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(
                    js_string!("type"),
                    JsValue::from(JsString::from(event_type.as_str())),
                    Attribute::all(),
                )
                .property(
                    js_string!("bubbles"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(
                    js_string!("cancelable"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(
                    js_string!("relatedTarget"),
                    JsValue::null(),
                    Attribute::all(),
                )
                .build();
            apply_init_dict(args, &obj, ctx, FOCUS_INIT_KEYS);
            setup_event_object(&obj, ctx);
            Ok(JsValue::from(obj))
        })
    };
    let _ = context.register_global_callable(js_string!("FocusEvent"), 1, focus_event_ctor);

    // --- DragEvent constructor (extends MouseEvent init keys) ---
    let drag_event_ctor = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(
                    js_string!("type"),
                    JsValue::from(JsString::from(event_type.as_str())),
                    Attribute::all(),
                )
                .property(js_string!("bubbles"), JsValue::from(true), Attribute::all())
                .property(
                    js_string!("cancelable"),
                    JsValue::from(true),
                    Attribute::all(),
                )
                .property(js_string!("clientX"), JsValue::from(0), Attribute::all())
                .property(js_string!("clientY"), JsValue::from(0), Attribute::all())
                .property(js_string!("button"), JsValue::from(0), Attribute::all())
                .property(js_string!("buttons"), JsValue::from(0), Attribute::all())
                .property(js_string!("screenX"), JsValue::from(0), Attribute::all())
                .property(js_string!("screenY"), JsValue::from(0), Attribute::all())
                .property(
                    js_string!("ctrlKey"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(
                    js_string!("shiftKey"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(js_string!("altKey"), JsValue::from(false), Attribute::all())
                .property(
                    js_string!("metaKey"),
                    JsValue::from(false),
                    Attribute::all(),
                )
                .property(
                    js_string!("dataTransfer"),
                    JsValue::null(),
                    Attribute::all(),
                )
                .build();
            apply_init_dict(args, &obj, ctx, MOUSE_INIT_KEYS);
            setup_event_object(&obj, ctx);
            Ok(JsValue::from(obj))
        })
    };
    let _ = context.register_global_callable(js_string!("DragEvent"), 1, drag_event_ctor);

    // --- document.createDocumentFragment (via eval) ---
    let _ = context.eval(Source::from_bytes(
        r#"
        document.createDocumentFragment = function() {
            var fragId = 1100000;
            return {
                nodeType: 11,
                __nodeId: fragId,
                appendChild: function(child) { return child; }
            };
        };
    "#,
    ));

    (context, job_queue)
}

// ---------------------------------------------------------------------------
// Document object registration
// ---------------------------------------------------------------------------

/// Register the `document` global object with DOM query methods.
fn register_document_object(
    ctx: &mut Context,
    dom_snapshot: &Arc<RwLock<Option<DomSnapshot>>>,
    mutations: &Arc<RwLock<Vec<DomMutation>>>,
    cookie_jar_arc: &Arc<RwLock<Option<Arc<RwLock<CookieJar>>>>>,
) {
    let dom_capture_title = dom_snapshot.clone();
    let title_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let dom = dom_capture_title.read();
            if let Some(ref s) = *dom {
                Ok(JsValue::from(JsString::from(s.title.as_str())))
            } else {
                Ok(JsValue::from(JsString::from("")))
            }
        })
    };
    let title_getter_fn = FunctionObjectBuilder::new(ctx.realm(), title_getter)
        .name(js_string!("get title"))
        .build();

    let dom_capture_url = dom_snapshot.clone();
    let url_getter: NativeFunction = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let dom = dom_capture_url.read();
            if let Some(ref s) = *dom {
                Ok(JsValue::from(JsString::from(s.url.as_str())))
            } else {
                Ok(JsValue::from(JsString::from("")))
            }
        })
    };
    let url_getter_fn = FunctionObjectBuilder::new(ctx.realm(), url_getter)
        .name(js_string!("get URL"))
        .build();

    let cookie_jar_for_get = cookie_jar_arc.clone();
    let dom_for_cookie = dom_snapshot.clone();
    let cookie_getter: NativeFunction = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let dom = dom_for_cookie.read();
            if let Some(ref s) = *dom
                && let Ok(url) = url::Url::parse(&s.url)
            {
                let guard = cookie_jar_for_get.read();
                if let Some(ref jar) = *guard {
                    let cookies = jar.read().cookies_for_js(&url);
                    return Ok(JsValue::from(JsString::from(cookies.as_str())));
                }
            }
            Ok(JsValue::from(JsString::from("")))
        })
    };
    let cookie_getter_fn = FunctionObjectBuilder::new(ctx.realm(), cookie_getter)
        .name(js_string!("get cookie"))
        .build();

    let cookie_jar_for_set = cookie_jar_arc.clone();
    let dom_for_cookie_set = dom_snapshot.clone();
    let cookie_setter: NativeFunction = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            if let Some(cookie_str) = args.first().and_then(|v| v.as_string()) {
                let cookie_string = cookie_str.to_std_string_escaped();
                let dom = dom_for_cookie_set.read();
                if let Some(ref s) = *dom
                    && let Ok(url) = url::Url::parse(&s.url)
                {
                    let guard = cookie_jar_for_set.read();
                    if let Some(ref jar) = *guard {
                        jar.write().store(&url, &cookie_string);
                    }
                }
            }
            Ok(JsValue::undefined())
        })
    };
    let cookie_setter_fn = FunctionObjectBuilder::new(ctx.realm(), cookie_setter)
        .name(js_string!("set cookie"))
        .build();

    // querySelector(selector)
    let dom_capture_qs = dom_snapshot.clone();
    let mutations_capture_qs = mutations.clone();
    let query_selector_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let selector = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = dom_capture_qs.read();
            if let Some(ref snapshot) = *dom
                && let Some(node_id) = snapshot.query_selector(&selector)
                && let Some(node) = snapshot.nodes.get(&node_id)
            {
                return Ok(create_element_object(
                    snapshot,
                    node,
                    ctx,
                    &mutations_capture_qs,
                    &dom_capture_qs,
                ));
            }
            Ok(JsValue::null())
        })
    };

    // querySelectorAll(selector)
    let dom_capture_qsa = dom_snapshot.clone();
    let mutations_capture_qsa = mutations.clone();
    let query_selector_all_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let selector = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = dom_capture_qsa.read();
            if let Some(ref snapshot) = *dom {
                let ids = snapshot.query_selector_all(&selector);
                let js_values: Vec<JsValue> = ids
                    .iter()
                    .filter_map(|&id| {
                        snapshot.nodes.get(&id).map(|node| {
                            create_element_object(
                                snapshot,
                                node,
                                ctx,
                                &mutations_capture_qsa,
                                &dom_capture_qsa,
                            )
                        })
                    })
                    .collect();
                let arr = JsArray::from_iter(js_values, ctx);
                return Ok(arr.into());
            }
            let arr = JsArray::from_iter(Vec::<JsValue>::new(), ctx);
            Ok(arr.into())
        })
    };

    // getElementById(id)
    let dom_capture_gbi = dom_snapshot.clone();
    let mutations_capture_gbi = mutations.clone();
    let get_element_by_id_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let id = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = dom_capture_gbi.read();
            if let Some(ref snapshot) = *dom
                && let Some(node_id) = snapshot.get_element_by_id(&id)
                && let Some(node) = snapshot.nodes.get(&node_id)
            {
                return Ok(create_element_object(
                    snapshot,
                    node,
                    ctx,
                    &mutations_capture_gbi,
                    &dom_capture_gbi,
                ));
            }
            Ok(JsValue::null())
        })
    };

    // getElementsByTagName(tag)
    let dom_capture_gtn = dom_snapshot.clone();
    let mutations_capture_gtn = mutations.clone();
    let get_elements_by_tag_name_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let tag = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = dom_capture_gtn.read();
            if let Some(ref snapshot) = *dom {
                let ids = snapshot.get_elements_by_tag_name(&tag);
                let js_values: Vec<JsValue> = ids
                    .iter()
                    .filter_map(|&id| {
                        snapshot.nodes.get(&id).map(|node| {
                            create_element_object(
                                snapshot,
                                node,
                                ctx,
                                &mutations_capture_gtn,
                                &dom_capture_gtn,
                            )
                        })
                    })
                    .collect();
                let arr = JsArray::from_iter(js_values, ctx);
                return Ok(arr.into());
            }
            let arr = JsArray::from_iter(Vec::<JsValue>::new(), ctx);
            Ok(arr.into())
        })
    };

    // getElementsByClassName(class)
    let dom_capture_gcn = dom_snapshot.clone();
    let mutations_capture_gcn = mutations.clone();
    let get_elements_by_class_name_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let class = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = dom_capture_gcn.read();
            if let Some(ref snapshot) = *dom {
                let ids = snapshot.get_elements_by_class_name(&class);
                let js_values: Vec<JsValue> = ids
                    .iter()
                    .filter_map(|&id| {
                        snapshot.nodes.get(&id).map(|node| {
                            create_element_object(
                                snapshot,
                                node,
                                ctx,
                                &mutations_capture_gcn,
                                &dom_capture_gcn,
                            )
                        })
                    })
                    .collect();
                let arr = JsArray::from_iter(js_values, ctx);
                return Ok(arr.into());
            }
            let arr = JsArray::from_iter(Vec::<JsValue>::new(), ctx);
            Ok(arr.into())
        })
    };

    // EventTarget methods for document — uses __listeners property on the document object
    let doc_add_event_listener_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let callback = args.get(1).cloned().unwrap_or(JsValue::undefined());

            if callback.is_undefined() || callback.is_null() {
                return Ok(JsValue::undefined());
            }

            let this_obj = match _this.as_object() {
                Some(o) => o,
                None => return Ok(JsValue::undefined()),
            };

            // Get or create __listeners object
            let listeners_val = this_obj
                .get(js_string!("__listeners"), ctx)
                .unwrap_or(JsValue::Null);
            // Create __listeners if missing
            if listeners_val.as_object().is_none() {
                let obj = boa_engine::object::ObjectInitializer::new(ctx).build();
                let _ = this_obj.set(js_string!("__listeners"), JsValue::from(obj), true, ctx);
            }
            let lv2 = this_obj
                .get(js_string!("__listeners"), ctx)
                .unwrap_or(JsValue::Null);
            let listeners_obj = match lv2.as_object() {
                Some(o) => o,
                None => return Ok(JsValue::undefined()),
            };

            // Ensure array for this event type
            let arr_key = JsString::from(event_type.as_str());
            let ev = listeners_obj
                .get(arr_key.clone(), ctx)
                .unwrap_or(JsValue::Null);
            if ev.as_object().is_none() {
                let a: JsValue = JsValue::from(JsArray::new(ctx));
                let _ = listeners_obj.set(arr_key.clone(), a, true, ctx);
            }
            let arr_val = listeners_obj.get(arr_key, ctx).unwrap_or(JsValue::Null);
            if let Some(arr_obj) = arr_val.as_object()
                && let Ok(arr) = JsArray::from_object(arr_obj.clone())
            {
                let _ = arr.push(callback, ctx);
            }

            Ok(JsValue::undefined())
        })
    };

    let doc_remove_event_listener_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            if let Some(this_obj) = _this.as_object()
                && let Ok(l_val) = this_obj.get(js_string!("__listeners"), ctx)
                && let Some(l_obj) = l_val.as_object()
            {
                let _ = l_obj.set(
                    JsString::from(event_type.as_str()),
                    JsValue::Null,
                    true,
                    ctx,
                );
            }
            Ok(JsValue::undefined())
        })
    };

    let doc_dispatch_event_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event = args.first().cloned().unwrap_or(JsValue::undefined());

            let event_type = if let Some(evt_obj) = event.as_object() {
                evt_obj
                    .get(js_string!("type"), ctx)
                    .ok()
                    .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
                    .unwrap_or_default()
            } else if let Some(s) = event.as_string() {
                s.to_std_string_escaped()
            } else {
                return Ok(JsValue::from(true));
            };

            if let Some(this_obj) = _this.as_object()
                && let Ok(l_val) = this_obj.get(js_string!("__listeners"), ctx)
                && let Some(l_obj) = l_val.as_object()
            {
                let arr_val = l_obj
                    .get(JsString::from(event_type.as_str()), ctx)
                    .unwrap_or(JsValue::Null);
                if let Some(arr_obj) = arr_val.as_object()
                    && let Ok(arr) = JsArray::from_object(arr_obj.clone())
                    && let Ok(len) = arr.length(ctx)
                {
                    for i in 0..len {
                        if let Ok(cb) = arr.at(i as i64, ctx)
                            && let Some(cb_obj) = cb.as_object()
                            && cb_obj.is_callable()
                        {
                            let _ = cb_obj.call(_this, std::slice::from_ref(&event), ctx);
                        }
                    }
                }
            }

            Ok(JsValue::from(true))
        })
    };

    // document.body / document.head / document.documentElement getters
    let dom_snap_body = dom_snapshot.clone();
    let dom_snap_body_clone = dom_snapshot.clone();
    let body_getter_fn = {
        let mutations_clone = mutations.clone();
        let getter: NativeFunction = unsafe {
            NativeFunction::from_closure(move |_this, _args, ctx| {
                let snap = dom_snap_body.read();
                if let Some(ref s) = *snap
                    && let Some(bid) = s.body_id
                    && let Some(node) = s.nodes.get(&bid)
                {
                    return Ok(create_element_object(
                        s,
                        node,
                        ctx,
                        &mutations_clone,
                        &dom_snap_body_clone,
                    ));
                }
                Ok(JsValue::null())
            })
        };
        FunctionObjectBuilder::new(ctx.realm(), getter)
            .name(js_string!("get body"))
            .build()
    };

    let dom_snap_head = dom_snapshot.clone();
    let dom_snap_head_clone = dom_snapshot.clone();
    let head_getter_fn = {
        let mutations_clone = mutations.clone();
        let getter: NativeFunction = unsafe {
            NativeFunction::from_closure(move |_this, _args, ctx| {
                let snap = dom_snap_head.read();
                if let Some(ref s) = *snap
                    && let Some(hid) = s.head_id
                    && let Some(node) = s.nodes.get(&hid)
                {
                    return Ok(create_element_object(
                        s,
                        node,
                        ctx,
                        &mutations_clone,
                        &dom_snap_head_clone,
                    ));
                }
                Ok(JsValue::null())
            })
        };
        FunctionObjectBuilder::new(ctx.realm(), getter)
            .name(js_string!("get head"))
            .build()
    };

    let dom_snap_de = dom_snapshot.clone();
    let document_element_getter_fn = {
        let mutations_clone = mutations.clone();
        let getter: NativeFunction = unsafe {
            NativeFunction::from_closure(move |_this, _args, ctx| {
                let snap = dom_snap_de.read();
                if let Some(ref s) = *snap {
                    // document.documentElement should be the <html> element,
                    // which is a child of the root Document node.
                    let html_node = s.nodes.get(&s.root_id).and_then(|root| {
                        root.children.iter().find_map(|&child_id| {
                            s.nodes.get(&child_id).and_then(|n| {
                                if n.tag == "html" {
                                    Some((child_id, n))
                                } else {
                                    None
                                }
                            })
                        })
                    });
                    if let Some((_, node)) = html_node {
                        return Ok(create_element_object(
                            s,
                            node,
                            ctx,
                            &mutations_clone,
                            &dom_snap_de,
                        ));
                    }
                }
                Ok(JsValue::null())
            })
        };
        FunctionObjectBuilder::new(ctx.realm(), getter)
            .name(js_string!("get documentElement"))
            .build()
    };

    // === document.write() ===
    let dw_snap = dom_snapshot.clone();
    let dw_mut = mutations.clone();
    let doc_write_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let html = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            if html.is_empty() {
                return Ok(JsValue::undefined());
            }

            // Parse HTML fragment into text nodes and append to body
            let mut dom = dw_snap.write();
            if let Some(ref mut snap) = *dom {
                let body_id = match snap.body_id {
                    Some(id) => id,
                    None => return Ok(JsValue::undefined()),
                };

                // Generate a new node ID
                let max_id = snap.nodes.keys().max().copied().unwrap_or(0);
                let new_id = max_id + 1;

                // Create a text node with the raw HTML content
                // (Full HTML fragment parsing requires html5ever tree builder —
                //  for now, insert as a single text node)
                let node = DomNode {
                    id: new_id,
                    tag: String::new(),
                    attributes: HashMap::new(),
                    text_content: html.clone(),
                    children: Vec::new(),
                    parent: Some(body_id),
                    node_type: 3, // TEXT_NODE
                };
                snap.nodes.insert(new_id, node);

                // Append to body's children
                if let Some(body) = snap.nodes.get_mut(&body_id) {
                    body.children.push(new_id);
                }

                dw_mut.write().push(DomMutation::AppendChild {
                    parent_id: body_id,
                    child_id: new_id,
                });
            }

            Ok(JsValue::undefined())
        })
    };

    // === DOM Mutation: createElement ===
    let dom_snap_ce = dom_snapshot.clone();
    let mutations_ce = mutations.clone();
    let create_element_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let tag = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            if tag.is_empty() {
                return Ok(JsValue::undefined());
            }

            // Generate a unique node ID using an atomic counter (avoids collisions in tight loops)
            let new_id = NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed) as u32;

            let tag_upper = tag.to_uppercase();

            // Create DomNode in snapshot
            {
                let mut dom = dom_snap_ce.write();
                if let Some(ref mut snap) = *dom {
                    let node = DomNode {
                        id: new_id,
                        tag: tag.clone(),
                        attributes: HashMap::new(),
                        text_content: String::new(),
                        children: Vec::new(),
                        parent: None,
                        node_type: 1,
                    };
                    snap.nodes.insert(new_id, node);
                }
            }

            // Record mutation
            mutations_ce.write().push(DomMutation::CreateElement {
                node_id: new_id,
                tag: tag.clone(),
            });

            // Build a JS element object
            let tag_for_obj = tag_upper.clone();
            let id_for_obj = new_id;
            // Shared attribute map so getAttribute sees setAttribute mutations
            let attrs_map: Arc<parking_lot::RwLock<HashMap<String, String>>> =
                Arc::new(parking_lot::RwLock::new(HashMap::new()));
            let dom_snap_el = dom_snap_ce.clone();
            let mutations_el = mutations_ce.clone();

            // setAttribute for this element
            let mut_set_attr = mutations_el.clone();
            let mut_set_id = id_for_obj;
            let attrs_for_set = attrs_map.clone();
            let dom_snap_for_setattr = dom_snap_el.clone();
            let set_attr_fn = {
                NativeFunction::from_closure(move |_this, args, _ctx| {
                    let name = args
                        .first()
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let value = args
                        .get(1)
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    // Update shared attribute map so getAttribute sees the change
                    attrs_for_set.write().insert(name.clone(), value.clone());
                    // Sync to snapshot so querySelector can find the attribute
                    {
                        let mut dom = dom_snap_for_setattr.write();
                        if let Some(ref mut snap) = *dom
                            && let Some(node) = snap.nodes.get_mut(&mut_set_id)
                        {
                            node.attributes.insert(name.clone(), value.clone());
                        }
                    }
                    mut_set_attr.write().push(DomMutation::SetAttribute {
                        node_id: mut_set_id,
                        name,
                        value,
                    });
                    Ok(JsValue::undefined())
                })
            };

            // getAttribute for this element — reads from shared Arc<RwLock<HashMap>>
            let attrs_for_get = attrs_map.clone();
            let get_attr_fn = {
                NativeFunction::from_closure(move |_this, args, _ctx| {
                    let name = args
                        .first()
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    match attrs_for_get.read().get(&name) {
                        Some(v) => Ok(JsValue::from(JsString::from(v.as_str()))),
                        None => Ok(JsValue::null()),
                    }
                })
            };

            // click for this element
            let mut_click = mutations_el.clone();
            let click_id = id_for_obj;
            let click_fn = {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    mut_click
                        .write()
                        .push(DomMutation::ClickElement { node_id: click_id });
                    Ok(JsValue::undefined())
                })
            };

            // appendChild for this element
            let dom_snap_ac = dom_snap_el.clone();
            let parent_id_ac = id_for_obj;
            let append_child_fn = {
                NativeFunction::from_closure(move |_this, args, ctx| {
                    let child = args.first().cloned().unwrap_or(JsValue::undefined());
                    let child_id = child
                        .as_object()
                        .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
                        .and_then(|v| v.as_number().map(|n| n as u32));

                    if let Some(cid) = child_id {
                        // Update snapshot
                        {
                            let mut dom = dom_snap_ac.write();
                            if let Some(ref mut snap) = *dom {
                                // Add child to parent's children list
                                if let Some(parent) = snap.nodes.get_mut(&parent_id_ac)
                                    && !parent.children.contains(&cid)
                                {
                                    parent.children.push(cid);
                                }
                                // Set child's parent
                                if let Some(child_node) = snap.nodes.get_mut(&cid) {
                                    child_node.parent = Some(parent_id_ac);
                                }
                            }
                        }
                        // Notify MutationObservers
                        notify_mutation_observers(ctx, "childList", parent_id_ac);
                    }

                    Ok(child)
                })
            };

            // 생성된 노드를 snapshot에서 찾아 create_element_object로 완전한 요소 생성
            // 이렇게 하면 새 요소도 style, classList, cloneNode, remove 등 모든 메서드를 가짐
            let dom = dom_snap_ce.read();
            if let Some(ref snap) = *dom
                && let Some(new_node) = snap.nodes.get(&new_id)
            {
                return Ok(create_element_object(
                    snap,
                    new_node,
                    ctx,
                    &mutations_ce,
                    &dom_snap_ce,
                ));
            }
            // fallback: snapshot에서 못 찾으면 기본 객체 반환
            let obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(
                    js_string!("tagName"),
                    JsValue::from(JsString::from(tag_for_obj.as_str())),
                    Attribute::all(),
                )
                .property(
                    js_string!("nodeName"),
                    JsValue::from(JsString::from(tag_for_obj.as_str())),
                    Attribute::all(),
                )
                .property(
                    js_string!("textContent"),
                    JsValue::from(JsString::from("")),
                    Attribute::all(),
                )
                .property(
                    js_string!("id"),
                    JsValue::from(JsString::from("")),
                    Attribute::all(),
                )
                .property(
                    js_string!("className"),
                    JsValue::from(JsString::from("")),
                    Attribute::all(),
                )
                .property(
                    js_string!("__nodeId"),
                    JsValue::from(id_for_obj),
                    Attribute::all(),
                )
                .function(get_attr_fn, js_string!("getAttribute"), 1)
                .function(set_attr_fn, js_string!("setAttribute"), 2)
                .function(click_fn, js_string!("click"), 0)
                .function(append_child_fn, js_string!("appendChild"), 1)
                .build();
            Ok(JsValue::from(obj))
        })
    };

    // === DOM Mutation: createTextNode ===
    let dom_snap_ct = dom_snapshot.clone();
    let mutations_ct = mutations.clone();
    let create_text_node_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let text = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let new_id = NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed) as u32;

            {
                let mut dom = dom_snap_ct.write();
                if let Some(ref mut snap) = *dom {
                    let node = DomNode {
                        id: new_id,
                        tag: String::new(),
                        attributes: HashMap::new(),
                        text_content: text.clone(),
                        children: Vec::new(),
                        parent: None,
                        node_type: 3,
                    };
                    snap.nodes.insert(new_id, node);
                }
            }

            mutations_ct.write().push(DomMutation::CreateTextNode {
                node_id: new_id,
                text: text.clone(),
            });

            let obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(
                    js_string!("textContent"),
                    JsValue::from(JsString::from(text.as_str())),
                    Attribute::all(),
                )
                .property(js_string!("nodeType"), JsValue::from(3), Attribute::all())
                .property(
                    js_string!("__nodeId"),
                    JsValue::from(new_id),
                    Attribute::all(),
                )
                .build();

            Ok(JsValue::from(obj))
        })
    };

    let document_obj = boa_engine::object::ObjectInitializer::new(ctx)
        .accessor(
            js_string!("title"),
            Some(title_getter_fn),
            None,
            Attribute::all(),
        )
        .accessor(
            js_string!("URL"),
            Some(url_getter_fn),
            None,
            Attribute::all(),
        )
        .accessor(
            js_string!("cookie"),
            Some(cookie_getter_fn),
            Some(cookie_setter_fn),
            Attribute::all(),
        )
        .function(query_selector_fn, js_string!("querySelector"), 1)
        .function(query_selector_all_fn, js_string!("querySelectorAll"), 1)
        .function(get_element_by_id_fn, js_string!("getElementById"), 1)
        .function(
            get_elements_by_tag_name_fn,
            js_string!("getElementsByTagName"),
            1,
        )
        .function(
            get_elements_by_class_name_fn,
            js_string!("getElementsByClassName"),
            1,
        )
        .function(doc_add_event_listener_fn, js_string!("addEventListener"), 2)
        .function(
            doc_remove_event_listener_fn,
            js_string!("removeEventListener"),
            2,
        )
        .function(doc_dispatch_event_fn, js_string!("dispatchEvent"), 1)
        .function(create_element_fn, js_string!("createElement"), 1)
        .function(create_text_node_fn, js_string!("createTextNode"), 1)
        .function(doc_write_fn, js_string!("write"), 1)
        // DOM tree accessors
        .accessor(
            js_string!("body"),
            Some(body_getter_fn.clone()),
            None,
            Attribute::all(),
        )
        .accessor(
            js_string!("head"),
            Some(head_getter_fn),
            None,
            Attribute::all(),
        )
        .accessor(
            js_string!("documentElement"),
            Some(document_element_getter_fn),
            None,
            Attribute::all(),
        )
        // activeElement — same as body (no real focus tracking yet)
        .accessor(
            js_string!("activeElement"),
            Some(body_getter_fn),
            None,
            Attribute::all(),
        )
        // elementFromPoint(x, y) — returns element at viewport coordinates.
        // Approximation: finds the Nth visible element by DOM order.
        // We approximate Y positions using estimated line heights.
        .function(
            {
                let snap_efp = dom_snapshot.clone();
                let mutations_efp = mutations.clone();
                let dom_efp = dom_snapshot.clone();
                unsafe {
                    let fn_ptr: NativeFunction =
                        NativeFunction::from_closure(move |_this, args, ctx| {
                            // elementFromPoint(x, y) — approximate element lookup.
                            //
                            // Since there is no real layout engine, we estimate Y positions
                            // from DOM order using tag-based height heuristics. X is used to
                            // narrow down among children at a given depth: if a parent element
                            // has multiple visible children at the estimated Y band, we pick
                            // the child whose index corresponds to X / (viewport_width / num_children).
                            let x = args
                                .first()
                                .and_then(|v| v.to_number(ctx).ok())
                                .unwrap_or(0.0);
                            let y = args
                                .get(1)
                                .and_then(|v| v.to_number(ctx).ok())
                                .unwrap_or(0.0);
                            let snap = snap_efp.read();
                            if let Some(ref s) = *snap
                                && let Some(bid) = s.body_id
                                && let Some(body) = s.nodes.get(&bid)
                            {
                                // Walk body children in order, estimate Y positions
                                let mut estimated_y = 0.0;
                                let mut last_visible_el: Option<&DomNode> = None;
                                for &child_id in &body.children {
                                    if let Some(el) = s.nodes.get(&child_id) {
                                        let el_h = estimate_element_height(el);
                                        if el_h <= 0.0 {
                                            continue; // skip invisible elements
                                        }
                                        if y >= estimated_y && y < estimated_y + el_h {
                                            // Found the approximate Y band.
                                            // If this element has visible children, try to
                                            // narrow down using X coordinate.
                                            let visible_children: Vec<u32> = el
                                                .children
                                                .iter()
                                                .filter(|&&cid| {
                                                    s.nodes
                                                        .get(&cid)
                                                        .map(|c| estimate_element_height(c) > 0.0)
                                                        .unwrap_or(false)
                                                })
                                                .copied()
                                                .collect();

                                            if !visible_children.is_empty() {
                                                // Estimate viewport width (fallback 1280).
                                                // TODO: pass actual viewport from the runtime config.
                                                let vp_w: f64 = 1280.0;
                                                // Pick child based on X position
                                                let idx = ((x / vp_w)
                                                    * visible_children.len() as f64)
                                                    .floor()
                                                    as usize;
                                                let idx = idx.min(visible_children.len() - 1);
                                                if let Some(&picked_id) = visible_children.get(idx)
                                                    && let Some(picked) = s.nodes.get(&picked_id)
                                                {
                                                    return Ok(create_element_object(
                                                        s,
                                                        picked,
                                                        ctx,
                                                        &mutations_efp,
                                                        &dom_efp,
                                                    ));
                                                }
                                            }

                                            // No suitable children — return this element
                                            return Ok(create_element_object(
                                                s,
                                                el,
                                                ctx,
                                                &mutations_efp,
                                                &dom_efp,
                                            ));
                                        }
                                        estimated_y += el_h;
                                        last_visible_el = Some(el);
                                    }
                                }
                                // If y exceeds all estimated heights, return the last visible element
                                if let Some(el) = last_visible_el {
                                    return Ok(create_element_object(
                                        s,
                                        el,
                                        ctx,
                                        &mutations_efp,
                                        &dom_efp,
                                    ));
                                }
                                // Fallback: return body itself
                                return Ok(create_element_object(
                                    s,
                                    body,
                                    ctx,
                                    &mutations_efp,
                                    &dom_efp,
                                ));
                            }
                            Ok(JsValue::null())
                        });
                    fn_ptr
                }
            },
            js_string!("elementFromPoint"),
            2,
        )
        .build();

    let _ = ctx.register_global_property(js_string!("document"), document_obj, Attribute::all());
}

/// Create a JS element object from a DomNode.
fn create_element_object(
    snapshot: &DomSnapshot,
    node: &DomNode,
    ctx: &mut Context,
    mutations: &Arc<RwLock<Vec<DomMutation>>>,
    dom_snapshot_arc: &Arc<RwLock<Option<DomSnapshot>>>,
) -> JsValue {
    let tag_upper = node.tag.to_uppercase();
    let href_val = node
        .attributes
        .get("href")
        .map(|s| s.as_str())
        .unwrap_or("");
    let src_val = node.attributes.get("src").map(|s| s.as_str()).unwrap_or("");

    // Inject data-oxi-node-id into attributes so that
    // Runtime.callFunctionOn can resolve nodes via querySelector.
    // We add it to the cloned attribute map so getAttribute/hasAttribute
    // can also see it.
    let mut enriched_attrs: HashMap<String, String> = node.attributes.clone();
    enriched_attrs.insert("data-oxi-node-id".to_string(), node.id.to_string());

    // getAttribute(name)
    // getAttribute(name) — reads from live snapshot (reflects setAttribute mutations)
    let dom_snap_ga = dom_snapshot_arc.clone();
    let node_id_ga = node.id;
    let get_attribute_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let name = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            // 읽기 전용 snapshot에서 attribute 조회 (setAttribute가 snapshot에 반영됨)
            let dom = dom_snap_ga.read();
            if let Some(ref snap) = *dom
                && let Some(n) = snap.nodes.get(&node_id_ga)
                && let Some(val) = n.attributes.get(&name)
            {
                return Ok(JsValue::from(JsString::from(val.as_str())));
            }
            Ok(JsValue::null())
        })
    };

    // hasAttribute(name) — reads from live snapshot
    let dom_snap_ha = dom_snapshot_arc.clone();
    let node_id_ha = node.id;
    let has_attribute_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let name = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let dom = dom_snap_ha.read();
            if let Some(ref snap) = *dom
                && let Some(n) = snap.nodes.get(&node_id_ha)
            {
                return Ok(JsValue::from(n.attributes.contains_key(&name)));
            }
            Ok(JsValue::from(false))
        })
    };

    // addEventListener — stores callback by event type on the JS object itself.
    // We use a hidden `__listeners` property: { "click": [fn1, fn2], "DOMContentLoaded": [fn3] }
    let node_id_ael = node.id;
    let add_event_listener_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let callback = args.get(1).cloned().unwrap_or(JsValue::undefined());

            if callback.is_undefined() || callback.is_null() {
                return Ok(JsValue::undefined());
            }

            let this_obj = match _this.as_object() {
                Some(o) => o,
                None => return Ok(JsValue::undefined()),
            };
            // Ensure __listeners exists
            let lv = this_obj
                .get(js_string!("__listeners"), ctx)
                .unwrap_or(JsValue::Null);
            if lv.as_object().is_none() {
                let obj = boa_engine::object::ObjectInitializer::new(ctx).build();
                let _ = this_obj.set(js_string!("__listeners"), JsValue::from(obj), true, ctx);
            }
            let listeners_val2 = this_obj
                .get(js_string!("__listeners"), ctx)
                .unwrap_or(JsValue::Null);
            let listeners_obj = match listeners_val2.as_object() {
                Some(o) => o,
                None => return Ok(JsValue::undefined()),
            };
            // Ensure array for this event type
            let arr_key = JsString::from(event_type.as_str());
            let ev = listeners_obj
                .get(arr_key.clone(), ctx)
                .unwrap_or(JsValue::Null);
            if ev.as_object().is_none() {
                let a: JsValue = JsValue::from(JsArray::new(ctx));
                let _ = listeners_obj.set(arr_key.clone(), a, true, ctx);
            }
            let arr_val = listeners_obj.get(arr_key, ctx).unwrap_or(JsValue::Null);
            if let Some(arr_obj) = arr_val.as_object()
                && let Ok(arr) = JsArray::from_object(arr_obj.clone())
            {
                let _ = arr.push(callback.clone(), ctx);
            }

            // Also store in the nodeId-keyed registry so bubbling can find
            // listeners registered through any element object instance.
            if let Some(cb_obj) = callback.as_object() {
                registry_add(node_id_ael, &event_type, cb_obj.clone());
            }

            Ok(JsValue::undefined())
        })
    };

    // removeEventListener — removes callback from __listeners
    let node_id_rel = node.id;
    let remove_event_listener_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = args
                .first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let _callback = args.get(1);

            let this_obj = _this.as_object().unwrap();
            let listeners = this_obj.get(js_string!("__listeners"), ctx);
            if let Ok(l_val) = listeners
                && let Some(l_obj) = l_val.as_object()
            {
                let _ = l_obj.set(
                    JsString::from(event_type.as_str()),
                    JsValue::Null,
                    true,
                    ctx,
                );
            }

            // Also remove from the nodeId-keyed registry.
            registry_remove(node_id_rel, &event_type);

            Ok(JsValue::undefined())
        })
    };

    // dispatchEvent — calls all registered callbacks for the event type
    let node_id_disp = node.id;
    let snap_disp = dom_snapshot_arc.clone();
    let dispatch_event_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event = args.first().cloned().unwrap_or(JsValue::undefined());

            // Need a valid event object with a "type" property
            let Some(evt_obj) = event.as_object() else {
                return Ok(JsValue::from(true));
            };

            let event_type = evt_obj
                .get(js_string!("type"), ctx)
                .ok()
                .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
                .unwrap_or_default();

            // Set target/currentTarget before any listener runs
            let _ = evt_obj.set(js_string!("target"), _this.clone(), true, ctx);
            let _ = evt_obj.set(js_string!("currentTarget"), _this.clone(), true, ctx);

            let this_obj = _this.as_object().unwrap();
            let listeners = this_obj.get(js_string!("__listeners"), ctx);
            if let Ok(l_val) = listeners
                && let Some(l_obj) = l_val.as_object()
            {
                let arr_val = l_obj
                    .get(JsString::from(event_type.as_str()), ctx)
                    .unwrap_or(JsValue::Null);
                if let Some(arr_obj) = arr_val.as_object()
                    && let Ok(arr) = JsArray::from_object(arr_obj.clone())
                    && let Ok(len) = arr.length(ctx)
                {
                    for i in 0..len {
                        // Check stopImmediatePropagation before each callback
                        if evt_obj
                            .get(js_string!("__stopImmediatePropagation"), ctx)
                            .ok()
                            .and_then(|v| v.as_boolean())
                            .unwrap_or(false)
                        {
                            break;
                        }

                        if let Ok(cb) = arr.at(i as i64, ctx)
                            && let Some(cb_obj) = cb.as_object()
                            && cb_obj.is_callable()
                        {
                            let evt_arg = event.clone();
                            let _ = cb_obj.call(_this, &[evt_arg], ctx);
                        }
                    }
                }
            }

            // === Bubbling phase ===
            // Walk the DomSnapshot parent chain (not JS parentNode stubs,
            // which lack __listeners). For each ancestor nodeId, look up
            // listeners in the thread-local registry.
            let bubbles = evt_obj
                .get(js_string!("bubbles"), ctx)
                .ok()
                .and_then(|v| v.as_boolean())
                .unwrap_or(true);
            if bubbles {
                // Build the ancestor nodeId chain from the snapshot.
                let mut ancestor_ids: Vec<u32> = Vec::new();
                let snap = snap_disp.read();
                if let Some(ref s) = *snap {
                    let mut current = s.nodes.get(&node_id_disp).and_then(|n| n.parent);
                    while let Some(pid) = current {
                        // Stop at document nodes (type 9) — they use a
                        // separate listener system on the document object.
                        match s.nodes.get(&pid) {
                            Some(pn) if pn.node_type == 1 => {
                                ancestor_ids.push(pid);
                                current = pn.parent;
                            }
                            _ => break,
                        }
                    }
                }
                drop(snap);

                for aid in ancestor_ids {
                    // Check stopPropagation
                    if evt_obj
                        .get(js_string!("__stopPropagation"), ctx)
                        .ok()
                        .and_then(|v| v.as_boolean())
                        .unwrap_or(false)
                    {
                        break;
                    }

                    // Look up listeners from the registry (NOT from JS
                    // __listeners, which lives on a different object instance).
                    let callbacks = registry_get(aid, &event_type);
                    if callbacks.is_empty() {
                        continue;
                    }

                    // Build a minimal ancestor JS object for currentTarget.
                    let ancestor_obj = boa_engine::object::ObjectInitializer::new(ctx)
                        .property(js_string!("__nodeId"), JsValue::from(aid), Attribute::all())
                        .build();
                    let _ = evt_obj.set(
                        js_string!("currentTarget"),
                        JsValue::from(ancestor_obj.clone()),
                        true,
                        ctx,
                    );

                    let ancestor_val = JsValue::from(ancestor_obj);
                    for cb in &callbacks {
                        if evt_obj
                            .get(js_string!("__stopImmediatePropagation"), ctx)
                            .ok()
                            .and_then(|v| v.as_boolean())
                            .unwrap_or(false)
                        {
                            break;
                        }
                        let _ = cb.call(&ancestor_val, std::slice::from_ref(&event), ctx);
                    }
                }
            }

            let prevented = evt_obj
                .get(js_string!("defaultPrevented"), ctx)
                .ok()
                .and_then(|v| v.as_boolean())
                .unwrap_or(false);

            Ok(JsValue::from(!prevented))
        })
    };

    // click() → fires JS event handlers + records DomMutation::ClickElement
    let node_id_click = node.id;
    let mutations_click = mutations.clone();
    let click_fn = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            // 1. mutation 기록
            mutations_click.write().push(DomMutation::ClickElement {
                node_id: node_id_click,
            });
            // 2. __listeners에서 click 핸들러 찾아서 실행
            if let Some(this_obj) = _this.as_object()
                && let Ok(listeners_val) = this_obj.get(js_string!("__listeners"), ctx)
                && let Some(listeners_obj) = listeners_val.as_object()
                && let Ok(arr_val) = listeners_obj.get(js_string!("click"), ctx)
                && let Some(arr_js) = arr_val.as_object()
                && let Ok(arr) = JsArray::from_object(arr_js.clone())
            {
                let len = arr.length(ctx).unwrap_or(0) as usize;
                let event_obj = boa_engine::object::ObjectInitializer::new(ctx)
                    .property(
                        js_string!("type"),
                        JsValue::from(JsString::from("click")),
                        Attribute::all(),
                    )
                    .property(js_string!("target"), _this.clone(), Attribute::all())
                    .property(js_string!("currentTarget"), _this.clone(), Attribute::all())
                    .property(js_string!("bubbles"), JsValue::from(true), Attribute::all())
                    .build();
                for i in 0..len {
                    if let Ok(cb) = arr.get(i as u64, ctx)
                        && let Some(cb_obj) = cb.as_object()
                    {
                        let _ = cb_obj.call(_this, &[JsValue::from(event_obj.clone())], ctx);
                    }
                }
            }
            Ok(JsValue::undefined())
        })
    };

    // setAttribute(name, value) → records DomMutation::SetAttribute
    let node_id_sa = node.id;
    let mutations_sa = mutations.clone();
    let dom_snap_sa = dom_snapshot_arc.clone();
    let set_attribute_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let name = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let value = args
                .get(1)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            // Sync to snapshot so querySelector can find the attribute
            {
                let mut dom = dom_snap_sa.write();
                if let Some(ref mut snap) = *dom
                    && let Some(node) = snap.nodes.get_mut(&node_id_sa)
                {
                    node.attributes.insert(name.clone(), value.clone());
                }
            }
            mutations_sa.write().push(DomMutation::SetAttribute {
                node_id: node_id_sa,
                name,
                value,
            });
            Ok(JsValue::undefined())
        })
    };

    // appendChild — update DomSnapshot parent/child relationships
    let node_id_ac = node.id;
    let dom_snap_ac = dom_snapshot_arc.clone();
    let mutations_ac = mutations.clone();
    let append_child_obj_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let child = args.first().cloned().unwrap_or(JsValue::undefined());
            let child_id = child
                .as_object()
                .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
                .and_then(|v| v.as_number().map(|n| n as u32));

            if let Some(cid) = child_id {
                {
                    let mut dom = dom_snap_ac.write();
                    if let Some(ref mut snap) = *dom {
                        if let Some(parent) = snap.nodes.get_mut(&node_id_ac)
                            && !parent.children.contains(&cid)
                        {
                            parent.children.push(cid);
                        }
                        if let Some(child_node) = snap.nodes.get_mut(&cid) {
                            child_node.parent = Some(node_id_ac);
                        }
                    }
                }
                mutations_ac.write().push(DomMutation::AppendChild {
                    parent_id: node_id_ac,
                    child_id: cid,
                });
                // Notify MutationObservers
                notify_mutation_observers(ctx, "childList", node_id_ac);
            }
            Ok(child)
        })
    };

    // removeChild — remove child from parent
    let node_id_rc = node.id;
    let dom_snap_rc = dom_snapshot_arc.clone();
    let mutations_rc = mutations.clone();
    let remove_child_obj_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let child = args.first().cloned().unwrap_or(JsValue::undefined());
            let child_id = child
                .as_object()
                .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
                .and_then(|v| v.as_number().map(|n| n as u32));

            if let Some(cid) = child_id {
                {
                    let mut dom = dom_snap_rc.write();
                    if let Some(ref mut snap) = *dom {
                        if let Some(parent) = snap.nodes.get_mut(&node_id_rc) {
                            parent.children.retain(|&id| id != cid);
                        }
                        if let Some(child_node) = snap.nodes.get_mut(&cid) {
                            child_node.parent = None;
                        }
                    }
                }
                mutations_rc.write().push(DomMutation::RemoveChild {
                    parent_id: node_id_rc,
                    child_id: cid,
                });
                // Notify MutationObservers
                notify_mutation_observers(ctx, "childList", node_id_rc);
            }
            Ok(child)
        })
    };

    // element.querySelector(selector)
    let qs_dom = dom_snapshot_arc.clone();
    let qs_mutations = mutations.clone();
    let qs_root_id = node.id;
    let element_qs_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let selector = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = qs_dom.read();
            if let Some(ref snapshot) = *dom
                && let Some(match_id) = snapshot.query_selector_from(qs_root_id, &selector)
                && let Some(match_node) = snapshot.nodes.get(&match_id)
            {
                return Ok(create_element_object(
                    snapshot,
                    match_node,
                    ctx,
                    &qs_mutations,
                    &qs_dom,
                ));
            }
            Ok(JsValue::null())
        })
    };

    // element.querySelectorAll(selector)
    let qsa_dom = dom_snapshot_arc.clone();
    let qsa_mutations = mutations.clone();
    let qsa_root_id = node.id;
    let element_qsa_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let selector = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = qsa_dom.read();
            if let Some(ref snapshot) = *dom {
                let ids = snapshot.query_selector_all_from(qsa_root_id, &selector);
                let js_values: Vec<JsValue> = ids
                    .iter()
                    .filter_map(|&id| {
                        snapshot.nodes.get(&id).map(|n| {
                            create_element_object(snapshot, n, ctx, &qsa_mutations, &qsa_dom)
                        })
                    })
                    .collect();
                let arr = JsArray::from_iter(js_values, ctx);
                return Ok(arr.into());
            }
            let arr = JsArray::from_iter(Vec::<JsValue>::new(), ctx);
            Ok(arr.into())
        })
    };

    // ── 트리 탐색 접근자 (firstChild, lastChild, nextSibling, previousSibling) ──

    let snap_fc = dom_snapshot_arc.clone();
    let nid_fc = node.id;
    let mut_fc = mutations.clone();
    let first_child_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let dom = snap_fc.read();
            if let Some(ref s) = *dom
                && let Some(fid) = s.first_child(nid_fc)
                && let Some(c) = s.nodes.get(&fid)
            {
                return Ok(create_element_object(s, c, ctx, &mut_fc, &snap_fc));
            }
            Ok(JsValue::null())
        })
    };
    let first_child_getter_fn = FunctionObjectBuilder::new(ctx.realm(), first_child_getter)
        .name(js_string!("get firstChild"))
        .build();

    let snap_lc = dom_snapshot_arc.clone();
    let nid_lc = node.id;
    let mut_lc = mutations.clone();
    let last_child_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let dom = snap_lc.read();
            if let Some(ref s) = *dom
                && let Some(lid) = s.last_child(nid_lc)
                && let Some(c) = s.nodes.get(&lid)
            {
                return Ok(create_element_object(s, c, ctx, &mut_lc, &snap_lc));
            }
            Ok(JsValue::null())
        })
    };
    let last_child_getter_fn = FunctionObjectBuilder::new(ctx.realm(), last_child_getter)
        .name(js_string!("get lastChild"))
        .build();

    let snap_ns = dom_snapshot_arc.clone();
    let nid_ns = node.id;
    let mut_ns = mutations.clone();
    let next_sibling_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let dom = snap_ns.read();
            if let Some(ref s) = *dom
                && let Some(nid) = s.next_sibling(nid_ns)
                && let Some(c) = s.nodes.get(&nid)
            {
                return Ok(create_element_object(s, c, ctx, &mut_ns, &snap_ns));
            }
            Ok(JsValue::null())
        })
    };
    let next_sibling_getter_fn = FunctionObjectBuilder::new(ctx.realm(), next_sibling_getter)
        .name(js_string!("get nextSibling"))
        .build();

    let snap_ps = dom_snapshot_arc.clone();
    let nid_ps = node.id;
    let mut_ps = mutations.clone();
    let prev_sibling_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let dom = snap_ps.read();
            if let Some(ref s) = *dom
                && let Some(pid) = s.previous_sibling(nid_ps)
                && let Some(c) = s.nodes.get(&pid)
            {
                return Ok(create_element_object(s, c, ctx, &mut_ps, &snap_ps));
            }
            Ok(JsValue::null())
        })
    };
    let prev_sibling_getter_fn = FunctionObjectBuilder::new(ctx.realm(), prev_sibling_getter)
        .name(js_string!("get previousSibling"))
        .build();

    // ── 트리 조작 메서드 (insertBefore, replaceChild, removeAttribute, cloneNode, remove) ──

    let snap_ib = dom_snapshot_arc.clone();
    let nid_ib = node.id;
    let mut_ib = mutations.clone();
    let insert_before_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let new_child = args.first().cloned().unwrap_or(JsValue::undefined());
            let ref_child = args.get(1).cloned().unwrap_or(JsValue::null());
            let new_id = new_child
                .as_object()
                .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
                .and_then(|v| v.as_number().map(|n| n as u32));
            let ref_id = if ref_child.is_null() || ref_child.is_undefined() {
                None
            } else {
                ref_child
                    .as_object()
                    .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
                    .and_then(|v| v.as_number().map(|n| n as u32))
            };
            if let Some(nid) = new_id {
                let mut dom = snap_ib.write();
                if let Some(ref mut s) = *dom {
                    // 기존 부모에서 제거
                    if let Some(old_parent) = s.nodes.get(&nid).and_then(|n| n.parent)
                        && old_parent != nid_ib
                        && let Some(p) = s.nodes.get_mut(&old_parent)
                    {
                        p.children.retain(|&c| c != nid);
                    }
                    // ref_id 위치에 삽입 또는 맨 뒤에 append
                    let children = s
                        .nodes
                        .get(&nid_ib)
                        .map(|p| p.children.clone())
                        .unwrap_or_default();
                    if let Some(rid) = ref_id {
                        if let Some(pos) = children.iter().position(|&c| c == rid)
                            && let Some(p) = s.nodes.get_mut(&nid_ib)
                        {
                            p.children.retain(|&c| c != nid);
                            p.children.insert(pos, nid);
                        }
                    } else {
                        if let Some(p) = s.nodes.get_mut(&nid_ib) {
                            p.children.retain(|&c| c != nid);
                            p.children.push(nid);
                        }
                    }
                    if let Some(c) = s.nodes.get_mut(&nid) {
                        c.parent = Some(nid_ib);
                    }
                    mut_ib.write().push(DomMutation::AppendChild {
                        parent_id: nid_ib,
                        child_id: nid,
                    });
                }
            }
            Ok(new_child)
        })
    };

    let snap_rc = dom_snapshot_arc.clone();
    let nid_rc = node.id;
    let mut_rc = mutations.clone();
    let replace_child_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let new_child = args.first().cloned().unwrap_or(JsValue::undefined());
            let old_child = args.get(1).cloned().unwrap_or(JsValue::undefined());
            let new_id = new_child
                .as_object()
                .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
                .and_then(|v| v.as_number().map(|n| n as u32));
            let old_id = old_child
                .as_object()
                .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
                .and_then(|v| v.as_number().map(|n| n as u32));
            if let (Some(nid), Some(oid)) = (new_id, old_id) {
                let mut dom = snap_rc.write();
                if let Some(ref mut s) = *dom {
                    if let Some(p) = s.nodes.get_mut(&nid_rc) {
                        p.children.retain(|&c| c != oid);
                        if let Some(pos) = p.children.iter().position(|&c| c == oid) {
                            p.children.insert(pos, nid);
                        } else {
                            p.children.push(nid);
                        }
                    }
                    if let Some(c) = s.nodes.get_mut(&nid) {
                        c.parent = Some(nid_rc);
                    }
                    if let Some(o) = s.nodes.get_mut(&oid) {
                        o.parent = None;
                    }
                    mut_rc.write().push(DomMutation::RemoveChild {
                        parent_id: nid_rc,
                        child_id: oid,
                    });
                    mut_rc.write().push(DomMutation::AppendChild {
                        parent_id: nid_rc,
                        child_id: nid,
                    });
                }
            }
            Ok(new_child)
        })
    };

    let snap_ra = dom_snapshot_arc.clone();
    let nid_ra = node.id;
    let mut_ra = mutations.clone();
    let remove_attr_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let name = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            if !name.is_empty() {
                let mut dom = snap_ra.write();
                if let Some(ref mut s) = *dom
                    && let Some(n) = s.nodes.get_mut(&nid_ra)
                {
                    n.attributes.remove(&name);
                }
                mut_ra.write().push(DomMutation::SetAttribute {
                    node_id: nid_ra,
                    name,
                    value: String::new(),
                });
            }
            Ok(JsValue::undefined())
        })
    };

    let snap_rm = dom_snapshot_arc.clone();
    let nid_rm = node.id;
    let mut_rm = mutations.clone();
    let remove_fn = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let mut dom = snap_rm.write();
            if let Some(ref mut s) = *dom {
                let pid = s.nodes.get(&nid_rm).and_then(|n| n.parent);
                if let Some(pid2) = pid {
                    if let Some(p) = s.nodes.get_mut(&pid2) {
                        p.children.retain(|&c| c != nid_rm);
                    }
                    mut_rm.write().push(DomMutation::RemoveChild {
                        parent_id: pid2,
                        child_id: nid_rm,
                    });
                }
                if let Some(n) = s.nodes.get_mut(&nid_rm) {
                    n.parent = None;
                }
            }
            Ok(JsValue::undefined())
        })
    };

    let snap_cl = dom_snapshot_arc.clone();
    let nid_cl = node.id;
    let mut_cl = mutations.clone();
    let clone_node_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let deep = args.first().and_then(|v| v.as_boolean()).unwrap_or(false);
            let new_id = NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed) as u32;
            // 먼저 필요한 값들을 clone (borrow 해제 후 insert)
            let (tag, attrs, text, ntype) = {
                let dom = snap_cl.read();
                if let Some(ref s) = *dom {
                    if let Some(src) = s.nodes.get(&nid_cl) {
                        (
                            src.tag.clone(),
                            if deep {
                                src.attributes.clone()
                            } else {
                                HashMap::new()
                            },
                            if deep {
                                src.text_content.clone()
                            } else {
                                String::new()
                            },
                            src.node_type,
                        )
                    } else {
                        return Ok(JsValue::null());
                    }
                } else {
                    return Ok(JsValue::null());
                }
            };
            let cloned = DomNode {
                id: new_id,
                tag: tag.clone(),
                attributes: attrs,
                text_content: text,
                children: Vec::new(),
                parent: None,
                node_type: ntype,
            };
            {
                let mut dom = snap_cl.write();
                if let Some(ref mut s) = *dom {
                    s.nodes.insert(new_id, cloned);
                }
            }
            mut_cl.write().push(DomMutation::CreateElement {
                node_id: new_id,
                tag: tag.clone(),
            });
            let dom = snap_cl.read();
            if let Some(ref s) = *dom
                && let Some(n) = s.nodes.get(&new_id)
            {
                return Ok(create_element_object(s, n, ctx, &mut_cl, &snap_cl));
            }
            Ok(JsValue::null())
        })
    };

    // ── 스타일/클래스 접근자 (style, classList) ──
    // .function()으로 등록 — 호출 시 객체 반환

    let snap_st = dom_snapshot_arc.clone();
    let nid_st = node.id;
    let style_fn = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let sp_arc = snap_st.clone();
            let sp_id = nid_st;
            let set_fn = {
                NativeFunction::from_closure(move |_this2, args2, _ctx2| {
                    let prop = args2
                        .first()
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let val = args2
                        .get(1)
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    if !prop.is_empty() {
                        let mut dom = sp_arc.write();
                        if let Some(ref mut s) = *dom
                            && let Some(n) = s.nodes.get_mut(&sp_id)
                        {
                            n.attributes.insert(format!("style:{}", prop), val);
                        }
                    }
                    Ok(JsValue::undefined())
                })
            };
            let gp_arc = snap_st.clone();
            let gp_id = nid_st;
            let get_fn = {
                NativeFunction::from_closure(move |_this2, args2, _ctx2| {
                    let prop = args2
                        .first()
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let dom = gp_arc.read();
                    if let Some(ref s) = *dom
                        && let Some(n) = s.nodes.get(&gp_id)
                    {
                        let key = format!("style:{}", prop);
                        if let Some(v) = n.attributes.get(&key) {
                            return Ok(JsValue::from(JsString::from(v.as_str())));
                        }
                    }
                    Ok(JsValue::from(JsString::from("")))
                })
            };
            let rp_arc = snap_st.clone();
            let rp_id = nid_st;
            let rm_fn = {
                NativeFunction::from_closure(move |_this2, args2, _ctx2| {
                    let prop = args2
                        .first()
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    if !prop.is_empty() {
                        let mut dom = rp_arc.write();
                        if let Some(ref mut s) = *dom
                            && let Some(n) = s.nodes.get_mut(&rp_id)
                        {
                            let key = format!("style:{}", prop);
                            n.attributes.remove(&key);
                        }
                    }
                    Ok(JsValue::undefined())
                })
            };
            let style_obj = boa_engine::object::ObjectInitializer::new(ctx)
                .function(set_fn, js_string!("setProperty"), 2)
                .function(get_fn, js_string!("getPropertyValue"), 1)
                .function(rm_fn, js_string!("removeProperty"), 1)
                .build();
            Ok(JsValue::from(style_obj))
        })
    };

    let snap_cls = dom_snapshot_arc.clone();
    let nid_cls = node.id;
    let classlist_fn = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            // 현재 class 속성 읽기
            let current = {
                let dom = snap_cls.read();
                if let Some(ref s) = *dom {
                    if let Some(n) = s.nodes.get(&nid_cls) {
                        n.attributes.get("class").cloned().unwrap_or_default()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            };
            let count = current.split_whitespace().count() as i32;

            let ca_arc = snap_cls.clone();
            let ca_id = nid_cls;
            let add_fn = {
                NativeFunction::from_closure(move |_this2, args2, _ctx2| {
                    let cls = args2
                        .first()
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    if !cls.is_empty() {
                        let mut dom = ca_arc.write();
                        if let Some(ref mut s) = *dom
                            && let Some(n) = s.nodes.get_mut(&ca_id)
                        {
                            let cur = n.attributes.get("class").cloned().unwrap_or_default();
                            if !cur.split_whitespace().any(|c| c == cls) {
                                let new_cls = if cur.is_empty() {
                                    cls.clone()
                                } else {
                                    format!("{} {}", cur, cls)
                                };
                                n.attributes.insert("class".to_string(), new_cls);
                            }
                        }
                    }
                    Ok(JsValue::undefined())
                })
            };

            let cr_arc = snap_cls.clone();
            let cr_id = nid_cls;
            let rm_cls_fn = {
                NativeFunction::from_closure(move |_this2, args2, _ctx2| {
                    let cls = args2
                        .first()
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    if !cls.is_empty() {
                        let mut dom = cr_arc.write();
                        if let Some(ref mut s) = *dom
                            && let Some(n) = s.nodes.get_mut(&cr_id)
                        {
                            let cur = n.attributes.get("class").cloned().unwrap_or_default();
                            let new_cls = cur
                                .split_whitespace()
                                .filter(|c| *c != cls)
                                .collect::<Vec<_>>()
                                .join(" ");
                            n.attributes.insert("class".to_string(), new_cls);
                        }
                    }
                    Ok(JsValue::undefined())
                })
            };

            let ch_arc = snap_cls.clone();
            let ch_id = nid_cls;
            let has_fn = {
                NativeFunction::from_closure(move |_this2, args2, _ctx2| {
                    let cls = args2
                        .first()
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let dom = ch_arc.read();
                    if let Some(ref s) = *dom
                        && let Some(n) = s.nodes.get(&ch_id)
                    {
                        let cur = n.attributes.get("class").cloned().unwrap_or_default();
                        return Ok(JsValue::from(cur.split_whitespace().any(|c| c == cls)));
                    }
                    Ok(JsValue::from(false))
                })
            };

            let ct_arc = snap_cls.clone();
            let ct_id = nid_cls;
            let toggle_fn = {
                NativeFunction::from_closure(move |_this2, args2, _ctx2| {
                    let cls = args2
                        .first()
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let dom = ct_arc.read();
                    let mut found = false;
                    if let Some(ref s) = *dom
                        && let Some(n) = s.nodes.get(&ct_id)
                    {
                        let cur = n.attributes.get("class").cloned().unwrap_or_default();
                        found = cur.split_whitespace().any(|c| c == cls);
                    }
                    drop(dom);
                    if !cls.is_empty() {
                        let mut dom2 = ct_arc.write();
                        if let Some(ref mut s) = *dom2
                            && let Some(n) = s.nodes.get_mut(&ct_id)
                        {
                            let cur = n.attributes.get("class").cloned().unwrap_or_default();
                            let new_cls = if found {
                                cur.split_whitespace()
                                    .filter(|c| *c != cls)
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            } else {
                                if cur.is_empty() {
                                    cls.clone()
                                } else {
                                    format!("{} {}", cur, cls)
                                }
                            };
                            n.attributes.insert("class".to_string(), new_cls);
                        }
                    }
                    Ok(JsValue::from(!found))
                })
            };

            let cl_obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(js_string!("length"), JsValue::from(count), Attribute::all())
                .function(add_fn, js_string!("add"), 1)
                .function(rm_cls_fn, js_string!("remove"), 1)
                .function(has_fn, js_string!("contains"), 1)
                .function(toggle_fn, js_string!("toggle"), 1)
                .build();
            Ok(JsValue::from(cl_obj))
        })
    };

    // style/classList를 accessor로 사용하기 위해 FunctionObjectBuilder로 변환
    // (ObjectInitializer::new(ctx)가 ctx를 mutable borrow하므로 미리 변환 필요)
    let style_getter_fn = FunctionObjectBuilder::new(ctx.realm(), style_fn)
        .name(js_string!("get style"))
        .build();
    let classlist_getter_fn = FunctionObjectBuilder::new(ctx.realm(), classlist_fn)
        .name(js_string!("get classList"))
        .build();

    // ── getBoundingClientRect ──
    let gbr_dom = dom_snapshot_arc.clone();
    let gbr_id = node.id;
    let get_bounding_client_rect_fn = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let dom = gbr_dom.read();
            let snapshot = match dom.as_ref() {
                Some(s) => s,
                None => return Ok(JsValue::null()),
            };
            let rect = LayoutEngine::compute_rect(snapshot, gbr_id);
            let obj = boa_engine::object::ObjectInitializer::new(_ctx)
                .property(js_string!("x"), JsValue::from(rect.x), Attribute::all())
                .property(js_string!("y"), JsValue::from(rect.y), Attribute::all())
                .property(
                    js_string!("width"),
                    JsValue::from(rect.width),
                    Attribute::all(),
                )
                .property(
                    js_string!("height"),
                    JsValue::from(rect.height),
                    Attribute::all(),
                )
                .property(js_string!("top"), JsValue::from(rect.top), Attribute::all())
                .property(
                    js_string!("right"),
                    JsValue::from(rect.right),
                    Attribute::all(),
                )
                .property(
                    js_string!("bottom"),
                    JsValue::from(rect.bottom),
                    Attribute::all(),
                )
                .property(
                    js_string!("left"),
                    JsValue::from(rect.left),
                    Attribute::all(),
                )
                .build();
            Ok(JsValue::from(obj))
        })
    };

    // ── offsetWidth / offsetHeight ──
    let ow_dom = dom_snapshot_arc.clone();
    let ow_id = node.id;
    let offset_width_getter_raw = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let dom = ow_dom.read();
            if let Some(ref snap) = *dom {
                let rect = LayoutEngine::compute_rect(snap, ow_id);
                return Ok(JsValue::from(rect.width));
            }
            Ok(JsValue::from(0.0))
        })
    };
    let offset_width_getter = FunctionObjectBuilder::new(ctx.realm(), offset_width_getter_raw)
        .name(js_string!("get offsetWidth"))
        .build();
    let oh_dom = dom_snapshot_arc.clone();
    let oh_id = node.id;
    let offset_height_getter_raw = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let dom = oh_dom.read();
            if let Some(ref snap) = *dom {
                let rect = LayoutEngine::compute_rect(snap, oh_id);
                return Ok(JsValue::from(rect.height));
            }
            Ok(JsValue::from(0.0))
        })
    };
    let offset_height_getter = FunctionObjectBuilder::new(ctx.realm(), offset_height_getter_raw)
        .name(js_string!("get offsetHeight"))
        .build();

    // ── 포커스/폼 (noop) ──

    let focus_fn =
        unsafe { NativeFunction::from_closure(move |_this, _args, _ctx| Ok(JsValue::undefined())) };
    let blur_fn =
        unsafe { NativeFunction::from_closure(move |_this, _args, _ctx| Ok(JsValue::undefined())) };
    let submit_fn =
        unsafe { NativeFunction::from_closure(move |_this, _args, _ctx| Ok(JsValue::undefined())) };

    // value getter
    // value getter — reads from live snapshot (reflects value setter)
    let dom_snap_vg = dom_snapshot_arc.clone();
    let node_id_vg = node.id;
    let value_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let dom = dom_snap_vg.read();
            if let Some(ref snap) = *dom
                && let Some(n) = snap.nodes.get(&node_id_vg)
            {
                return Ok(JsValue::from(JsString::from(
                    n.attributes.get("value").map(|s| s.as_str()).unwrap_or(""),
                )));
            }
            Ok(JsValue::from(JsString::from("")))
        })
    };
    let value_getter_fn = FunctionObjectBuilder::new(ctx.realm(), value_getter)
        .name(js_string!("get value"))
        .build();

    // value setter → updates snapshot + records DomMutation::InputElement
    let node_id_vs = node.id;
    let mutations_vs = mutations.clone();
    let dom_snap_vs = dom_snapshot_arc.clone();
    let value_setter = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let val = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            // snapshot에 value attribute 업데이트 (getter가 즉시 반영)
            {
                let mut dom = dom_snap_vs.write();
                if let Some(ref mut snap) = *dom
                    && let Some(n) = snap.nodes.get_mut(&node_id_vs)
                {
                    n.attributes.insert("value".to_string(), val.clone());
                }
            }
            mutations_vs.write().push(DomMutation::InputElement {
                node_id: node_id_vs,
                value: val,
            });
            Ok(JsValue::undefined())
        })
    };
    let value_setter_fn = FunctionObjectBuilder::new(ctx.realm(), value_setter)
        .name(js_string!("set value"))
        .build();

    // textContent getter — reads from live snapshot
    let dom_snap_tcg = dom_snapshot_arc.clone();
    let nid_tcg = node.id;
    let text_content_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let dom = dom_snap_tcg.read();
            if let Some(ref s) = *dom
                && let Some(n) = s.nodes.get(&nid_tcg)
            {
                return Ok(JsValue::from(JsString::from(n.text_content.as_str())));
            }
            Ok(JsValue::from(JsString::from("")))
        })
    };
    let text_content_getter_fn = FunctionObjectBuilder::new(ctx.realm(), text_content_getter)
        .name(js_string!("get textContent"))
        .build();

    // textContent setter — updates snapshot + records mutation
    let dom_snap_tcs = dom_snapshot_arc.clone();
    let nid_tcs = node.id;
    let mut_tcs = mutations.clone();
    let text_content_setter = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let text = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            {
                let mut dom = dom_snap_tcs.write();
                if let Some(ref mut s) = *dom
                    && let Some(n) = s.nodes.get_mut(&nid_tcs)
                {
                    n.text_content = text.clone();
                }
            }
            mut_tcs.write().push(DomMutation::SetTextContent {
                node_id: nid_tcs,
                text,
            });
            Ok(JsValue::undefined())
        })
    };
    let text_content_setter_fn = FunctionObjectBuilder::new(ctx.realm(), text_content_setter)
        .name(js_string!("set textContent"))
        .build();

    // innerHTML getter — serializes the node's children from the live snapshot.
    let dom_snap_ihg = dom_snapshot_arc.clone();
    let nid_ihg = node.id;
    let inner_html_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let dom = dom_snap_ihg.read();
            let mut buf = String::new();
            if let Some(s) = &*dom
                && let Some(n) = s.nodes.get(&nid_ihg)
            {
                crate::js::dom_serializer::serialize_children(n, s, &mut buf);
                return Ok(JsValue::from(JsString::from(buf.as_str())));
            }
            Ok(JsValue::from(JsString::from("")))
        })
    };
    let inner_html_getter_fn = FunctionObjectBuilder::new(ctx.realm(), inner_html_getter)
        .name(js_string!("get innerHTML"))
        .build();

    // innerHTML setter — updates snapshot + records mutation
    let dom_snap_ihs = dom_snapshot_arc.clone();
    let nid_ihs = node.id;
    let mut_ihs = mutations.clone();
    let inner_html_setter = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let html = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            {
                let mut dom = dom_snap_ihs.write();
                if let Some(s) = &mut *dom {
                    s.set_inner_html(nid_ihs, &html);
                    s.rebuild_indices();
                }
            }
            mut_ihs.write().push(DomMutation::SetInnerHtml {
                node_id: nid_ihs,
                html,
            });
            Ok(JsValue::undefined())
        })
    };
    let inner_html_setter_fn = FunctionObjectBuilder::new(ctx.realm(), inner_html_setter)
        .name(js_string!("set innerHTML"))
        .build();

    // outerHTML getter — serializes the node itself (tag + attrs + children).
    // Read-only; matches browser semantics (no setter).
    let dom_snap_ohg = dom_snapshot_arc.clone();
    let nid_ohg = node.id;
    let outer_html_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let dom = dom_snap_ohg.read();
            let mut buf = String::new();
            if let Some(s) = &*dom
                && let Some(n) = s.nodes.get(&nid_ohg)
            {
                crate::js::dom_serializer::serialize_node(n, s, &mut buf);
                return Ok(JsValue::from(JsString::from(buf.as_str())));
            }
            Ok(JsValue::from(JsString::from("")))
        })
    };
    let outer_html_getter_fn = FunctionObjectBuilder::new(ctx.realm(), outer_html_getter)
        .name(js_string!("get outerHTML"))
        .build();

    // children → [element children IDs as lightweight objects]
    // Note: we avoid recursively calling create_element_object for children/parentNode
    // to prevent stack overflow on deeply nested DOMs. Instead, children get a
    // minimal stub with tagName and id.
    let child_ids = node
        .children
        .iter()
        .filter(|&&c| {
            snapshot
                .nodes
                .get(&c)
                .map(|n| n.node_type == 1)
                .unwrap_or(false)
        })
        .copied()
        .collect::<Vec<u32>>();
    let children_js: Vec<JsValue> = child_ids
        .iter()
        .filter_map(|&cid| {
            snapshot.nodes.get(&cid).map(|child| {
                let child_obj = boa_engine::object::ObjectInitializer::new(ctx)
                    .property(
                        js_string!("tagName"),
                        JsValue::from(JsString::from(child.tag.to_uppercase().as_str())),
                        Attribute::all(),
                    )
                    .property(
                        js_string!("id"),
                        JsValue::from(JsString::from(
                            child.attributes.get("id").map(|s| s.as_str()).unwrap_or(""),
                        )),
                        Attribute::all(),
                    )
                    .build();
                child_obj.into()
            })
        })
        .collect();
    let children_arr = JsArray::from_iter(children_js, ctx);

    // parentNode — stub (avoid recursion)
    let parent_val: JsValue = match node.parent {
        Some(pid) => match snapshot.nodes.get(&pid) {
            Some(pnode) if pnode.node_type == 1 => {
                let parent_obj = boa_engine::object::ObjectInitializer::new(ctx)
                    .property(
                        js_string!("tagName"),
                        JsValue::from(JsString::from(pnode.tag.to_uppercase().as_str())),
                        Attribute::all(),
                    )
                    .property(
                        js_string!("id"),
                        JsValue::from(JsString::from(
                            pnode.attributes.get("id").map(|s| s.as_str()).unwrap_or(""),
                        )),
                        Attribute::all(),
                    )
                    .build();
                parent_obj.into()
            }
            _ => JsValue::null(),
        },
        None => JsValue::null(),
    };

    // id — accessor that reads/writes from live DomSnapshot
    let snap_id = dom_snapshot_arc.clone();
    let nid_id = node.id;
    let mut_id = mutations.clone();
    let id_getter_fn = {
        let snap = snap_id.clone();
        let nid = nid_id;
        let getter = unsafe {
            NativeFunction::from_closure(move |_this, _args, _ctx| {
                let dom = snap.read();
                if let Some(ref s) = *dom
                    && let Some(n) = s.nodes.get(&nid)
                {
                    return Ok(JsValue::from(JsString::from(
                        n.attributes.get("id").map(|s| s.as_str()).unwrap_or(""),
                    )));
                }
                Ok(JsValue::from(JsString::from("")))
            })
        };
        FunctionObjectBuilder::new(ctx.realm(), getter)
            .name(js_string!("get id"))
            .build()
    };
    let id_setter_fn = {
        let snap = snap_id.clone();
        let nid = nid_id;
        let m = mut_id.clone();
        let setter = unsafe {
            NativeFunction::from_closure(move |_this, args, _ctx| {
                let value = args
                    .first()
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default();
                {
                    let mut dom = snap.write();
                    if let Some(ref mut s) = *dom
                        && let Some(n) = s.nodes.get_mut(&nid)
                    {
                        n.attributes.insert("id".to_string(), value.clone());
                    }
                }
                m.write().push(DomMutation::SetAttribute {
                    node_id: nid,
                    name: "id".to_string(),
                    value,
                });
                Ok(JsValue::undefined())
            })
        };
        FunctionObjectBuilder::new(ctx.realm(), setter)
            .name(js_string!("set id"))
            .build()
    };

    // className — accessor
    let snap_cn = dom_snapshot_arc.clone();
    let nid_cn = node.id;
    let mut_cn = mutations.clone();
    let class_getter_fn = {
        let snap = snap_cn.clone();
        let nid = nid_cn;
        let getter = unsafe {
            NativeFunction::from_closure(move |_this, _args, _ctx| {
                let dom = snap.read();
                if let Some(ref s) = *dom
                    && let Some(n) = s.nodes.get(&nid)
                {
                    return Ok(JsValue::from(JsString::from(
                        n.attributes.get("class").map(|s| s.as_str()).unwrap_or(""),
                    )));
                }
                Ok(JsValue::from(JsString::from("")))
            })
        };
        FunctionObjectBuilder::new(ctx.realm(), getter)
            .name(js_string!("get className"))
            .build()
    };
    let class_setter_fn = {
        let snap = snap_cn.clone();
        let nid = nid_cn;
        let m = mut_cn.clone();
        let setter = unsafe {
            NativeFunction::from_closure(move |_this, args, _ctx| {
                let value = args
                    .first()
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default();
                {
                    let mut dom = snap.write();
                    if let Some(ref mut s) = *dom
                        && let Some(n) = s.nodes.get_mut(&nid)
                    {
                        n.attributes.insert("class".to_string(), value.clone());
                    }
                }
                m.write().push(DomMutation::SetAttribute {
                    node_id: nid,
                    name: "class".to_string(),
                    value,
                });
                Ok(JsValue::undefined())
            })
        };
        FunctionObjectBuilder::new(ctx.realm(), setter)
            .name(js_string!("set className"))
            .build()
    };

    let obj = boa_engine::object::ObjectInitializer::new(ctx)
        .property(
            js_string!("tagName"),
            JsValue::from(JsString::from(tag_upper.as_str())),
            Attribute::all(),
        )
        .accessor(
            js_string!("textContent"),
            Some(text_content_getter_fn.clone()),
            Some(text_content_setter_fn),
            Attribute::all(),
        )
        .accessor(
            js_string!("innerText"),
            Some(text_content_getter_fn),
            None,
            Attribute::all(),
        )
        .accessor(
            js_string!("innerHTML"),
            Some(inner_html_getter_fn),
            Some(inner_html_setter_fn),
            Attribute::all(),
        )
        .accessor(
            js_string!("outerHTML"),
            Some(outer_html_getter_fn),
            None,
            Attribute::all(),
        )
        .accessor(
            js_string!("id"),
            Some(id_getter_fn),
            Some(id_setter_fn),
            Attribute::all(),
        )
        .accessor(
            js_string!("className"),
            Some(class_getter_fn),
            Some(class_setter_fn),
            Attribute::all(),
        )
        .property(
            js_string!("href"),
            JsValue::from(JsString::from(href_val)),
            Attribute::all(),
        )
        .property(
            js_string!("src"),
            JsValue::from(JsString::from(src_val)),
            Attribute::all(),
        )
        .property(
            js_string!("children"),
            JsValue::from(children_arr),
            Attribute::all(),
        )
        .property(js_string!("parentNode"), parent_val, Attribute::all())
        .function(get_attribute_fn, js_string!("getAttribute"), 1)
        .function(has_attribute_fn, js_string!("hasAttribute"), 1)
        .function(add_event_listener_fn, js_string!("addEventListener"), 2)
        .function(
            remove_event_listener_fn,
            js_string!("removeEventListener"),
            2,
        )
        .function(dispatch_event_fn, js_string!("dispatchEvent"), 1)
        .function(click_fn, js_string!("click"), 0)
        .function(set_attribute_fn, js_string!("setAttribute"), 2)
        .function(append_child_obj_fn, js_string!("appendChild"), 1)
        .function(remove_child_obj_fn, js_string!("removeChild"), 1)
        .function(element_qs_fn, js_string!("querySelector"), 1)
        .function(element_qsa_fn, js_string!("querySelectorAll"), 1)
        .function(
            {
                let snap_cn = dom_snapshot_arc.clone();
                let nid_cn = node.id;
                let mut_cn = mutations.clone();
                unsafe {
                    NativeFunction::from_closure(move |_this, _args, ctx| {
                        let dom = snap_cn.read();
                        if let Some(ref snap) = *dom
                            && let Some(cur) = snap.nodes.get(&nid_cn)
                        {
                            let items: Vec<JsValue> = cur
                                .children
                                .iter()
                                .filter_map(|&cid| snap.nodes.get(&cid))
                                .map(|child| {
                                    create_element_object(snap, child, ctx, &mut_cn, &snap_cn)
                                })
                                .collect();
                            let arr = JsArray::from_iter(items, ctx);
                            return Ok(arr.into());
                        }
                        let arr = JsArray::new(ctx);
                        Ok(arr.into())
                    })
                }
            },
            js_string!("childNodes"),
            0,
        )
        // ── 트리 탐색 접근자 ──
        .accessor(
            js_string!("firstChild"),
            Some(first_child_getter_fn),
            None,
            Attribute::all(),
        )
        .accessor(
            js_string!("lastChild"),
            Some(last_child_getter_fn),
            None,
            Attribute::all(),
        )
        .accessor(
            js_string!("nextSibling"),
            Some(next_sibling_getter_fn),
            None,
            Attribute::all(),
        )
        .accessor(
            js_string!("previousSibling"),
            Some(prev_sibling_getter_fn),
            None,
            Attribute::all(),
        )
        // ── 트리 조작 메서드 ──
        .function(insert_before_fn, js_string!("insertBefore"), 2)
        .function(replace_child_fn, js_string!("replaceChild"), 2)
        .function(remove_attr_fn, js_string!("removeAttribute"), 1)
        .function(clone_node_fn, js_string!("cloneNode"), 1)
        .function(remove_fn, js_string!("remove"), 0)
        // ── 스타일/클래스 (함수 — 호출 시 객체 반환) ──
        // style/classList accessors — el.style (not el.style())
        .accessor(
            js_string!("style"),
            Some(style_getter_fn),
            None,
            Attribute::all(),
        )
        .accessor(
            js_string!("classList"),
            Some(classlist_getter_fn),
            None,
            Attribute::all(),
        )
        // ── 레이아웃 평가 ──
        .function(
            get_bounding_client_rect_fn,
            js_string!("getBoundingClientRect"),
            0,
        )
        .accessor(
            js_string!("offsetWidth"),
            Some(offset_width_getter),
            None,
            Attribute::all(),
        )
        .accessor(
            js_string!("offsetHeight"),
            Some(offset_height_getter),
            None,
            Attribute::all(),
        )
        // ── _visible / _interactive — methods that read live from DomSnapshot ──
        .function(
            {
                let vis_dom = dom_snapshot_arc.clone();
                let vis_id = node.id;
                unsafe {
                    NativeFunction::from_closure(move |_this, _args, _ctx| {
                        let dom = vis_dom.read();
                        if let Some(ref snap) = *dom
                            && let Some(cs) = LayoutEngine::compute_style(snap, vis_id)
                        {
                            return Ok(JsValue::from(cs.visible));
                        }
                        Ok(JsValue::from(true))
                    })
                }
            },
            js_string!("_visible"),
            0,
        )
        .function(
            {
                let int_dom = dom_snapshot_arc.clone();
                let int_id = node.id;
                unsafe {
                    NativeFunction::from_closure(move |_this, _args, _ctx| {
                        let dom = int_dom.read();
                        if let Some(ref snap) = *dom
                            && let Some(cs) = LayoutEngine::compute_style(snap, int_id)
                        {
                            return Ok(JsValue::from(cs.interactive));
                        }
                        Ok(JsValue::from(false))
                    })
                }
            },
            js_string!("_interactive"),
            0,
        )
        // ── 포커스/폼 ──
        .function(focus_fn, js_string!("focus"), 0)
        .function(blur_fn, js_string!("blur"), 0)
        .function(submit_fn, js_string!("submit"), 0)
        .property(
            js_string!("__nodeId"),
            JsValue::from(node.id),
            Attribute::all(),
        )
        .accessor(
            js_string!("value"),
            Some(value_getter_fn),
            Some(value_setter_fn),
            Attribute::all(),
        )
        .build();

    // ── _visible / _interactive — live computed visibility from DomSnapshot ──
    // Define after .build() to avoid borrow conflicts with ObjectInitializer::new(ctx)
    obj.into()
}

// ---------------------------------------------------------------------------
// JsValue ↔ serde_json::Value conversions
// ---------------------------------------------------------------------------

/// Convert a serde_json Value to a boa_engine JsValue.
fn json_to_js_value(value: &Value, context: &mut Context) -> JsValue {
    use std::ops::Deref;

    match value {
        Value::Null => JsValue::null(),
        Value::Bool(b) => JsValue::from(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                JsValue::from(i as f64)
            } else {
                JsValue::from(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => JsValue::from(JsString::from(s.as_str())),
        Value::Array(arr) => {
            let js_values: Vec<JsValue> =
                arr.iter().map(|v| json_to_js_value(v, context)).collect();
            let js_arr = JsArray::from_iter(js_values, context);
            js_arr.deref().clone().into()
        }
        Value::Object(map) => {
            let pairs: Vec<(String, JsValue)> = map
                .iter()
                .map(|(k, v)| (k.clone(), json_to_js_value(v, context)))
                .collect();
            let mut obj = boa_engine::object::ObjectInitializer::new(context);
            for (k, v) in pairs {
                obj.property(JsString::from(k.as_str()), v, Attribute::all());
            }
            obj.build().into()
        }
    }
}

/// Convert a boa_engine JsValue to serde_json::Value.
fn js_value_to_json(value: &JsValue, context: &mut Context) -> Value {
    match value {
        JsValue::Null | JsValue::Undefined => Value::Null,
        JsValue::Boolean(b) => Value::Bool(*b),
        JsValue::Integer(n) => Value::Number(serde_json::Number::from(*n)),
        JsValue::Rational(n) => serde_json::Number::from_f64(*n)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        JsValue::String(s) => Value::String(s.to_std_string_escaped()),
        JsValue::Symbol(_) => Value::String("[symbol]".to_string()),
        JsValue::BigInt(_) => {
            let s = value
                .to_string(context)
                .unwrap_or_else(|_| JsString::from("0n"));
            Value::String(s.to_std_string_escaped())
        }
        JsValue::Object(obj) => {
            if obj.is_array()
                && let Ok(arr) = JsArray::from_object(obj.clone())
            {
                let len = arr.length(context).unwrap_or(0) as usize;
                let mut vec = Vec::with_capacity(len);
                for i in 0..len {
                    match arr.at(i as i64, context) {
                        Ok(elem) => vec.push(js_value_to_json(&elem, context)),
                        Err(_) => vec.push(Value::Null),
                    }
                }
                return Value::Array(vec);
            }
            object_to_json_via_stringify(obj, context)
        }
    }
}

/// Convert a JS object to JSON via `JSON.stringify`.
fn object_to_json_via_stringify(obj: &boa_engine::JsObject, context: &mut Context) -> Value {
    let json_global = context
        .global_object()
        .get(js_string!("JSON"), context)
        .unwrap_or_else(|_| JsValue::undefined());

    if let Some(json_obj) = json_global.as_object()
        && let Ok(stringify_fn) = json_obj.get(js_string!("stringify"), context)
        && stringify_fn.is_callable()
        && let Some(obj_inner) = stringify_fn.as_object()
        && let Ok(result) = obj_inner.call(&JsValue::undefined(), &[obj.clone().into()], context)
        && let Some(s) = result.as_string()
    {
        let json_str = s.to_std_string_escaped();
        if let Ok(parsed) = serde_json::from_str::<Value>(&json_str) {
            return parsed;
        }
        return Value::String(json_str);
    }

    if let Ok(s) = JsValue::from(obj.clone()).to_string(context) {
        let s = s.to_std_string_escaped();
        if s != "[object Object]" {
            return Value::String(s);
        }
    }

    Value::Object(serde_json::Map::new())
}

// ---------------------------------------------------------------------------
// localStorage
// ---------------------------------------------------------------------------

/// Register the `localStorage` global object (Storage interface).
///
/// localStorage is a simple key-value store with synchronous getItem/setItem.
/// Changes are propagated back to the Session via read-only Arc (since JS thread
/// can't mutate Session directly).
fn register_local_storage(
    ctx: &mut Context,
    storage: std::collections::HashMap<String, String>,
    _dom_snapshot: &Arc<RwLock<Option<DomSnapshot>>>,
    local_storage_tx: Arc<RwLock<Option<std::sync::mpsc::Sender<LocalStorageMsg>>>>,
) {
    // Build a JS object with Storage interface methods
    // We store the HashMap in a RefCell so JS can mutate it.
    use std::cell::RefCell;
    let storage_arc = Arc::new(RefCell::new(storage));
    let _storage_for_methods = storage_arc.clone();

    // --- getItem ---
    let get_storage = storage_arc.clone();
    let get_item_fn = unsafe {
        NativeFunction::from_closure(
            move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                let key = args
                    .first()
                    .and_then(|v| v.to_string(ctx).ok())
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default();
                let val = get_storage.borrow().get(&key).cloned();
                match val {
                    Some(v) => Ok(JsValue::from(JsString::from(v.as_str()))),
                    None => Ok(JsValue::null()),
                }
            },
        )
    };

    // --- setItem ---
    let set_storage = storage_arc.clone();
    let set_ls_tx = local_storage_tx.clone();
    let set_item_fn = unsafe {
        NativeFunction::from_closure(
            move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                if args.len() >= 2 {
                    let key = args[0]
                        .to_string(ctx)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let val = args[1]
                        .to_string(ctx)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    set_storage.borrow_mut().insert(key.clone(), val.clone());
                    // Sync to Session
                    let tx_opt = { set_ls_tx.read().as_ref().cloned() };
                    if let Some(tx) = tx_opt {
                        let _ = tx.send(LocalStorageMsg::SetItem(key, val));
                    }
                }
                Ok(JsValue::undefined())
            },
        )
    };

    // --- removeItem ---
    let rem_storage = storage_arc.clone();
    let rem_ls_tx = local_storage_tx.clone();
    let remove_item_fn = unsafe {
        NativeFunction::from_closure(
            move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                if let Some(key_arg) = args.first() {
                    let key = key_arg
                        .to_string(ctx)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    rem_storage.borrow_mut().remove(&key);
                    // Sync to Session
                    let tx_opt = { rem_ls_tx.read().as_ref().cloned() };
                    if let Some(tx) = tx_opt {
                        let _ = tx.send(LocalStorageMsg::RemoveItem(key));
                    }
                }
                Ok(JsValue::undefined())
            },
        )
    };

    // --- clear ---
    let clear_storage = storage_arc.clone();
    let clear_ls_tx = local_storage_tx.clone();
    let clear_fn = unsafe {
        NativeFunction::from_closure(
            move |_this: &JsValue, _args: &[JsValue], _ctx: &mut Context| {
                clear_storage.borrow_mut().clear();
                // Sync to Session
                let tx_opt = { clear_ls_tx.read().as_ref().cloned() };
                if let Some(tx) = tx_opt {
                    let _ = tx.send(LocalStorageMsg::Clear);
                }
                Ok(JsValue::undefined())
            },
        )
    };

    // --- key ---
    let key_storage = storage_arc.clone();
    let key_fn = unsafe {
        NativeFunction::from_closure(
            move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                if let Some(idx_arg) = args.first() {
                    let idx = idx_arg.to_index(ctx).unwrap_or(0) as usize;
                    let keys: Vec<_> = key_storage.borrow().keys().cloned().collect();
                    match keys.get(idx) {
                        Some(k) => Ok(JsValue::from(JsString::from(k.as_str()))),
                        None => Ok(JsValue::null()),
                    }
                } else {
                    Ok(JsValue::null())
                }
            },
        )
    };

    // --- get length (snapshot) ---
    let len_storage = storage_arc.clone();
    let _len_fn = unsafe {
        NativeFunction::from_closure(
            move |_this: &JsValue, _args: &[JsValue], _ctx: &mut Context| {
                Ok(JsValue::from(len_storage.borrow().len() as i32))
            },
        )
    };

    // Build localStorage object (Storage interface)
    let local_storage_obj = boa_engine::object::ObjectInitializer::new(ctx)
        .function(get_item_fn, js_string!("getItem"), 1)
        .function(set_item_fn, js_string!("setItem"), 2)
        .function(remove_item_fn, js_string!("removeItem"), 1)
        .function(clear_fn, js_string!("clear"), 0)
        .function(key_fn, js_string!("key"), 1)
        .build();

    let _ = ctx.register_global_property(
        js_string!("localStorage"),
        local_storage_obj,
        Attribute::all(),
    );

    // --- sessionStorage object ---
    //
    // Identical to localStorage but separate storage (same origin, different storage area).
    // In a real browser, localStorage persists and sessionStorage is per-tab.
    // For our implementation, both use an empty HashMap (synced from Session).
    let empty_session = std::collections::HashMap::new();
    register_storage_obj(ctx, js_string!("sessionStorage"), empty_session);
}

/// Register a Storage interface object (localStorage / sessionStorage pattern).
///
/// Creates a JS object with getItem/setItem/removeItem/clear/key/length methods,
/// backed by a RefCell<HashMap<String, String>>.
fn register_storage_obj(
    ctx: &mut Context,
    name: boa_engine::JsString,
    storage: std::collections::HashMap<String, String>,
) {
    use std::cell::RefCell;
    let storage_arc = Arc::new(RefCell::new(storage));

    // getItem
    let get_s = storage_arc.clone();
    let get_item_fn = unsafe {
        NativeFunction::from_closure(
            move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                let key = args
                    .first()
                    .and_then(|v| v.to_string(ctx).ok())
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default();
                match get_s.borrow().get(&key).cloned() {
                    Some(v) => Ok(JsValue::from(JsString::from(v.as_str()))),
                    None => Ok(JsValue::null()),
                }
            },
        )
    };

    // setItem
    let set_s = storage_arc.clone();
    let set_item_fn = unsafe {
        NativeFunction::from_closure(
            move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                if args.len() >= 2 {
                    let key = args[0]
                        .to_string(ctx)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let val = args[1]
                        .to_string(ctx)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    set_s.borrow_mut().insert(key, val);
                }
                Ok(JsValue::undefined())
            },
        )
    };

    // removeItem
    let rem_s = storage_arc.clone();
    let remove_item_fn = unsafe {
        NativeFunction::from_closure(
            move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                if let Some(k) = args.first() {
                    let key = k
                        .to_string(ctx)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    rem_s.borrow_mut().remove(&key);
                }
                Ok(JsValue::undefined())
            },
        )
    };

    // clear
    let clr_s = storage_arc.clone();
    let clear_fn = unsafe {
        NativeFunction::from_closure(
            move |_this: &JsValue, _args: &[JsValue], _ctx: &mut Context| {
                clr_s.borrow_mut().clear();
                Ok(JsValue::undefined())
            },
        )
    };

    // key
    let key_s = storage_arc.clone();
    let key_fn = unsafe {
        NativeFunction::from_closure(
            move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                if let Some(idx_arg) = args.first() {
                    let idx = idx_arg.to_index(ctx).unwrap_or(0) as usize;
                    let keys: Vec<_> = key_s.borrow().keys().cloned().collect();
                    match keys.get(idx) {
                        Some(k) => Ok(JsValue::from(JsString::from(k.as_str()))),
                        None => Ok(JsValue::null()),
                    }
                } else {
                    Ok(JsValue::null())
                }
            },
        )
    };

    // length (dynamic getter that reads current storage size)
    let len_s = storage_arc.clone();
    let len_getter_fn = {
        let getter: NativeFunction = unsafe {
            NativeFunction::from_closure(move |_this, _args, _ctx| {
                Ok(JsValue::from(len_s.borrow().len() as i32))
            })
        };
        FunctionObjectBuilder::new(ctx.realm(), getter)
            .name(js_string!("get length"))
            .build()
    };
    let storage_obj = boa_engine::object::ObjectInitializer::new(ctx)
        .function(get_item_fn, js_string!("getItem"), 1)
        .function(set_item_fn, js_string!("setItem"), 2)
        .function(remove_item_fn, js_string!("removeItem"), 1)
        .function(clear_fn, js_string!("clear"), 0)
        .function(key_fn, js_string!("key"), 1)
        .accessor(
            js_string!("length"),
            Some(len_getter_fn),
            None,
            Attribute::all(),
        )
        .build();

    let _ = ctx.register_global_property(name, storage_obj, Attribute::all());
}

// ---------------------------------------------------------------------------
// Error formatting
// ---------------------------------------------------------------------------

fn format_js_error(err: &boa_engine::JsError, context: &mut Context) -> String {
    if let Some(native) = err.as_native() {
        let kind = format!("{:?}", native.kind).to_lowercase();
        let msg = native.message();
        if msg.is_empty() {
            return kind;
        }
        return format!("{}: {}", kind, msg);
    }

    if let Some(opaque) = err.as_opaque() {
        if let Ok(s) = opaque.to_string(context) {
            let s = s.to_std_string_escaped();
            if !s.is_empty() && s != "undefined" {
                return s;
            }
        }
        if let Some(obj) = opaque.as_object()
            && let Ok(msg_val) = obj.get(js_string!("message"), context)
            && let Some(msg) = msg_val.as_string()
        {
            let msg_str = msg.to_std_string_escaped();
            if !msg_str.is_empty() {
                if let Ok(name_val) = obj.get(js_string!("name"), context)
                    && let Some(name) = name_val.as_string()
                {
                    return format!("{}: {}", name.to_std_string_escaped(), msg_str);
                }
                return msg_str;
            }
        }
        return format!("Error: {:?}", opaque);
    }

    "Unknown JavaScript error".to_string()
}

// ---------------------------------------------------------------------------
// `window` global object
// ---------------------------------------------------------------------------

/// Register `window` global object with browser property stubs.
///
/// This makes `typeof window === 'object'` true and provides common
/// properties that most JS libraries expect.
fn register_window_globals(
    ctx: &mut Context,
    dom_snapshot: &Arc<RwLock<Option<DomSnapshot>>>,
    mutations: &Arc<RwLock<Vec<DomMutation>>>,
    viewport: (u32, u32),
    page_url: &str,
    user_agent: &str,
    fetch_tx_arc: &Arc<RwLock<Option<std::sync::mpsc::Sender<FetchRequestMsg>>>>,
) {
    let _ = fetch_tx_arc; // suppress unused warning
    let url_owned = page_url.to_string();
    let ua_owned = user_agent.to_string();
    let (vp_w, vp_h) = viewport;

    // --- document.body / head / documentElement getters ---
    // We re-register `document` as a getter-based object that resolves these
    // from the DomSnapshot dynamically.
    let snap_body = dom_snapshot.clone();
    let mutations_body = mutations.clone();
    let body_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let snap = snap_body.read();
            if let Some(ref s) = *snap
                && let Some(bid) = s.body_id
                && let Some(node) = s.nodes.get(&bid)
            {
                return Ok(create_element_object(
                    s,
                    node,
                    ctx,
                    &mutations_body,
                    &snap_body,
                ));
            }
            Ok(JsValue::null())
        })
    };

    let snap_head = dom_snapshot.clone();
    let mutations_head = mutations.clone();
    let head_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let snap = snap_head.read();
            if let Some(ref s) = *snap
                && let Some(hid) = s.head_id
                && let Some(node) = s.nodes.get(&hid)
            {
                return Ok(create_element_object(
                    s,
                    node,
                    ctx,
                    &mutations_head,
                    &snap_head,
                ));
            }
            Ok(JsValue::null())
        })
    };

    let snap_de = dom_snapshot.clone();
    let mutations_de = mutations.clone();
    let document_element_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let snap = snap_de.read();
            if let Some(ref s) = *snap {
                let html_node = s.nodes.get(&s.root_id).and_then(|root| {
                    root.children.iter().find_map(|&child_id| {
                        s.nodes.get(&child_id).and_then(|n| {
                            if n.tag == "html" {
                                Some((child_id, n))
                            } else {
                                None
                            }
                        })
                    })
                });
                if let Some((_, node)) = html_node {
                    return Ok(create_element_object(s, node, ctx, &mutations_de, &snap_de));
                }
            }
            Ok(JsValue::null())
        })
    };

    // Register document.body, document.head, document.documentElement as
    // callable getters (not accessors, since boa 0.20 ObjectInitializer
    // doesn't support adding accessors to existing objects easily).
    // These appear as methods but act like properties: document.body()
    // We also register proper property-like access via a wrapper.
    //
    // Simpler approach: register them as methods on the global document
    // and also register a $body / $head / $documentElement that return
    // the element directly.
    //
    // Actually, the simplest working approach for boa 0.20: register them
    // as regular functions called body(), head(), documentElement().
    // Most JS code does document.body (property), not document.body().
    //
    // To make document.body work as a property, we need to build the
    // document object with accessors from the start. Let's modify
    // the existing document construction.

    // We'll add these functions to the global document object by
    // registering global getters that the JS code can use.
    // For now, register as document_get_body() etc. and also
    // as window.document properties via a wrapper.

    // --- Pre-build values that need ctx (to avoid double borrow) ---
    let languages_arr: JsValue = JsArray::from_iter(
        [
            JsValue::from(js_string!("en-US")),
            JsValue::from(js_string!("en")),
        ],
        ctx,
    )
    .into();

    // `navigator.platform` — via the stealth profile so it always agrees with
    // the WebGL renderer and userAgentData.platform (single source of truth).
    let nav_platform = crate::js::stealth::ChromeProfile::platform_for(&ua_owned);

    // window.navigator
    let nav_obj = boa_engine::object::ObjectInitializer::new(ctx)
        .property(
            js_string!("userAgent"),
            JsValue::from(js_string!(ua_owned.as_str())),
            Attribute::all(),
        )
        .property(
            js_string!("language"),
            JsValue::from(js_string!("en-US")),
            Attribute::all(),
        )
        .property(js_string!("languages"), languages_arr, Attribute::all())
        .property(
            js_string!("platform"),
            JsValue::from(js_string!(nav_platform)),
            Attribute::all(),
        )
        .property(
            js_string!("vendor"),
            JsValue::from(js_string!("Google Inc.")),
            Attribute::all(),
        )
        .property(
            js_string!("appName"),
            JsValue::from(js_string!("Netscape")),
            Attribute::all(),
        )
        .property(
            js_string!("appVersion"),
            JsValue::from(js_string!(ua_owned.as_str())),
            Attribute::all(),
        )
        .property(
            js_string!("webdriver"),
            JsValue::from(false),
            Attribute::all(),
        )
        .property(
            js_string!("hardwareConcurrency"),
            JsValue::from(8),
            Attribute::all(),
        )
        .property(
            js_string!("deviceMemory"),
            JsValue::from(8),
            Attribute::all(),
        )
        .property(
            js_string!("maxTouchPoints"),
            JsValue::from(0),
            Attribute::all(),
        )
        .property(js_string!("doNotTrack"), JsValue::null(), Attribute::all())
        .property(
            js_string!("cookieEnabled"),
            JsValue::from(true),
            Attribute::all(),
        )
        .property(js_string!("onLine"), JsValue::from(true), Attribute::all())
        .property(
            js_string!("pdfViewerEnabled"),
            JsValue::from(true),
            Attribute::all(),
        )
        .property(
            js_string!("product"),
            JsValue::from(js_string!("Gecko")),
            Attribute::all(),
        )
        .property(
            js_string!("productSub"),
            JsValue::from(js_string!("20030107")),
            Attribute::all(),
        )
        .property(
            js_string!("vendorSub"),
            JsValue::from(js_string!("")),
            Attribute::all(),
        )
        .build();

    // Level-1 stealth surface: navigator.plugins/mimeTypes/userAgentData/
    // permissions/connection (attached here) plus window.chrome and WebGL
    // constructors (wired into window_final / globals below). Attached before
    // `nav_obj` is cloned to window.navigator + the global navigator, so both
    // see the surface. See `js::stealth` for scope and limitations.
    let stealth = crate::js::stealth::build(ctx, &ua_owned);
    let _ = crate::js::stealth::attach_to_navigator(ctx, &nav_obj, &stealth);

    // window.location
    let parsed_url = url::Url::parse(&url_owned);
    let loc_href = url_owned.clone();
    let loc_origin = parsed_url
        .as_ref()
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_default();
    let loc_protocol = parsed_url
        .as_ref()
        .map(|u| u.scheme().to_string() + ":")
        .unwrap_or_default();
    let loc_hostname = parsed_url
        .as_ref()
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default();
    let loc_pathname = parsed_url
        .as_ref()
        .ok()
        .map(|u| u.path().to_string())
        .unwrap_or_default();

    let location_obj = boa_engine::object::ObjectInitializer::new(ctx)
        .property(
            js_string!("href"),
            JsValue::from(js_string!(loc_href.as_str())),
            Attribute::all(),
        )
        .property(
            js_string!("origin"),
            JsValue::from(js_string!(loc_origin.as_str())),
            Attribute::all(),
        )
        .property(
            js_string!("protocol"),
            JsValue::from(js_string!(loc_protocol.as_str())),
            Attribute::all(),
        )
        .property(
            js_string!("hostname"),
            JsValue::from(js_string!(loc_hostname.as_str())),
            Attribute::all(),
        )
        .property(
            js_string!("pathname"),
            JsValue::from(js_string!(loc_pathname.as_str())),
            Attribute::all(),
        )
        .build();

    // window.performance
    let perf_obj = boa_engine::object::ObjectInitializer::new(ctx)
        .function(
            unsafe {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    let ms = js_sys_helpers::now_ms();
                    Ok(JsValue::from(ms))
                })
            },
            js_string!("now"),
            0,
        )
        .build();

    // Build final window by combining all sub-objects
    // Since boa 0.20 doesn't have with_object, we register properties
    // on a fresh object that includes everything.
    // Clone objects before using them in window_final (they get moved)
    let nav_obj_for_window = nav_obj.clone();
    let location_obj_for_window = location_obj.clone();
    let perf_obj_for_window = perf_obj.clone();
    let global_doc = ctx
        .global_object()
        .get(js_string!("document"), ctx)
        .unwrap_or(JsValue::undefined());
    let global_console = ctx
        .global_object()
        .get(js_string!("console"), ctx)
        .unwrap_or(JsValue::undefined());

    // ── getComputedStyle(element) ──
    let gcs_dom = dom_snapshot.clone();
    let get_computed_style_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let element = args.first().cloned().unwrap_or(JsValue::null());
            let node_id = element
                .as_object()
                .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
                .and_then(|v| v.as_number().map(|n| n as u32));

            let node_id = match node_id {
                Some(id) => id,
                None => return Ok(JsValue::null()),
            };

            let dom = gcs_dom.read();
            let snapshot = match dom.as_ref() {
                Some(s) => s,
                None => return Ok(JsValue::null()),
            };

            let cs = match LayoutEngine::compute_style(snapshot, node_id) {
                Some(c) => c,
                None => return Ok(JsValue::null()),
            };

            let gcs_obj = boa_engine::object::ObjectInitializer::new(ctx)
                .property(js_string!("display"), JsValue::from(JsString::from(cs.display.clone())), Attribute::all())
                .property(js_string!("visibility"), JsValue::from(JsString::from(cs.visibility.clone())), Attribute::all())
                .property(js_string!("opacity"), JsValue::from(cs.opacity), Attribute::all())
                .property(js_string!("color"), JsValue::from(JsString::from(cs.color.clone())), Attribute::all())
                .property(js_string!("backgroundColor"), JsValue::from(JsString::from(cs.background_color.clone())), Attribute::all())
                .property(js_string!("fontSize"), JsValue::from(JsString::from(format!("{}px", cs.font_size))), Attribute::all())
                .property(js_string!("fontWeight"), JsValue::from(JsString::from(cs.font_weight.clone())), Attribute::all())
                .property(js_string!("textAlign"), JsValue::from(JsString::from(cs.text_align.clone())), Attribute::all())
                .property(js_string!("overflow"), JsValue::from(JsString::from(cs.overflow.clone())), Attribute::all())
                .property(js_string!("pointerEvents"), JsValue::from(JsString::from(cs.pointer_events.clone())), Attribute::all())
                .property(js_string!("position"), JsValue::from(JsString::from(cs.position.clone())), Attribute::all())
                .property(js_string!("width"), cs.width.map(|w| JsValue::from(JsString::from(format!("{}px", w)))).unwrap_or(JsValue::from(JsString::from("auto"))), Attribute::all())
                .property(js_string!("height"), cs.height.map(|h| JsValue::from(JsString::from(format!("{}px", h)))).unwrap_or(JsValue::from(JsString::from("auto"))), Attribute::all())
                .property(js_string!("zIndex"), cs.z_index.map(|z| JsValue::from(JsString::from(z.to_string()))).unwrap_or(JsValue::from(JsString::from("auto"))), Attribute::all())
                .property(js_string!("_visible"), JsValue::from(cs.visible), Attribute::all())
                .property(js_string!("_interactive"), JsValue::from(cs.interactive), Attribute::all())
                // getPropertyValue(name) — look up property by camelCase name
                .function({
                    let props = serde_json::json!({
                        "display": cs.display,
                        "visibility": cs.visibility,
                        "opacity": cs.opacity,
                        "color": cs.color,
                        "backgroundColor": cs.background_color,
                        "fontSize": format!("{}px", cs.font_size),
                        "fontWeight": cs.font_weight,
                        "textAlign": cs.text_align,
                        "overflow": cs.overflow,
                        "position": cs.position,
                        "pointerEvents": cs.pointer_events,
                        "width": cs.width.map(|w| format!("{}px", w)).unwrap_or_else(|| "auto".to_string()),
                        "height": cs.height.map(|h| format!("{}px", h)).unwrap_or_else(|| "auto".to_string()),
                        "zIndex": cs.z_index.map(|z| z.to_string()).unwrap_or_else(|| "auto".to_string()),
                    });
                    {
                        NativeFunction::from_closure(move |_this, args, _ctx| {
                            let name = args.first()
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_std_string_escaped())
                                .unwrap_or_default();
                            let val = props.get(&name)
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            Ok(JsValue::from(JsString::from(val)))
                        })
                    }
                }, js_string!("getPropertyValue"), 1)
                .build();

            Ok(JsValue::from(gcs_obj))
        })
    };

    let window_final = boa_engine::object::ObjectInitializer::new(ctx)
        // Copy viewport props
        .property(
            js_string!("innerWidth"),
            JsValue::from(vp_w as f64),
            Attribute::all(),
        )
        .property(
            js_string!("innerHeight"),
            JsValue::from(vp_h as f64),
            Attribute::all(),
        )
        .property(
            js_string!("outerWidth"),
            JsValue::from(vp_w as f64),
            Attribute::all(),
        )
        .property(
            js_string!("outerHeight"),
            JsValue::from(vp_h as f64),
            Attribute::all(),
        )
        .property(
            js_string!("devicePixelRatio"),
            JsValue::from(1.0),
            Attribute::all(),
        )
        .property(
            js_string!("name"),
            JsValue::from(js_string!("")),
            Attribute::all(),
        )
        .property(js_string!("length"), JsValue::from(0), Attribute::all())
        .property(js_string!("closed"), JsValue::from(false), Attribute::all())
        // Sub-objects
        .property(js_string!("document"), global_doc, Attribute::all())
        .property(js_string!("console"), global_console, Attribute::all())
        .property(
            js_string!("navigator"),
            JsValue::from(nav_obj_for_window),
            Attribute::all(),
        )
        .property(
            js_string!("location"),
            JsValue::from(location_obj_for_window),
            Attribute::all(),
        )
        .property(
            js_string!("performance"),
            JsValue::from(perf_obj_for_window),
            Attribute::all(),
        )
        // DOM shortcuts (as functions since boa 0.20 doesn't support
        // adding accessors to pre-existing objects)
        .function(body_getter, js_string!("getBody"), 0)
        .function(head_getter, js_string!("getHead"), 0)
        .function(document_element_getter, js_string!("getDocumentElement"), 0)
        .function(get_computed_style_fn, js_string!("getComputedStyle"), 1)
        .property(
            js_string!("chrome"),
            stealth.chrome.clone(),
            Attribute::all(),
        )
        .build();

    let _ = ctx.register_global_property(
        js_string!("window"),
        JsValue::from(window_final.clone()),
        Attribute::all(),
    );
    // Register getComputedStyle as a standalone global before moving window_final
    let gcs_fn_val = window_final
        .get(js_string!("getComputedStyle"), ctx)
        .unwrap_or(JsValue::undefined());
    let _ =
        ctx.register_global_property(js_string!("getComputedStyle"), gcs_fn_val, Attribute::all());
    let _ = ctx.register_global_property(
        js_string!("self"),
        JsValue::from(window_final),
        Attribute::all(),
    );

    // Also register navigator and location as standalone globals (browser spec)
    let _ = ctx.register_global_property(
        js_string!("navigator"),
        JsValue::from(nav_obj.clone()),
        Attribute::all(),
    );
    let _ = ctx.register_global_property(
        js_string!("location"),
        JsValue::from(location_obj.clone()),
        Attribute::all(),
    );
    let _ = ctx.register_global_property(
        js_string!("performance"),
        JsValue::from(perf_obj.clone()),
        Attribute::all(),
    );
    // crypto global (for window.crypto)
    let crypto_get_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            // crypto.getRandomValues — fill ArrayBuffer/TypedArray with random bytes
            // For now, just return the buffer as-is (full impl would copy random bytes)
            if let Some(arg) = args.first() {
                Ok(arg.clone())
            } else {
                Ok(JsValue::undefined())
            }
        })
    };
    let crypto_obj = boa_engine::object::ObjectInitializer::new(ctx)
        .function(crypto_get_fn, js_string!("getRandomValues"), 1)
        .build();
    let _ = ctx.register_global_property(
        js_string!("crypto"),
        JsValue::from(crypto_obj),
        Attribute::all(),
    );
    // Stealth globals: real Chrome exposes `chrome`, `WebGLRenderingContext`,
    // and `WebGL2RenderingContext` as top-level globals (not only on `window`).
    let _ = ctx.register_global_property(
        js_string!("chrome"),
        stealth.chrome.clone(),
        Attribute::all(),
    );
    let _ = ctx.register_global_property(
        js_string!("WebGLRenderingContext"),
        stealth.webgl1.clone(),
        Attribute::all(),
    );
    let _ = ctx.register_global_property(
        js_string!("WebGL2RenderingContext"),
        stealth.webgl2.clone(),
        Attribute::all(),
    );
    // ── SPA routing: history + location navigation ──
    // Native triggers push `DomMutation::Navigate`/`Reload`, which `Session`
    // drains and executes as real (async) navigations. The `history`/`location`
    // surface itself is installed by a JS bootstrap below (real JS getters and
    // closures), seeded idempotently so client-side routing survives navigation.
    let nav_mut = mutations.clone();
    let navigate_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            if let Some(v) = args.first()
                && let Some(s) = v.as_string()
            {
                let url = s.to_std_string_escaped();
                if !url.is_empty() {
                    nav_mut.write().push(DomMutation::Navigate { url });
                }
            }
            Ok(JsValue::undefined())
        })
    };
    let _ = ctx.register_global_callable(js_string!("__oxiNavigate"), 1, navigate_fn);
    let rld_mut = mutations.clone();
    let reload_fn = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            rld_mut.write().push(DomMutation::Reload);
            Ok(JsValue::undefined())
        })
    };
    let _ = ctx.register_global_callable(js_string!("__oxiReload"), 0, reload_fn);

    let page_url_json = serde_json::to_string(page_url).unwrap_or_else(|_| "\"\"".to_string());
    let bootstrap = HISTORY_LOCATION_BOOTSTRAP.replace("/*PAGE_URL*/", &page_url_json);
    if let Err(e) = ctx.eval(Source::from_bytes(&bootstrap)) {
        tracing::warn!(error = %e, "history/location bootstrap failed");
    }
    if let Err(e) = ctx.eval(Source::from_bytes(OBSERVER_BOOTSTRAP)) {
        tracing::warn!(error = %e, "observer bootstrap failed");
    }
    {
        let tz = serde_json::to_string(&detect_system_timezone())
            .unwrap_or_else(|_| "\"UTC\"".to_string());
        let parity = V8_PARITY_BOOTSTRAP
            .replace("/*TZ*/", &tz)
            .replace("/*LOCALE*/", "\"en-US\"");
        if let Err(e) = ctx.eval(Source::from_bytes(&parity)) {
            tracing::warn!(error = %e, "v8 parity bootstrap failed");
        }
    }
    fn detect_system_timezone() -> String {
        if let Ok(tz) = std::env::var("TZ")
            && tz.contains('/')
        {
            return tz;
        }
        if let Ok(target) = std::fs::read_link("/etc/localtime") {
            let s = target.to_string_lossy().into_owned();
            if let Some(idx) = s.rfind("zoneinfo/") {
                let tail = &s[idx + "zoneinfo/".len()..];
                if tail.contains('/') {
                    return tail.to_string();
                }
            }
        }
        "UTC".to_string()
    }
    if let Err(e) = ctx.eval(Source::from_bytes(WEB_COMPONENTS_BOOTSTRAP)) {
        tracing::warn!(error = %e, "web components bootstrap failed");
    }

    const OBSERVER_BOOTSTRAP: &str = r#"
(function () {
  // Headless: no layout-driven intersection, so use real-browser initial-fire
  // semantics — observe() invokes the callback once with isIntersecting:true.
  // This makes lazy-load + feature-detection code work while keeping the full
  // API surface (observe/unobserve/disconnect/takeRecords) present.
  function IO(cb, opts) {
    if (!(this instanceof IO)) return new IO(cb, opts);
    this.__cb = cb;
    this.root = (opts && opts.root) || null;
    this.rootMargin = (opts && opts.rootMargin) || '0px';
    var th = opts && opts.threshold != null ? opts.threshold : 0;
    this.thresholds = typeof th === 'number' ? [th] : [0];
  }
  IO.prototype.observe = function (t) {
    try {
      this.__cb([{
        target: t, isIntersecting: true, isVisible: true, intersectionRatio: 1,
        time: Date.now(), rootBounds: null,
        intersectionRect: { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 1, height: 1 },
        boundingClientRect: { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 1, height: 1 }
      }], this);
    } catch (e) {}
    return this;
  };
  IO.prototype.unobserve = function () { return this; };
  IO.prototype.disconnect = function () {};
  IO.prototype.takeRecords = function () { return []; };
  globalThis.IntersectionObserver = IO;

  function RO(cb) {
    if (!(this instanceof RO)) return new RO(cb);
    this.__cb = cb;
  }
  RO.prototype.observe = function (t) {
    try {
      this.__cb([{
        target: t,
        contentRect: { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 1, height: 1 },
        borderBoxSize: [{ inlineSize: 1, blockSize: 1 }],
        contentBoxSize: [{ inlineSize: 1, blockSize: 1 }],
        devicePixelContentBoxSize: [{ inlineSize: 1, blockSize: 1 }]
      }], this);
    } catch (e) {}
    return this;
  };
  RO.prototype.unobserve = function () { return this; };
  RO.prototype.disconnect = function () {};
  globalThis.ResizeObserver = RO;

  // Feature-detection code uses `'IntersectionObserver' in window`, so mirror
  // the constructors onto the window object too (re-applied on every navigation).
  if (globalThis.window) {
    globalThis.window.IntersectionObserver = IO;
    globalThis.window.ResizeObserver = RO;
  }
})();
"#;
    const V8_PARITY_BOOTSTRAP: &str = r#"
(function () {
  function def(g, name, value) {
    try { if (typeof g[name] === 'undefined') g[name] = value; } catch (e) {}
  }
  // Intl: boa has none. Provide the fingerprint surface reads rely on —
  // DateTimeFormat/NumberFormat .resolvedOptions().{locale,timeZone,calendar,
  // numberingSystem} + Collator. Numeric/date formatting is best-effort.
  if (typeof globalThis.Intl === 'undefined') {
    var TZ = /*TZ*/;
    var LOCALE = /*LOCALE*/;
    function resolved(locale) {
      return { locale: locale || LOCALE, timeZone: TZ, calendar: 'gregory', numberingSystem: 'latn' };
    }
    function normLocale(l) { return (typeof l === 'string') ? l : (l && l.length ? l[0] : LOCALE); }
    function DT(locale, options) {
      if (!(this instanceof DT)) return new DT(locale, options);
      this.__l = normLocale(locale);
      this.__opts = options || {};
    }
    DT.prototype.resolvedOptions = function () {
      var o = resolved(this.__l);
      // Honor an explicitly requested timeZone (real browsers do); otherwise
      // resolvedOptions().timeZone is the system default TZ baked into `resolved`.
      if (this.__opts.timeZone) o.timeZone = this.__opts.timeZone;
      return o;
    };
    DT.prototype.format = function (d) {
      var n = (d instanceof Date) ? d : new Date();
      return n.getFullYear() + '/' + (n.getMonth() + 1) + '/' + n.getDate();
    };
    DT.supportedLocalesOf = function (l) { return (typeof l === 'string') ? [l] : (l && l.length ? [l[0]] : [LOCALE]); };
    function NF(locale, options) {
      if (!(this instanceof NF)) return new NF(locale, options);
      this.__l = normLocale(locale);
    }
    NF.prototype.format = function (n) { return String(n); };
    NF.prototype.resolvedOptions = function () { return { locale: this.__l, numberingSystem: 'latn' }; };
    NF.supportedLocalesOf = DT.supportedLocalesOf;
    function Col(locale) {
      if (!(this instanceof Col)) return new Col(locale);
      this.__l = (typeof locale === 'string') ? locale : LOCALE;
    }
    Col.prototype.compare = function (a, b) { a = String(a); b = String(b); return a < b ? -1 : (a > b ? 1 : 0); };
    Col.prototype.resolvedOptions = function () { return resolved(this.__l); };
    Col.supportedLocalesOf = DT.supportedLocalesOf;
    var IntlObj = {
      DateTimeFormat: DT, NumberFormat: NF, Collator: Col,
      getCanonicalLocales: function (l) { return (typeof l === 'string') ? [l] : l; }
    };
    globalThis.Intl = IntlObj;
  }
  if (globalThis.window) globalThis.window.Intl = globalThis.Intl;
  // Error.stack: boa leaves it undefined; give a V8-shaped trace so sandbox/
  // headless detectors reading `new Error().stack` see a Chrome frame, not
  // `undefined`. Exact line numbers are unknowable from JS — this passes the
  // common "is .stack a non-empty string starting with the error name" check.
  if (typeof Error.prototype.stack === 'undefined') {
    Object.defineProperty(Error.prototype, 'stack', {
      configurable: true,
      get: function () {
        var name = (this && this.constructor && this.constructor.name) ? this.constructor.name : 'Error';
        var msg = (this && typeof this.message === 'string' && this.message.length) ? (': ' + this.message) : '';
        return name + msg + '\n    at Object.<anonymous> (<anonymous>:1:1)';
      }
    });
  }
  // structuredClone: deep-clone plain data via JSON.
  def(globalThis, 'structuredClone', function (v) {
    if (v === null || typeof v !== 'object') return v;
    try { return JSON.parse(JSON.stringify(v)); } catch (e) { return v; }
  });
  // queueMicrotask: schedule on the microtask queue via Promise.
  def(globalThis, 'queueMicrotask', function (cb) { Promise.resolve().then(cb); });
  // FinalizationRegistry presence stub (WeakRef is already present in boa).
  if (typeof globalThis.FinalizationRegistry === 'undefined') {
    function FR(cb) { this.__cb = cb; }
    FR.prototype.register = function () { return this; };
    FR.prototype.unregister = function () { return false; };
    globalThis.FinalizationRegistry = FR;
  }
  if (globalThis.window) globalThis.window.FinalizationRegistry = globalThis.FinalizationRegistry;
  // Page-context booleans real Chrome exposes.
  def(globalThis, 'crossOriginIsolated', false);
  def(globalThis, 'isSecureContext', true);
  def(globalThis, 'originAgentCluster', false);
  if (globalThis.window) {
    def(globalThis.window, 'crossOriginIsolated', false);
    def(globalThis.window, 'isSecureContext', true);
  }
})();
"#;
    const WEB_COMPONENTS_BOOTSTRAP: &str = r#"
(function () {
  // DOM constructor presence: Element/HTMLElement/Node/ShadowRoot/
  // DocumentFragment/EventTarget. Real element objects here are plain object
  // literals (createElement returns {}), NOT instances of these — parsed
  // elements won't gain the prototype chain. These exist for feature-detection
  // ('attachShadow' in Element.prototype, typeof HTMLElement) and so
  // customElements.define can validate constructors.
  function defCtor(name, parent) {
    if (typeof globalThis[name] !== 'undefined') return;
    var P = parent || function () {};
    function C() { if (!(this instanceof C)) throw new TypeError("Failed to construct '" + name + "': Please use the 'new' operator"); }
    C.prototype = Object.create(P.prototype);
    C.prototype.constructor = C;
    globalThis[name] = C;
  }
  defCtor('EventTarget');
  defCtor('Node', globalThis.EventTarget);
  defCtor('Element', globalThis.Node);
  defCtor('HTMLElement', globalThis.Element);
  defCtor('DocumentFragment', globalThis.Node);
  defCtor('ShadowRoot', globalThis.DocumentFragment);
  // attachShadow / getRootNode / shadowRoot on Element.prototype.
  if (globalThis.Element && !globalThis.Element.prototype.attachShadow) {
    globalThis.Element.prototype.attachShadow = function (init) {
      if (this.__shadowRoot) throw new Error("Failed to execute 'attachShadow': Shadow root already attached");
      var root = (typeof document !== 'undefined' && document.createDocumentFragment) ? document.createDocumentFragment() : {};
      root.host = this; root.mode = (init && init.mode) || 'open';
      this.__shadowRoot = root; return root;
    };
    Object.defineProperty(globalThis.Element.prototype, 'shadowRoot', {
      configurable: true,
      get: function () { return (this.__shadowRoot && this.__shadowRoot.mode === 'closed') ? null : (this.__shadowRoot || null); }
    });
    globalThis.Element.prototype.getRootNode = function () {
      var n = this, g = 0;
      while (n && n.parentNode && g < 100) { n = n.parentNode; g++; }
      return n || this;
    };
  }
  // customElements registry: define/get/whenDefined/upgrade. Full
  // upgrade-on-parse needs a parser hook (unavailable from JS); presence +
  // explicit registration + best-effort upgrade of existing matched nodes.
  if (typeof globalThis.customElements === 'undefined') {
    var registry = {}; var waiting = {};
    function valid(name) { return typeof name === 'string' && name.indexOf('-') > 0 && name === name.toLowerCase(); }
    var CE = {
      define: function (name, ctor) {
        if (!valid(name)) throw new TypeError("Failed to execute 'define': '" + name + "' is not a valid custom element name");
        if (registry[name]) throw new Error("Failed to execute 'define': this name has already been used: '" + name + "'");
        registry[name] = ctor;
        try { var ex = document.querySelectorAll(name); for (var i = 0; ex && i < ex.length; i++) CE.upgrade(ex[i], ctor); } catch (e) {}
        if (waiting[name]) { for (var j = 0; j < waiting[name].length; j++) { try { waiting[name][j](ctor); } catch (e) {} } delete waiting[name]; }
      },
      get: function (name) { return registry[name]; },
      whenDefined: function (name) {
        return new Promise(function (resolve) {
          if (registry[name]) resolve(registry[name]);
          else (waiting[name] = waiting[name] || []).push(resolve);
        });
      },
      upgrade: function (node, ctor) {
        ctor = ctor || (node && node.tagName && registry[node.tagName.toLowerCase()]);
        if (!ctor) return;
        try { if (ctor.prototype) Object.setPrototypeOf(node, ctor.prototype); } catch (e) {}
        try { ctor.call(node); } catch (e) {}
      }
    };
    globalThis.customElements = CE;
  }
  // window is rebuilt every navigation → sync unconditionally.
  if (globalThis.window) {
    var w = globalThis.window;
    var names = ['EventTarget','Node','Element','HTMLElement','DocumentFragment','ShadowRoot'];
    for (var k = 0; k < names.length; k++) { var nm = names[k]; if (globalThis[nm] && typeof w[nm] === 'undefined') w[nm] = globalThis[nm]; }
    w.customElements = globalThis.customElements;
  }
})();
"#;

    const HISTORY_LOCATION_BOOTSTRAP: &str = r#"
(function () {
  var PAGE_URL = /*PAGE_URL*/;
  function isAbs(u) { return /^(https?:|data:|blob:|file:|ftp:)/i.test(u) || u.indexOf('//') === 0; }
  function originOf(u) { var m = /^(https?:\/\/[^\/#?]+)/i.exec(u); return m ? m[1] : ''; }
  if (!globalThis.__oxiHistoryInit) {
    globalThis.__oxiHistoryEntries = [{ url: PAGE_URL, state: null }];
    globalThis.__oxiHistoryIndex = 0;
    globalThis.__oxiPopstateListeners = [];
    globalThis.__oxiHistoryInit = true;
  } else {
    var top = globalThis.__oxiHistoryEntries[globalThis.__oxiHistoryIndex];
    if (PAGE_URL && (!top || top.url !== PAGE_URL)) {
      globalThis.__oxiHistoryEntries = globalThis.__oxiHistoryEntries.slice(0, globalThis.__oxiHistoryIndex + 1);
      globalThis.__oxiHistoryEntries.push({ url: PAGE_URL, state: null });
      globalThis.__oxiHistoryIndex = globalThis.__oxiHistoryEntries.length - 1;
    }
  }
  function cur() { return globalThis.__oxiHistoryEntries[globalThis.__oxiHistoryIndex] || { url: PAGE_URL, state: null }; }
  function resolveUrl(url) {
    if (!url) return cur().url;
    url = String(url);
    if (isAbs(url)) return url;
    var base = cur().url || PAGE_URL;
    if (url.charAt(0) === '#') return base.split('#')[0] + url;
    if (url.charAt(0) === '/') return originOf(base) + url;
    var b = base.split('#')[0].split('?')[0];
    var i = b.lastIndexOf('/');
    return (i >= 0 ? b.substring(0, i + 1) : b + '/') + url;
  }
  function firePopstate() {
    var ev = { type: 'popstate', state: cur().state };
    (globalThis.__oxiPopstateListeners || []).forEach(function (cb) { try { cb(ev); } catch (e) {} });
  }
  globalThis.history = {
    get length() { return globalThis.__oxiHistoryEntries.length; },
    get state() { var e = cur(); return e ? e.state : null; },
    scrollRestoration: 'auto',
    pushState: function (state, unused, url) {
      var abs = url ? resolveUrl(url) : cur().url;
      globalThis.__oxiHistoryEntries = globalThis.__oxiHistoryEntries.slice(0, globalThis.__oxiHistoryIndex + 1);
      globalThis.__oxiHistoryEntries.push({ url: abs, state: state });
      globalThis.__oxiHistoryIndex = globalThis.__oxiHistoryEntries.length - 1;
    },
    replaceState: function (state, unused, url) {
      var abs = url ? resolveUrl(url) : cur().url;
      globalThis.__oxiHistoryEntries[globalThis.__oxiHistoryIndex] = { url: abs, state: state };
    },
    back: function () { globalThis.history.go(-1); },
    forward: function () { globalThis.history.go(1); },
    go: function (delta) {
      delta = (typeof delta === 'number') ? delta : 0;
      var ni = globalThis.__oxiHistoryIndex + delta;
      if (ni < 0) ni = 0;
      if (ni >= globalThis.__oxiHistoryEntries.length) ni = globalThis.__oxiHistoryEntries.length - 1;
      if (ni === globalThis.__oxiHistoryIndex) return;
      globalThis.__oxiHistoryIndex = ni;
      firePopstate();
    },
  };
  globalThis.addEventListener = globalThis.addEventListener || function (type, cb) {
    if (type === 'popstate') (globalThis.__oxiPopstateListeners = globalThis.__oxiPopstateListeners || []).push(cb);
  };
  globalThis.removeEventListener = globalThis.removeEventListener || function (type, cb) {
    if (type === 'popstate' && globalThis.__oxiPopstateListeners) {
      globalThis.__oxiPopstateListeners = globalThis.__oxiPopstateListeners.filter(function (x) { return x !== cb; });
    }
  };
  function augment(loc) {
    if (!loc) return;
    try {
      loc.assign = function (url) { __oxiNavigate(resolveUrl(url)); };
      loc.replace = function (url) { __oxiNavigate(resolveUrl(url)); };
      loc.reload = function () { __oxiReload(); };
      Object.defineProperty(loc, 'href', {
        configurable: true, enumerable: true,
        get: function () { return cur().url; },
        set: function (v) { __oxiNavigate(resolveUrl(v)); },
      });
    } catch (e) {}
  }
  augment(globalThis.location);
  if (globalThis.window && globalThis.window.location) augment(globalThis.window.location);
})();
"#;
}

/// Simple time helper for performance.now().
mod js_sys_helpers {
    pub fn now_ms() -> f64 {
        use std::time::SystemTime;
        let duration = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        duration.as_millis() as f64
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Estimate the height of a DOM element for elementFromPoint approximation.
fn estimate_element_height(node: &DomNode) -> f64 {
    let tag = node.tag.to_uppercase();
    match tag.as_str() {
        "H1" => 40.0,
        "H2" => 36.0,
        "H3" => 32.0,
        "H4" | "H5" | "H6" => 28.0,
        "P" => 24.0,
        "DIV" | "SECTION" | "ARTICLE" | "HEADER" | "FOOTER" | "NAV" | "MAIN" | "ASIDE" => 40.0,
        "UL" | "OL" => 24.0,
        "LI" => 20.0,
        "TABLE" => 32.0,
        "TR" => 24.0,
        "IMG" | "IFRAME" => 300.0,
        "INPUT" | "TEXTAREA" | "SELECT" => 32.0,
        "SCRIPT" | "STYLE" | "LINK" | "META" => 0.0, // invisible
        "SVG" | "CANVAS" => 200.0,
        _ => 24.0, // default line height
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;
    use url::Url;

    // --- Render façades (RenderDocument on the JS thread) ---

    #[tokio::test]
    async fn test_set_document_then_capture_png() {
        // set_document builds a RenderDocument on the JS thread; capture_png
        // returns a valid PNG of the rendered HTML — no JS involved.
        let mut rt = JsRuntime::new();
        let html = concat!(
            "<!DOCTYPE html><html><head><style>",
            "body { margin: 0; } .box { width: 40px; height: 40px; background: red; }",
            "</style></head><body><div class=\"box\"></div></body></html>"
        );
        rt.set_document(html, Some("https://example.com/"), (400, 300))
            .await
            .expect("set_document should build the render doc");

        let png = rt
            .capture_png(CaptureOpts {
                full_page: true,
                ..Default::default()
            })
            .await
            .expect("capture_png should render a PNG");

        // PNG magic header.
        assert!(png.len() > 8, "PNG data should be more than 8 bytes");
        assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);

        // Decode and confirm real (non-blank) CSS-rendered content.
        let img = image::load_from_memory(&png).expect("decode captured png");
        let rgba = img.to_rgba8();
        let has_red = rgba
            .pixels()
            .any(|p| p[0] > 200 && p[1] < 80 && p[2] < 80);
        assert!(has_red, "the red .box should be rendered");
    }

    #[tokio::test]
    async fn test_capture_without_document_errors() {
        let mut rt = JsRuntime::new();
        let err = rt
            .capture_png(CaptureOpts::default())
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::ScreenshotError(_)),
            "capture without a document should be a screenshot error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn test_query_selector_all_returns_nodes() {
        let mut rt = JsRuntime::new();
        let html = "<!DOCTYPE html><html><body>\
                    <div class=\"item\" data-n=\"1\">one</div>\
                    <div class=\"item\" data-n=\"2\">two</div>\
                    <span class=\"item\">three</span>\
                    </body></html>";
        rt.set_document(html, None, (400, 300)).await.unwrap();

        let nodes = rt.query_selector_all(".item").await.unwrap();
        assert_eq!(nodes.len(), 3, "three .item elements");
        assert_eq!(nodes[0].tag.as_deref(), Some("div"));
        assert_eq!(nodes[0].text, "one");
        assert_eq!(
            nodes[0].attributes.iter().find(|(k, _)| k == "data-n").map(|(_, v)| v.as_str()),
            Some("1"),
            "first item data-n attribute"
        );
        assert_eq!(nodes[2].tag.as_deref(), Some("span"));
    }

    // --- Basic types ---

    #[tokio::test]
    async fn test_evaluate_literal() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("42").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(42.into())));
    }

    #[tokio::test]
    async fn test_evaluate_string() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("\"hello\"").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("hello".into())));
    }

    #[tokio::test]
    async fn test_evaluate_boolean() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("true").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_evaluate_null() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("null").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Null));
    }

    #[tokio::test]
    async fn test_evaluate_undefined() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("undefined").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Null));
    }

    // --- Arithmetic & expressions ---

    #[tokio::test]
    async fn test_evaluate_arithmetic() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("2 + 3 * 4").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(14.into())));
    }

    #[tokio::test]
    async fn test_evaluate_expression() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("'hello ' + 'world'").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("hello world".into())));
    }

    // --- Functions ---

    #[tokio::test]
    async fn test_evaluate_function() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("function add(a, b) { return a + b; } add(1, 2)")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(3.into())));
    }

    // --- Console ---

    #[tokio::test]
    async fn test_console_log_capture() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("console.log('Hello, world!')").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.console_output, vec!["Hello, world!"]);
    }

    #[tokio::test]
    async fn test_console_log_multiple_args() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("console.log('a', 1, true)").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.console_output, vec!["a 1 true"]);
    }

    #[tokio::test]
    async fn test_console_warn_error_info() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("console.warn('w'); console.error('e'); console.info('i')")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.console_output.len(), 3);
    }

    #[tokio::test]
    async fn test_console_log_with_expressions() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("let x = 10; console.log('x is', x)")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.console_output, vec!["x is 10"]);
    }

    #[tokio::test]
    async fn test_multiple_console_logs() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("console.log('line1'); console.log('line2')")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.console_output.len(), 2);
    }

    // --- Errors ---

    #[tokio::test]
    async fn test_evaluate_error() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("throw new Error('oops')").await.unwrap();
        assert!(!result.is_ok());
        let msg = result.exception.unwrap();
        assert!(msg.contains("Error"), "msg: {}", msg);
        assert!(msg.contains("oops"), "msg: {}", msg);
    }

    #[tokio::test]
    async fn test_syntax_error() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("function {").await.unwrap();
        assert!(!result.is_ok());
        assert!(result.exception.unwrap().to_lowercase().contains("syntax"));
    }

    #[tokio::test]
    async fn test_type_error() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("undefined.foo").await.unwrap();
        assert!(!result.is_ok());
        assert!(result.exception.unwrap().to_lowercase().contains("type"));
    }

    // --- Globals ---

    #[tokio::test]
    async fn test_global_math() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("Math.PI").await.unwrap();
        assert!(result.is_ok());
        let pi = result.value.unwrap().as_f64().unwrap();
        assert!((pi - std::f64::consts::PI).abs() < 0.0001);
    }

    #[tokio::test]
    async fn test_set_global_string() {
        let mut rt = JsRuntime::new();
        rt.set_global("myVar", Value::String("hello".into()));
        let result = rt.evaluate("myVar").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("hello".into())));
    }

    #[tokio::test]
    async fn test_set_global_number() {
        let mut rt = JsRuntime::new();
        rt.set_global("count", Value::Number(42.into()));
        let result = rt.evaluate("count + 8").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap().as_f64().unwrap(), 50.0);
    }

    #[tokio::test]
    async fn test_set_global_object() {
        let mut rt = JsRuntime::new();
        rt.set_global("cfg", serde_json::json!({ "name": "test" }));
        let result = rt.evaluate("cfg.name").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("test".into())));
    }

    // --- Objects & Arrays ---

    #[tokio::test]
    async fn test_object_literal() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("({ a: 1, b: 'hi' })").await.unwrap();
        assert!(result.is_ok());
        let val = result.value.unwrap();
        assert!(val.is_object());
        let map = val.as_object().unwrap();
        assert_eq!(map.get("a"), Some(&Value::Number(1.into())));
    }

    #[tokio::test]
    async fn test_array_literal() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("[1, 2, 3]").await.unwrap();
        assert!(result.is_ok());
        let arr = result.value.unwrap().as_array().unwrap().clone();
        assert_eq!(arr.len(), 3);
    }

    #[tokio::test]
    async fn test_array_map() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("[1, 2, 3].map(x => x * 2)").await.unwrap();
        assert!(result.is_ok());
        let arr = result.value.unwrap().as_array().unwrap().clone();
        assert_eq!(arr[0], Value::Number(2.into()));
        assert_eq!(arr[2], Value::Number(6.into()));
    }

    #[tokio::test]
    async fn test_json_stringify() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("JSON.stringify({x: 1})").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("{\"x\":1}".to_string())));
    }

    #[tokio::test]
    async fn test_json_parse() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("JSON.parse('{\"a\": 1}').a").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(1.into())));
    }

    // --- JS features ---

    #[tokio::test]
    async fn test_template_literal() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("`hello ${1 + 2}`").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("hello 3".into())));
    }

    #[tokio::test]
    async fn test_arrow_function() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("const sq = x => x * x; sq(5)").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(25.into())));
    }

    #[tokio::test]
    async fn test_try_catch() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("try { throw 'oops'; } catch(e) { 'caught: ' + e }")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("caught: oops".into())));
    }

    #[tokio::test]
    async fn test_class() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("class Foo { constructor(x) { this.x = x; } getX() { return this.x; } } new Foo(42).getX()")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(42.into())));
    }

    #[tokio::test]
    async fn test_destructuring() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("const {a, b} = {a: 1, b: 2}; a + b")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(3.into())));
    }

    #[tokio::test]
    async fn test_regex() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("/hello/.test('hello world')").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_for_loop() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("let sum = 0; for (let i = 1; i <= 10; i++) sum += i; sum")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(55.into())));
    }

    #[tokio::test]
    async fn test_array_reduce() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("[1,2,3,4,5].reduce((a,x) => a+x, 0)")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(15.into())));
    }

    // ========================================
    // State persistence tests
    // ========================================

    #[tokio::test]
    async fn test_state_persists_across_evals() {
        let mut rt = JsRuntime::new();
        rt.evaluate("let x = 42").await.unwrap();
        let result = rt.evaluate("x + 8").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap().as_f64().unwrap(), 50.0);
    }

    #[tokio::test]
    async fn test_function_persists() {
        let mut rt = JsRuntime::new();
        rt.evaluate("function add(a, b) { return a + b; }")
            .await
            .unwrap();
        let result = rt.evaluate("add(3, 4)").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(7.into())));
    }

    #[tokio::test]
    async fn test_var_persists_across_evals() {
        let mut rt = JsRuntime::new();
        rt.evaluate("var greeting = 'hello'").await.unwrap();
        let result = rt.evaluate("greeting + ' world'").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("hello world".into())));
    }

    #[tokio::test]
    async fn test_closure_state_persists() {
        let mut rt = JsRuntime::new();
        rt.evaluate("const counter = (function() { let n = 0; return () => ++n; })()")
            .await
            .unwrap();

        let r1 = rt.evaluate("counter()").await.unwrap();
        assert_eq!(r1.value, Some(Value::Number(1.into())));

        let r2 = rt.evaluate("counter()").await.unwrap();
        assert_eq!(r2.value, Some(Value::Number(2.into())));

        let r3 = rt.evaluate("counter()").await.unwrap();
        assert_eq!(r3.value, Some(Value::Number(3.into())));
    }

    #[tokio::test]
    async fn test_set_global_persists_in_js_state() {
        let mut rt = JsRuntime::new();
        rt.set_global("baseUrl", Value::String("https://example.com".into()));
        rt.evaluate("let path = '/api'").await.unwrap();
        let result = rt.evaluate("baseUrl + path").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(
            result.value,
            Some(Value::String("https://example.com/api".into()))
        );
    }

    // ========================================
    // DOM snapshot + document object tests
    // ========================================

    fn make_frame(html: &str) -> Frame {
        let url = Url::parse("https://example.com").unwrap();
        let doc = oxibrowser_webapi::dom::Document::parse(html);
        // Recreate what Frame::from_html does, but synchronously
        Frame::from_doc(url, doc, html)
    }

    #[tokio::test]
    async fn test_document_title_no_snapshot() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("document.title").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String(String::new())));
    }

    #[tokio::test]
    async fn test_document_title_with_snapshot() {
        let mut rt = JsRuntime::new();
        let html = "<html><head><title>My Page</title></head><body><p>Hello</p></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt.evaluate("document.title").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("My Page".into())));
    }

    #[tokio::test]
    async fn test_document_url() {
        let mut rt = JsRuntime::new();
        let html = "<html><body></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt.evaluate("document.URL").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(
            result.value,
            Some(Value::String("https://example.com/".into()))
        );
    }

    #[tokio::test]
    async fn test_document_query_selector() {
        let mut rt = JsRuntime::new();
        let html =
            r#"<html><body><p class="intro">Hello</p><a href="/link">click</a></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelector('a').tagName")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("A".into())));
    }

    #[tokio::test]
    async fn test_document_query_selector_not_found() {
        let mut rt = JsRuntime::new();
        let html = "<html><body><p>Hello</p></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelector('video')")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Null));
    }

    #[tokio::test]
    async fn test_element_query_selector() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><div><p class="intro">Hello</p><a href="/link">click</a></div><a href="/other">other</a></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        // element.querySelector should find child element
        let result = rt
            .evaluate("document.querySelector('div').querySelector('a').href")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("/link".into())));

        // element.querySelector should not find elements outside subtree
        let result = rt
            .evaluate("document.querySelector('div').querySelector('span')")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Null));
    }

    #[tokio::test]
    async fn test_element_query_selector_all() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><ul><li>a</li><li>b</li></ul><li>c</li></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        // element.querySelectorAll should find only descendants
        let result = rt
            .evaluate("document.querySelector('ul').querySelectorAll('li').length")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(serde_json::json!(2)));

        // document.querySelectorAll should find all
        let result = rt
            .evaluate("document.querySelectorAll('li').length")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(serde_json::json!(3)));
    }

    #[tokio::test]
    async fn test_element_query_selector_class() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><div class="outer"><span class="inner">yes</span><span class="other">no</span></div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelector('.outer').querySelector('.inner').textContent")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("yes".into())));
    }

    #[tokio::test]
    async fn test_document_query_selector_all() {
        let mut rt = JsRuntime::new();
        let html = "<html><body><ul><li>a</li><li>b</li><li>c</li></ul></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelectorAll('li').length")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap().as_f64().unwrap(), 3.0);
    }

    #[tokio::test]
    async fn test_document_get_element_by_id() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><div id="main">content</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.getElementById('main').id")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("main".into())));
    }

    #[tokio::test]
    async fn test_document_get_elements_by_tag_name() {
        let mut rt = JsRuntime::new();
        let html = "<html><body><p>a</p><p>b</p></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.getElementsByTagName('p').length")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap().as_f64().unwrap(), 2.0);
    }

    #[tokio::test]
    async fn test_document_get_elements_by_class_name() {
        let mut rt = JsRuntime::new();
        let html =
            r#"<html><body><div class="item">a</div><div class="item">b</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.getElementsByClassName('item').length")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap().as_f64().unwrap(), 2.0);
    }

    #[tokio::test]
    async fn test_element_href_attribute() {
        let mut rt = JsRuntime::new();
        let html =
            r#"<html><body><a href="https://example.com" class="link">click</a></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelector('a').href")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(
            result.value,
            Some(Value::String("https://example.com".into()))
        );
    }

    #[tokio::test]
    async fn test_element_get_attribute() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><a href="/page" id="link">go</a></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelector('a').getAttribute('href')")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("/page".into())));
    }

    #[tokio::test]
    async fn test_element_has_attribute() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><a href="/page">go</a></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelector('a').hasAttribute('href')")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_element_class_name() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><div class="foo bar">content</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelector('div').className")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("foo bar".into())));
    }

    #[tokio::test]
    async fn test_element_text_content() {
        let mut rt = JsRuntime::new();
        let html = "<html><body><p>Hello World</p></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelector('p').textContent")
            .await
            .unwrap();
        assert!(result.is_ok());
        let text = result.value.unwrap().as_str().unwrap().to_string();
        assert!(
            text.contains("Hello World"),
            "textContent should contain 'Hello World', got: {:?}",
            text
        );
    }

    #[tokio::test]
    async fn test_element_inner_text_matches_text_content() {
        let mut rt = JsRuntime::new();
        let html = "<html><body><p>Hello World</p></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate(
                "var p = document.querySelector('p'); p.innerText === p.textContent && p.innerText",
            )
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("Hello World".into())));
    }

    #[tokio::test]
    async fn test_performance_now_standalone_global_matches_window() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("var t = performance.now(); typeof performance === 'object' && typeof performance.now === 'function' && window.performance === performance && typeof t === 'number' && t > 1000000000000")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_element_children() {
        let mut rt = JsRuntime::new();
        let html = "<html><body><div><p>a</p><p>b</p></div></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelector('div').children.length")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap().as_f64().unwrap(), 2.0);
    }

    #[tokio::test]
    async fn test_document_snapshot_update() {
        let mut rt = JsRuntime::new();

        // First snapshot
        let html1 = "<html><head><title>Page 1</title></head><body></body></html>";
        let frame1 = make_frame(html1);
        let snapshot1 = DomSnapshot::from_frame(&frame1);
        rt.set_dom_snapshot(Some(snapshot1));

        let r1 = rt.evaluate("document.title").await.unwrap();
        assert_eq!(r1.value, Some(Value::String("Page 1".into())));

        // Second snapshot replaces
        let html2 = "<html><head><title>Page 2</title></head><body></body></html>";
        let frame2 = make_frame(html2);
        let snapshot2 = DomSnapshot::from_frame(&frame2);
        rt.set_dom_snapshot(Some(snapshot2));

        let r2 = rt.evaluate("document.title").await.unwrap();
        assert_eq!(r2.value, Some(Value::String("Page 2".into())));
    }

    // ========================================
    // Runtime limits & timeout tests
    // ========================================

    #[tokio::test]
    async fn test_max_recursive_calls() {
        // Infinite recursion should be caught by recursion limit
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("function f() { return f(); } f()")
            .await
            .unwrap();
        assert!(!result.is_ok(), "infinite recursion should fail");
        let msg = result.exception.unwrap();
        assert!(
            msg.to_lowercase().contains("exceeded")
                || msg.to_lowercase().contains("recursion")
                || msg.to_lowercase().contains("stack"),
            "error should mention stack/recursion limit, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_max_loop_iterations() {
        // Infinite loop should be caught by loop iteration limit
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("while(true) {}").await.unwrap();
        assert!(!result.is_ok(), "infinite loop should fail");
        let msg = result.exception.unwrap();
        assert!(
            msg.contains("loop iteration limit"),
            "error should mention loop iteration limit, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_evaluate_after_infinite_loop() {
        // After an infinite loop error, the runtime should still work
        let mut rt = JsRuntime::new();

        // Trigger an infinite loop
        let result = rt.evaluate("while(true) {}").await.unwrap();
        assert!(!result.is_ok());

        // Runtime should still be functional for normal evals
        let result = rt.evaluate("1 + 1").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(2.into())));
    }

    #[tokio::test]
    async fn test_evaluate_after_infinite_recursion() {
        // After infinite recursion error, the runtime should still work
        let mut rt = JsRuntime::new();

        // Trigger infinite recursion
        let result = rt
            .evaluate("function f() { return f(); } f()")
            .await
            .unwrap();
        assert!(!result.is_ok());

        // Runtime should still be functional
        let result = rt.evaluate("42").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(42.into())));
    }

    #[tokio::test]
    async fn test_normal_loop_within_limits() {
        // Normal loops should work fine within limits
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("let sum = 0; for (let i = 0; i < 1000; i++) { sum += i; } sum")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap().as_f64().unwrap(), 499500.0);
    }

    #[tokio::test]
    async fn test_normal_recursion_within_limits() {
        // Normal recursion should work fine within limits
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("function fib(n) { return n <= 1 ? n : fib(n-1) + fib(n-2); } fib(10)")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(55.into())));
    }

    #[tokio::test]
    async fn test_js_runtime_config_custom() {
        // Verify custom config is applied
        let config = JsRuntimeConfig {
            timeout_ms: 1000,
            max_recursion: 10,
            max_loop_iterations: 50,
            max_stack_size: 256,
            viewport_width: 1280,
            viewport_height: 720,
            user_agent: "Test/1.0".to_string(),
        };
        let mut rt = JsRuntime::with_config(config);

        // A loop of 50 iterations should fail with limit of 50
        let result = rt
            .evaluate("let x = 0; for (let i = 0; i < 100; i++) { x++; } x")
            .await
            .unwrap();
        assert!(!result.is_ok(), "loop exceeding limit should fail");
    }

    // ========================================
    // Timer / async API tests (sync emulation)
    // ========================================

    #[tokio::test]
    async fn test_set_timeout() {
        let mut rt = JsRuntime::new();
        // setTimeout schedules the callback; it fires during timer drain after eval.
        rt.evaluate("let x = 0; setTimeout(() => { x = 42; }, 0)")
            .await
            .unwrap();
        // Verify on the next evaluate() that x was set by the timer callback.
        let result = rt.evaluate("x").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(42.into())));
    }

    #[tokio::test]
    async fn test_set_timeout_with_args() {
        let mut rt = JsRuntime::new();
        rt.evaluate("let r; setTimeout((a, b) => { r = a + b; }, 0, 3, 4)")
            .await
            .unwrap();
        let result = rt.evaluate("r").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(7.into())));
    }

    #[tokio::test]
    async fn test_set_interval_executes_once() {
        let mut rt = JsRuntime::new();
        // setInterval fires once during timer drain, then re-schedules
        rt.evaluate("let c = 0; setInterval(() => { c++; }, 0)")
            .await
            .unwrap();
        let result = rt.evaluate("c").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(1.into())));
    }

    #[tokio::test]
    async fn test_clear_timeout_cancels_timer() {
        let mut rt = JsRuntime::new();
        rt.evaluate("let x = 0; let id = setTimeout(() => { x = 99; }, 0); clearTimeout(id)")
            .await
            .unwrap();
        let result = rt.evaluate("x").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(0.into())));
    }

    #[tokio::test]
    async fn test_clear_interval_cancels_timer() {
        let mut rt = JsRuntime::new();
        rt.evaluate("let c = 0; let id = setInterval(() => { c++; }, 0); clearInterval(id)")
            .await
            .unwrap();
        let result = rt.evaluate("c").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(0.into())));
    }

    #[tokio::test]
    async fn test_fetch_returns_promise() {
        let mut rt = JsRuntime::new();
        // fetch() returns a Promise (no channel set, so returns Promise.reject)
        let result = rt.evaluate("fetch('https://example.com')").await.unwrap();
        // Should return a Promise object
        assert!(result.is_ok());
        assert!(result.value.is_some());
    }

    #[tokio::test]
    async fn test_fetch_no_channel_returns_promise() {
        let mut rt = JsRuntime::new();
        // fetch() returns Promise.reject when no channel is set
        // Just verify fetch() doesn't panic and returns something
        let result = rt.evaluate("typeof fetch").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("function".into())));
    }

    #[tokio::test]
    async fn test_fetch_with_mock_channel() {
        let mut rt = JsRuntime::new();
        // Set up a mock fetch channel
        let (tx, rx) = std::sync::mpsc::channel();
        rt.set_fetch_channel(tx);

        // Drop the receiver so the fetch fails gracefully
        drop(rx);

        let result = rt
            .evaluate("fetch('https://example.com').catch(e => 'error: ' + e.message)")
            .await
            .unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_element_add_event_listener_noop() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><div id="test">hi</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelector('div').addEventListener('click', () => {}); 'ok'")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("ok".into())));
    }

    #[tokio::test]
    async fn test_element_dispatch_event_noop() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><button id="btn">click</button></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.querySelector('button').dispatchEvent({})")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_document_add_event_listener_noop() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("document.addEventListener('DOMContentLoaded', () => {}); document.removeEventListener('DOMContentLoaded', () => {}); 'ok'")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("ok".into())));
    }

    // ========================================
    // Mutation tests
    // ========================================

    #[tokio::test]
    async fn test_mutation_set_attribute() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><input id="q" value="old"></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        rt.evaluate("document.getElementById('q').setAttribute('value', 'new')")
            .await
            .unwrap();

        let mutations = rt.drain_mutations();
        assert_eq!(mutations.len(), 1);
        match &mutations[0] {
            DomMutation::SetAttribute { name, value, .. } => {
                assert_eq!(name, "value");
                assert_eq!(value, "new");
            }
            _ => panic!("Expected SetAttribute, got {:?}", mutations[0]),
        }
    }

    #[tokio::test]
    async fn test_mutation_click() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><button id="btn">Click</button></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        rt.evaluate("document.getElementById('btn').click()")
            .await
            .unwrap();
        let mutations = rt.drain_mutations();
        assert!(!mutations.is_empty());
        match &mutations[0] {
            DomMutation::ClickElement { .. } => {}
            _ => panic!("Expected ClickElement, got {:?}", mutations[0]),
        }
    }
    #[tokio::test]
    async fn test_history_pushstate_updates_length_and_location() {
        let mut rt = JsRuntime::new();
        rt.set_page_url("https://example.com/");
        let before = rt
            .evaluate("history.length")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        rt.evaluate("history.pushState({ page: 2 }, '', '/p2')")
            .await
            .unwrap();
        let after = rt
            .evaluate("history.length")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        assert_eq!(after, before + 1, "pushState must grow history.length");
        let href = rt
            .evaluate("location.href")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        assert!(
            href.contains("/p2"),
            "location.href must reflect pushState, got {href}"
        );
        // pushState is pure client-side routing — it must not trigger navigation.
        let muts = rt.drain_mutations();
        assert!(
            !muts
                .iter()
                .any(|m| matches!(m, DomMutation::Navigate { .. })),
            "pushState must not push a Navigate mutation, got {muts:?}"
        );
    }

    #[tokio::test]
    async fn test_location_assign_triggers_navigation() {
        let mut rt = JsRuntime::new();
        rt.set_page_url("https://example.com/");
        rt.evaluate("location.assign('https://example.com/next')")
            .await
            .unwrap();
        let muts = rt.drain_mutations();
        assert!(
            muts.iter().any(
                |m| matches!(m, DomMutation::Navigate { url } if url == "https://example.com/next")
            ),
            "location.assign must queue a Navigate mutation, got {muts:?}"
        );
    }

    #[tokio::test]
    async fn test_history_back_dispatches_popstate() {
        let mut rt = JsRuntime::new();
        rt.set_page_url("https://example.com/");
        rt.evaluate(
            "globalThis.__pcount = 0;\
             addEventListener('popstate', function () { globalThis.__pcount++; });\
             history.pushState({}, '', '/a');\
             history.pushState({}, '', '/b');\
             history.back();",
        )
        .await
        .unwrap();
        let count = rt
            .evaluate("globalThis.__pcount")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        assert_eq!(count, 1, "history.back() must dispatch one popstate event");
    }
    #[tokio::test]
    async fn test_intersection_and_resize_observers() {
        let mut rt = JsRuntime::new();
        rt.set_page_url("https://example.com/");
        rt.evaluate(
            "globalThis.__ioFired = false;\
             globalThis.__roFired = false;\
             new IntersectionObserver(function (entries) {\
               globalThis.__ioFired = entries[0] && entries[0].isIntersecting === true;\
             }).observe({});\
             new ResizeObserver(function (entries) {\
               globalThis.__roFired = entries.length === 1 && entries[0].target !== undefined;\
             }).observe({});",
        )
        .await
        .unwrap();
        let io = rt
            .evaluate("globalThis.__ioFired")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(io, "IntersectionObserver callback must fire on observe");
        let ro = rt
            .evaluate("globalThis.__roFired")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(ro, "ResizeObserver callback must fire on observe");
        // Feature detection — the #1 stealth-relevant check.
        let detect = rt
            .evaluate("'IntersectionObserver' in window && 'ResizeObserver' in window")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(detect, "observers must be detectable via 'in window'");
    }
    #[tokio::test]
    async fn test_v8_parity_js_surface() {
        let mut rt = JsRuntime::new();
        rt.set_page_url("https://example.com/");

        // Intl present + timezone is a valid IANA zone (contains '/').
        let tz = rt
            .evaluate("Intl.DateTimeFormat().resolvedOptions().timeZone")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        assert!(
            tz.contains('/') || tz == "UTC",
            "Intl timeZone must be IANA or UTC (got {:?})",
            tz
        );
        // An explicitly requested timeZone is honored (real browsers do); this
        // is the #1 Intl fingerprint cross-check, so it must not leak system TZ.
        let req_tz = rt
            .evaluate(
                "new Intl.DateTimeFormat('en', { timeZone: 'UTC' }).resolvedOptions().timeZone",
            )
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        assert_eq!(
            req_tz, "UTC",
            "Intl must honor an explicitly requested timeZone"
        );
        let loc = rt
            .evaluate("Intl.DateTimeFormat().resolvedOptions().locale")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        assert!(!loc.is_empty(), "Intl locale must be non-empty");

        // Error.stack: V8-shaped, non-empty, starts with the error name.
        let stack = rt
            .evaluate("(function(){ try { throw new Error('boom') } catch(e){ return String(e.stack) } })()")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        assert!(
            stack.starts_with("Error"),
            "Error.stack must start with error name (got {:?})",
            stack
        );
        assert!(stack.contains("at "), "Error.stack must contain a frame");

        // structuredClone deep-copies plain data.
        let cloned = rt
            .evaluate("(function(){ var o = {a:1,b:{c:2}}; var c = structuredClone(o); o.b.c = 99; return c.b.c })()")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        assert_eq!(cloned, 2, "structuredClone must deep-clone");

        // queueMicrotask callback fires before evaluate returns (it runs jobs).
        rt.evaluate("queueMicrotask(function(){ globalThis.__qm = 'fired' })")
            .await
            .unwrap();
        let qm = rt
            .evaluate("globalThis.__qm")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        assert_eq!(qm, "fired", "queueMicrotask callback must execute");

        // FinalizationRegistry present + constructible.
        let fr = rt
            .evaluate("typeof FinalizationRegistry === 'function' && typeof new FinalizationRegistry(function(){}) === 'object'")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(fr, "FinalizationRegistry must be constructible");

        // crossOriginIsolated is a boolean (false on a normal page).
        let coi = rt
            .evaluate("typeof crossOriginIsolated === 'boolean'")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(coi, "crossOriginIsolated must be a boolean");

        // Feature-detection: Intl + FinalizationRegistry detectable via 'in window'.
        let detect = rt
            .evaluate("'Intl' in window && 'FinalizationRegistry' in window")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(
            detect,
            "Intl and FinalizationRegistry must be detectable on window"
        );
    }
    #[tokio::test]
    async fn test_custom_elements_registry_and_shadow_dom() {
        let mut rt = JsRuntime::new();
        rt.set_page_url("https://example.com/");

        // customElements registry: define stores, get retrieves.
        rt.evaluate(
            "class FooBar extends HTMLElement {}\
             customElements.define('foo-bar', FooBar);\
             globalThis.__got = (customElements.get('foo-bar') === FooBar);",
        )
        .await
        .unwrap();
        let got = rt
            .evaluate("globalThis.__got")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(got, "customElements.define + get must round-trip");

        // whenDefined resolves for an already-defined element (microtask fires
        // after the scheduling evaluate returns — read in a follow-up eval).
        rt.evaluate("customElements.whenDefined('foo-bar').then(function(c){ globalThis.__wd = (c === FooBar); })")
            .await
            .unwrap();
        let wd = rt
            .evaluate("globalThis.__wd")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(
            wd,
            "customElements.whenDefined must resolve to the constructor"
        );

        // Invalid name throws.
        let bad = rt
            .evaluate("(function(){ try { customElements.define('NoHyphen', function(){}); return false } catch(e){ return true } })()")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(
            bad,
            "customElements.define must reject names without a hyphen"
        );

        // attachShadow returns a root with .host + .mode; shadowRoot getter.
        rt.evaluate(
            "var host = Object.create(Element.prototype);\
             var sr = Element.prototype.attachShadow.call(host, { mode: 'open' });\
             globalThis.__srHost = (sr.host === host);\
             globalThis.__srMode = (sr.mode === 'open');\
             globalThis.__sRoot = (host.shadowRoot === sr);",
        )
        .await
        .unwrap();
        let sr_host = rt
            .evaluate("globalThis.__srHost")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let sr_mode = rt
            .evaluate("globalThis.__srMode")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let s_root = rt
            .evaluate("globalThis.__sRoot")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(sr_host, "attachShadow root must reference host");
        assert!(sr_mode, "attachShadow root must carry mode");
        assert!(
            s_root,
            "element.shadowRoot must return the attached open root"
        );

        // Feature-detection surface.
        let detect = rt
            .evaluate("'customElements' in window && 'attachShadow' in Element.prototype && typeof HTMLElement === 'function' && typeof ShadowRoot === 'function'")
            .await
            .unwrap()
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(detect, "web component surface must be feature-detectable");
    }

    #[tokio::test]
    async fn test_mutation_input_value_setter() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><input id="inp" value="old"></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        rt.evaluate("document.getElementById('inp').value = 'new'")
            .await
            .unwrap();
        let mutations = rt.drain_mutations();
        assert_eq!(mutations.len(), 1);
        match &mutations[0] {
            DomMutation::InputElement { value, .. } => {
                assert_eq!(value, "new");
            }
            _ => panic!("Expected InputElement, got {:?}", mutations[0]),
        }
    }

    #[tokio::test]
    async fn test_mutation_value_getter() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><input id="inp" value="hello"></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        let result = rt
            .evaluate("document.getElementById('inp').value")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("hello".into())));
    }

    #[tokio::test]
    async fn test_drain_mutations_clears_buffer() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><button id="btn">Click</button></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        rt.evaluate("document.getElementById('btn').click()")
            .await
            .unwrap();
        let first = rt.drain_mutations();
        assert!(!first.is_empty());

        let second = rt.drain_mutations();
        assert!(second.is_empty());
    }

    #[tokio::test]
    async fn test_set_dom_snapshot_clears_mutations() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><button id="btn">Click</button></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        rt.evaluate("document.getElementById('btn').click()")
            .await
            .unwrap();

        let snapshot2 = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot2));
        let mutations = rt.drain_mutations();
        assert!(mutations.is_empty());
    }

    #[tokio::test]
    async fn test_mutation_set_attribute_via_query_selector() {
        let mut rt = JsRuntime::new();
        let html = r#"<html><body><a href="/page" id="link">go</a></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);
        rt.set_dom_snapshot(Some(snapshot));

        rt.evaluate("document.querySelector('a').setAttribute('href', '/new-page')")
            .await
            .unwrap();
        let mutations = rt.drain_mutations();
        assert_eq!(mutations.len(), 1);
        match &mutations[0] {
            DomMutation::SetAttribute { name, value, .. } => {
                assert_eq!(name, "href");
                assert_eq!(value, "/new-page");
            }
            _ => panic!("Expected SetAttribute, got {:?}", mutations[0]),
        }
    }

    // ------------------------------------------------------------------------
    // atob / btoa / URL / URLSearchParams tests
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn test_btoa_basic() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("btoa('Hello')").await.unwrap();
        assert!(result.is_ok());
        let val = result.value.unwrap();
        assert!(val.is_string());
        // Should be base64 encoded
    }

    #[tokio::test]
    async fn test_atob_basic() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("atob('SGVsbG8=')").await.unwrap();
        assert!(result.is_ok());
        let val = result.value.unwrap();
        assert_eq!(val, Value::String("Hello".into()));
    }

    #[tokio::test]
    async fn test_url_class() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("new URL('https://example.com:8080/path?foo=bar#hash').hostname")
            .await
            .unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_url_search_params() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("new URLSearchParams('foo=bar&baz=1').get('foo')")
            .await
            .unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_array_from() {
        let mut rt = JsRuntime::new();

        // Array.from with array
        let result = rt.evaluate("Array.from([1, 2, 3]).length").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap(), 3);

        // Array.from with array-like object
        let result = rt
            .evaluate("Array.from({length: 2, 0: 'a', 1: 'b'}).join(',')")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap(), "a,b");

        // Array.from with single string value → iterates chars (array-like with .length)
        let result = rt.evaluate("Array.from('hello').length").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap(), 5);
    }

    // ── Layout evaluation integration tests ──

    #[tokio::test]
    async fn test_get_computed_style_display_none() {
        let mut rt = JsRuntime::new();
        let html = r##"<html><body><div id="box" style="display:none">hidden</div></body></html>"##;
        let frame = make_frame(html);
        rt.set_dom_snapshot(Some(DomSnapshot::from_frame(&frame)));

        let result = rt
            .evaluate(r#"getComputedStyle(document.getElementById("box"))._visible"#)
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap(), false);
    }

    #[tokio::test]
    async fn test_get_computed_style_visible_div() {
        let mut rt = JsRuntime::new();
        let html = r##"<html><body><div id="box">visible</div></body></html>"##;
        let frame = make_frame(html);
        rt.set_dom_snapshot(Some(DomSnapshot::from_frame(&frame)));

        let result = rt
            .evaluate(r#"getComputedStyle(document.getElementById("box"))._visible"#)
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap(), true);
    }

    #[tokio::test]
    async fn test_get_computed_style_color() {
        let mut rt = JsRuntime::new();
        let html = r##"<html><body><p id="red" style="color:red">Red</p></body></html>"##;
        let frame = make_frame(html);
        rt.set_dom_snapshot(Some(DomSnapshot::from_frame(&frame)));

        let result = rt
            .evaluate(r#"getComputedStyle(document.getElementById("red")).color"#)
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap(), "#ff0000");
    }

    #[tokio::test]
    async fn test_get_computed_style_interactive_button() {
        let mut rt = JsRuntime::new();
        let html = r##"<html><body><button id="btn">Click</button></body></html>"##;
        let frame = make_frame(html);
        rt.set_dom_snapshot(Some(DomSnapshot::from_frame(&frame)));

        let result = rt
            .evaluate(r#"getComputedStyle(document.getElementById("btn"))._interactive"#)
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap(), true);
    }

    #[tokio::test]
    async fn test_get_computed_style_disabled_button() {
        let mut rt = JsRuntime::new();
        let html = r##"<html><body><button id="btn" disabled>Click</button></body></html>"##;
        let frame = make_frame(html);
        rt.set_dom_snapshot(Some(DomSnapshot::from_frame(&frame)));

        let result = rt
            .evaluate(r#"getComputedStyle(document.getElementById("btn"))._interactive"#)
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap(), false);
    }

    #[tokio::test]
    async fn test_get_computed_style_get_property_value() {
        let mut rt = JsRuntime::new();
        let html =
            r##"<html><body><div id="box" style="position:absolute">Abs</div></body></html>"##;
        let frame = make_frame(html);
        rt.set_dom_snapshot(Some(DomSnapshot::from_frame(&frame)));

        let result = rt
            .evaluate(
                r#"getComputedStyle(document.getElementById("box")).getPropertyValue("position")"#,
            )
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap(), "absolute");
    }

    #[tokio::test]
    async fn test_get_bounding_client_rect() {
        let mut rt = JsRuntime::new();
        let html = r##"<html><body><div id="box" style="width:200px;height:100px">Box</div></body></html>"##;
        let frame = make_frame(html);
        rt.set_dom_snapshot(Some(DomSnapshot::from_frame(&frame)));

        let result = rt
            .evaluate(r#"var r = document.getElementById("box").getBoundingClientRect(); r.width + "x" + r.height"#)
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap(), "200x100");
    }

    #[tokio::test]
    async fn test_offset_width_height() {
        let mut rt = JsRuntime::new();
        let html = r##"<html><body><div id="box" style="width:300px;height:150px">Box</div></body></html>"##;
        let frame = make_frame(html);
        rt.set_dom_snapshot(Some(DomSnapshot::from_frame(&frame)));

        let result = rt
            .evaluate(r#"document.getElementById("box").offsetWidth + "x" + document.getElementById("box").offsetHeight"#)
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value.unwrap(), "300x150");
    }
}
