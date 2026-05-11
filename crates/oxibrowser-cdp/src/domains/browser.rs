//! CDP Browser domain handler.
//!
//! Handles Browser.getVersion, Browser.getWindowForTarget, Browser.close.

use crate::domains::DomainResult;
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Dispatch Browser domain methods.
pub fn handle(method: &str, params: Option<Value>) -> DomainResult {
    match method {
        "getVersion" => get_version(),
        "getWindowForTarget" => get_window_for_target(params),
        "close" => close(),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Browser.{} not implemented", method),
        }),
    }
}

/// Browser.getVersion — returns protocol version information.
fn get_version() -> DomainResult {
    Ok(Some(json!({
        "protocolVersion": "1.3",
        "product": "OxiBrowser/0.1.0",
        "revision": "@oxibrowser",
        "userAgent": "OxiBrowser/0.1.0",
        "jsVersion": "0.1.0"
    })))
}

/// Browser.getWindowForTarget — returns the window ID for a target.
fn get_window_for_target(_params: Option<Value>) -> DomainResult {
    Ok(Some(json!({
        "windowId": 1
    })))
}

/// Browser.close — closes the browser.
fn close() -> DomainResult {
    // In a real implementation, this would signal the browser to shut down.
    Ok(Some(json!({})))
}
