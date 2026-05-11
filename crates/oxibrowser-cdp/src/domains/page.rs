//! CDP Page domain handler.
//!
//! Handles Page.enable, Page.disable, Page.navigate, Page.reload,
//! Page.getFrameTree, Page.getFrameMetrics, Page.captureScreenshot,
//! Page.printToPDF.

use crate::domains::DomainResult;
use crate::protocol::CdpError;
use serde_json::{json, Value};
/// Dispatch Page domain methods.
pub fn handle(method: &str, params: Option<Value>) -> DomainResult {
    match method {
        "enable" => enable(),
        "disable" => disable(),
        "navigate" => navigate(params),
        "reload" => reload(params),
        "getFrameTree" => get_frame_tree(),
        "getFrameMetrics" => get_frame_metrics(),
        "captureScreenshot" => capture_screenshot(params),
        "printToPDF" => print_to_pdf(params),
        "getLifecycleEvents" => Ok(Some(json!({ "events": [] }))),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Page.{} not implemented", method),
        }),
    }
}

/// Page.enable — enables page domain events.
fn enable() -> DomainResult {
    Ok(Some(json!({})))
}

/// Page.disable — disables page domain events.
fn disable() -> DomainResult {
    Ok(Some(json!({})))
}

/// Page.navigate — navigates to a URL.
fn navigate(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _url = params
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("about:blank");

    Ok(Some(json!({
        "frameId": "main",
        "loaderId": format!("loader-{}", uuid::Uuid::new_v4()),
        "errorText": Value::Null
    })))
}

/// Page.reload — reloads the current page.
fn reload(_params: Option<Value>) -> DomainResult {
    // In a real implementation, this would trigger a page reload.
    Ok(Some(json!({
        "frameId": "main",
        "loaderId": format!("loader-{}", uuid::Uuid::new_v4())
    })))
}

/// Page.getFrameTree — returns the frame tree.
fn get_frame_tree() -> DomainResult {
    Ok(Some(json!({
        "frameTree": {
            "frame": {
                "id": "main",
                "url": "about:blank",
                "securityOrigin": "",
                "mimeType": "text/html"
            },
            "childFrames": []
        }
    })))
}

/// Page.getFrameMetrics — returns frame layout metrics.
fn get_frame_metrics() -> DomainResult {
    Ok(Some(json!({
        "layoutViewport": {
            "pageX": 0,
            "pageY": 0,
            "clientWidth": 1280,
            "clientHeight": 720
        },
        "visualViewport": {
            "offsetX": 0,
            "offsetY": 0,
            "pageX": 0,
            "pageY": 0,
            "clientWidth": 1280,
            "clientHeight": 720,
            "scale": 1,
            "zoom": 1
        },
        "contentSize": {
            "width": 1280,
            "height": 720
        }
    })))
}

/// Page.captureScreenshot — captures a screenshot of the page.
fn capture_screenshot(params: Option<Value>) -> DomainResult {
    // In a real implementation with servo rendering, this would capture
    // actual pixel data. For now, return a 1x1 transparent PNG.
    let params = params.unwrap_or_default();
    let _format = params
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("png");

    // Minimal valid 1x1 transparent PNG (base64 encoded)
    let placeholder = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPj/HwADBwIAMCbHYQAAAABJRU5ErkJggg==";

    Ok(Some(json!({
        "data": placeholder,
        "metadata": {
            "pageScaleFactor": 1,
            "deviceWidth": 1280,
            "deviceHeight": 720
        }
    })))
}

/// Page.printToPDF — prints the page to PDF.
fn print_to_pdf(_params: Option<Value>) -> DomainResult {
    // In a real implementation, this would render the page to PDF.
    Ok(Some(json!({
        "data": "",
        "stream": ""
    })))
}
