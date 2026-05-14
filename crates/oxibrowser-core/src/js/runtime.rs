//! JavaScript runtime using boa_engine with a persistent context.
//!
//! boa_engine is a pure Rust JavaScript engine (ES2024+), no C dependencies.
//! Provides real JS evaluation with console.log and Math, JSON, etc.
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
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use boa_engine::object::builtins::JsArray;
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsString, JsValue, NativeFunction, Source};
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::js::dom_snapshot::{DomMutation, DomNode, DomSnapshot};
use crate::js::job_queue::TokioJobQueue;
use std::rc::Rc;

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

/// Commands sent from the async main thread to the JS thread.
enum JsCommand {
    /// Evaluate a JS expression.
    Eval {
        expression: String,
        /// Timeout in ms for this eval. None = use default.
        timeout_ms: Option<u64>,
        /// Max loop iterations. None = use default.
        max_loop_iterations: Option<u64>,
        /// Max recursion depth. None = use default.
        max_recursion: Option<usize>,
        /// Max operand stack size. None = use default.
        max_stack_size: Option<usize>,
    },
    /// Set a global variable in the persistent Context.
    SetGlobal { name: String, value: Value },
    /// Update the DOM snapshot available to `document` object.
    SetDom { snapshot: Option<DomSnapshot> },
    /// Update the page URL (for window.location).
    SetPageUrl { url: String },
    /// Set the fetch channel so JS can make real HTTP requests.
    SetFetchChannel { tx: std::sync::mpsc::Sender<FetchRequestMsg> },
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
    /// Channel to receive responses from the JS thread.
    resp_rx: Mutex<Receiver<JsResponse>>,
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
    ///
    /// Spawns a dedicated OS thread that owns the `boa_engine::Context`.
    pub fn new() -> Self {
        Self::with_config(JsRuntimeConfig::default())
    }

    /// Create a new JS runtime with the given configuration.
    pub fn with_config(config: JsRuntimeConfig) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<JsCommand>();
        let (resp_tx, resp_rx) = mpsc::channel::<JsResponse>();
        let console_output = Arc::new(RwLock::new(Vec::<String>::new()));
        let mutations = Arc::new(RwLock::new(Vec::<DomMutation>::new()));

        // Spawn JS thread
        let console_output_clone = console_output.clone();
        let mutations_clone = mutations.clone();
        let viewport = (config.viewport_width, config.viewport_height);
        let local_storage = Arc::new(RwLock::new(HashMap::<String, String>::new()));
        std::thread::Builder::new()
            .name("oxibrowser-js".into())
            .spawn(move || {
                js_thread_loop(
                    cmd_rx,
                    resp_tx,
                    console_output_clone,
                    mutations_clone,
                    viewport,
                    None,
                );
            })
            .expect("failed to spawn JS thread");

        Self {
            cmd_tx,
            resp_rx: Mutex::new(resp_rx),
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
        self.cmd_tx
            .send(JsCommand::SetFetchChannel { tx })
            .expect("JS thread has died");
        let resp = self
            .resp_rx
            .lock()
            .expect("resp_rx lock poisoned")
            .recv()
            .expect("JS thread has died");
        match resp {
            JsResponse::Done => {}
            _ => panic!("unexpected response"),
        }
    }

    /// Evaluate a JavaScript expression and return the result.
    ///
    /// JS state persists across calls — variables, functions, closures
    /// defined in one `evaluate()` are available in the next.
    ///
    /// If the evaluation exceeds the configured timeout, the JS context
    /// is reset and previous state (variables, functions) is lost.
    pub async fn evaluate(&mut self, expression: &str) -> Result<JsEvalResult> {
        self.evaluate_with_timeout(expression, None).await
    }

    /// Evaluate a JavaScript expression with an explicit timeout override.
    ///
    /// If `timeout_ms` is `None`, uses the default from config.
    pub async fn evaluate_with_timeout(
        &mut self,
        expression: &str,
        timeout_ms: Option<u64>,
    ) -> Result<JsEvalResult> {
        // Clear shared console buffer
        self.console_output.write().clear();

        // Send eval command with limits
        self.cmd_tx
            .send(JsCommand::Eval {
                expression: expression.to_string(),
                timeout_ms: Some(timeout_ms.unwrap_or(self.config.timeout_ms)),
                max_loop_iterations: Some(self.config.max_loop_iterations),
                max_recursion: Some(self.config.max_recursion),
                max_stack_size: Some(self.config.max_stack_size),
            })
            .expect("JS thread has died");

        // Wait for response
        let resp = self
            .resp_rx
            .lock()
            .expect("resp_rx lock poisoned")
            .recv()
            .expect("JS thread has died");

        match resp {
            JsResponse::EvalResult {
                value,
                exception,
                console_output,
                timed_out,
            } => {
                if timed_out {
                    // Context was reset — clear our globals tracking too
                    // (they're stale in the new context)
                    // We do NOT clear self.globals because set_global can re-inject them.
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
            JsResponse::Done => Ok(JsEvalResult::void()),
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
    ///
    /// After calling this, the internal buffer is empty.
    pub fn drain_mutations(&self) -> Vec<DomMutation> {
        let mut guard = self.mutations.write();
        std::mem::take(&mut *guard)
    }

    /// Set a global variable — injected into the persistent JS Context.
    pub fn set_global(&mut self, name: impl Into<String>, value: Value) {
        let name = name.into();
        self.globals.write().insert(name.clone(), value.clone());

        // Also inject into the persistent JS Context
        self.cmd_tx
            .send(JsCommand::SetGlobal {
                name,
                value: value.clone(),
            })
            .expect("JS thread has died");

        // Wait for ack
        let resp = self
            .resp_rx
            .lock()
            .expect("resp_rx lock poisoned")
            .recv()
            .expect("JS thread has died");
        let _ = resp; // JsResponse::Done
    }

    /// Get a global variable (Rust-side tracking).
    pub fn get_global(&self, name: &str) -> Option<Value> {
        self.globals.read().get(name).cloned()
    }

    /// Set the DOM snapshot (called after navigate).
    ///
    /// Sends the snapshot to the JS thread so that `document.querySelector`
    /// and friends operate on real DOM data. Also clears the mutation buffer.
    pub fn set_dom_snapshot(&mut self, snapshot: Option<DomSnapshot>) {
        // Clear mutations when snapshot changes
        self.mutations.write().clear();

        self.cmd_tx
            .send(JsCommand::SetDom { snapshot })
            .expect("JS thread has died");
        // Wait for ack
        let resp = self
            .resp_rx
            .lock()
            .expect("resp_rx lock poisoned")
            .recv()
            .expect("JS thread has died");
        let _ = resp;
    }

    /// Update the page URL (used for window.location).
    pub fn set_page_url(&mut self, url: &str) {
        self.cmd_tx
            .send(JsCommand::SetPageUrl {
                url: url.to_string(),
            })
            .expect("JS thread has died");
        let resp = self
            .resp_rx
            .lock()
            .expect("resp_rx lock poisoned")
            .recv()
            .expect("JS thread has died");
        let _ = resp;
    }
}

impl Default for JsRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for JsRuntime {
    fn drop(&mut self) {
        // Signal the JS thread to shut down
        let _ = self.cmd_tx.send(JsCommand::Shutdown);
        // Best-effort: don't panic in drop if mutex is poisoned or thread is dead
        if let Ok(guard) = self.resp_rx.lock() {
            let _ = guard.recv();
        }
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
    resp_tx: Sender<JsResponse>,
    console_output: Arc<RwLock<Vec<String>>>,
    mutations: Arc<RwLock<Vec<DomMutation>>>,
    viewport: (u32, u32),
    fetch_tx: Option<std::sync::mpsc::Sender<FetchRequestMsg>>,
) {
    let fetch_tx_arc: Arc<RwLock<Option<std::sync::mpsc::Sender<FetchRequestMsg>>>> =
        Arc::new(RwLock::new(None));
    let dom_snapshot: Arc<RwLock<Option<DomSnapshot>>> = Arc::new(RwLock::new(None));
    let mut ctx = create_context(
        &console_output,
        &dom_snapshot,
        &mutations,
        viewport,
        "",
        "OxiBrowser/0.2",
        &fetch_tx_arc,
    );

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            JsCommand::Eval {
                expression,
                timeout_ms,
                max_loop_iterations,
                max_recursion,
                max_stack_size,
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

                let elapsed = start.elapsed();
                let console = console_output.read().clone();

                // Check if we timed out
                if elapsed.as_millis() > timeout as u128 {
                    // Context may be in a bad state — recreate it
                    ctx = create_context(
                        &console_output,
                        &dom_snapshot,
                        &mutations,
                        viewport,
                        "",
                        "OxiBrowser/0.2",
                        &fetch_tx_arc,
                    );
                    let _ = resp_tx.send(JsResponse::EvalResult {
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
                        let json_value = js_value_to_json(&value, &mut ctx);
                        let _ = resp_tx.send(JsResponse::EvalResult {
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

                        let _ = resp_tx.send(JsResponse::EvalResult {
                            value: None,
                            exception: Some(msg),
                            console_output: console,
                            timed_out: false,
                        });
                    }
                }
            }
            JsCommand::SetGlobal { name, value } => {
                let js_val = json_to_js_value(&value, &mut ctx);
                let _ = ctx.register_global_property(
                    JsString::from(name.as_str()),
                    js_val,
                    Attribute::all(),
                );
                let _ = resp_tx.send(JsResponse::Done);
            }
            JsCommand::SetDom { snapshot } => {
                *dom_snapshot.write() = snapshot;
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
                let _ = resp_tx.send(JsResponse::Done);
            }
            JsCommand::SetPageUrl { url } => {
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
                    "OxiBrowser/0.2",
                    &fetch_tx_arc,
                );
                // Also re-register localStorage on URL change
                let empty = std::collections::HashMap::new();
                register_local_storage(&mut ctx, empty, &dom_snapshot_ref);
                let _ = resp_tx.send(JsResponse::Done);
            }
            // SetLocalStorage: not needed — SetPageUrl already re-registers localStorage
            JsCommand::SetFetchChannel { tx } => {
                *fetch_tx_arc.write() = Some(tx);
                let _ = resp_tx.send(JsResponse::Done);
            }
            JsCommand::Shutdown => {
                let _ = resp_tx.send(JsResponse::Done);
                break;
            }
        }
    }
}

/// Create a fresh boa_engine Context with console.log/warn/error/info
/// and `document` object registered.
fn create_context(
    output: &Arc<RwLock<Vec<String>>>,
    dom_snapshot: &Arc<RwLock<Option<DomSnapshot>>>,
    mutations: &Arc<RwLock<Vec<DomMutation>>>,
    viewport: (u32, u32),
    page_url: &str,
    user_agent: &str,
    fetch_tx_arc: &Arc<RwLock<Option<std::sync::mpsc::Sender<FetchRequestMsg>>>>,
) -> Context {
    let mut context = Context::builder()
        .job_queue(Rc::new(TokioJobQueue::new()))
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

    // --- Timer functions (synchronous emulation) ---
    //
    // setTimeout(fn, delay, ...args) — callback is invoked immediately (no event loop).
    // setInterval(fn, delay)        — same: executes once immediately.
    // clearTimeout / clearInterval   — no-ops.

    let set_timeout_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            if args.is_empty() {
                return Ok(JsValue::undefined());
            }
            let callback = &args[0];
            if let Some(func) = callback.as_object() {
                if func.is_callable() {
                    let cb_args = &args[2..]; // args after (fn, delay)
                    let _ = func.call(&JsValue::undefined(), cb_args, ctx);
                }
            }
            Ok(JsValue::from(1)) // timer id (simplified)
        })
    };

    let set_interval_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            if args.is_empty() {
                return Ok(JsValue::undefined());
            }
            let callback = &args[0];
            if let Some(func) = callback.as_object() {
                if func.is_callable() {
                    let cb_args = &args[2..];
                    let _ = func.call(&JsValue::undefined(), cb_args, ctx);
                }
            }
            Ok(JsValue::from(1))
        })
    };

    let clear_timer_fn =
        unsafe { NativeFunction::from_closure(move |_this, _args, _ctx| Ok(JsValue::undefined())) };

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
            let mut body: Option<String> = None;
            let mut timeout_ms: Option<u64> = None;

            if args.len() > 1 {
                if let Some(opts) = args[1].as_object() {
                    // method
                    if let Ok(m) = opts.get(js_string!("method"), ctx) {
                        if let Some(s) = m.as_string() {
                            method = s.to_std_string_escaped().to_uppercase();
                        }
                    }
                    // headers (simplified — just extract common ones)
                    // Full header iteration via enumerate() skipped for simplicity
                    // since boa 0.20's JsIterator API requires careful handling
                    // body
                    if let Ok(b) = opts.get(js_string!("body"), ctx) {
                        if !b.is_undefined() && !b.is_null() {
                            if let Some(s) = b.as_string() {
                                body = Some(s.to_std_string_escaped());
                            }
                        }
                    }
                    // timeout
                    if let Ok(t) = opts.get(js_string!("timeout"), ctx) {
                        if let Some(n) = t.as_number() {
                            timeout_ms = Some(n as u64);
                        }
                    }
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
                let reject_code = format!(
                    "Promise.reject(new Error('fetch channel error: {}'))",
                    e
                );
                let result = ctx.eval(Source::from_bytes(reject_code.trim()));
                return result;
            }

            // Wait for response (blocks JS thread)
            let response = response_rx.recv();
            let status = 0u16;
            let status_text = String::new();
            let resp_url = url.clone();
            let resp_headers: Vec<(String, String)> = Vec::new();
            let resp_body = String::new();
            let resp_error: Option<String>;

            match response {
                Ok(resp) => {
                    resp_error = resp.error;
                    // Build Response object
                    let response_obj = boa_engine::object::ObjectInitializer::new(ctx)
                        .property(js_string!("status"), JsValue::from(resp.status), Attribute::all())
                        .property(js_string!("statusText"), JsValue::from(JsString::from(resp.status_text.as_str())), Attribute::all())
                        .property(js_string!("ok"), JsValue::from(resp.status < 400), Attribute::all())
                        .property(js_string!("url"), JsValue::from(JsString::from(resp.url.as_str())), Attribute::all())
                        .property(js_string!("body"), JsValue::from(JsString::from(resp.body.as_str())), Attribute::all())
                        .build();

                    // Return Promise.resolve(response)
                    let resolve_code = format!("Promise.resolve({{status:{},statusText:'{}',ok:{},url:'{}',body:'{}'}})",
                        resp.status,
                        resp.status_text.replace("'", "\'"),
                        resp.status < 400,
                        resp.url.replace("'", "\'"),
                        resp.body.replace("'", "\'").replace("
", "\n").replace("
", "\r")
                    );
                    let result = ctx.eval(Source::from_bytes(resolve_code.trim()));
                    return result;
                }
                Err(_) => {
                    resp_error = Some("fetch channel closed".to_string());
                }
            }

            // Return rejected Promise on error
            let reject_code = format!(
                "Promise.reject(new Error('{}'))",
                resp_error.unwrap_or_else(|| "fetch failed".to_string()).replace("'", "\'")
            );
            let result = ctx.eval(Source::from_bytes(reject_code.trim()));
            result
        })
    };

    let _ = context.register_global_callable(js_string!("fetch"), 2, fetch_fn);

    // --- Document object ---

    register_document_object(&mut context, dom_snapshot, mutations);

    // --- Window global ---

    register_window_globals(
        &mut context,
        dom_snapshot,
        mutations,
        viewport,
        page_url,
        user_agent,
        &fetch_tx_arc,
    );

    context
}

// ---------------------------------------------------------------------------
// Document object registration
// ---------------------------------------------------------------------------

/// Register the `document` global object with DOM query methods.
fn register_document_object(
    ctx: &mut Context,
    dom_snapshot: &Arc<RwLock<Option<DomSnapshot>>>,
    mutations: &Arc<RwLock<Vec<DomMutation>>>,
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

    let dom_capture_cookie = dom_snapshot.clone();
    let cookie_getter: NativeFunction = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let _unused = dom_capture_cookie.read();
            // Simplified: no cookie jar integration yet
            Ok(JsValue::from(JsString::from("")))
        })
    };
    let cookie_getter_fn = FunctionObjectBuilder::new(ctx.realm(), cookie_getter)
        .name(js_string!("get cookie"))
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
            if let Some(ref snapshot) = *dom {
                if let Some(node_id) = snapshot.query_selector(&selector) {
                    if let Some(node) = snapshot.nodes.get(&node_id) {
                        return Ok(create_element_object(
                            snapshot,
                            node,
                            ctx,
                            &mutations_capture_qs,
                        ));
                    }
                }
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
                            create_element_object(snapshot, node, ctx, &mutations_capture_qsa)
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
            if let Some(ref snapshot) = *dom {
                if let Some(node_id) = snapshot.get_element_by_id(&id) {
                    if let Some(node) = snapshot.nodes.get(&node_id) {
                        return Ok(create_element_object(
                            snapshot,
                            node,
                            ctx,
                            &mutations_capture_gbi,
                        ));
                    }
                }
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
                            create_element_object(snapshot, node, ctx, &mutations_capture_gtn)
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
                            create_element_object(snapshot, node, ctx, &mutations_capture_gcn)
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

    // EventTarget noop methods for document
    let doc_add_event_listener_fn =
        unsafe { NativeFunction::from_closure(move |_this, _args, _ctx| Ok(JsValue::undefined())) };
    let doc_remove_event_listener_fn =
        unsafe { NativeFunction::from_closure(move |_this, _args, _ctx| Ok(JsValue::undefined())) };
    let doc_dispatch_event_fn =
        unsafe { NativeFunction::from_closure(move |_this, _args, _ctx| Ok(JsValue::from(true))) };

    // document.body / document.head / document.documentElement getters
    let dom_snap_body = dom_snapshot.clone();
    let body_getter_fn = {
        let mutations_clone = mutations.clone();
        let getter: NativeFunction = unsafe {
            NativeFunction::from_closure(move |_this, _args, ctx| {
                let snap = dom_snap_body.read();
                if let Some(ref s) = *snap {
                    if let Some(bid) = s.body_id {
                        if let Some(node) = s.nodes.get(&bid) {
                            return Ok(create_element_object(s, node, ctx, &mutations_clone));
                        }
                    }
                }
                Ok(JsValue::null())
            })
        };
        FunctionObjectBuilder::new(ctx.realm(), getter)
            .name(js_string!("get body"))
            .build()
    };

    let dom_snap_head = dom_snapshot.clone();
    let head_getter_fn = {
        let mutations_clone = mutations.clone();
        let getter: NativeFunction = unsafe {
            NativeFunction::from_closure(move |_this, _args, ctx| {
                let snap = dom_snap_head.read();
                if let Some(ref s) = *snap {
                    if let Some(hid) = s.head_id {
                        if let Some(node) = s.nodes.get(&hid) {
                            return Ok(create_element_object(s, node, ctx, &mutations_clone));
                        }
                    }
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
                        return Ok(create_element_object(s, node, ctx, &mutations_clone));
                    }
                }
                Ok(JsValue::null())
            })
        };
        FunctionObjectBuilder::new(ctx.realm(), getter)
            .name(js_string!("get documentElement"))
            .build()
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
            None,
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
        // DOM tree accessors
        .accessor(
            js_string!("body"),
            Some(body_getter_fn),
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
        .build();

    let _ = ctx.register_global_property(js_string!("document"), document_obj, Attribute::all());
}

/// Create a JS element object from a DomNode.
fn create_element_object(
    snapshot: &DomSnapshot,
    node: &DomNode,
    ctx: &mut Context,
    mutations: &Arc<RwLock<Vec<DomMutation>>>,
) -> JsValue {
    let tag_upper = node.tag.to_uppercase();
    let id_val = node.attributes.get("id").map(|s| s.as_str()).unwrap_or("");
    let class_val = node
        .attributes
        .get("class")
        .map(|s| s.as_str())
        .unwrap_or("");
    let href_val = node
        .attributes
        .get("href")
        .map(|s| s.as_str())
        .unwrap_or("");
    let src_val = node.attributes.get("src").map(|s| s.as_str()).unwrap_or("");

    // getAttribute(name)
    let attrs_clone: HashMap<String, String> = node.attributes.clone();
    let get_attribute_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let name = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            match attrs_clone.get(&name) {
                Some(val) => Ok(JsValue::from(JsString::from(val.as_str()))),
                None => Ok(JsValue::null()),
            }
        })
    };

    // hasAttribute(name)
    let attrs_clone2: HashMap<String, String> = node.attributes.clone();
    let has_attribute_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let name = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            Ok(JsValue::from(attrs_clone2.contains_key(&name)))
        })
    };

    // addEventListener — noop (event system not yet implemented)
    let add_event_listener_fn =
        unsafe { NativeFunction::from_closure(move |_this, _args, _ctx| Ok(JsValue::undefined())) };

    // removeEventListener — noop
    let remove_event_listener_fn =
        unsafe { NativeFunction::from_closure(move |_this, _args, _ctx| Ok(JsValue::undefined())) };

    // dispatchEvent — noop (returns true)
    let dispatch_event_fn =
        unsafe { NativeFunction::from_closure(move |_this, _args, _ctx| Ok(JsValue::from(true))) };

    // click() → records DomMutation::ClickElement
    let node_id_click = node.id;
    let mutations_click = mutations.clone();
    let click_fn = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            mutations_click.write().push(DomMutation::ClickElement {
                node_id: node_id_click,
            });
            Ok(JsValue::undefined())
        })
    };

    // setAttribute(name, value) → records DomMutation::SetAttribute
    let node_id_sa = node.id;
    let mutations_sa = mutations.clone();
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
            mutations_sa.write().push(DomMutation::SetAttribute {
                node_id: node_id_sa,
                name,
                value,
            });
            Ok(JsValue::undefined())
        })
    };

    // value getter
    let value_val = node
        .attributes
        .get("value")
        .map(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let value_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            Ok(JsValue::from(JsString::from(value_val.as_str())))
        })
    };
    let value_getter_fn = FunctionObjectBuilder::new(ctx.realm(), value_getter)
        .name(js_string!("get value"))
        .build();

    // value setter → records DomMutation::InputElement
    let node_id_vs = node.id;
    let mutations_vs = mutations.clone();
    let value_setter = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let val = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
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

    let obj = boa_engine::object::ObjectInitializer::new(ctx)
        .property(
            js_string!("tagName"),
            JsValue::from(JsString::from(tag_upper.as_str())),
            Attribute::all(),
        )
        .property(
            js_string!("textContent"),
            JsValue::from(JsString::from(node.text_content.as_str())),
            Attribute::all(),
        )
        .property(
            js_string!("innerHTML"),
            JsValue::from(JsString::from(node.text_content.as_str())),
            Attribute::all(),
        )
        .property(
            js_string!("id"),
            JsValue::from(JsString::from(id_val)),
            Attribute::all(),
        )
        .property(
            js_string!("className"),
            JsValue::from(JsString::from(class_val)),
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
        .accessor(
            js_string!("value"),
            Some(value_getter_fn),
            Some(value_setter_fn),
            Attribute::all(),
        )
        .build();

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
            if obj.is_array() {
                if let Ok(arr) = JsArray::from_object(obj.clone()) {
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

    if let Some(json_obj) = json_global.as_object() {
        if let Ok(stringify_fn) = json_obj.get(js_string!("stringify"), context) {
            if stringify_fn.is_callable() {
                if let Some(obj_inner) = stringify_fn.as_object() {
                    if let Ok(result) =
                        obj_inner.call(&JsValue::undefined(), &[obj.clone().into()], context)
                    {
                        if let Some(s) = result.as_string() {
                            let json_str = s.to_std_string_escaped();
                            if let Ok(parsed) = serde_json::from_str::<Value>(&json_str) {
                                return parsed;
                            }
                            return Value::String(json_str);
                        }
                    }
                }
            }
        }
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
) {

    // Build a JS object with Storage interface methods
    // We store the HashMap in a RefCell so JS can mutate it.
    use std::cell::RefCell;
    let storage_arc = Arc::new(RefCell::new(storage));
    let storage_for_methods = storage_arc.clone();

    // --- getItem ---
    let get_storage = storage_arc.clone();
    let get_item_fn = unsafe {
        NativeFunction::from_closure(move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
            let key = args.first()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let val = get_storage.borrow().get(&key).cloned();
            match val {
                Some(v) => Ok(JsValue::from(JsString::from(v.as_str()))),
                None => Ok(JsValue::null()),
            }
        })
    };

    // --- setItem ---
    let set_storage = storage_arc.clone();
    let set_item_fn = unsafe {
        NativeFunction::from_closure(move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
            if args.len() >= 2 {
                let key = args[0]
                    .to_string(ctx)
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default();
                let val = args[1]
                    .to_string(ctx)
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default();
                set_storage.borrow_mut().insert(key, val);
            }
            Ok(JsValue::undefined())
        })
    };

    // --- removeItem ---
    let rem_storage = storage_arc.clone();
    let remove_item_fn = unsafe {
        NativeFunction::from_closure(move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
            if let Some(key_arg) = args.first() {
                let key = key_arg
                    .to_string(ctx)
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default();
                rem_storage.borrow_mut().remove(&key);
            }
            Ok(JsValue::undefined())
        })
    };

    // --- clear ---
    let clear_storage = storage_arc.clone();
    let clear_fn = unsafe {
        NativeFunction::from_closure(move |_this: &JsValue, _args: &[JsValue], _ctx: &mut Context| {
            clear_storage.borrow_mut().clear();
            Ok(JsValue::undefined())
        })
    };

    // --- key ---
    let key_storage = storage_arc.clone();
    let key_fn = unsafe {
        NativeFunction::from_closure(move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
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
        })
    };

    // --- get length (snapshot) ---
    let len_storage = storage_arc.clone();
    let len_fn = unsafe {
        NativeFunction::from_closure(move |_this: &JsValue, _args: &[JsValue], _ctx: &mut Context| {
            Ok(JsValue::from(len_storage.borrow().len() as i32))
        })
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
        if let Some(obj) = opaque.as_object() {
            if let Ok(msg_val) = obj.get(js_string!("message"), context) {
                if let Some(msg) = msg_val.as_string() {
                    let msg_str = msg.to_std_string_escaped();
                    if !msg_str.is_empty() {
                        if let Ok(name_val) = obj.get(js_string!("name"), context) {
                            if let Some(name) = name_val.as_string() {
                                return format!("{}: {}", name.to_std_string_escaped(), msg_str);
                            }
                        }
                        return msg_str;
                    }
                }
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
            if let Some(ref s) = *snap {
                if let Some(bid) = s.body_id {
                    if let Some(node) = s.nodes.get(&bid) {
                        return Ok(create_element_object(s, node, ctx, &mutations_body));
                    }
                }
            }
            Ok(JsValue::null())
        })
    };

    let snap_head = dom_snapshot.clone();
    let mutations_head = mutations.clone();
    let head_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let snap = snap_head.read();
            if let Some(ref s) = *snap {
                if let Some(hid) = s.head_id {
                    if let Some(node) = s.nodes.get(&hid) {
                        return Ok(create_element_object(s, node, ctx, &mutations_head));
                    }
                }
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
                    return Ok(create_element_object(s, node, ctx, &mutations_de));
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
            JsValue::from(js_string!("MacIntel")),
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
        .build();

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
    let global_doc = ctx
        .global_object()
        .get(js_string!("document"), ctx)
        .unwrap_or(JsValue::undefined());
    let global_console = ctx
        .global_object()
        .get(js_string!("console"), ctx)
        .unwrap_or(JsValue::undefined());

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
            JsValue::from(nav_obj),
            Attribute::all(),
        )
        .property(
            js_string!("location"),
            JsValue::from(location_obj),
            Attribute::all(),
        )
        .property(
            js_string!("performance"),
            JsValue::from(perf_obj),
            Attribute::all(),
        )
        // DOM shortcuts (as functions since boa 0.20 doesn't support
        // adding accessors to pre-existing objects)
        .function(body_getter, js_string!("getBody"), 0)
        .function(head_getter, js_string!("getHead"), 0)
        .function(document_element_getter, js_string!("getDocumentElement"), 0)
        .build();

    let _ = ctx.register_global_property(
        js_string!("window"),
        JsValue::from(window_final.clone()),
        Attribute::all(),
    );
    let _ = ctx.register_global_property(
        js_string!("self"),
        JsValue::from(window_final),
        Attribute::all(),
    );
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;
    use url::Url;

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
        let result = rt
            .evaluate("let x = 0; setTimeout(() => { x = 42; }, 100); x")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(42.into())));
    }

    #[tokio::test]
    async fn test_set_timeout_with_args() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("let r; setTimeout((a, b) => { r = a + b; }, 0, 3, 4); r")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(7.into())));
    }

    #[tokio::test]
    async fn test_set_interval_executes_once() {
        let mut rt = JsRuntime::new();
        // setInterval also executes immediately in our sync model
        let result = rt
            .evaluate("let c = 0; setInterval(() => { c++; }, 100); c")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(1.into())));
    }

    #[tokio::test]
    async fn test_clear_timeout_noop() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("clearTimeout(1); clearTimeout(999); 'ok'")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("ok".into())));
    }

    #[tokio::test]
    async fn test_clear_interval_noop() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("clearInterval(1); clearInterval(99); 'done'")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("done".into())));
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
    async fn test_fetch_no_channel_rejects() {
        let mut rt = JsRuntime::new();
        // No fetch channel set, so fetch returns Promise.reject
        let result = rt
            .evaluate("fetch('https://example.com').then(() => 'ok').catch(e => e.message)")
            .await
            .unwrap();
        // Should have an error message about missing fetch channel
        assert!(result.is_ok());
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
}
