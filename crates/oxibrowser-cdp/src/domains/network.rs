//! CDP Network domain handler.
//!
//! Handles Network domain methods and emits network lifecycle events when enabled:
//! - Network.enable / disable
//! - Network.requestWillBeSent, responseReceived, loadingFinished (events)
//! - Cookie CRUD: getAllCookies, setCookie, deleteCookies
//! - Response body: getResponseBody (stub)

use crate::domains::{DispatchContext, DomainResult};
use crate::event::EventSender;
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Dispatch Network domain methods.
pub async fn handle(method: &str, params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        // --- State ---
        "enable" => enable(ctx),
        "disable" => disable(ctx),
        "setCacheDisabled" => Ok(Some(json!({}))),
        "setExtraHTTPHeaders" => Ok(Some(json!({}))),
        "emulateNetworkConditions" => Ok(Some(json!({}))),

        // --- Cookies ---
        "getAllCookies" => get_all_cookies(ctx).await,
        "getCookies" => get_cookies(params, ctx).await,
        "setCookie" => set_cookie(params, ctx).await,
        "deleteCookies" => delete_cookies(params, ctx).await,

        // --- Response body ---
        "getResponseBody" => get_response_body(params, ctx).await,

        // --- Extra ---
        "setRequestInterception" => Ok(Some(json!({}))),
        "authRequired" => Ok(Some(json!({}))),

        _ => Err(CdpError {
            code: -32601,
            message: format!("Network.{} not implemented", method),
        }),
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Network.enable — enables network tracking.
fn enable(ctx: &DispatchContext) -> DomainResult {
    ctx.events.set_network_enabled(true);
    Ok(Some(json!({})))
}

/// Network.disable — disables network tracking.
fn disable(ctx: &DispatchContext) -> DomainResult {
    ctx.events.set_network_enabled(false);
    Ok(Some(json!({})))
}

// ---------------------------------------------------------------------------
// Cookies
// ---------------------------------------------------------------------------

/// Network.getAllCookies — returns all cookies for the session.
async fn get_all_cookies(ctx: &DispatchContext) -> DomainResult {
    let session = ctx.session.read().await;
    let jar_guard = session.cookie_jar().read();
    let cookies = jar_guard.get_all();

    let result: Vec<serde_json::Value> = cookies
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "value": c.value,
                "domain": c.domain.as_deref().unwrap_or(""),
                "path": c.path.as_deref().unwrap_or("/"),
                "expires": -1.0_f64,
                "size": c.name.len() + c.value.len(),
                "httpOnly": c.http_only,
                "secure": c.secure,
                "session": true,
                "sameParty": false,
                "sameSite": "None",
                "priority": "Medium",
                "partitionKey": serde_json::Value::Null,
            })
        })
        .collect();

    Ok(Some(json!({ "cookies": result })))
}

/// Network.getCookies — returns cookies for specific URLs.
async fn get_cookies(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let urls = params
        .as_ref()
        .and_then(|p| p.get("urls"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let session = ctx.session.read().await;
    let jar_guard = session.cookie_jar().read();

    let mut result = Vec::new();
    for url_str in &urls {
        if let Ok(url) = url::Url::parse(url_str) {
            let cookies_str = jar_guard.cookies_for_url(&url);
            for cookie_str in cookies_str.split(';') {
                let cookie_str = cookie_str.trim();
                if let Some(eq) = cookie_str.find('=') {
                    let name = cookie_str[..eq].trim().to_string();
                    let value = cookie_str[eq + 1..].trim().to_string();
                    result.push(json!({
                        "name": name,
                        "value": value,
                        "domain": url.domain().unwrap_or(""),
                        "path": "/",
                        "expires": -1.0_f64,
                        "size": name.len() + value.len(),
                        "httpOnly": false,
                        "secure": url.scheme() == "https",
                        "session": true,
                        "sameSite": "Lax",
                    }));
                }
            }
        }
    }

    Ok(Some(json!({ "cookies": result })))
}

/// Network.setCookie — creates a cookie with given properties.
async fn set_cookie(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "setCookie requires parameters".to_string(),
    })?;

    let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let value = p.get("value").and_then(|v| v.as_str()).unwrap_or("");
    let url = p.get("url").and_then(|v| v.as_str());
    let path = p.get("path").and_then(|v| v.as_str()).unwrap_or("/");
    let secure = p.get("secure").and_then(|v| v.as_bool()).unwrap_or(false);
    let http_only = p.get("httpOnly").and_then(|v| v.as_bool()).unwrap_or(false);

    let target_url = url.ok_or_else(|| CdpError {
        code: -32602,
        message: "setCookie requires url".to_string(),
    })?;

    let parsed = url::Url::parse(target_url).map_err(|_| CdpError {
        code: -32602,
        message: format!("invalid URL: {}", target_url),
    })?;

    let session = ctx.session.read().await;
    let mut jar_guard = session.cookie_jar().write();

    let mut header = format!("{}={}", name, value);
    header.push_str(&format!("; Path={}", path));
    if secure {
        header.push_str("; Secure");
    }
    if http_only {
        header.push_str("; HttpOnly");
    }

    jar_guard.store(&parsed, &header);

    Ok(Some(json!({ "success": true })))
}

/// Network.deleteCookies — removes cookies matching the given name for a URL.
async fn delete_cookies(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "deleteCookies requires parameters".to_string(),
    })?;

    let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let url = p.get("url").and_then(|v| v.as_str()).unwrap_or("");

    let parsed = url::Url::parse(url).map_err(|_| CdpError {
        code: -32602,
        message: format!("invalid URL: {}", url),
    })?;

    let session = ctx.session.read().await;
    let mut jar_guard = session.cookie_jar().write();
    jar_guard.remove(&parsed, name);

    Ok(Some(json!({ "success": true })))
}

// ---------------------------------------------------------------------------
// Response body
// ---------------------------------------------------------------------------

/// Network.getResponseBody — returns body of a network response.
async fn get_response_body(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "getResponseBody requires parameters".to_string(),
    })?;

    let request_id = params.get("requestId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CdpError {
            code: -32602,
            message: "requestId required".to_string(),
        })?;

    let session = ctx.session.read().await;

    if let Some(captured) = session.get_response_body(request_id) {
        let body = if captured.base64 {
            // For binary content, we could base64-encode here
            captured.body
        } else {
            captured.body
        };
        Ok(Some(json!({
            "body": body,
            "base64Encoded": captured.base64,
        })))
    } else {
        Err(CdpError {
            code: -32602,
            message: format!("Could not find body for requestId: {}", request_id),
        })
    }
}

// ---------------------------------------------------------------------------
// Event emission (called from Page domain during navigation)
// ---------------------------------------------------------------------------

/// Emit network events for a navigation request.
///
/// Emits all three events (requestWillBeSent, responseReceived, loadingFinished).
/// Used by callers that need the full lifecycle in one call.
pub fn emit_navigation_events(
    events: &EventSender,
    request_id: &str,
    url: &str,
    loader_id: &str,
    status: u16,
    content_type: &str,
) {
    let timestamp = EventSender::timestamp_ms();

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
            "initiator": { "type": "other" },
            "type": "Document",
            "frameId": "main",
            "hasUserGesture": false,
        }),
    );

    emit_response_events(events, request_id, url, loader_id, status, content_type);
}

/// Emit only the response lifecycle events (responseReceived + loadingFinished).
///
/// Used when requestWillBeSent was already emitted before navigation,
/// and only the response events are needed after navigation completes.
pub fn emit_response_events(
    events: &EventSender,
    request_id: &str,
    url: &str,
    loader_id: &str,
    status: u16,
    content_type: &str,
) {
    let timestamp = EventSender::timestamp_ms();

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
                "headers": { "Content-Type": content_type },
                "mimeType": content_type,
                "connectionReused": false,
                "connectionId": 0.0,
                "encodedDataLength": 0.0,
                "securityState": "secure",
            },
            "frameId": "main",
        }),
    );

    events.send_network_event(
        "Network.loadingFinished",
        json!({
            "requestId": request_id,
            "timestamp": timestamp,
            "encodedDataLength": 0.0,
        }),
    );
}