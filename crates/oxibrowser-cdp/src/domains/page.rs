//! CDP Page domain handler.
//!
//! Handles Page.enable, Page.disable, Page.navigate, Page.reload,
//! Page.getFrameTree, Page.getFrameMetrics, Page.captureScreenshot,
//! Page.printToPDF.
//!
//! Domain handlers that need access to page/frame data receive the
//! browser `Session` and perform async operations.

use crate::domains::DomainResult;
use crate::protocol::CdpError;
use oxibrowser_core::session::Session;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Dispatch Page domain methods.
pub async fn handle(
    method: &str,
    params: Option<Value>,
    session: &Arc<RwLock<Session>>,
) -> DomainResult {
    match method {
        "enable" => enable(),
        "disable" => disable(),
        "navigate" => navigate(params, session).await,
        "reload" => reload(params, session).await,
        "getFrameTree" => get_frame_tree(session).await,
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

/// Page.navigate — navigates to a URL using the real browser session.
async fn navigate(
    params: Option<Value>,
    session: &Arc<RwLock<Session>>,
) -> DomainResult {
    let params = params.unwrap_or_default();
    let url = params
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("about:blank");

    let mut guard = session.write().await;
    match guard.navigate(url).await {
        Ok(()) => {
            // Get the frame ID from the current page
            let frame_id = guard
                .page()
                .map(|p| p.root_frame().id().to_string())
                .unwrap_or_else(|| "main".to_string());

            Ok(Some(json!({
                "frameId": frame_id,
                "loaderId": format!("loader-{}", uuid::Uuid::new_v4()),
                "errorText": Value::Null
            })))
        }
        Err(e) => Err(CdpError {
            code: -32000,
            message: format!("Navigation failed: {e}"),
        }),
    }
}

/// Page.reload — reloads the current page using the real browser session.
async fn reload(
    _params: Option<Value>,
    session: &Arc<RwLock<Session>>,
) -> DomainResult {
    let mut guard = session.write().await;
    match guard.reload().await {
        Ok(()) => {
            let frame_id = guard
                .page()
                .map(|p| p.root_frame().id().to_string())
                .unwrap_or_else(|| "main".to_string());

            Ok(Some(json!({
                "frameId": frame_id,
                "loaderId": format!("loader-{}", uuid::Uuid::new_v4())
            })))
        }
        Err(e) => Err(CdpError {
            code: -32000,
            message: format!("Reload failed: {e}"),
        }),
    }
}

/// Page.getFrameTree — returns the actual frame tree from the session.
async fn get_frame_tree(session: &Arc<RwLock<Session>>) -> DomainResult {
    let guard = session.read().await;
    match guard.page() {
        Some(page) => {
            let frame = page.root_frame();
            let url = frame.url();
            Ok(Some(json!({
                "frameTree": {
                    "frame": {
                        "id": frame.id().to_string(),
                        "url": url.to_string(),
                        "securityOrigin": url.origin().unicode_serialization(),
                        "mimeType": "text/html"
                    },
                    "childFrames": []
                }
            })))
        }
        None => Ok(Some(json!({
            "frameTree": {
                "frame": {
                    "id": "main",
                    "url": "about:blank",
                    "securityOrigin": "",
                    "mimeType": "text/html"
                },
                "childFrames": []
            }
        }))),
    }
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
///
/// Placeholder: returns a 1x1 transparent PNG until full rendering is available.
fn capture_screenshot(params: Option<Value>) -> DomainResult {
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
///
/// Placeholder until rendering is available.
fn print_to_pdf(_params: Option<Value>) -> DomainResult {
    Ok(Some(json!({
        "data": "",
        "stream": ""
    })))
}
