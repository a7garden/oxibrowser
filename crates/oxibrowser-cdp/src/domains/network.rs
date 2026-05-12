//! CDP Network domain handler.
//!
//! Handles Network.enable, Network.disable, and emits network lifecycle
//! events when enabled:
//! - Network.requestWillBeSent
//! - Network.responseReceived
//! - Network.loadingFinished

use crate::domains::{DispatchContext, DomainResult};
use crate::event::EventSender;
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Dispatch Network domain methods.
pub async fn handle(method: &str, _params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        "enable" => enable(ctx),
        "disable" => disable(ctx),
        "setCacheDisabled" => Ok(Some(json!({}))),
        "setExtraHTTPHeaders" => Ok(Some(json!({}))),
        "emulateNetworkConditions" => Ok(Some(json!({}))),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Network.{} not implemented", method),
        }),
    }
}

/// Network.enable — enables network tracking, events will be sent.
fn enable(ctx: &DispatchContext) -> DomainResult {
    ctx.events.set_network_enabled(true);
    Ok(Some(json!({})))
}

/// Network.disable — disables network tracking.
fn disable(ctx: &DispatchContext) -> DomainResult {
    ctx.events.set_network_enabled(false);
    Ok(Some(json!({})))
}

/// Emit network events for a navigation request.
///
/// Called from Page.navigate when Network domain is enabled.
pub fn emit_navigation_events(
    events: &EventSender,
    request_id: &str,
    url: &str,
    loader_id: &str,
    status: u16,
    content_type: &str,
) {
    let timestamp = EventSender::timestamp_ms();

    // Network.requestWillBeSent
    events.send_network_event(
        "Network.requestWillBeSent",
        json!({
            "requestId": request_id,
            "loaderId": loader_id,
            "documentURL": url,
            "request": {
                "url": url,
                "method": "GET",
                "headers": {},
                "initialPriority": "VeryHigh",
                "urlFragment": "",
            },
            "timestamp": timestamp,
            "wallTime": timestamp / 1000.0,
            "initiator": {
                "type": "other"
            },
            "type": "Document",
            "frameId": "main",
            "hasUserGesture": false,
        }),
    );

    // Network.responseReceived
    events.send_network_event(
        "Network.responseReceived",
        json!({
            "requestId": request_id,
            "loaderId": loader_id,
            "timestamp": timestamp,
            "type": "Document",
            "response": {
                "url": url,
                "status": status,
                "statusText": if status == 200 { "OK" } else { "" },
                "headers": {
                    "Content-Type": content_type,
                },
                "mimeType": content_type,
                "connectionReused": false,
                "connectionId": 0,
                "encodedDataLength": 0,
                "securityState": "secure",
            },
            "frameId": "main",
        }),
    );

    // Network.loadingFinished
    events.send_network_event(
        "Network.loadingFinished",
        json!({
            "requestId": request_id,
            "timestamp": timestamp,
            "encodedDataLength": 0,
        }),
    );
}
