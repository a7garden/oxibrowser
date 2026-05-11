//! CDP Target domain handler.
//!
//! Handles Target.setAutoAttach, Target.attachToTarget,
//! Target.detachFromTarget, Target.createTarget, Target.closeTarget,
//! Target.getTargetInfo.

use crate::domains::DomainResult;
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Dispatch Target domain methods.
pub fn handle(method: &str, params: Option<Value>) -> DomainResult {
    match method {
        "setAutoAttach" => set_auto_attach(params),
        "attachToTarget" => attach_to_target(params),
        "detachFromTarget" => detach_from_target(params),
        "createTarget" => create_target(params),
        "closeTarget" => close_target(params),
        "getTargetInfo" => get_target_info(params),
        "getTargets" => get_targets(),
        "setDiscoverTargets" => set_discover_targets(params),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Target.{} not implemented", method),
        }),
    }
}

/// Target.setAutoAttach — auto-attach to new targets.
fn set_auto_attach(_params: Option<Value>) -> DomainResult {
    Ok(Some(json!({})))
}

/// Target.attachToTarget — attach to a target by ID.
fn attach_to_target(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _target_id = params
        .get("targetId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    Ok(Some(json!({
        "sessionId": format!("session-{}", uuid::Uuid::new_v4())
    })))
}

/// Target.detachFromTarget — detach from a target.
fn detach_from_target(_params: Option<Value>) -> DomainResult {
    Ok(Some(json!({})))
}

/// Target.createTarget — creates a new page target.
fn create_target(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _url = params
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("about:blank");

    let target_id = format!("target-{}", uuid::Uuid::new_v4());

    Ok(Some(json!({
        "targetId": target_id
    })))
}

/// Target.closeTarget — closes a target.
fn close_target(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _target_id = params
        .get("targetId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    Ok(Some(json!({
        "success": true
    })))
}

/// Target.getTargetInfo — returns information about a target.
fn get_target_info(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let target_id = params
        .get("targetId")
        .and_then(|v| v.as_str())
        .unwrap_or("default");

    Ok(Some(json!({
        "targetInfo": {
            "targetId": target_id,
            "type": "page",
            "title": "",
            "url": "about:blank",
            "attached": true,
            "canAccessOpener": false,
            "browserContextId": "default"
        }
    })))
}

/// Target.getTargets — returns all available targets.
fn get_targets() -> DomainResult {
    Ok(Some(json!({
        "targetInfos": [
            {
                "targetId": "default",
                "type": "page",
                "title": "OxiBrowser",
                "url": "about:blank",
                "attached": false,
                "canAccessOpener": false,
                "browserContextId": "default"
            }
        ]
    })))
}

/// Target.setDiscoverTargets — enables target discovery.
fn set_discover_targets(_params: Option<Value>) -> DomainResult {
    Ok(Some(json!({})))
}
