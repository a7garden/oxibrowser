//! CDP Network domain handler.
//!
//! Handles Network.enable, Network.disable, Network.loadResource,
//! Network.getResponseBody.

use crate::domains::DomainResult;
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Dispatch Network domain methods.
pub fn handle(method: &str, params: Option<Value>) -> DomainResult {
    match method {
        "enable" => enable(),
        "disable" => disable(),
        "loadResource" => load_resource(params),
        "getResponseBody" => get_response_body(params),
        "setCacheDisabled" => Ok(Some(json!({}))),
        "setExtraHTTPHeaders" => Ok(Some(json!({}))),
        "emulateNetworkConditions" => Ok(Some(json!({}))),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Network.{} not implemented", method),
        }),
    }
}

/// Network.enable — enables network tracking.
fn enable() -> DomainResult {
    // In a real implementation, this would start intercepting network events.
    Ok(Some(json!({})))
}

/// Network.disable — disables network tracking.
fn disable() -> DomainResult {
    Ok(Some(json!({})))
}

/// Network.loadResource — loads a resource from the network.
fn load_resource(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _url = params.get("url").and_then(|v| v.as_str()).unwrap_or("");

    // In a real implementation, this would fetch the resource.
    Ok(Some(json!({
        "resource": {
            "success": true,
            "httpStatusCode": 200,
            "stream": "",
            "headers": {}
        }
    })))
}

/// Network.getResponseBody — returns the body of a previously loaded resource.
fn get_response_body(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _request_id = params
        .get("requestId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // In a real implementation, this would return the cached response body.
    Ok(Some(json!({
        "body": "",
        "base64Encoded": false
    })))
}
