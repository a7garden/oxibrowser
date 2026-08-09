//! CDP Log domain handler.
//!
//! Minimal surface: `Log.enable` / `Log.disable` (and a few related methods)
//! are acknowledged so Playwright/Puppeteer's `Log.enable` does not error.
//!
//! `Log.entryAdded` (and the equivalent `Runtime.consoleAPICalled`) require a
//! core→CDP event sink wired from the JS console/exception paths — that is the
//! remaining Phase 5 plumbing tracked separately.

use crate::domains::{DispatchContext, DomainResult};
use crate::protocol::CdpError;
use serde_json::{Value, json};

/// Dispatch Log domain methods.
pub async fn handle(method: &str, _params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        "enable" => {
            ctx.events.set_log_enabled(true);
            Ok(Some(json!({})))
        }
        "disable" => {
            ctx.events.set_log_enabled(false);
            Ok(Some(json!({})))
        }
        // Acknowledge — no state to track.
        "clear" | "startViolationsReport" | "stopViolationsReport" => Ok(Some(json!({}))),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Log.{method} not implemented"),
        }),
    }
}
