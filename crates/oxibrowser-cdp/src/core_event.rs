//! Bridge from core-originated [`CoreEvent`]s to CDP events.
//!
//! The JS thread (in `oxibrowser-core`) cannot name CDP types, so it emits a
//! neutral [`CoreEvent`] enum onto a shared channel. The CDP session spawns a
//! drainer task that calls [`emit_core_event`] for each one, translating it
//! into the appropriate CDP events (gated by the relevant domain's enabled
//! flag on the [`EventSender`]).

use crate::event::EventSender;
use oxibrowser_core::js::{ConsoleArg, ConsoleLevel, CoreEvent, WsDirection};
use serde_json::{Value, json};

/// Translate one [`CoreEvent`] into its CDP event(s) on the given sender.
///
/// Each translation is independently gated by the owning domain's enabled flag,
/// mirroring real Chrome: a `console.log` emits both `Runtime.consoleAPICalled`
/// (gated by `Runtime.enable`) and `Log.entryAdded` (gated by `Log.enable`).
pub fn emit_core_event(events: &EventSender, ev: CoreEvent) {
    match ev {
        CoreEvent::Console {
            level,
            args,
            timestamp,
        } => emit_console(events, level, args, timestamp),
        CoreEvent::Exception {
            message,
            name,
            stack,
            timestamp,
        } => emit_exception(events, message, name, stack, timestamp),
        CoreEvent::FetchRequest {
            request_id,
            url,
            method,
            headers,
            post_data,
            timestamp,
        } => emit_request_will_be_sent(
            events, request_id, url, method, headers, post_data, timestamp,
        ),
        CoreEvent::FetchResponse {
            request_id,
            url,
            status,
            mime_type,
            timestamp,
        } => emit_response_received(events, request_id, url, status, mime_type, timestamp),
        CoreEvent::FetchLoadingFinished {
            request_id,
            timestamp,
        } => {
            events.send_network_event(
                "Network.loadingFinished",
                json!({
                    "requestId": request_id,
                    "timestamp": timestamp / 1000.0,
                    "encodedDataLength": 0.0,
                }),
            );
        }
        CoreEvent::WsFrame {
            direction,
            request_id,
            opcode: _,
            data,
            timestamp,
        } => {
            let method = match direction {
                WsDirection::Sent => "Network.webSocketFrameSent",
                WsDirection::Received => "Network.webSocketFrameReceived",
            };
            events.send_network_event(
                method,
                json!({
                    "requestId": request_id,
                    "timestamp": timestamp / 1000.0,
                    "response": {
                        "opcode": 1,
                        "mask": true,
                        "payloadData": data,
                    },
                }),
            );
        }
        CoreEvent::Dialog {
            dialog_type,
            message,
            default_value,
        } => {
            events.send_page_event(
                "Page.javascriptDialogOpening",
                json!({
                    "url": "",
                    "message": message,
                    "type": dialog_type.as_str(),
                    "defaultPrompt": default_value.unwrap_or_default(),
                }),
            );
        }
    }
}

fn emit_console(events: &EventSender, level: ConsoleLevel, args: Vec<ConsoleArg>, timestamp: f64) {
    // Runtime.consoleAPICalled — typed RemoteObjects (number/boolean/object/
    // null/undefined), not always-string.
    let remote_args: Vec<Value> = args.iter().map(console_arg_to_remote_object).collect();
    events.send_runtime_event(
        "Runtime.consoleAPICalled",
        json!({
            "type": level.api_type(),
            "args": remote_args,
            "executionContextId": 1,
            "timestamp": timestamp,
        }),
    );

    // Log.entryAdded — mirror console messages into the Log domain.
    let text = args
        .iter()
        .map(|a| a.display())
        .collect::<Vec<_>>()
        .join(" ");
    events.send_log_event(
        "Log.entryAdded",
        json!({
            "entry": {
                "source": "console",
                "level": log_level(&level),
                "text": text,
                "timestamp": timestamp,
                "url": Value::Null,
                "lineNumber": Value::Null,
            }
        }),
    );
}

/// Map a [`ConsoleArg`] to a CDP `RemoteObject` JSON value.
fn console_arg_to_remote_object(arg: &ConsoleArg) -> Value {
    match arg {
        ConsoleArg::String(s) => json!({ "type": "string", "value": s }),
        ConsoleArg::Number(n) => {
            // CDP encodes NaN/Infinity as their string forms inside `value`.
            let value = if n.is_nan() {
                json!("NaN")
            } else if *n == f64::INFINITY {
                json!("Infinity")
            } else if *n == f64::NEG_INFINITY {
                json!("-Infinity")
            } else {
                json!(n)
            };
            json!({ "type": "number", "value": value })
        }
        ConsoleArg::Boolean(b) => json!({ "type": "boolean", "value": b }),
        ConsoleArg::Null => json!({ "type": "object", "subtype": "null", "value": Value::Null }),
        ConsoleArg::Undefined => json!({ "type": "undefined" }),
        ConsoleArg::Object {
            class_name,
            description,
        } => json!({
            "type": "object",
            "className": class_name,
            "description": description,
        }),
    }
}

fn emit_exception(
    events: &EventSender,
    message: String,
    name: String,
    stack: Option<String>,
    timestamp: f64,
) {
    // boa 0.20 has no real source locations; the stack (if any) is a synthetic
    // trace string. Surface it as the stackTrace description with a single
    // placeholder frame so clients that demand a non-empty callFrames array
    // don't choke.
    let stack_trace = match &stack {
        Some(s) => json!({
            "callFrames": [{ "functionName": "", "scriptId": "0", "url": "", "lineNumber": 0, "columnNumber": 0 }],
            "description": s
        }),
        None => json!({ "callFrames": [] }),
    };
    events.send_runtime_event(
        "Runtime.exceptionThrown",
        json!({
            "timestamp": timestamp,
            "exceptionDetails": {
                "exceptionId": 1,
                "text": message,
                "lineNumber": 0,
                "columnNumber": 0,
                "scriptId": "0",
                "stackTrace": stack_trace,
                "exception": {
                    "type": "object",
                    "subtype": "error",
                    "className": name,
                    "description": message,
                }
            }
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_request_will_be_sent(
    events: &EventSender,
    request_id: String,
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    post_data: Option<Vec<u8>>,
    timestamp: f64,
) {
    let hdrs: Value = headers
        .into_iter()
        .fold(serde_json::Map::new(), |mut acc, (k, v)| {
            acc.insert(k, Value::String(v));
            acc
        })
        .into();
    let mut request = json!({
        "url": url,
        "method": method,
        "headers": hdrs,
        "initialPriority": "High",
        "urlFragment": "",
    });
    if let Some(body) = post_data
        && let Ok(s) = String::from_utf8(body)
    {
        request["postData"] = Value::String(s);
    }
    events.send_network_event(
        "Network.requestWillBeSent",
        json!({
            "requestId": request_id,
            "loaderId": "0",
            "documentURL": "",
            "request": request,
            "timestamp": timestamp / 1000.0,
            "wallTime": timestamp / 1000.0,
            "initiator": { "type": "script" },
            "type": "XHR",
            "frameId": "main",
            "hasUserGesture": false,
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_response_received(
    events: &EventSender,
    request_id: String,
    url: String,
    status: u16,
    mime_type: String,
    timestamp: f64,
) {
    let (hdrs, mime) = if mime_type.is_empty() {
        (json!({}), "text/plain")
    } else {
        (json!({ "Content-Type": mime_type }), mime_type.as_str())
    };
    events.send_network_event(
        "Network.responseReceived",
        json!({
            "requestId": request_id,
            "loaderId": "0",
            "timestamp": timestamp / 1000.0,
            "type": "XHR",
            "response": {
                "url": url,
                "status": status,
                "statusText": "",
                "headers": hdrs,
                "mimeType": mime,
                "connectionReused": false,
                "connectionId": 0.0,
                "encodedDataLength": 0.0,
                "securityState": "secure",
            },
            "frameId": "main",
        }),
    );
}

/// Map a [`ConsoleLevel`] to a `Log.entryAdded` level string.
fn log_level(level: &ConsoleLevel) -> &'static str {
    match level {
        ConsoleLevel::Log | ConsoleLevel::Info => "info",
        ConsoleLevel::Warn => "warning",
        ConsoleLevel::Error => "error",
    }
}
