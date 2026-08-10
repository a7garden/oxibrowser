//! CDP Target domain handler.
//!
//! Handles Target.setDiscoverTargets, Target.setAutoAttach,
//! Target.attachToTarget, Target.createTarget, Target.closeTarget.
//!
//! After setDiscoverTargets(true), emits Target.targetCreated for the
//! current target. After setAutoAttach(true), emits Target.attachedToTarget.

use crate::domains::{DispatchContext, DomainResult};
use crate::protocol::CdpError;
use serde_json::{Value, json};

/// Dispatch Target domain methods.
pub async fn handle(method: &str, params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        "setDiscoverTargets" => set_discover_targets(params, ctx),
        "setAutoAttach" => set_auto_attach(params, ctx),
        "attachToTarget" => attach_to_target(params, ctx),
        "detachFromTarget" => Ok(Some(json!({}))),
        "createTarget" => create_target(params, ctx).await,
        "closeTarget" => Ok(Some(json!({ "success": true }))),
        "getTargets" => get_targets(),
        "getTargetInfo" => get_target_info(params),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Target.{} not implemented", method),
        }),
    }
}

/// Target.setDiscoverTargets — enables target discovery.
///
/// When enabled, emits Target.targetCreated for the current target
/// so that Puppeteer/Playwright can discover it.
fn set_discover_targets(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let discover = params
        .get("discover")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    if discover {
        // Emit targetCreated for the default page target
        ctx.events.send_event(
            "Target.targetCreated",
            json!({
                "targetInfo": {
                    "targetId": "default",
                    "type": "page",
                    "title": "OxiBrowser",
                    "url": "about:blank",
                    "attached": false,
                    "canAccessOpener": false,
                    "browserContextId": "default"
                }
            }),
        );
    }

    Ok(Some(json!({})))
}

/// Target.setAutoAttach — enables auto-attaching to new targets.
///
/// When enabled, emits Target.attachedToTarget for the current session
/// so that Puppeteer/Playwright can begin interacting.
fn set_auto_attach(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let auto_attach = params
        .get("autoAttach")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let _flatten = params
        .get("flatten")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    if auto_attach {
        let session_id = format!("session-{}", uuid::Uuid::new_v4().as_simple());
        let attached_session_id = session_id.clone();

        // Emit attachedToTarget for the default target
        ctx.events.send_event(
            "Target.attachedToTarget",
            json!({
                "sessionId": session_id,
                "targetInfo": {
                    "targetId": "default",
                    "type": "page",
                    "title": "OxiBrowser",
                    "url": "about:blank",
                    "attached": true,
                    "canAccessOpener": false,
                    "browserContextId": "default"
                },
                "waitingForDebugger": false
            }),
        );
        // Stamp subsequent target events with this sessionId (flat protocol).
        ctx.events.set_session_id(attached_session_id);
    }

    Ok(Some(json!({})))
}

/// Target.attachToTarget — attaches to a target.
fn attach_to_target(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let _target_id = params
        .get("targetId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let session_id = format!("session-{}", uuid::Uuid::new_v4().as_simple());
    // Subsequent commands arrive with this sessionId; stamp events with it.
    ctx.events.set_session_id(session_id.clone());
    Ok(Some(json!({ "sessionId": session_id })))
}

/// Target.createTarget — creates a new page target (a real Browser session).
///
/// The new session is registered under a fresh `sessionId` so flat-protocol
/// commands routed by that `sessionId` reach it. Emits `Target.targetCreated`
/// and `Target.attachedToTarget`.
///
/// Child-target lifecycle events (load, etc.) currently do not flow (each child
/// needs its own CoreEvent drainer); the command surface (navigate/evaluate/DOM)
/// works.
async fn create_target(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let url = params
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("about:blank");

    let new_session = ctx.browser.new_session().await.map_err(|e| CdpError {
        code: -32000,
        message: format!("failed to create new session: {e}"),
    })?;

    let target_id = format!("TID-{}", uuid::Uuid::new_v4().as_simple());
    let session_id = format!("session-{}", uuid::Uuid::new_v4().as_simple());

    // Wire the child session's CoreEvent sink so its JS-thread events
    // (console, exceptions, fetch/WS lifecycle) flow to the client stamped
    // with this child's sessionId.
    let (core_tx, core_rx) = std::sync::mpsc::channel::<oxibrowser_core::js::CoreEvent>();
    {
        let mut s = new_session.write().await;
        s.set_event_sink(core_tx);
    }
    let child_events = ctx.events.clone();
    let child_sid = session_id.clone();
    tokio::spawn(async move {
        loop {
            match core_rx.try_recv() {
                Ok(ev) => {
                    crate::core_event::emit_core_event_with_session(&child_events, ev, &child_sid)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }
    });

    // Register the child session so commands routed by sessionId reach it.
    ctx.child_targets
        .write()
        .await
        .insert(session_id.clone(), new_session.clone());

    ctx.events.send_event(
        "Target.targetCreated",
        json!({
            "targetInfo": {
                "targetId": target_id,
                "type": "page",
                "title": "about:blank",
                "url": url,
                "attached": false,
                "canAccessOpener": false,
                "browserContextId": "default"
            }
        }),
    );
    ctx.events.send_event(
        "Target.attachedToTarget",
        json!({
            "sessionId": session_id,
            "targetInfo": {
                "targetId": target_id,
                "type": "page",
                "title": "about:blank",
                "url": url,
                "attached": true,
                "canAccessOpener": false,
                "browserContextId": "default"
            },
            "waitingForDebugger": false
        }),
    );

    Ok(Some(json!({
        "targetId": target_id
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
