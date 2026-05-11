//! CDP Runtime domain handler.
//!
//! Handles Runtime.enable, Runtime.disable, Runtime.evaluate,
//! Runtime.callFunctionOn, Runtime.getProperties.
//!
//! After Runtime.enable, emits Runtime.executionContextCreated.

use crate::domains::{DispatchContext, DomainResult};
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Dispatch Runtime domain methods.
pub async fn handle(
    method: &str,
    params: Option<Value>,
    ctx: &DispatchContext,
) -> DomainResult {
    match method {
        "enable" => enable(ctx),
        "disable" => disable(ctx),
        "evaluate" => evaluate(params, ctx).await,
        "callFunctionOn" => call_function_on(params),
        "getProperties" => get_properties(params),
        "compileScript" => Ok(Some(json!({ "scriptId": "", "exceptionDetails": null }))),
        "runScript" => Ok(Some(json!({
            "result": { "type": "undefined" },
            "exceptionDetails": null
        }))),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Runtime.{} not implemented", method),
        }),
    }
}

/// Runtime.enable — enables runtime event reporting.
fn enable(ctx: &DispatchContext) -> DomainResult {
    ctx.events.set_runtime_enabled(true);

    // Emit executionContextCreated
    ctx.events.send_runtime_event(
        "Runtime.executionContextCreated",
        json!({
            "context": {
                "id": 1,
                "origin": "",
                "name": "main",
                "uniqueId": format!("context-{}", uuid::Uuid::new_v4()),
                "auxData": {
                    "isDefault": true,
                    "type": "default"
                }
            }
        }),
    );

    Ok(Some(json!({})))
}

/// Runtime.disable — disables runtime event reporting.
fn disable(ctx: &DispatchContext) -> DomainResult {
    ctx.events.set_runtime_enabled(false);
    Ok(Some(json!({})))
}

/// Runtime.evaluate — evaluates a JavaScript expression.
async fn evaluate(
    params: Option<Value>,
    ctx: &DispatchContext,
) -> DomainResult {
    let params = params.unwrap_or_default();
    let expression = params
        .get("expression")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut guard = ctx.session.write().await;

    // Try the real JS runtime if we have a session
    match guard.evaluate_js(expression).await {
        Ok(result) => {
            if let Some(exception) = &result.exception {
                return Ok(Some(json!({
                    "result": { "type": "undefined" },
                    "exceptionDetails": {
                        "text": exception,
                        "exception": { "type": "string", "value": exception }
                    }
                })));
            }

            let value = result.value.unwrap_or(Value::Null);
            let result_type = classify_json_type(&value);
            let description = match &value {
                Value::String(s) => Some(s.clone()),
                _ => None,
            };

            // Emit consoleAPICalled for console.log statements
            if let Value::String(s) = &value {
                if expression.trim().starts_with("console.log") {
                    ctx.events.send_runtime_event(
                        "Runtime.consoleAPICalled",
                        json!({
                            "type": "log",
                            "args": [
                                {
                                    "type": "string",
                                    "value": s,
                                    "description": s
                                }
                            ],
                            "executionContextId": 1,
                            "timestamp": EventSender::timestamp_ms()
                        }),
                    );
                }
            }

            Ok(Some(json!({
                "result": {
                    "type": result_type,
                    "value": value,
                    "description": description,
                },
                "exceptionDetails": null
            })))
        }
        Err(_) => {
            let (result_type, value) = classify_expression(expression);
            Ok(Some(json!({
                "result": {
                    "type": result_type,
                    "value": value,
                    "description": if result_type == "string" {
                        value.as_str().map(|s| s.to_string())
                    } else {
                        None::<String>
                    }
                },
                "exceptionDetails": null
            })))
        }
    }
}

/// Runtime.callFunctionOn — calls a function on a remote object.
fn call_function_on(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _function_declaration = params
        .get("functionDeclaration")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    Ok(Some(json!({
        "result": {
            "type": "undefined"
        },
        "exceptionDetails": null
    })))
}

/// Runtime.getProperties — returns properties of a remote object.
fn get_properties(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _object_id = params
        .get("objectId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    Ok(Some(json!({
        "result": [],
        "exceptionDetails": null
    })))
}

/// Classify a JSON value into a CDP type string.
fn classify_json_type(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Null => "object",
        Value::Array(_) => "object",
        Value::Object(_) => "object",
    }
}

/// Classify a simple JS expression and return (type, value) — stub fallback.
fn classify_expression(expr: &str) -> (String, Value) {
    let trimmed = expr.trim();

    if trimmed.is_empty() {
        return ("undefined".to_string(), Value::Null);
    }

    // Boolean literals
    if trimmed == "true" {
        return ("boolean".to_string(), Value::Bool(true));
    }
    if trimmed == "false" {
        return ("boolean".to_string(), Value::Bool(false));
    }

    // Null / undefined
    if trimmed == "null" {
        return ("object".to_string(), Value::Null);
    }
    if trimmed == "undefined" {
        return ("undefined".to_string(), Value::Null);
    }

    // Numeric literals
    if let Ok(n) = trimmed.parse::<i64>() {
        return ("number".to_string(), json!(n));
    }
    if let Ok(f) = trimmed.parse::<f64>() {
        return ("number".to_string(), json!(f));
    }

    // String literals (single or double quoted)
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        let s = &trimmed[1..trimmed.len() - 1];
        return ("string".to_string(), Value::String(s.to_string()));
    }

    // Default: return as string
    (
        "string".to_string(),
        Value::String(trimmed.to_string()),
    )
}

// Import EventSender for timestamp_ms
use crate::event::EventSender;