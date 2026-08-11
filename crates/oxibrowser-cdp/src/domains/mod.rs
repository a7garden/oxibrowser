//! CDP domains — implementations of CDP domain methods.
//!
//!
//! The `dispatch` function is async and receives a `DispatchContext` that
//! includes the browser `Session` for page interaction AND the `EventSender`
//! so domain handlers can emit CDP events.
pub mod browser;
pub mod dom;
pub mod emulation;
pub mod fetch;
pub mod input;
pub mod log;
pub mod network;
pub mod oxi;
pub mod page;
pub mod runtime;
pub mod target;
pub mod tracing;

use crate::event::EventSender;
use crate::protocol::CdpError;
use oxibrowser_core::network::SharedRegistry;
use oxibrowser_core::session::Session;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Map of attached child-target sessionId → its Browser Session (multi-tab).
/// Populated by `Target.createTarget`; the dispatcher resolves the session for
/// an incoming command from its `sessionId`.
pub type ChildTargets = Arc<RwLock<HashMap<String, Arc<RwLock<Session>>>>>;

/// Context passed to all domain handlers.
///
/// Combines browser session access with the event sender, so handlers can
/// both read/write page data AND emit CDP events.
pub struct DispatchContext {
    /// Browser session (read/write for navigation, DOM access, JS eval).
    pub session: Arc<RwLock<Session>>,
    /// Event sender for emitting CDP events to the client.
    pub events: EventSender,
    /// Registry of paused requests for Fetch domain interception.
    pub fetch_registry: SharedRegistry,
    /// Shared dialog-resolution gate (for `Page.handleJavaScriptDialog`).
    /// Accessible without the session lock so dialogs resolve while a blocking
    /// `evaluate` holds the session write lock.
    pub dialog_gate: oxibrowser_core::js::DialogGate,
    /// Shared browser instance (for `Target.createTarget` to mint sessions).
    pub browser: Arc<oxibrowser_core::Browser>,
    /// Attached child targets (multi-tab), keyed by sessionId.
    pub child_targets: ChildTargets,
}

/// Result of handling a CDP domain method.
pub type DomainResult = std::result::Result<Option<Value>, CdpError>;

/// Dispatch a CDP method to the appropriate domain handler.
///
/// Returns `Ok(Some(result))` on success, `Ok(None)` for empty results,
/// or `Err(CdpError)` for unknown methods.
pub async fn dispatch(method: &str, params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
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
        "DOM" => dom::handle(method_name, params, ctx).await,
        "Emulation" => emulation::handle(method_name, params),
        "Fetch" => fetch::handle(method_name, params, ctx).await,
        "Input" => input::handle(method_name, params, ctx).await,
        "Network" => network::handle(method_name, params, ctx).await,
        "OXI" => oxi::handle(method_name, params, ctx).await,
        "Log" => log::handle(method_name, params, ctx).await,
        "Page" => page::handle(method_name, params, ctx).await,
        "Runtime" => runtime::handle(method_name, params, ctx).await,
        "Target" => target::handle(method_name, params, ctx).await,
        "Tracing" => tracing::handle(method_name, params, ctx),
        _ => Err(CdpError {
            code: -32601,
            message: format!("unknown domain: {domain}"),
        }),
    }
}
