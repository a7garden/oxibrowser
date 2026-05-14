//! OXI domain — OxiBrowser AI agent extensions.
//!
//! Provides AI-agent-friendly methods beyond standard CDP.

use crate::domains::{DispatchContext, DomainResult};
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Handle OXI domain methods.
pub fn handle(method: &str, _params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        "getMarkdown" => get_markdown(ctx),
        "getPageInfo" => get_page_info(ctx),
        _ => Err(CdpError {
            code: -32601,
            message: format!("unknown method: OXI.{}", method),
        }),
    }
}

fn get_markdown(ctx: &DispatchContext) -> DomainResult {
    let markdown = ctx.session.blocking_read()
        .page()
        .map(|p| p.to_markdown())
        .unwrap_or_default();
    Ok(Some(json!({ "markdown": markdown })))
}

fn get_page_info(ctx: &DispatchContext) -> DomainResult {
    let session = ctx.session.blocking_read();
    let url = session.current_url()
        .map(|u| u.to_string())
        .unwrap_or_default();
    let title = session.page()
        .and_then(|p| p.title().map(|t| t.to_string()))
        .unwrap_or_default();
    Ok(Some(json!({
        "url": url,
        "title": title,
        "readyState": "complete"
    })))
}
