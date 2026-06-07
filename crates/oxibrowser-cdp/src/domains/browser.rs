//! CDP Browser domain handler.
//!
//! Handles Browser.getVersion, Browser.close, Browser.getWindowForTarget.

use crate::domains::DomainResult;
use crate::protocol::CdpError;
use serde_json::{Value, json};

/// Dispatch Browser domain methods.
pub fn handle(method: &str, _params: Option<Value>) -> DomainResult {
    match method {
        "getVersion" => get_version(),
        "close" => close(),
        "getWindowForTarget" => get_window_for_target(),
        "setWindowBounds" => Ok(Some(json!({}))),
        "getWindowBounds" => Ok(Some(json!({
            "bounds": {
                "left": 0,
                "top": 0,
                "width": 1280,
                "height": 720,
                "windowState": "normal"
            }
        }))),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Browser.{} not implemented", method),
        }),
    }
}

/// Browser.getVersion — returns browser and protocol version info.
fn get_version() -> DomainResult {
    Ok(Some(json!({
        "protocolVersion": "1.3",
        "product": "OxiBrowser/0.1.0",
        "revision": "@@revision",
        "userAgent": "OxiBrowser/0.1.0",
        "jsVersion": "0.1.0"
    })))
}

/// Browser.close — closes the browser.
fn close() -> DomainResult {
    Ok(Some(json!({})))
}

/// Browser.getWindowForTarget — returns window ID for a target.
fn get_window_for_target() -> DomainResult {
    Ok(Some(json!({
        "windowId": 1,
        "bounds": {
            "left": 0,
            "top": 0,
            "width": 1280,
            "height": 720,
            "windowState": "normal"
        }
    })))
}
