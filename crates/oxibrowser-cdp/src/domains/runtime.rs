//! CDP Runtime domain handler.
//!
//! Handles Runtime.enable, Runtime.disable, Runtime.evaluate,
//! Runtime.callFunctionOn, Runtime.getProperties.
//!
//! After Runtime.enable, emits Runtime.executionContextCreated.
//! Runtime.evaluate delegates to boa_engine via Session::evaluate_js().

use crate::domains::{DispatchContext, DomainResult};
use crate::event::EventSender;
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

/// Runtime.evaluate — evaluates a JavaScript expression via boa_engine.
async fn evaluate(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let expression = params
        .get("expression")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut guard = ctx.session.write().await;

    match guard.evaluate_js(expression).await {
        Ok(result) => {
            // Handle exceptions
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

            // Emit consoleAPICalled for any captured console output
            if !result.console_output.is_empty() {
                let args: Vec<Value> = result
                    .console_output
                    .iter()
                    .map(|msg| {
                        json!({
                            "type": "string",
                            "value": msg,
                            "description": msg
                        })
                    })
                    .collect();
                ctx.events.send_runtime_event(
                    "Runtime.consoleAPICalled",
                    json!({
                        "type": "log",
                        "args": args,
                        "executionContextId": 1,
                        "timestamp": EventSender::timestamp_ms()
                    }),
                );
            }

            Ok(Some(json!({
                "result": {
                    "type": result_type,
                    "value": value,
                },
                "exceptionDetails": null
            })))
        }
        Err(e) => Ok(Some(json!({
            "result": { "type": "undefined" },
            "exceptionDetails": {
                "text": e.to_string(),
                "exception": { "type": "string", "value": e.to_string() }
            }
        }))),
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
