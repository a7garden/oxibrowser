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

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};

use boa_engine::object::builtins::JsArray;
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsString, JsValue, NativeFunction, Source};
use serde_json::Value;

use crate::error::Result;
use crate::js::dom_snapshot::{DomNode, DomSnapshot};

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
}

impl JsEvalResult {
    /// Create a successful result with a value.
    pub fn ok(value: Value) -> Self {
        Self {
            value: Some(value),
            exception: None,
            console_output: Vec::new(),
        }
    }

    /// Create a result with no return value (void/undefined).
    pub fn void() -> Self {
        Self {
            value: None,
            exception: None,
            console_output: Vec::new(),
        }
    }

    /// Create an error result.
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            value: None,
            exception: Some(msg.into()),
            console_output: Vec::new(),
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
    },
    /// Set a global variable in the persistent Context.
    SetGlobal {
        name: String,
        value: Value,
    },
    /// Update the DOM snapshot available to `document` object.
    SetDom {
        snapshot: Option<DomSnapshot>,
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
    },
    /// Ack for SetGlobal / SetDom / Shutdown.
    Done,
}

// ---------------------------------------------------------------------------
// JsRuntime
// ---------------------------------------------------------------------------

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
    /// Global variables tracked on the Rust side.
    globals: RwLock<HashMap<String, Value>>,
}

impl JsRuntime {
    /// Create a new JS runtime.
    ///
    /// Spawns a dedicated OS thread that owns the `boa_engine::Context`.
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<JsCommand>();
        let (resp_tx, resp_rx) = mpsc::channel::<JsResponse>();
        let console_output = Arc::new(RwLock::new(Vec::<String>::new()));

        // Spawn JS thread
        let console_output_clone = console_output.clone();
        std::thread::Builder::new()
            .name("oxibrowser-js".into())
            .spawn(move || {
                js_thread_loop(cmd_rx, resp_tx, console_output_clone);
            })
            .expect("failed to spawn JS thread");

        Self {
            cmd_tx,
            resp_rx: Mutex::new(resp_rx),
            console_output,
            globals: RwLock::new(HashMap::new()),
        }
    }

    /// Evaluate a JavaScript expression and return the result.
    ///
    /// JS state persists across calls — variables, functions, closures
    /// defined in one `evaluate()` are available in the next.
    pub async fn evaluate(&mut self, expression: &str) -> Result<JsEvalResult> {
        // Clear shared console buffer
        self.console_output.write().unwrap().clear();

        // Send eval command
        self.cmd_tx
            .send(JsCommand::Eval {
                expression: expression.to_string(),
            })
            .expect("JS thread has died");

        // Wait for response
        let resp = self
            .resp_rx
            .lock()
            .unwrap()
            .recv()
            .expect("JS thread has died");

        match resp {
            JsResponse::EvalResult {
                value,
                exception,
                console_output,
            } => Ok(JsEvalResult {
                value,
                exception,
                console_output,
            }),
            JsResponse::Done => Ok(JsEvalResult::void()),
        }
    }

    /// Evaluate a script (multiple statements, no return value needed).
    pub async fn execute(&mut self, script: &str) -> Result<JsEvalResult> {
        self.evaluate(script).await
    }

    /// Get captured console output from the last eval.
    pub fn console_output(&self) -> Vec<String> {
        self.console_output.read().unwrap().clone()
    }

    /// Clear captured console output.
    pub fn clear_console(&mut self) {
        self.console_output.write().unwrap().clear();
    }

    /// Set a global variable — injected into the persistent JS Context.
    pub fn set_global(&mut self, name: impl Into<String>, value: Value) {
        let name = name.into();
        self.globals
            .write()
            .unwrap()
            .insert(name.clone(), value.clone());

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
            .unwrap()
            .recv()
            .expect("JS thread has died");
        let _ = resp; // JsResponse::Done
    }

    /// Get a global variable (Rust-side tracking).
    pub fn get_global(&self, name: &str) -> Option<Value> {
        self.globals.read().unwrap().get(name).cloned()
    }

    /// Set the DOM snapshot (called after navigate).
    ///
    /// Sends the snapshot to the JS thread so that `document.querySelector`
    /// and friends operate on real DOM data.
    pub fn set_dom_snapshot(&mut self, snapshot: Option<DomSnapshot>) {
        self.cmd_tx
            .send(JsCommand::SetDom { snapshot })
            .expect("JS thread has died");
        // Wait for ack
        let resp = self
            .resp_rx
            .lock()
            .unwrap()
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
        let _ = self.resp_rx.lock().unwrap().recv();
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
) {
    let dom_snapshot: Arc<RwLock<Option<DomSnapshot>>> = Arc::new(RwLock::new(None));
    let mut ctx = create_context(&console_output, &dom_snapshot);

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            JsCommand::Eval { expression } => {
                // Clear console buffer before eval
                console_output.write().unwrap().clear();

                let source = Source::from_bytes(&expression);
                let result = ctx.eval(source);

                let console = console_output.read().unwrap().clone();

                match result {
                    Ok(value) => {
                        let json_value = js_value_to_json(&value, &mut ctx);
                        let _ = resp_tx.send(JsResponse::EvalResult {
                            value: Some(json_value),
                            exception: None,
                            console_output: console,
                        });
                    }
                    Err(err) => {
                        let msg = format_js_error(&err, &mut ctx);
                        let _ = resp_tx.send(JsResponse::EvalResult {
                            value: None,
                            exception: Some(msg),
                            console_output: console,
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
                *dom_snapshot.write().unwrap() = snapshot;
                // Update document title/URL in the JS context
                let snap = dom_snapshot.read().unwrap();
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
) -> Context {
    let mut context = Context::default();

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
                        if let Ok(mut guard) = $out.write() {
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

    // --- Document object ---

    register_document_object(&mut context, dom_snapshot);

    context
}

// ---------------------------------------------------------------------------
// Document object registration
// ---------------------------------------------------------------------------

/// Register the `document` global object with DOM query methods.
fn register_document_object(
    ctx: &mut Context,
    dom_snapshot: &Arc<RwLock<Option<DomSnapshot>>>,
) {
    let dom_capture_title = dom_snapshot.clone();
    let title_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let dom = dom_capture_title.read().unwrap();
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
            let dom = dom_capture_url.read().unwrap();
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
            let _unused = dom_capture_cookie.read().unwrap();
            // Simplified: no cookie jar integration yet
            Ok(JsValue::from(JsString::from("")))
        })
    };
    let cookie_getter_fn = FunctionObjectBuilder::new(ctx.realm(), cookie_getter)
        .name(js_string!("get cookie"))
        .build();

    // querySelector(selector)
    let dom_capture_qs = dom_snapshot.clone();
    let query_selector_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let selector = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = dom_capture_qs.read().unwrap();
            if let Some(ref snapshot) = *dom {
                if let Some(node_id) = snapshot.query_selector(&selector) {
                    if let Some(node) = snapshot.nodes.get(&node_id) {
                        return Ok(create_element_object(snapshot, node, ctx));
                    }
                }
            }
            Ok(JsValue::null())
        })
    };

    // querySelectorAll(selector)
    let dom_capture_qsa = dom_snapshot.clone();
    let query_selector_all_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let selector = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = dom_capture_qsa.read().unwrap();
            if let Some(ref snapshot) = *dom {
                let ids = snapshot.query_selector_all(&selector);
                let js_values: Vec<JsValue> = ids
                    .iter()
                    .filter_map(|&id| {
                        snapshot.nodes.get(&id).map(|node| create_element_object(snapshot, node, ctx))
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
    let get_element_by_id_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let id = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = dom_capture_gbi.read().unwrap();
            if let Some(ref snapshot) = *dom {
                if let Some(node_id) = snapshot.get_element_by_id(&id) {
                    if let Some(node) = snapshot.nodes.get(&node_id) {
                        return Ok(create_element_object(snapshot, node, ctx));
                    }
                }
            }
            Ok(JsValue::null())
        })
    };

    // getElementsByTagName(tag)
    let dom_capture_gtn = dom_snapshot.clone();
    let get_elements_by_tag_name_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let tag = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = dom_capture_gtn.read().unwrap();
            if let Some(ref snapshot) = *dom {
                let ids = snapshot.get_elements_by_tag_name(&tag);
                let js_values: Vec<JsValue> = ids
                    .iter()
                    .filter_map(|&id| {
                        snapshot.nodes.get(&id).map(|node| create_element_object(snapshot, node, ctx))
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
    let get_elements_by_class_name_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let class = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let dom = dom_capture_gcn.read().unwrap();
            if let Some(ref snapshot) = *dom {
                let ids = snapshot.get_elements_by_class_name(&class);
                let js_values: Vec<JsValue> = ids
                    .iter()
                    .filter_map(|&id| {
                        snapshot.nodes.get(&id).map(|node| create_element_object(snapshot, node, ctx))
                    })
                    .collect();
                let arr = JsArray::from_iter(js_values, ctx);
                return Ok(arr.into());
            }
            let arr = JsArray::from_iter(Vec::<JsValue>::new(), ctx);
            Ok(arr.into())
        })
    };

    let document_obj = boa_engine::object::ObjectInitializer::new(ctx)
        .accessor(js_string!("title"), Some(title_getter_fn), None, Attribute::all())
        .accessor(js_string!("URL"), Some(url_getter_fn), None, Attribute::all())
        .accessor(js_string!("cookie"), Some(cookie_getter_fn), None, Attribute::all())
        .function(query_selector_fn, js_string!("querySelector"), 1)
        .function(query_selector_all_fn, js_string!("querySelectorAll"), 1)
        .function(get_element_by_id_fn, js_string!("getElementById"), 1)
        .function(get_elements_by_tag_name_fn, js_string!("getElementsByTagName"), 1)
        .function(get_elements_by_class_name_fn, js_string!("getElementsByClassName"), 1)
        .build();

    let _ = ctx.register_global_property(
        js_string!("document"),
        document_obj,
        Attribute::all(),
    );
}

/// Create a JS element object from a DomNode.
fn create_element_object(
    snapshot: &DomSnapshot,
    node: &DomNode,
    ctx: &mut Context,
) -> JsValue {
    let tag_upper = node.tag.to_uppercase();
    let id_val = node
        .attributes
        .get("id")
        .map(|s| s.as_str())
        .unwrap_or("");
    let class_val = node
        .attributes
        .get("class")
        .map(|s| s.as_str())
        .unwrap_or("");
    let href_val = node.attributes.get("href").map(|s| s.as_str()).unwrap_or("");
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
                            child.attributes.get("id").map(|s| s.as_str()).unwrap_or("")
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
                            pnode.attributes.get("id").map(|s| s.as_str()).unwrap_or("")
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
                if let Ok(result) = stringify_fn
                    .as_object()
                    .unwrap()
                    .call(&JsValue::undefined(), &[obj.clone().into()], context)
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

    if let Ok(s) = JsValue::from(obj.clone()).to_string(context) {
        let s = s.to_std_string_escaped();
        if s != "[object Object]" {
            return Value::String(s);
        }
    }

    Value::Object(serde_json::Map::new())
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
        let result = rt
            .evaluate("const sq = x => x * x; sq(5)")
            .await
            .unwrap();
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
        let result = rt
            .evaluate("/hello/.test('hello world')")
            .await
            .unwrap();
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
        let html = r#"<html><body><p class="intro">Hello</p><a href="/link">click</a></body></html>"#;
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
        let html = r#"<html><body><div class="item">a</div><div class="item">b</div></body></html>"#;
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
        let html = r#"<html><body><a href="https://example.com" class="link">click</a></body></html>"#;
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
}
