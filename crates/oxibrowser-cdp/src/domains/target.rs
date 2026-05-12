//! CDP Target domain handler.
//!
//! Handles Target.setDiscoverTargets, Target.setAutoAttach,
//! Target.attachToTarget, Target.createTarget, Target.closeTarget.

use crate::domains::DomainResult;
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Dispatch Target domain methods.
pub fn handle(method: &str, params: Option<Value>) -> DomainResult {
    match method {
        "setDiscoverTargets" => Ok(Some(json!({}))),
        "setAutoAttach" => set_auto_attach(params),
        "attachToTarget" => attach_to_target(params),
        "detachFromTarget" => Ok(Some(json!({}))),
        "createTarget" => create_target(params),
        "closeTarget" => Ok(Some(json!({ "success": true }))),
        "getTargets" => get_targets(),
        "getTargetInfo" => get_target_info(params),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Target.{} not implemented", method),
        }),
    }
}

/// Target.setAutoAttach — enables auto-attaching to new targets.
fn set_auto_attach(_params: Option<Value>) -> DomainResult {
    Ok(Some(json!({})))
}

/// Target.attachToTarget — attaches to a target.
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

/// Target.createTarget — creates a new page target.
fn create_target(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _url = params
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("about:blank");

    Ok(Some(json!({
        "targetId": format!("TID-{}", uuid::Uuid::new_v4().as_simple())
    })))
}

/// Target.getTargets — returns list of available targets.
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

/// Target.getTargetInfo — returns info about a specific target.
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
            "title": "OxiBrowser",
            "url": "about:blank",
            "attached": false,
            "canAccessOpener": false,
            "browserContextId": "default"
        }
    })))
}
