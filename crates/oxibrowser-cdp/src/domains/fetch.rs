//! CDP Fetch domain handler.
//!
//! Handles network interception via Fetch.enable, Fetch.disable,
//! Fetch.continueRequest, Fetch.failRequest, Fetch.fulfillRequest, Fetch.continueResponse.
//!
//! When enabled, outgoing HTTP requests are matched against patterns
//! and a `Fetch.requestPaused` event is emitted for each matching request.
//! The client responds with continue/fail/fulfill.
//!
//! Architecture:
//! - Patterns stored in EventSender (globally, for all CDP sessions)
//! - emit_request_paused() called from network layer
//! - continue/fail/fulfill tracked via request registry

use crate::domains::{DispatchContext, DomainResult};
use crate::event::EventSender;
use crate::protocol::CdpError;
use serde_json::{json, Value};

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
        "getResponseBody" => get_response_body(params).await,
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
fn enable(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let mut patterns = vec![FetchPattern::default()]; // Match all by default

    if let Some(p) = params {
        if let Some(arr) = p.get("patterns").and_then(|v| v.as_array()) {
            patterns.clear();
            for item in arr {
                if let Some(p) = parse_fetch_pattern(item) {
                    patterns.push(p);
                }
            }
        }
    }

    ctx.events.set_fetch_enabled(true);
    ctx.events.set_fetch_patterns(patterns.clone());
    tracing::info!("Fetch domain enabled with {} pattern(s)", patterns.len());
    Ok(Some(json!({})))
}

/// Fetch.disable — disables request interception.
fn disable(ctx: &DispatchContext) -> DomainResult {
    ctx.events.set_fetch_enabled(false);
    ctx.events.set_fetch_patterns(vec![]);
    tracing::info!("Fetch domain disabled");
    Ok(Some(json!({})))
}

// ---------------------------------------------------------------------------
// Request interception actions
// ---------------------------------------------------------------------------

/// Fetch.continueRequest — resume a paused request with modifications.
async fn continue_request(params: Option<Value>, _ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "continueRequest requires parameters".to_string(),
    })?;

    let request_id = p.get("requestId").and_then(|v| v.as_str()).unwrap_or("");
    tracing::debug!("Fetch.continueRequest for requestId={}", request_id);
    // TODO: Look up request in registry, modify headers/url, resume
    Ok(Some(json!({})))
}

/// Fetch.failRequest — fail a paused request with an error.
async fn fail_request(params: Option<Value>, _ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "failRequest requires parameters".to_string(),
    })?;

    let request_id = p.get("requestId").and_then(|v| v.as_str()).unwrap_or("");
    let error_reason = p.get("errorReason").and_then(|v| v.as_str()).unwrap_or("Failed");
    tracing::debug!("Fetch.failRequest for requestId={}, reason={}", request_id, error_reason);
    Ok(Some(json!({})))
}

/// Fetch.fulfillRequest — return a fake response for a paused request.
async fn fulfill_request(params: Option<Value>, _ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "fulfillRequest requires parameters".to_string(),
    })?;

    let request_id = p.get("requestId").and_then(|v| v.as_str()).unwrap_or("");
    let status_code = p.get("statusCode").and_then(|v| v.as_i64()).unwrap_or(200) as u16;
    let status_text = p.get("statusText").and_then(|v| v.as_str()).unwrap_or("OK");
    let body = p.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let base64 = p.get("base64Encoded").and_then(|v| v.as_bool()).unwrap_or(false);

    // Extract response headers
    let mut headers = serde_json::Map::new();
    if let Some(h) = p.get("responseHeaders").and_then(|v| v.as_array()) {
        for item in h {
            if let (Some(k), Some(v)) = (item.get("name").and_then(|x| x.as_str()),
                                          item.get("value").and_then(|x| x.as_str())) {
                headers.insert(k.to_string(), json!(v));
            }
        }
    }

    let _content_type = headers.get("Content-Type")
        .and_then(|v| v.as_str())
        .unwrap_or("text/html");
    let body_size = body.len();

    tracing::debug!(
        "Fetch.fulfillRequest for requestId={}, status={}, body_size={}",
        request_id, status_code, body_size
    );

    Ok(Some(json!({
        "responseCode": status_code,
        "responsePhrase": status_text,
        "responseHeaders": headers,
        "binary": base64,
    })))
}

/// Fetch.continueResponse — continue a paused request with a modified response.
async fn continue_response(params: Option<Value>, _ctx: &DispatchContext) -> DomainResult {
    let _p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "continueResponse requires parameters".to_string(),
    })?;
    Ok(Some(json!({})))
}

/// Fetch.getResponseBody — returns body for an intercepted request.
async fn get_response_body(_params: Option<Value>) -> DomainResult {
    Ok(Some(json!({
        "body": "",
        "base64Encoded": false,
    })))
}

// ---------------------------------------------------------------------------
// Event emission (called from network layer during requests)
// ---------------------------------------------------------------------------

/// Emit a `Fetch.requestPaused` event for an intercepted request.
pub fn emit_request_paused(
    events: &EventSender,
    request_id: &str,
    url: &str,
    method: &str,
    headers: &[(String, String)],
    resource_type: &str,
) {
    let headers_json: serde_json::Map<String, serde_json::Value> = headers
        .iter()
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();

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
}

// ---------------------------------------------------------------------------
// Pattern matching
// ---------------------------------------------------------------------------

/// A request interception pattern.
#[derive(Debug, Clone, Default)]
pub struct FetchPattern {
    /// URL pattern (glob or regex pattern).
    pub url_pattern: String,
    /// Resource type filter (Document, Script, Image, XHR, etc.).
    pub resource_type: Option<String>,
    /// Request stage filter (Request, Response).
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
            let inner = &pattern[1..pattern.len() - 1];
            url.contains(inner)
        } else if pattern.ends_with('*') {
            let prefix = &pattern[..pattern.len() - 1];
            url.starts_with(prefix)
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
        url_pattern: obj.get("urlPattern")
            .and_then(|v| v.as_str())
            .unwrap_or("*")
            .to_string(),
        resource_type: obj.get("resourceType")
            .and_then(|v| v.as_str())
            .map(String::from),
        request_stage: obj.get("requestStage")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

/// Check if a request URL matches any enabled pattern.
pub fn matches_patterns(url: &str, patterns: &[FetchPattern]) -> bool {
    patterns.iter().any(|p| p.matches_url(url))
}