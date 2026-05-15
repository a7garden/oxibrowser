//! OXI domain — OxiBrowser AI agent extensions.
//!
//! Provides AI-agent-friendly methods beyond standard CDP.

use crate::domains::{DispatchContext, DomainResult};
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Handle OXI domain methods.
pub async fn handle(method: &str, _params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        "getMarkdown" => get_markdown(ctx).await,
        "getPageInfo" => get_page_info(ctx).await,
        _ => Err(CdpError {
            code: -32601,
            message: format!("unknown method: OXI.{}", method),
        }),
    }
}

async fn get_markdown(ctx: &DispatchContext) -> DomainResult {
    let guard = ctx.session.read().await;
    let markdown = guard
        .page()
        .map(|p| p.to_markdown())
        .unwrap_or_default();
    Ok(Some(json!({ "markdown": markdown })))
}

async fn get_page_info(ctx: &DispatchContext) -> DomainResult {
    let guard = ctx.session.read().await;
    let url = guard
        .current_url()
        .map(|u| u.to_string())
        .unwrap_or_default();
    let title = guard
        .page()
        .and_then(|p| p.title().map(|t| t.to_string()))
        .unwrap_or_default();
    Ok(Some(json!({
        "url": url,
        "title": title,
        "readyState": "complete"
    })))
}
