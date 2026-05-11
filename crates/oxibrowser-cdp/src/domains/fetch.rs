//! CDP Fetch domain handler.
//!
//! Handles network interception via Fetch.enable, Fetch.disable,
//! Fetch.continueRequest, Fetch.failRequest, Fetch.fulfillRequest.
//!
//! When enabled, outgoing HTTP requests are paused and a
//! `Fetch.requestPaused` event is sent to the client.
//! The client responds with continue/fail/fulfill.
//!
//! Note: Interception is currently notification-only (emit events).
//! Full request pausing (blocking the HTTP request until the client
//! responds) requires deeper integration with the HTTP client pipeline.

use crate::domains::{DispatchContext, DomainResult};
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Dispatch Fetch domain methods.
pub async fn handle(
    method: &str,
    params: Option<Value>,
    ctx: &DispatchContext,
) -> DomainResult {
    match method {
        "enable" => enable(params, ctx),
        "disable" => disable(ctx),
        "continueRequest" => Ok(Some(json!({}))),
        "failRequest" => Ok(Some(json!({}))),
        "fulfillRequest" => Ok(Some(json!({}))),
        "continueResponse" => Ok(Some(json!({}))),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Fetch.{} not implemented", method),
        }),
    }
}

/// Fetch.enable — enables request interception.
///
/// Accepts optional `patterns` array and `handleAuthRequests` flag.
fn enable(_params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    // TODO: Parse patterns and store them for request matching.
    // Currently, interception is notification-only.
    ctx.events.set_fetch_enabled(true);
    tracing::info!("Fetch domain enabled");
    Ok(Some(json!({})))
}

/// Fetch.disable — disables request interception.
fn disable(ctx: &DispatchContext) -> DomainResult {
    ctx.events.set_fetch_enabled(false);
    tracing::info!("Fetch domain disabled");
    Ok(Some(json!({})))
}

/// Emit a `Fetch.requestPaused` event for an intercepted request.
///
/// Called from the navigation pipeline when Fetch domain is enabled.
pub fn emit_request_paused(
    events: &crate::event::EventSender,
    request_id: &str,
    url: &str,
    resource_type: &str,
) {
    events.send_fetch_event(
        "Fetch.requestPaused",
        serde_json::json!({
            "requestId": request_id,
            "request": {
                "url": url,
                "method": "GET",
                "headers": {},
                "initialPriority": "VeryHigh",
                "urlFragment": "",
            },
            "resourceType": resource_type,
            "frameId": "main",
            "networkIntercepted": true,
        }),
    );
}
