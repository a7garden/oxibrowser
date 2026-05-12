//! JavaScript runtime using boa_engine.
//!
//! boa_engine is a pure Rust JavaScript engine (ES2024+), no C dependencies.
//! Provides real JS evaluation with console.log and Math, JSON, etc.
//!
//! ## Architecture note
//!
//! `boa_engine::Context` is `!Send` (internal GC pointers use `NonNull`).
//! To keep `Session: Send` for tokio, we create a fresh `Context` on each
//! `evaluate()` call rather than storing one persistently. This is a small
//! overhead per eval (~μs) and avoids complex channel-based evaluator designs.

use std::ops::Deref;
use std::sync::{Arc, RwLock};

use crate::error::Result;
use boa_engine::object::builtins::JsArray;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsString, JsValue, NativeFunction, Source};
use serde_json::Value;
use std::collections::HashMap;

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

/// A JavaScript runtime backed by boa_engine.
///
/// Since `boa_engine::Context` is `!Send`, we create it fresh on each
/// `evaluate()` call. Global state (like variables) is tracked separately
/// in Rust and injected before each eval.
///
/// Uses `Arc<RwLock<...>>` for thread-safety since this runs in a tokio
/// multi-thread environment where the parent `Session` must be `Send`.
pub struct JsRuntime {
    /// Shared console output buffer (shared with boa closures).
    console_output: Arc<RwLock<Vec<String>>>,
    /// Global variables tracked in Rust (injected per-eval).
    globals: RwLock<HashMap<String, Value>>,
}

impl JsRuntime {
    /// Create a new JS runtime.
    pub fn new() -> Self {
        Self {
            console_output: Arc::new(RwLock::new(Vec::new())),
            globals: RwLock::new(HashMap::new()),
        }
    }

    /// Create a fresh boa_engine Context with console.log registered.
    fn create_context(output: Arc<RwLock<Vec<String>>>) -> Context {
        let mut context = Context::default();

        // Clone the Arc for each closure
        let out_log = output.clone();
        let out_warn = output.clone();
        let out_error = output.clone();
        let out_info = output.clone();

        // Helper: build a console.* function closure
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

        let log_fn = console_fn!(out_log);

        // Register standalone `log(...)` function
        let _ = context.register_global_callable(js_string!("log"), 1, log_fn.clone());

        // Build console object with .log, .warn, .error, .info methods
        let console = boa_engine::object::ObjectInitializer::new(&mut context)
            .function(log_fn, js_string!("log"), 1)
            .function(console_fn!(out_warn), js_string!("warn"), 1)
            .function(console_fn!(out_error), js_string!("error"), 1)
            .function(console_fn!(out_info), js_string!("info"), 1)
            .build();

        let _ = context.register_global_property(js_string!("console"), console, Attribute::all());

        context
    }

    /// Evaluate a JavaScript expression and return the result.
    pub async fn evaluate(&mut self, expression: &str) -> Result<JsEvalResult> {
        // Clear previous output
        {
            let mut guard = self.console_output.write().unwrap();
            guard.clear();
        }

        // Create a fresh context for this evaluation
        let mut ctx = Self::create_context(self.console_output.clone());

        // Inject tracked globals
        {
            let globals = self.globals.read().unwrap();
            for (name, value) in globals.iter() {
                let js_val = json_to_js_value(value, &mut ctx);
                let _ = ctx.register_global_property(
                    JsString::from(name.as_str()),
                    js_val,
                    Attribute::all(),
                );
            }
        }

        let source = Source::from_bytes(expression);
        let result = ctx.eval(source);

        // Collect console output
        let console_output = self.console_output.read().unwrap().clone();

        match result {
            Ok(value) => {
                let json_value = js_value_to_json(&value, &mut ctx);
                Ok(JsEvalResult {
                    value: Some(json_value),
                    exception: None,
                    console_output,
                })
            }
            Err(err) => {
                let msg = format_js_error(&err, &mut ctx);
                Ok(JsEvalResult {
                    value: None,
                    exception: Some(msg),
                    console_output,
                })
            }
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

    /// Set a global variable (Rust-side, injected before each eval).
    pub fn set_global(&mut self, name: impl Into<String>, value: Value) {
        self.globals.write().unwrap().insert(name.into(), value);
    }

    /// Get a global variable.
    pub fn get_global(&self, name: &str) -> Option<Value> {
        self.globals.read().unwrap().get(name).cloned()
    }
}

impl Default for JsRuntime {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// JsValue ↔ serde_json::Value conversions
// ---------------------------------------------------------------------------

/// Convert a serde_json Value to a boa_engine JsValue.
fn json_to_js_value(value: &Value, context: &mut Context) -> JsValue {
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
///
/// Uses `JSON.stringify` for objects to get proper serialization.
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
            // Arrays → JSON array via JsArray::at()
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

            // Objects → use JSON.stringify to serialize, then parse
            object_to_json_via_stringify(obj, context)
        }
    }
}

/// Convert a JS object to JSON by calling `JSON.stringify(obj)` and parsing the result.
///
/// This is the most reliable way to convert arbitrary JS objects to serde_json,
/// since it handles nested objects, arrays, etc. correctly.
fn object_to_json_via_stringify(obj: &boa_engine::JsObject, context: &mut Context) -> Value {
    // Call JSON.stringify(obj)
    let json_global = context
        .global_object()
        .get(js_string!("JSON"), context)
        .unwrap_or_else(|_| JsValue::undefined());

    if let Some(json_obj) = json_global.as_object() {
        if let Ok(stringify_fn) = json_obj.get(js_string!("stringify"), context) {
            if stringify_fn.is_callable() {
                if let Ok(result) =
                    stringify_fn
                        .as_object()
                        .unwrap()
                        .call(&JsValue::undefined(), &[obj.clone().into()], context)
                {
                    if let Some(s) = result.as_string() {
                        let json_str = s.to_std_string_escaped();
                        if let Ok(parsed) = serde_json::from_str::<Value>(&json_str) {
                            return parsed;
                        }
                        // If parsing failed, return the raw string
                        return Value::String(json_str);
                    }
                }
            }
        }
    }

    // Fallback: try toString()
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

/// Format a JsError into a human-readable string with as much detail as possible.
fn format_js_error(err: &boa_engine::JsError, context: &mut Context) -> String {
    // Try native error first (SyntaxError, TypeError, RangeError, etc.)
    if let Some(native) = err.as_native() {
        let kind = format!("{:?}", native.kind).to_lowercase();
        let msg = native.message();
        if msg.is_empty() {
            return kind;
        }
        return format!("{}: {}", kind, msg);
    }

    // For opaque errors (thrown JS values), try to extract info
    if let Some(opaque) = err.as_opaque() {
        // Try to convert to string
        if let Ok(s) = opaque.to_string(context) {
            let s = s.to_std_string_escaped();
            if !s.is_empty() && s != "undefined" {
                return s;
            }
        }

        // Try to extract .message and .name from error objects
        if let Some(obj) = opaque.as_object() {
            if let Ok(msg_val) = obj.get(js_string!("message"), context) {
                if let Some(msg) = msg_val.as_string() {
                    let msg_str = msg.to_std_string_escaped();
                    if !msg_str.is_empty() {
                        if let Ok(name_val) = obj.get(js_string!("name"), context) {
                            if let Some(name) = name_val.as_string() {
                                return format!(
                                    "{}: {}",
                                    name.to_std_string_escaped(),
                                    msg_str
                                );
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
    async fn test_evaluate_arithmetic() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("2 + 3 * 4").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(14.into())));
    }

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

    #[tokio::test]
    async fn test_console_log_capture() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("console.log('Hello, world!')").await.unwrap();
        assert!(result.is_ok(), "console.log should not error");
        assert_eq!(result.value, Some(Value::Null));
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
            .evaluate("console.warn('warning'); console.error('err'); console.info('info')")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.console_output.len(), 3);
        assert_eq!(result.console_output[0], "warning");
        assert_eq!(result.console_output[1], "err");
        assert_eq!(result.console_output[2], "info");
    }

    #[tokio::test]
    async fn test_evaluate_expression() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("'hello ' + 'world'").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("hello world".into())));
    }

    #[tokio::test]
    async fn test_evaluate_error() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("throw new Error('oops')").await.unwrap();
        assert!(!result.is_ok());
        let msg = result.exception.unwrap();
        assert!(msg.contains("Error"), "should contain error type: {}", msg);
        assert!(msg.contains("oops"), "should contain error message: {}", msg);
    }

    #[tokio::test]
    async fn test_syntax_error() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("function {").await.unwrap();
        assert!(!result.is_ok());
        let msg = result.exception.unwrap();
        assert!(
            msg.to_lowercase().contains("syntax"),
            "should be syntax error: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_type_error() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("undefined.foo").await.unwrap();
        assert!(!result.is_ok());
        let msg = result.exception.unwrap();
        assert!(
            msg.to_lowercase().contains("type"),
            "should be type error: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_evaluate_undefined() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("undefined").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Null));
    }

    #[tokio::test]
    async fn test_global_math() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("Math.PI").await.unwrap();
        assert!(result.is_ok());
        if let Some(Value::Number(n)) = result.value {
            let pi = n.as_f64().unwrap_or(0.0);
            assert!((pi - 3.14159).abs() < 0.0001);
        } else {
            panic!("expected number, got {:?}", result.value);
        }
    }

    #[tokio::test]
    async fn test_array_length() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("[1, 2, 3].length").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(3.into())));
    }

    #[tokio::test]
    async fn test_object_literal() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("({ a: 1, b: 'hello' })").await.unwrap();
        assert!(result.is_ok());
        let val = result.value.unwrap();
        assert!(val.is_object(), "object literal should produce JSON object");
        let map = val.as_object().unwrap();
        assert_eq!(map.get("a"), Some(&Value::Number(1.into())));
        assert_eq!(map.get("b"), Some(&Value::String("hello".into())));
    }

    #[tokio::test]
    async fn test_nested_object() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("({ user: { name: 'Alice', age: 30 }, active: true })")
            .await
            .unwrap();
        assert!(result.is_ok());
        let val = result.value.unwrap();
        let map = val.as_object().unwrap();
        assert_eq!(map.get("active"), Some(&Value::Bool(true)));
        let user = map.get("user").unwrap().as_object().unwrap();
        assert_eq!(user.get("name"), Some(&Value::String("Alice".into())));
    }

    #[tokio::test]
    async fn test_array_literal() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("[1, 2, 3]").await.unwrap();
        assert!(result.is_ok());
        let val = result.value.unwrap();
        let arr = val.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], Value::Number(1.into()));
        assert_eq!(arr[2], Value::Number(3.into()));
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
        let val = result.value.unwrap();
        // boa stores all numbers as f64, so 42+8 = 50.0
        assert_eq!(val.as_f64().unwrap(), 50.0);
    }

    #[tokio::test]
    async fn test_set_global_object() {
        let mut rt = JsRuntime::new();
        rt.set_global(
            "config",
            serde_json::json!({ "name": "test", "value": 123 }),
        );
        let result = rt.evaluate("config.name").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("test".into())));
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
        assert_eq!(result.console_output[0], "line1");
        assert_eq!(result.console_output[1], "line2");
    }

    #[tokio::test]
    async fn test_array_map() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("[1, 2, 3].map(x => x * 2)").await.unwrap();
        assert!(result.is_ok());
        let arr = result.value.unwrap().as_array().unwrap().clone();
        assert_eq!(
            arr,
            vec![
                Value::Number(2.into()),
                Value::Number(4.into()),
                Value::Number(6.into())
            ]
        );
    }

    #[tokio::test]
    async fn test_string_split() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("'hello world'.split(' ')").await.unwrap();
        assert!(result.is_ok());
        let arr = result.value.unwrap().as_array().unwrap().clone();
        assert_eq!(arr[0], Value::String("hello".into()));
        assert_eq!(arr[1], Value::String("world".into()));
    }

    #[tokio::test]
    async fn test_template_literal() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("`hello ${1 + 2}`").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("hello 3".into())));
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
    async fn test_arrow_function() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("const square = x => x * x; square(5)")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(25.into())));
    }

    #[tokio::test]
    async fn test_closure() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("const counter = (function() { let n = 0; return () => ++n; })(); counter()")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(1.into())));
    }

    #[tokio::test]
    async fn test_spread_operator() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("[...[1,2], 3]").await.unwrap();
        assert!(result.is_ok());
        let arr = result.value.unwrap().as_array().unwrap().clone();
        assert_eq!(arr.len(), 3);
    }

    #[tokio::test]
    async fn test_object_methods() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("Object.keys({a: 1, b: 2}).length")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(2.into())));
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
    async fn test_regex() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("/hello/.test('hello world')").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Bool(true)));
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
    async fn test_array_reduce() {
        let mut rt = JsRuntime::new();
        let result = rt
            .evaluate("[1,2,3,4,5].reduce((acc, x) => acc + x, 0)")
            .await
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(15.into())));
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
}
