//! CDP Page domain handler.
//!
//! Handles Page.enable, Page.disable, Page.navigate, Page.reload,
//! Page.getFrameTree, Page.getFrameMetrics, Page.captureScreenshot,
//! Page.printToPDF.
//!
//! After Page.enable, navigation events are emitted:
//! - Page.frameNavigated
//! - Page.domContentLoadedEventFired
//! - Page.loadEventFired
//!
//! Network events are emitted in the correct order:
//! 1. Network.requestWillBeSent (before navigation)
//! 2. Navigation executes
//! 3. Page.frameNavigated
//! 4. Network.responseReceived
//! 5. Network.loadingFinished
//! 6. Page.domContentLoadedEventFired
//! 7. Page.loadEventFired

use crate::domains::network;
use crate::domains::{DispatchContext, DomainResult};
use crate::event::EventSender;
use crate::protocol::CdpError;
use serde_json::{Value, json};

/// Dispatch Page domain methods.
pub async fn handle(method: &str, params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        "enable" => enable(ctx),
        "disable" => disable(ctx),
        "navigate" => navigate(params, ctx).await,
        "reload" => reload(params, ctx).await,
        "getFrameTree" => get_frame_tree(ctx).await,
        "getFrameMetrics" => get_frame_metrics(),
        "captureScreenshot" => capture_screenshot(params, ctx).await,
        "printToPDF" => print_to_pdf(params, ctx).await,
        "setDownloadBehavior" => set_download_behavior(params),
        "getLifecycleEvents" => Ok(Some(json!({ "events": [] }))),
        "setLifecycleEventsEnabled" => set_lifecycle_events_enabled(params, ctx),
        // Dialog handling: alert/confirm/prompt default to non-blocking
        // (no-throw), so acknowledging the dialog is a no-op ack.
        // Resolve a pending alert/confirm/prompt. The page's JS thread blocks
        // polling the dialog gate until this writes the resolution.
        "handleJavaScriptDialog" => handle_javascript_dialog(params, ctx).await,
        // Common Playwright/Puppeteer Page methods — acknowledged as no-ops so
        // they don't 404 the client. Real implementations land per phase.
        "addScriptToEvaluateOnNewDocument" => Ok(Some(json!({ "identifier": "0" }))),
        "removeScriptToEvaluateOnNewDocument" => Ok(Some(json!({}))),
        "bringToFront" => Ok(Some(json!({}))),
        "getNavigationHistory" => Ok(Some(json!({ "currentIndex": 0, "entries": [] }))),
        "setBypassCSP" => Ok(Some(json!({}))),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Page.{} not implemented", method),
        }),
    }
}

/// Page.enable — enables page domain events.
fn enable(ctx: &DispatchContext) -> DomainResult {
    ctx.events.set_page_enabled(true);
    Ok(Some(json!({})))
}

/// Page.disable — disables page domain events.
fn disable(ctx: &DispatchContext) -> DomainResult {
    ctx.events.set_page_enabled(false);
    Ok(Some(json!({})))
}

/// Page.setLifecycleEventsEnabled — controls lifecycle event emission.
fn set_lifecycle_events_enabled(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let enabled = params
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    ctx.events.set_page_enabled(enabled);
    Ok(Some(json!({})))
}

/// Page.navigate — navigates to a URL using the real browser session.
///
/// Emits events in correct CDP order:
/// 1. Network.requestWillBeSent
/// 2. Navigation executes
/// 3. Page.frameNavigated
/// 4. Network.responseReceived
/// 5. Network.loadingFinished
/// 6. Page.domContentLoadedEventFired
/// 7. Page.loadEventFired
async fn navigate(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let url = params
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("about:blank");

    let loader_id = format!("LID-{}", uuid::Uuid::new_v4().as_simple());
    let request_id = format!("REQ-{}", uuid::Uuid::new_v4().as_simple());

    // 1. Emit Network.requestWillBeSent FIRST (before navigation)
    let pre_timestamp = EventSender::timestamp_ms();
    ctx.events.send_network_event(
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
            "timestamp": pre_timestamp,
            "wallTime": pre_timestamp / 1000.0,
            "initiator": { "type": "other" },
            "type": "Document",
            "frameId": "main",
            "hasUserGesture": false,
        }),
    );

    // 1b. Fetch interception: if enabled and the URL matches a pattern, emit
    // `Fetch.requestPaused` and await the client's decision (continue/fail/fulfill).
    let mut effective_url = url.to_string();
    let patterns = ctx.events.get_fetch_patterns();
    if !patterns.is_empty() && crate::domains::fetch::matches_patterns(url, &patterns) {
        use base64::Engine;
        use oxibrowser_core::network::InterceptAction;
        let intercept_id = format!("INT-{}", uuid::Uuid::new_v4().as_simple());
        let decision = crate::domains::fetch::emit_request_paused(
            &intercept_id,
            url,
            "GET",
            &[],
            "Document",
            &ctx.fetch_registry,
            &ctx.events,
        )
        .await;
        match decision {
            Ok(InterceptAction::Fail { error_reason }) => {
                return Ok(Some(json!({
                    "frameId": "main",
                    "loaderId": loader_id,
                    "errorText": error_reason
                })));
            }
            Ok(InterceptAction::Continue { url: Some(u), .. }) => effective_url = u,
            Ok(InterceptAction::Fulfill { body, .. }) => {
                // Mock the response: navigate to a data: URL carrying the body.
                let b64 = base64::engine::general_purpose::STANDARD.encode(&body);
                effective_url = format!("data:text/html;charset=utf-8;base64,{b64}");
            }
            _ => {} // Continue unmodified, or no decision — proceed normally.
        }
    }

    // 2. Execute navigation
    let mut guard = ctx.session.write().await;
    match guard.navigate(&effective_url).await {
        Ok(()) => {
            // Capture timestamp after navigation completes
            let timestamp = EventSender::timestamp_ms();
            let frame_id = guard
                .page()
                .map(|p| p.root_frame().id().to_string())
                .unwrap_or_else(|| "main".to_string());

            let final_url = guard
                .current_url()
                .map(|u| u.to_string())
                .unwrap_or_else(|| url.to_string());

            // 3. Emit Page.frameNavigated
            ctx.events.send_page_event(
                "Page.frameNavigated",
                json!({
                    "frame": {
                        "id": frame_id,
                        "loaderId": loader_id,
                        "url": final_url,
                        "domainAndRegistry": "",
                        "securityOrigin": final_url,
                        "mimeType": "text/html",
                        "adFrameStatus": { "adFrameType": "none" },
                        "secureContextType": "Secure",
                        "crossOriginIsolatedContextType": "NotIsolated",
                    },
                    "type": "Navigation"
                }),
            );

            // 3b. Emit Page.frameNavigated + executionContextCreated for each
            // child iframe (Phase 8).
            if let Some(page) = guard.page() {
                let frame_map = guard.frame_context_map().read().clone();
                for child in page.root_frame().children() {
                    let child_url = child.url();
                    let child_frame_id = child.id().to_string();
                    ctx.events.send_page_event(
                        "Page.frameNavigated",
                        json!({
                            "frame": {
                                "id": child_frame_id,
                                "parentId": frame_id,
                                "loaderId": loader_id,
                                "url": child_url.to_string(),
                                "domainAndRegistry": "",
                                "securityOrigin": child_url.origin().unicode_serialization(),
                                "mimeType": "text/html",
                                "secureContextType": "Secure",
                                "crossOriginIsolatedContextType": "NotIsolated",
                            },
                            "type": "Navigation"
                        }),
                    );
                    // Emit the matching execution context.
                    if let Some(&context_id) = frame_map.get(&child_frame_id) {
                        ctx.events.send_runtime_event(
                            "Runtime.executionContextCreated",
                            json!({
                                "context": {
                                    "id": context_id,
                                    "origin": child_url.origin().unicode_serialization(),
                                    "name": format!("iframe:{child_frame_id}"),
                                    "uniqueId": format!("context-{}", uuid::Uuid::new_v4()),
                                    "auxData": {
                                        "isDefault": true,
                                        "type": "default",
                                        "frameId": child_frame_id
                                    }
                                }
                            }),
                        );
                    }
                }
            }

            // 4-5. Emit Network.responseReceived and Network.loadingFinished
            network::emit_response_events(
                &ctx.events,
                &request_id,
                &final_url,
                &loader_id,
                200,
                "text/html",
            );

            // 6. Emit Page.domContentLoadedEventFired
            ctx.events.send_page_event(
                "Page.domContentLoadedEventFired",
                json!({ "timestamp": timestamp }),
            );

            // 7. Emit Page.loadEventFired
            ctx.events
                .send_page_event("Page.loadEventFired", json!({ "timestamp": timestamp }));

            // Fetch.requestPaused will be emitted from Session::navigate
            // once Fetch interception is fully integrated with the HTTP client

            Ok(Some(json!({
                "frameId": frame_id,
                "loaderId": loader_id,
                "errorText": Value::Null
            })))
        }
        Err(e) => Err(CdpError {
            code: -32000,
            message: format!("Navigation failed: {e}"),
        }),
    }
}

/// Page.reload — reloads the current page and emits lifecycle events.
///
/// Emits events in the same order as navigate:
/// 1. Network.requestWillBeSent
/// 2. Reload executes
/// 3. Page.frameNavigated
/// 4. Network.responseReceived
/// 5. Network.loadingFinished
/// 6. Page.domContentLoadedEventFired
/// 7. Page.loadEventFired
async fn reload(_params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let loader_id = format!("LID-{}", uuid::Uuid::new_v4().as_simple());
    let request_id = format!("REQ-{}", uuid::Uuid::new_v4().as_simple());

    // 1. Capture current URL before emitting events (read lock)
    let current_url = {
        let guard = ctx.session.read().await;
        guard
            .current_url()
            .map(|u| u.to_string())
            .unwrap_or_else(|| "about:blank".to_string())
    };

    // 2. Emit Network.requestWillBeSent FIRST with the current URL
    let pre_timestamp = EventSender::timestamp_ms();
    ctx.events.send_network_event(
        "Network.requestWillBeSent",
        json!({
            "requestId": request_id,
            "loaderId": loader_id,
            "documentURL": current_url,
            "request": {
                "url": current_url,
                "method": "GET",
                "headers": {},
                "initialPriority": "VeryHigh",
                "urlFragment": "",
            },
            "timestamp": pre_timestamp,
            "wallTime": pre_timestamp / 1000.0,
            "initiator": { "type": "other" },
            "type": "Document",
            "frameId": "main",
            "hasUserGesture": false,
        }),
    );

    // 3. Execute reload
    let mut guard = ctx.session.write().await;
    match guard.reload().await {
        Ok(()) => {
            // Capture timestamp after reload completes
            let timestamp = EventSender::timestamp_ms();
            let frame_id = guard
                .page()
                .map(|p| p.root_frame().id().to_string())
                .unwrap_or_else(|| "main".to_string());

            let final_url = guard
                .current_url()
                .map(|u| u.to_string())
                .unwrap_or_else(|| "about:blank".to_string());

            // 3. Emit Page.frameNavigated
            ctx.events.send_page_event(
                "Page.frameNavigated",
                json!({
                    "frame": {
                        "id": frame_id,
                        "loaderId": loader_id,
                        "url": final_url,
                        "mimeType": "text/html",
                    },
                    "type": "Navigation"
                }),
            );

            // 4-5. Emit Network.responseReceived and Network.loadingFinished
            network::emit_response_events(
                &ctx.events,
                &request_id,
                &final_url,
                &loader_id,
                200,
                "text/html",
            );

            // 6. Emit Page.domContentLoadedEventFired
            ctx.events.send_page_event(
                "Page.domContentLoadedEventFired",
                json!({ "timestamp": timestamp }),
            );

            // 7. Emit Page.loadEventFired
            ctx.events
                .send_page_event("Page.loadEventFired", json!({ "timestamp": timestamp }));

            Ok(Some(json!({
                "frameId": frame_id,
                "loaderId": loader_id
            })))
        }
        Err(e) => Err(CdpError {
            code: -32000,
            message: format!("Reload failed: {e}"),
        }),
    }
}

/// Page.getFrameTree — returns the actual frame tree from the session,
/// including child iframe frames (Phase 8).
async fn get_frame_tree(ctx: &DispatchContext) -> DomainResult {
    let guard = ctx.session.read().await;
    match guard.page() {
        Some(page) => {
            let frame = page.root_frame();
            Ok(Some(json!({
                "frameTree": frame_tree_node(frame)
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

/// Build a recursive frame-tree JSON node for `Page.getFrameTree`.
fn frame_tree_node(frame: &oxibrowser_core::frame::Frame) -> Value {
    let url = frame.url();
    let child_frames: Vec<Value> = frame.children().iter().map(frame_tree_node).collect();
    json!({
        "frame": {
            "id": frame.id().to_string(),
            "url": url.to_string(),
            "securityOrigin": url.origin().unicode_serialization(),
            "mimeType": "text/html"
        },
        "childFrames": child_frames
    })
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
/// Renders the DOM as a PNG image using text-based rendering with bitmap font.
async fn capture_screenshot(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let _format = params
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("png");
    let viewport_width = params
        .get("clip")
        .and_then(|v| v.get("width"))
        .and_then(|v| v.as_f64())
        .unwrap_or(1280.0) as u32;

    // Render the live (post-JS) RenderDocument via the JS thread. Falls back to
    // a blank PNG if no document is loaded.
    let mut guard = ctx.session.write().await;
    let png_bytes: Vec<u8> = guard
        .capture_screenshot_png(viewport_width.max(64))
        .await
        .unwrap_or_else(|_| oxibrowser_core::blank_png(viewport_width.max(64), 800));

    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

    Ok(Some(json!({
        "data": data,
        "metadata": {
            "pageScaleFactor": 1,
            "deviceWidth": viewport_width,
            "deviceHeight": 720
        }
    })))
}

/// Page.printToPDF — prints the page to PDF.
///
/// Captures the rendered page (same path as `captureScreenshot`) and embeds
/// the PNG in a single-page PDF via `printpdf`. The PDF page matches the
/// captured image's aspect ratio. Parameters (paperWidth/Height, landscape,
/// etc.) are accepted but ignored — the output is always one full-page image.
async fn print_to_pdf(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let viewport_width = params
        .get("clip")
        .and_then(|v| v.get("width"))
        .and_then(|v| v.as_f64())
        .unwrap_or(1280.0) as u32;

    let mut guard = ctx.session.write().await;
    let png_bytes: Vec<u8> = guard
        .capture_screenshot_png(viewport_width.max(64))
        .await
        .unwrap_or_else(|_| oxibrowser_core::blank_png(viewport_width.max(64), 800));
    drop(guard);

    let pdf_bytes = oxibrowser_core::png_to_pdf(&png_bytes).unwrap_or_default();
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(&pdf_bytes);
    Ok(Some(json!({ "data": data, "stream": "" })))
}

/// `Page.setDownloadBehavior` — configure the directory downloads are saved to.
fn set_download_behavior(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let path = params.get("downloadPath").and_then(|v| v.as_str());
    let behavior = params
        .get("behavior")
        .and_then(|v| v.as_str())
        .unwrap_or("allow");
    let dir = if behavior == "deny" {
        None
    } else {
        path.map(std::path::PathBuf::from)
    };
    oxibrowser_core::session::set_download_behavior(dir);
    Ok(Some(json!({})))
}

/// Page.handleJavaScriptDialog — accept or dismiss a pending
/// `alert`/`confirm`/`prompt` dialog. Writes the resolution into the session's
/// shared dialog gate, waking the blocked JS thread.
async fn handle_javascript_dialog(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let accept = params
        .get("accept")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let prompt_text = params
        .get("promptText")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    // Write directly to the shared gate — no session lock, so this resolves a
    // dialog even while a blocking evaluate holds the session write lock.
    *ctx.dialog_gate.lock() = Some(oxibrowser_core::js::DialogResult {
        accept,
        prompt_text,
    });
    Ok(Some(json!({})))
}
