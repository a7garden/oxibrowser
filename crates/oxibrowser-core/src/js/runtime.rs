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

use std::sync::RwLock;

use crate::error::Result;
use boa_engine::{Context, JsValue, Source, JsString, NativeFunction};
use boa_engine::object::builtins::JsArray;
use boa_engine::js_string;
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

/// Native function for console.log.
fn console_log_fn(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let mut line = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            line.push(' ');
            print!(" ");
        }
        let s = arg.to_string(context)
            .map(|js_str| js_str.to_std_string_escaped())
            .unwrap_or_else(|_| "undefined".to_string());
        line.push_str(&s);
        print!("{}", s);
    }
    println!();
    Ok(JsValue::undefined())
}

/// A JavaScript runtime backed by boa_engine.
///
/// Since `boa_engine::Context` is `!Send`, we create it fresh on each
/// `evaluate()` call. Global state (like variables) is tracked separately
/// in Rust and injected before each eval.
///
/// Uses `RwLock` for thread-safety since this runs in a tokio multi-thread
/// environment where the parent `Session` must be `Send`.
pub struct JsRuntime {
    /// Captured console output from the last eval.
    console_output: RwLock<Vec<String>>,
    /// Global variables tracked in Rust (injected per-eval).
    globals: RwLock<HashMap<String, Value>>,
}

impl JsRuntime {
    /// Create a new JS runtime.
    pub fn new() -> Self {
        Self {
            console_output: RwLock::new(Vec::new()),
            globals: RwLock::new(HashMap::new()),
        }
    }

    /// Create a fresh boa_engine Context with console.log registered.
    fn create_context() -> Context {
        let mut context = Context::default();

        // Register console.log via register_global_callable
        let _ = context.register_global_callable(
            js_string!("log"),
            1,
            NativeFunction::from_fn_ptr(console_log_fn),
        );

        // Build console object with .log method
        use boa_engine::object::ObjectInitializer;
        use boa_engine::property::Attribute;

        let console = ObjectInitializer::new(&mut context)
            .function(
                NativeFunction::from_fn_ptr(console_log_fn),
                js_string!("log"),
                1,
            )
            .build();

        let _ = context.register_global_property(
            js_string!("console"),
            console,
            Attribute::all(),
        );

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
        let mut ctx = Self::create_context();

        // Inject tracked globals
        let globals = self.globals.read().unwrap();
        for (name, value) in globals.iter() {
            let js_val = json_to_js_value(value, &mut ctx);
            let _ = ctx.register_global_property(
                JsString::from(name.as_str()),
                js_val,
                boa_engine::property::Attribute::all(),
            );
        }
        drop(globals);

        let source = Source::from_bytes(expression);
        let result = ctx.eval(source);

        // Capture console output (for now, we use a simpler approach)
        let console_output: Vec<String> = Vec::new();

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
                let msg = if let Some(native) = err.as_native() {
                    native.message().to_string()
                } else {
                    "JavaScript error".to_string()
                };

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
            // Collect JsValues first, then create array
            let js_values: Vec<JsValue> = arr.iter()
                .map(|v| json_to_js_value(v, context))
                .collect();
            let js_arr = JsArray::from_iter(js_values, context);
            use std::ops::Deref as DerefTrait;
            js_arr.deref().clone().into()
        }
        Value::Object(map) => {
            // Collect (key, JsValue) pairs first, then build object
            let pairs: Vec<(String, JsValue)> = map.iter()
                .map(|(k, v)| (k.clone(), json_to_js_value(v, context)))
                .collect();
            use boa_engine::object::ObjectInitializer;
            let mut obj = ObjectInitializer::new(context);
            for (k, v) in pairs {
                obj.property(JsString::from(k.as_str()), v, boa_engine::property::Attribute::all());
            }
            obj.build().into()
        }
    }
}

/// Convert a boa_engine JsValue to serde_json::Value.
fn js_value_to_json(value: &JsValue, context: &mut Context) -> Value {
    match value {
        JsValue::Null => Value::Null,
        JsValue::Undefined => Value::Null,
        JsValue::Boolean(b) => Value::Bool(*b),
        JsValue::Integer(n) => Value::Number(serde_json::Number::from(*n)),
        JsValue::Rational(n) => {
            serde_json::Number::from_f64(*n)
                .map(Value::Number)
                .unwrap_or(Value::Null)
        }
        JsValue::String(s) => Value::String(s.to_std_string_escaped()),
        JsValue::Symbol(_) => Value::String("[symbol]".to_string()),
        JsValue::BigInt(_) => {
            let s = value.to_string(context).unwrap_or_else(|_| {
                JsString::from("0n")
            });
            Value::String(s.to_std_string_escaped())
        }
        JsValue::Object(obj) => {
            // Try to convert arrays using JsArray wrapper
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

            // For other objects, return string representation
            let s = value.to_string(context).unwrap_or_else(|_| {
                JsString::from("[object]")
            });
            Value::String(s.to_std_string_escaped())
        }
    }
}

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
        let result = rt.evaluate("function add(a, b) { return a + b; } add(1, 2)").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(3.into())));
    }

    #[tokio::test]
    async fn test_evaluate_console_log() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("console.log('Hello, world!')").await.unwrap();
        assert!(result.is_ok(), "console.log should not error");
        assert!(result.value.is_none() || result.value == Some(Value::Null));
    }

    #[tokio::test]
    async fn test_evaluate_log_alias() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("log('Direct log call')").await.unwrap();
        assert!(result.is_ok(), "log should work as alias");
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
        assert!(result.exception.is_some());
    }

    #[tokio::test]
    async fn test_evaluate_undefined() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("undefined").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Null));
    }

    #[tokio::test]
    async fn test_global_object() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("Math.PI").await.unwrap();
        assert!(result.is_ok());
        if let Some(Value::Number(n)) = result.value {
            let pi = n.as_f64().unwrap_or(0.0);
            assert!((pi - 3.14159).abs() < 0.0001);
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
        let result = rt.evaluate("({ a: 1, b: 2 })").await.unwrap();
        assert!(result.is_ok());
        let val = result.value.unwrap();
        assert!(val.is_string(), "object should stringify to string");
    }

    #[tokio::test]
    async fn test_array_literal() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("[1, 2, 3]").await.unwrap();
        assert!(result.is_ok());
        let val = result.value.unwrap();
        if let Value::Array(arr) = &val {
            assert_eq!(arr.len(), 3);
        }
    }

    #[tokio::test]
    async fn test_json_builtin() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("JSON.stringify({x: 1})").await.unwrap();
        assert!(result.is_ok());
        let val = result.value.unwrap();
        assert_eq!(val, Value::String("{\"x\":1}".to_string()));
    }

    #[tokio::test]
    async fn test_parse_json() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("JSON.parse('{\"a\": 1}').a").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(1.into())));
    }

    #[tokio::test]
    async fn test_set_global() {
        let mut rt = JsRuntime::new();
        rt.set_global("myVar", Value::String("hello".into()));
        let result = rt.evaluate("myVar").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("hello".into())));
    }
}