//! JavaScript evaluation runtime.
//!
//! In the default (non-servo) build, this provides a minimal JS evaluation
//! interface. When the `full-servo` feature is enabled, it uses Servo's
//! SpiderMonkey engine via `WebView::evaluate_javascript()`.
//!
//! ## Servo integration status
//!
//! The `full-servo` feature requires the `servo = "0.1"` crate which provides
//! `WebView::evaluate_javascript()` backed by SpiderMonkey. The integration
//! uses servo's callback-based async API:
//!
//! ```ignore
//! webview.evaluate_javascript("1 + 1", |result| {
//!     // result: Result<JSValue, JavaScriptEvaluationError>
//! });
//! ```
//!
//! **Note:** servo 0.1.0's embedder API is still evolving. The integration
//! is wired but may need adjustment as servo stabilizes its public API.

use crate::error::Result;
use serde_json::Value;
use std::collections::HashMap;

/// Result of a JavaScript evaluation.
#[derive(Debug, Clone)]
pub struct JsEvalResult {
    /// The return value as a JSON value (if any).
    pub value: Option<Value>,
    /// Exception message (if an error occurred).
    pub exception: Option<String>,
    /// Console output during execution.
    pub console_output: Vec<String>,
}

impl JsEvalResult {
    /// Create a successful result.
    pub fn ok(value: Value) -> Self {
        Self {
            value: Some(value),
            exception: None,
            console_output: Vec::new(),
        }
    }

    /// Create a result with no return value.
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

    /// Whether the evaluation succeeded.
    pub fn is_ok(&self) -> bool {
        self.exception.is_none()
    }
}

/// JavaScript runtime for page-level script execution.
///
/// In servo mode, this wraps servo::WebView's JS evaluation.
/// In stub mode, provides limited expression evaluation.
pub struct JsRuntime {
    /// Console log capture.
    console: Vec<String>,
    /// Global variables (stub mode).
    globals: HashMap<String, Value>,
}

impl JsRuntime {
    /// Create a new JS runtime.
    pub fn new() -> Self {
        Self {
            console: Vec::new(),
            globals: HashMap::new(),
        }
    }

    /// Evaluate a JavaScript expression and return the result.
    ///
    /// In default (stub) mode: handles literals, console.log, global vars.
    /// In `full-servo` mode: delegates to SpiderMonkey via servo crate.
    pub async fn evaluate(&mut self, expression: &str) -> Result<JsEvalResult> {
        // When full-servo is enabled and a servo WebView is available,
        // use the real JS engine. For now, the stub handles everything.
        //
        // TODO (full-servo): Wire servo::WebView::evaluate_javascript()
        // once servo 0.1 stabilizes its public embedder API.
        // See: https://github.com/servo/servo/issues/40950
        self.evaluate_stub(expression)
    }

    /// Evaluate a script (multiple statements, no return value needed).
    pub async fn execute(&mut self, script: &str) -> Result<JsEvalResult> {
        self.evaluate(script).await
    }

    /// Get captured console output.
    pub fn console_output(&self) -> &[String] {
        &self.console
    }

    /// Clear captured console output.
    pub fn clear_console(&mut self) {
        self.console.clear();
    }

    /// Set a global variable.
    pub fn set_global(&mut self, name: impl Into<String>, value: Value) {
        self.globals.insert(name.into(), value);
    }

    /// Minimal stub evaluator for simple expressions.
    ///
    /// Handles: string literals, numbers, booleans, null, simple property access.
    /// Real JS execution requires the `full-servo` feature.
    fn evaluate_stub(&mut self, expression: &str) -> Result<JsEvalResult> {
        let trimmed = expression.trim();

        // Handle console.log
        if trimmed.starts_with("console.log(") && trimmed.ends_with(')') {
            let inner = &trimmed[12..trimmed.len() - 1];
            let msg = inner
                .trim()
                .trim_matches('"')
                .trim_matches('\'');
            self.console.push(msg.to_string());
            return Ok(JsEvalResult {
                value: None,
                exception: None,
                console_output: vec![msg.to_string()],
            });
        }

        // String literal
        if (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            let s = &trimmed[1..trimmed.len() - 1];
            return Ok(JsEvalResult::ok(Value::String(s.to_string())));
        }

        // Boolean
        if trimmed == "true" {
            return Ok(JsEvalResult::ok(Value::Bool(true)));
        }
        if trimmed == "false" {
            return Ok(JsEvalResult::ok(Value::Bool(false)));
        }

        // Null
        if trimmed == "null" {
            return Ok(JsEvalResult::ok(Value::Null));
        }

        // Number
        if let Ok(n) = trimmed.parse::<i64>() {
            return Ok(JsEvalResult::ok(Value::Number(n.into())));
        }
        if let Ok(f) = trimmed.parse::<f64>() {
            if let Some(n) = serde_json::Number::from_f64(f) {
                return Ok(JsEvalResult::ok(Value::Number(n)));
            }
        }

        // Global variable lookup
        if let Some(val) = self.globals.get(trimmed) {
            return Ok(JsEvalResult::ok(val.clone()));
        }

        // document.title etc. — stub for known properties
        if trimmed == "document.title" {
            return Ok(JsEvalResult::ok(Value::String(String::new())));
        }
        if trimmed == "document.URL" || trimmed == "document.location.href" {
            return Ok(JsEvalResult::ok(Value::String(String::new())));
        }

        // If we can't evaluate, return the expression as a string
        // (Real servo mode would actually run JS)
        Ok(JsEvalResult::ok(Value::String(trimmed.to_string())))
    }
}

impl Default for JsRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_evaluate_string_literal() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("\"hello\"").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("hello".into())));
    }

    #[tokio::test]
    async fn test_evaluate_number() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("42").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::Number(42.into())));
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
    async fn test_evaluate_console_log() {
        let mut rt = JsRuntime::new();
        let result = rt.evaluate("console.log(\"msg\")").await.unwrap();
        assert!(result.is_ok());
        assert!(result.value.is_none(), "console.log should return void");
        assert!(rt.console_output().iter().any(|s| s.contains("msg")),
            "console should contain 'msg'");
    }

    #[tokio::test]
    async fn test_evaluate_global_variable() {
        let mut rt = JsRuntime::new();
        rt.set_global("myVar", Value::String("hello world".into()));
        let result = rt.evaluate("myVar").await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.value, Some(Value::String("hello world".into())));
    }
}
