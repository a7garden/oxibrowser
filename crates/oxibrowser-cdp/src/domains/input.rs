//! CDP Input domain handler.
//!
//! Handles keyboard and mouse input simulation for testing and automation.
//! - Input.dispatchKeyEvent — keyboard events (keydown, keyup, rawKeyDown, char)
//! - Input.dispatchMouseEvent — mouse events (mousePressed, mouseReleased, mouseMoved)
//!
//! Note: These are simulation-only. Real keyboard/mouse events require a
//! rendering engine (Servo). Here we mainly handle the CDP protocol correctly
//! so Puppeteer/Playwright don't error out.

use crate::domains::{DispatchContext, DomainResult};
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Dispatch Input domain methods.
pub async fn handle(method: &str, params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        "dispatchKeyEvent" => dispatch_key_event(params, ctx).await,
        "dispatchMouseEvent" => dispatch_mouse_event(params, ctx).await,
        "dispatchDragEvent" => dispatch_drag_event(params, ctx).await,
        "insertText" => insert_text(params, ctx).await,
        "imeSetComposition" => ime_set_composition(params, ctx).await,
        "synthesizePinchGesture" => Ok(Some(json!({}))),
        "synthesizeScrollGesture" => Ok(Some(json!({}))),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Input.{} not implemented", method),
        }),
    }
}

// ---------------------------------------------------------------------------
// Keyboard events
// ---------------------------------------------------------------------------

/// Input.dispatchKeyEvent — simulate keyboard events.
///
/// Params:
/// - type: "keyDown" | "keyUp" | "rawKeyDown" | "char"
/// - key: string (e.g., "a", "Enter", "F12")
/// - code: string (e.g., "KeyA", "Enter")
/// - windowsVirtualKeyCode: number
/// - nativeVirtualKeyCode: number
/// - autoRepeat: boolean
/// - isKeypad: boolean
/// - isLeft: boolean (left modifier)
/// - isRight: boolean (right modifier)
/// - location: 0=standard, 1=left, 2=right, 3=numpad
async fn dispatch_key_event(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "dispatchKeyEvent requires parameters".to_string(),
    })?;

    let event_type = p.get("type").and_then(|v| v.as_str()).unwrap_or("keyDown");
    let key = p.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let code = p.get("code").and_then(|v| v.as_str()).unwrap_or("");
    let modifiers = calculate_modifiers(&p);

    tracing::debug!(
        "Input.dispatchKeyEvent: type={}, key={}, code={}, modifiers={}",
        event_type, key, code, modifiers
    );

    // Emit input event for debugging/inspection
    ctx.events.send_page_event(
        "Input.dispatchKeyEvent",
        json!({
            "type": event_type,
            "key": key,
            "code": code,
            "modifiers": modifiers,
        }),
    );

    // TODO: Forward to rendering engine (Servo) when integrated
    // For now: accept the event without actual keypress

    let keydown_time = p.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
    Ok(Some(json!({
        "timestamp": keydown_time,
    })))
}

/// Input.insertText — insert text as if typed (IME composition).
async fn insert_text(params: Option<Value>, _ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "insertText requires parameters".to_string(),
    })?;

    let text = p.get("text").and_then(|v| v.as_str()).unwrap_or("");
    tracing::debug!("Input.insertText: text={}", text);

    Ok(Some(json!({})))
}

/// Input.imeSetComposition — set IME composition.
async fn ime_set_composition(params: Option<Value>, _ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "imeSetComposition requires parameters".to_string(),
    })?;

    let selections = p.get("segments").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
    tracing::debug!("Input.imeSetComposition: {} segments", selections);

    Ok(Some(json!({})))
}

// ---------------------------------------------------------------------------
// Mouse events
// ---------------------------------------------------------------------------

/// Input.dispatchMouseEvent — simulate mouse events.
///
/// Params:
/// - type: "mousePressed" | "mouseReleased" | "mouseMoved"
/// - x: number (viewport-relative)
/// - y: number (viewport-relative)
/// - button: "left" | "right" | "middle" | "none"
/// - clickCount: number
/// - modifiers: number
async fn dispatch_mouse_event(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "dispatchMouseEvent requires parameters".to_string(),
    })?;

    let event_type = p.get("type").and_then(|v| v.as_str()).unwrap_or("mouseMoved");
    let x = p.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = p.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let button = p.get("button").and_then(|v| v.as_str()).unwrap_or("none");
    let click_count = p.get("clickCount").and_then(|v| v.as_i64()).unwrap_or(0);
    let modifiers = p.get("modifiers").and_then(|v| v.as_i64()).unwrap_or(0) as u32;

    tracing::debug!(
        "Input.dispatchMouseEvent: type={}, x={}, y={}, button={}, clicks={}",
        event_type, x, y, button, click_count
    );

    // Emit mouse event for inspection
    ctx.events.send_page_event(
        "Input.dispatchMouseEvent",
        json!({
            "type": event_type,
            "x": x,
            "y": y,
            "button": button,
            "clickCount": click_count,
            "modifiers": modifiers,
        }),
    );

    // TODO: Forward to rendering engine (Servo) when integrated

    Ok(Some(json!({})))
}

/// Input.dispatchDragEvent — simulate drag events.
async fn dispatch_drag_event(params: Option<Value>, _ctx: &DispatchContext) -> DomainResult {
    let p = params.ok_or_else(|| CdpError {
        code: -32602,
        message: "dispatchDragEvent requires parameters".to_string(),
    })?;

    let event_type = p.get("type").and_then(|v| v.as_str()).unwrap_or("dragOver");
    let x = p.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = p.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);

    tracing::debug!("Input.dispatchDragEvent: type={}, x={}, y={}", event_type, x, y);

    Ok(Some(json!({})))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Calculate modifier flags from CDP key event params.
/// CDP modifiers: 0=none, 1=click, 2=alt, 4=ctrl, 8=shift, 16=meta
fn calculate_modifiers(params: &serde_json::Value) -> u32 {
    let mut m = 0u32;
    if params.get("modifiers").and_then(|v| v.as_i64()).unwrap_or(0) > 0 {
        m |= 1; // click
    }
    if params.get("shiftKey").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= 8;
    }
    if params.get("ctrlKey").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= 4;
    }
    if params.get("altKey").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= 2;
    }
    if params.get("metaKey").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= 16;
    }
    m
}