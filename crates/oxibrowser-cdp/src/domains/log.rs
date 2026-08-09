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
pub async fn handle(method: &str, _params: Option<Value>, _ctx: &DispatchContext) -> DomainResult {
    match method {
        // Acknowledge — no state to track until entryAdded is wired.
        "enable" | "disable" | "clear" | "startViolationsReport" | "stopViolationsReport" => {
            Ok(Some(json!({})))
        }
        _ => Err(CdpError {
            code: -32601,
            message: format!("Log.{method} not implemented"),
        }),
    }
}
