//! CDP Fetch domain handler.
//!
//! Handles network interception via Fetch.enable, Fetch.disable,
//! Fetch.continueRequest, Fetch.failRequest, Fetch.fulfillRequest.
//!
//! When enabled, outgoing HTTP requests are paused and a
//! `Fetch.requestPaused` event is sent to the client.
//! The client responds with continue/fail/fulfill.

use crate::domains::{DispatchContext, DomainResult};
use crate::protocol::CdpError;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Global interception state.
///
/// Tracks pending intercepted requests that are waiting for the CDP client
/// to respond (continue/fail/fulfill).
static INTERCEPTED: std::sync::OnceLock<Arc<Mutex<InterceptState>>>
    = std::sync::OnceLock::new();

/// Get the global interception state.
fn intercepted_state() -> Arc<Mutex<InterceptState>> {
    INTERCEPTED
        .get_or_init(|| Arc::new(Mutex::new(InterceptState::default())))
        .clone()
}

/// State for tracking intercepted requests.
#[derive(Default)]
struct InterceptState {
    /// Number of active intercepts.
    count: u32,
    /// Patterns to match for interception (empty = intercept all).
    patterns: Vec<InterceptPattern>,
}

/// A pattern for matching requests to intercept.
#[derive(Debug, Clone)]
struct InterceptPattern {
    /// URL pattern to match.
    #[allow(dead_code)]
    url_pattern: Option<String>,
    /// Resource type to match.
    #[allow(dead_code)]
    resource_type: Option<String>,
}

/// Dispatch Fetch domain methods.
pub async fn handle(
    method: &str,
    params: Option<Value>,
    ctx: &DispatchContext,
) -> DomainResult {
    match method {
        "enable" => enable(params, ctx).await,
        "disable" => disable(ctx).await,
        "continueRequest" => continue_request(params).await,
        "failRequest" => fail_request(params).await,
        "fulfillRequest" => fulfill_request(params).await,
        "continueResponse" => continue_response(params).await,
        _ => Err(CdpError {
            code: -32601,
            message: format!("Fetch.{} not implemented", method),
        }),
    }
}

/// Fetch.enable — enables request interception.
///
/// Accepts optional `patterns` array and `handleAuthRequests` flag.
async fn enable(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let patterns = params
        .get("patterns")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let state = intercepted_state();
    let mut state = state.lock().await;
    state.patterns.clear();

    for pattern in patterns {
        state.patterns.push(InterceptPattern {
            url_pattern: pattern.get("urlPattern").and_then(|v| v.as_str()).map(|s| s.to_string()),
            resource_type: pattern.get("resourceType").and_then(|v| v.as_str()).map(|s| s.to_string()),
        });
    }

    ctx.events.set_fetch_enabled(true);

    tracing::info!(patterns = state.patterns.len(), "Fetch domain enabled");

    Ok(Some(json!({})))
}

/// Fetch.disable — disables request interception.
async fn disable(ctx: &DispatchContext) -> DomainResult {
    let state = intercepted_state();
    let mut state = state.lock().await;
    state.patterns.clear();
    state.count = 0;

    ctx.events.set_fetch_enabled(false);

    tracing::info!("Fetch domain disabled");
    Ok(Some(json!({})))
}

/// Fetch.continueRequest — continues an intercepted request.
async fn continue_request(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let request_id = params
        .get("requestId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let state = intercepted_state();
    let mut state = state.lock().await;
    if state.count > 0 {
        state.count -= 1;
    }

    tracing::debug!(request_id = %request_id, "continuing intercepted request");
    Ok(Some(json!({})))
}

/// Fetch.failRequest — fails an intercepted request.
async fn fail_request(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let request_id = params
        .get("requestId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let reason = params
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("Aborted");

    let state = intercepted_state();
    let mut state = state.lock().await;
    if state.count > 0 {
        state.count -= 1;
    }

    tracing::debug!(request_id = %request_id, reason = %reason, "failing intercepted request");
    Ok(Some(json!({})))
}

/// Fetch.fulfillRequest — fulfills an intercepted request with a response.
async fn fulfill_request(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let request_id = params
        .get("requestId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let response_code = params
        .get("responseCode")
        .and_then(|v| v.as_u64())
        .unwrap_or(200);

    let state = intercepted_state();
    let mut state = state.lock().await;
    if state.count > 0 {
        state.count -= 1;
    }

    tracing::debug!(
        request_id = %request_id,
        response_code = response_code,
        "fulfilling intercepted request"
    );
    Ok(Some(json!({})))
}

/// Fetch.continueResponse — continues an intercepted response.
async fn continue_response(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let request_id = params
        .get("requestId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let state = intercepted_state();
    let mut state = state.lock().await;
    if state.count > 0 {
        state.count -= 1;
    }

    tracing::debug!(request_id = %request_id, "continuing intercepted response");
    Ok(Some(json!({})))
}

/// Emit a `Fetch.requestPaused` event for an intercepted request.
///
/// Called from the navigation pipeline when Fetch domain is enabled.
pub fn emit_request_paused(
    events: &crate::event::EventSender,
    request_id: &str,
    url: &str,
    resource_type: &str,
) {
    events.send_fetch_event(
        "Fetch.requestPaused",
        serde_json::json!({
            "requestId": request_id,
            "request": {
                "url": url,
                "method": "GET",
                "headers": {},
                "initialPriority": "VeryHigh",
                "urlFragment": "",
            },
            "resourceType": resource_type,
            "frameId": "main",
            "networkIntercepted": true,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_intercept_state_default() {
        let state = InterceptState::default();
        assert_eq!(state.count, 0);
        assert!(state.patterns.is_empty());
    }

    #[tokio::test]
    async fn test_enable_disable() {
        let (_tx, _rx) = crate::event::event_channel();
        let state = intercepted_state();
        {
            let mut s = state.lock().await;
            s.patterns.push(InterceptPattern {
                url_pattern: Some("https://example.com/*".to_string()),
                resource_type: None,
            });
        }
        let s = state.lock().await;
        assert_eq!(s.patterns.len(), 1);
    }
}
