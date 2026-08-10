//! CDP Fetch domain handler.
//!
//! Handles network interception via Fetch.enable, Fetch.disable,
//! Fetch.continueRequest, Fetch.failRequest, Fetch.fulfillRequest, Fetch.continueResponse.
//!
//! Request interception works as follows:
//! 1. `Fetch.enable` registers patterns in EventSender
//! 2. `Session::navigate` calls `emit_request_paused` before fetching
//! 3. If pattern matches: request is stored in `PausedRequestRegistry`,
//!    `Fetch.requestPaused` event is emitted, and the decision is awaited
//! 4. CDP client responds with continue/fail/fulfill
//! 5. HTTP client applies the decision via `HttpClient::intercept`

use crate::domains::{DispatchContext, DomainResult};
use crate::event::EventSender;
use crate::protocol::CdpError;
use oxibrowser_core::network::InterceptAction;
use serde_json::{Value, json};

/// Dispatch Fetch domain methods.
pub async fn handle(method: &str, params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        // --- Enable/disable ---
        "enable" => enable(params, ctx),
        "disable" => disable(ctx),

        // --- Request interception actions ---
        "continueRequest" => continue_request(params, ctx).await,
        "failRequest" => fail_request(params, ctx).await,
        "fulfillRequest" => fulfill_request(params, ctx).await,
        "continueResponse" => continue_response(params, ctx).await,
        "getResponseBody" => get_response_body(params, ctx).await,
        "takeResponseBodyAsStream" => Ok(Some(json!({"streamId": 0}))),
        "restoreResponseBodyAsStream" => Ok(Some(json!({}))),

        _ => Err(CdpError {
            code: -32601,
            message: format!("Fetch.{} not implemented", method),
        }),
    }
}

// ---------------------------------------------------------------------------
// Enable / Disable
// ---------------------------------------------------------------------------

/// Fetch.enable — enables request interception with optional patterns.
///
/// If no patterns are provided (empty or missing `patterns` array), the
/// domain is enabled but NO requests will be intercepted. This prevents
/// accidentally intercepting all requests when the client just enables
/// the domain without specifying patterns.
fn enable(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let mut patterns = Vec::new();

    if let Some(p) = params
        && let Some(arr) = p.get("patterns").and_then(|v| v.as_array())
    {
        for item in arr {
            if let Some(p) = parse_fetch_pattern(item) {
                patterns.push(p);
            }
        }
    }

    // Only enable interception if patterns were explicitly provided.
    // If patterns is empty, enable the domain but don't intercept anything.
    let has_patterns = !patterns.is_empty();
    ctx.events.set_fetch_enabled(has_patterns);
    ctx.events.set_fetch_patterns(patterns.clone());
    // Mirror the raw url-pattern strings into core so the JS fetch bridge can
    // decide whether to pause a JS-originated request.
    let raw: Vec<String> = patterns.iter().map(|p| p.url_pattern.clone()).collect();
    oxibrowser_core::session::set_fetch_patterns(raw);
    if has_patterns {
        tracing::info!(
            patterns = patterns.len(),
            "Fetch domain enabled with patterns"
        );
    } else {
        tracing::info!("Fetch domain enabled (no patterns — interception disabled)");
    }
    Ok(Some(json!({})))
}

/// Fetch.disable — disables request interception.
fn disable(ctx: &DispatchContext) -> DomainResult {
    ctx.events.set_fetch_enabled(false);
    ctx.events.set_fetch_patterns(vec![]);
    oxibrowser_core::session::set_fetch_patterns(vec![]);
    tracing::info!("Fetch domain disabled");
    Ok(Some(json!({})))
}

// ---------------------------------------------------------------------------
// Request interception actions
// ---------------------------------------------------------------------------

/// Fetch.continueRequest — resume a paused request with modifications.
async fn continue_request(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "continueRequest requires parameters".to_string(),
    })?;

    let request_id = p
        .get("requestId")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    // Look up in registry
    let request = match ctx.fetch_registry.take(request_id) {
        Some(r) => r,
        None => {
            tracing::warn!(request_id, "unknown requestId in continueRequest");
            return Err(CdpError {
                code: -32601,
                message: format!("requestId not found: {}", request_id),
            });
        }
    };

    // Build headers
    let mut headers = request.headers.clone();
    if let Some(hdrs) = p.get("modifiedHeaders").and_then(|v| v.as_array()) {
        for item in hdrs {
            if let (Some(k), Some(v)) = (
                item.get("name").and_then(|x| x.as_str()),
                item.get("value").and_then(|x| x.as_str()),
            ) {
                if let Some((_, existing)) = headers
                    .iter_mut()
                    .find(|(k2, _)| k2.eq_ignore_ascii_case(k))
                {
                    *existing = v.to_string();
                } else {
                    headers.push((k.to_string(), v.to_string()));
                }
            }
        }
    }

    // Remove headers
    if let Some(names) = p.get("deletedHeaders").and_then(|v| v.as_array()) {
        for item in names {
            if let Some(name) = item.as_str() {
                headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
            }
        }
    }

    let url = p.get("url").and_then(|v| v.as_str()).map(String::from);
    let method = p.get("method").and_then(|v| v.as_str()).map(String::from);
    let post_data = p.get("postData").and_then(|v| v.as_str()).map(String::from);

    let action = build_continue(url, method, headers, post_data);
    let _ = request.tx.send(action);

    tracing::debug!(request_id, "Fetch.continueRequest resumed");
    Ok(Some(json!({})))
}

/// Fetch.failRequest — fail a paused request with an error.
async fn fail_request(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "failRequest requires parameters".to_string(),
    })?;

    let request_id = p
        .get("requestId")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let error_reason = p
        .get("errorReason")
        .and_then(|v| v.as_str())
        .unwrap_or("Failed");

    let request = match ctx.fetch_registry.take(request_id) {
        Some(r) => r,
        None => {
            tracing::warn!(request_id, "unknown requestId in failRequest");
            return Err(CdpError {
                code: -32601,
                message: format!("requestId not found: {}", request_id),
            });
        }
    };

    let action = build_fail(error_reason.to_string());
    let _ = request.tx.send(action);

    tracing::debug!(request_id, error_reason, "Fetch.failRequest");
    Ok(Some(json!({})))
}

/// Fetch.fulfillRequest — return a fake response for a paused request.
async fn fulfill_request(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "fulfillRequest requires parameters".to_string(),
    })?;

    let request_id = p
        .get("requestId")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let status_code = p.get("statusCode").and_then(|v| v.as_i64()).unwrap_or(200) as u16;
    let status_text = p.get("statusText").and_then(|v| v.as_str()).unwrap_or("OK");

    // Extract response headers
    let mut headers = Vec::new();
    if let Some(h) = p.get("responseHeaders").and_then(|v| v.as_array()) {
        for item in h {
            if let (Some(k), Some(v)) = (
                item.get("name").and_then(|x| x.as_str()),
                item.get("value").and_then(|x| x.as_str()),
            ) {
                headers.push((k.to_string(), v.to_string()));
            }
        }
    }

    // Body — CDP `Fetch.fulfillRequest.body` is base64-encoded by spec (there
    // is no `base64Encoded` flag on this method; Chrome/Playwright/Puppeteer
    // always send base64). Decode unconditionally, falling back to raw bytes
    // for malformed input so a bad payload can't hard-fail the interception.
    let raw_body = p.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let body_bytes = if raw_body.is_empty() {
        Vec::new()
    } else {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(raw_body)
            .unwrap_or_else(|_| raw_body.as_bytes().to_vec())
    };

    let body_size = body_bytes.len();
    let headers_count = headers.len();

    let request = match ctx.fetch_registry.take(request_id) {
        Some(r) => r,
        None => {
            tracing::warn!(request_id, "unknown requestId in fulfillRequest");
            return Err(CdpError {
                code: -32601,
                message: format!("requestId not found: {}", request_id),
            });
        }
    };

    let action = build_fulfill(status_code, status_text.to_string(), headers, body_bytes);
    let _ = request.tx.send(action);

    tracing::debug!(
        request_id,
        status_code,
        body_size,
        headers_count,
        "Fetch.fulfillRequest completed"
    );
    Ok(Some(json!({
        "responseCode": status_code,
        "responsePhrase": status_text,
        "responseHeadersCount": headers_count,
        "binary": false,
    })))
}

/// Fetch.continueResponse — continue a paused response (same as continue for now).
async fn continue_response(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let p = params.unwrap_or_default();
    let request_id = p
        .get("requestId")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let request = match ctx.fetch_registry.take(request_id) {
        Some(r) => r,
        None => {
            return Err(CdpError {
                code: -32601,
                message: format!("requestId not found: {}", request_id),
            });
        }
    };

    let action = InterceptAction::Continue {
        url: None,
        method: None,
        headers: Vec::new(),
        post_data: None,
    };
    let _ = request.tx.send(action);

    Ok(Some(json!({})))
}

/// Fetch.getResponseBody — returns body for an intercepted request.
async fn get_response_body(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "getResponseBody requires parameters".to_string(),
    })?;

    let request_id = p
        .get("requestId")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let guard = ctx.session.read().await;
    if let Some(body) = guard.get_response_body(request_id) {
        Ok(Some(json!({
            "body": body.body,
            "base64Encoded": body.base64,
        })))
    } else {
        Ok(Some(json!({
            "body": "",
            "base64Encoded": false,
        })))
    }
}

// ---------------------------------------------------------------------------
// Event emission helpers
// ---------------------------------------------------------------------------

/// Emit a `Fetch.requestPaused` event for an intercepted request.
///
/// Called from the network layer during navigation when a request matches a pattern.
/// The `PausedRequest` is inserted into the registry with a oneshot channel so the
/// CDP handler can deliver the client's decision back to the network layer.
///
/// Returns the `oneshot::Receiver<InterceptAction>` so the caller can await the decision.
pub fn emit_request_paused(
    request_id: &str,
    url: &str,
    method: &str,
    headers: &[(String, String)],
    resource_type: &str,
    registry: &oxibrowser_core::network::SharedRegistry,
    events: &EventSender,
) -> tokio::sync::oneshot::Receiver<InterceptAction> {
    use oxibrowser_core::network::PausedRequest;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel();

    let request = PausedRequest {
        url: url.to_string(),
        method: method.to_string(),
        headers: headers.to_vec(),
        resource_type: resource_type.to_string(),
        tx,
    };

    let _ = registry.insert(request_id.to_string(), request);

    let headers_json: serde_json::Map<String, serde_json::Value> =
        headers.iter().map(|(k, v)| (k.clone(), json!(v))).collect();

    events.send_fetch_event(
        "Fetch.requestPaused",
        json!({
            "requestId": request_id,
            "request": {
                "url": url,
                "method": method,
                "headers": headers_json,
                "initialPriority": "VeryHigh",
                "urlFragment": "",
                "postData": serde_json::Value::Null,
            },
            "resourceType": resource_type,
            "frameId": "main",
            "networkIntercepted": true,
        }),
    );

    tracing::debug!(request_id, url, method, "Fetch.requestPaused");

    rx
}

// ---------------------------------------------------------------------------
// Pattern matching
// ---------------------------------------------------------------------------

/// A request interception pattern.
#[derive(Debug, Clone, Default)]
pub struct FetchPattern {
    pub url_pattern: String,
    pub resource_type: Option<String>,
    pub request_stage: Option<String>,
}

impl FetchPattern {
    /// Check if a URL matches this pattern.
    pub fn matches_url(&self, url: &str) -> bool {
        if self.url_pattern.is_empty() || self.url_pattern == "*" {
            return true;
        }
        let pattern = &self.url_pattern;
        if pattern.starts_with('*') && pattern.ends_with('*') {
            url.contains(&pattern[1..pattern.len() - 1])
        } else if pattern.ends_with('*') {
            url.starts_with(&pattern[..pattern.len() - 1])
        } else if let Some(suffix) = pattern.strip_prefix('*') {
            url.ends_with(suffix)
        } else {
            url == pattern
        }
    }
}

/// Parse a CDP FetchPattern JSON object.
fn parse_fetch_pattern(value: &serde_json::Value) -> Option<FetchPattern> {
    let obj = value.as_object()?;
    Some(FetchPattern {
        url_pattern: obj
            .get("urlPattern")
            .and_then(|v| v.as_str())
            .unwrap_or("*")
            .to_string(),
        resource_type: obj
            .get("resourceType")
            .and_then(|v| v.as_str())
            .map(String::from),
        request_stage: obj
            .get("requestStage")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

/// Check if a request URL matches any enabled pattern.
pub fn matches_patterns(url: &str, patterns: &[FetchPattern]) -> bool {
    patterns.iter().any(|p| p.matches_url(url))
}

// ---------------------------------------------------------------------------
// InterceptAction builder helpers (mirrored from core network/intercept.rs)
// ---------------------------------------------------------------------------

/// Build an InterceptAction::Continue from CDP params.
fn build_continue(
    url: Option<String>,
    method: Option<String>,
    headers: Vec<(String, String)>,
    post_data: Option<String>,
) -> oxibrowser_core::network::InterceptAction {
    use oxibrowser_core::network::InterceptAction;
    InterceptAction::Continue {
        url,
        method,
        headers,
        post_data,
    }
}

/// Build an InterceptAction::Fail from CDP params.
fn build_fail(error_reason: String) -> oxibrowser_core::network::InterceptAction {
    use oxibrowser_core::network::InterceptAction;
    InterceptAction::Fail { error_reason }
}

/// Build an InterceptAction::Fulfill from CDP params.
fn build_fulfill(
    status_code: u16,
    status_text: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
) -> oxibrowser_core::network::InterceptAction {
    use oxibrowser_core::network::InterceptAction;
    InterceptAction::Fulfill {
        status_code,
        status_text,
        headers,
        body,
    }
}
