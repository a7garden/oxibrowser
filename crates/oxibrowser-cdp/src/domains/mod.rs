//! CDP domains — implementations of CDP domain methods.
//!
//! Mirrors Lightpanda's `src/cdp/domains/` structure.

pub mod browser;
pub mod dom;
pub mod network;
pub mod page;
pub mod runtime;
pub mod target;

use crate::protocol::CdpError;
use serde_json::Value;

/// Result of handling a CDP domain method.
pub type DomainResult = std::result::Result<Option<Value>, CdpError>;

/// Dispatch a CDP method to the appropriate domain handler.
///
/// Returns `Ok(Some(result))` on success, `Ok(None)` for empty results,
/// or `Err(CdpError)` for unknown methods.
pub fn dispatch(method: &str, params: Option<Value>) -> DomainResult {
    let parts: Vec<&str> = method.splitn(2, '.').collect();
    if parts.len() != 2 {
        return Err(CdpError {
            code: -32601,
            message: format!("invalid method: {method}"),
        });
    }

    let (domain, method_name) = (parts[0], parts[1]);

    match domain {
        "Browser" => browser::handle(method_name, params),
        "DOM" => dom::handle(method_name, params),
        "Network" => network::handle(method_name, params),
        "Page" => page::handle(method_name, params),
        "Runtime" => runtime::handle(method_name, params),
        "Target" => target::handle(method_name, params),
        _ => Err(CdpError {
            code: -32601,
            message: format!("unknown domain: {domain}"),
        }),
    }
}
