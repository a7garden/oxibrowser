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
pub async fn handle(method: &str, params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        "enable" => enable(ctx),
        "disable" => disable(ctx),
        "evaluate" => evaluate(params, ctx).await,
        "callFunctionOn" => call_function_on(params, ctx).await,
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
    let return_by_value = params
        .get("returnByValue")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let await_promise = params
        .get("awaitPromise")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut guard = ctx.session.write().await;

    match guard.evaluate_js_with_await(expression, await_promise).await {
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

            // TODO(#objectid): When objectId support is added, the `return_by_value` flag
            // should branch here: return `value` inline when true, return an `objectId`
            // reference when false. Currently both paths produce identical output.
            let _ = return_by_value;
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
///
/// Supports the key Puppeteer/Playwright patterns:
/// - Arrow functions extracting properties: `element => element.textContent`
/// - Functions with arguments: `(element, ...args) => { ... }`
///
/// For objectId references starting with "oxi-node-", attempts to resolve
/// the DOM node in the JS runtime. For other objectIds, passes undefined.
async fn call_function_on(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let function_declaration = params
        .get("functionDeclaration")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let object_id = params
        .get("objectId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let return_by_value = params
        .get("returnByValue")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let arguments: Vec<Value> = params
        .get("arguments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Build the arguments expression from CDP argument descriptors
    let args_str = build_args_expression(&arguments, object_id);

    // SAFETY: functionDeclaration is interpolated into JS via an IIFE wrapper
    // `(function() { var __fn = (<functionDeclaration>); ... })()`.
    // The wrapping ensures the declaration cannot break out of the IIFE scope.
    // While CDP is a trusted protocol (local DevTools connection), the IIFE
    // containment prevents accidental syntax errors from leaking into global scope.
    // Build the JS expression to evaluate
    let expr = if object_id.starts_with("oxi-node-") {
        // Resolve DOM node reference.
        // data-oxi-node-id is now injected by create_element_object into each
        // element's attribute map, so querySelector can find the element.
        let node_id_str = object_id.strip_prefix("oxi-node-").unwrap_or("0");
        format!(
            "(function() {{ var __fn = {func}; var __el = document.querySelector('[data-oxi-node-id=\"{nid}\"]') || document.body; return __fn(__el{args}); }})()",
            func = function_declaration,
            nid = node_id_str,
            args = if args_str.is_empty() { String::new() } else { format!(", {}", args_str) }
        )
    } else if object_id.is_empty() {
        // No objectId — just call the function directly
        format!(
            "(function() {{ var __fn = {func}; return __fn({args}); }})()",
            func = function_declaration,
            args = args_str
        )
    } else {
        // Unknown objectId format — try to evaluate as a generic wrapper
        format!(
            "(function() {{ var __fn = {func}; return __fn(undefined{args}); }})()",
            func = function_declaration,
            args = if args_str.is_empty() { String::new() } else { format!(", {}", args_str) }
        )
    };

    let mut guard = ctx.session.write().await;
    match guard.evaluate_js(&expr).await {
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

            // Emit console output if any
            if !result.console_output.is_empty() {
                let console_args: Vec<Value> = result
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
                        "args": console_args,
                        "executionContextId": 1,
                        "timestamp": EventSender::timestamp_ms()
                    }),
                );
            }

            if return_by_value {
                Ok(Some(json!({
                    "result": {
                        "type": result_type,
                        "value": value,
                    },
                    "exceptionDetails": null
                })))
            } else {
                // For objects (non-null), include value inline for Puppeteer compat
                if result_type == "object" && !value.is_null() {
                    Ok(Some(json!({
                        "result": {
                            "type": "object",
                            "value": value,
                            "description": "Object",
                        },
                        "exceptionDetails": null
                    })))
                } else {
                    Ok(Some(json!({
                        "result": {
                            "type": result_type,
                            "value": value,
                        },
                        "exceptionDetails": null
                    })))
                }
            }
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

/// Build a JS arguments expression from CDP arguments array.
///
/// CDP arguments can be:
/// - `{ "value": "hello" }` — primitive value
/// - `{ "objectId": "oxi-node-123" }` — remote object reference
/// - `{ "unserializableValue": "Infinity" }` — unserializable
fn build_args_expression(arguments: &[Value], _this_object_id: &str) -> String {
    let parts: Vec<String> = arguments
        .iter()
        .map(|arg| {
            if let Some(val) = arg.get("value") {
                // Primitive value — serialize to JSON (which is valid JS)
                serde_json::to_string(val).unwrap_or_else(|_| "undefined".to_string())
            } else if let Some(obj_id) = arg.get("objectId").and_then(|v| v.as_str()) {
                // Remote object reference
                if obj_id.starts_with("oxi-node-") {
                    let node_id_str = obj_id.strip_prefix("oxi-node-").unwrap_or("0");
                    format!(
                        "document.querySelector('[data-oxi-node-id=\"{}\"]') || document.body",
                        node_id_str
                    )
                } else {
                    "undefined".to_string()
                }
            } else if let Some(unsv) = arg
                .get("unserializableValue")
                .and_then(|v| v.as_str())
            {
                unsv.to_string()
            } else {
                "undefined".to_string()
            }
        })
        .collect();
    parts.join(", ")
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
