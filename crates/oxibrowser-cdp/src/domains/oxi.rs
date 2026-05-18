//! OXI domain — OxiBrowser AI agent extensions.
//!
//! Provides AI-agent-friendly methods beyond standard CDP:
//! - `OXI.getMarkdown` — page content as Markdown
//! - `OXI.getPageInfo` — URL, title, status
//! - `OXI.getStructuredPage` — headings, links, meta as structured JSON
//! - `OXI.getAccessibilityTree` — semantic tree of what's on the page
//! - `OXI.getBoxModelScreenshot` — PNG with colored boxes for each element

use crate::domains::{DispatchContext, DomainResult};
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Handle OXI domain methods.
pub async fn handle(method: &str, params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        "getMarkdown" => get_markdown(ctx).await,
        "getPageInfo" => get_page_info(ctx).await,
        "getStructuredPage" => get_structured_page(params, ctx).await,
        "getAccessibilityTree" => get_accessibility_tree(ctx).await,
        "getBoxModelScreenshot" => get_box_model_screenshot(params, ctx).await,
        _ => Err(CdpError {
            code: -32601,
            message: format!("unknown method: OXI.{}", method),
        }),
    }
}

async fn get_markdown(ctx: &DispatchContext) -> DomainResult {
    let guard = ctx.session.read().await;
    let markdown = guard.page().map(|p| p.to_markdown()).unwrap_or_default();
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
    let status = guard.page().map(|p| p.status()).unwrap_or(0);
    Ok(Some(json!({
        "url": url,
        "title": title,
        "status": status,
        "readyState": "complete"
    })))
}

/// OXI.getStructuredPage — return structured page data.
///
/// Returns headings, links, meta tags, and basic page info as JSON.
/// This is optimized for AI agent consumption.
///
/// Optional params:
/// - `maxLinks` (number): limit number of links returned (default: 200)
async fn get_structured_page(_params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let guard = ctx.session.read().await;

    let url = guard
        .current_url()
        .map(|u| u.to_string())
        .unwrap_or_default();

    let title = guard
        .page()
        .and_then(|p| p.title().map(|t| t.to_string()))
        .unwrap_or_default();

    let max_links = _params
        .as_ref()
        .and_then(|p| p.get("maxLinks"))
        .and_then(|v| v.as_u64())
        .unwrap_or(200) as usize;

    // Build a DomSnapshot from the current frame
    let snapshot = guard
        .page()
        .map(|p| oxibrowser_core::js::dom_snapshot::DomSnapshot::from_frame(p.root_frame()));

    let (headings, links, meta) = match snapshot {
        Some(s) => {
            let headings: Vec<Value> = s
                .headings()
                .into_iter()
                .map(|(level, text)| {
                    json!({
                        "level": level,
                        "text": text
                    })
                })
                .collect();

            let links: Vec<Value> = s
                .links()
                .into_iter()
                .take(max_links)
                .map(|(text, href)| {
                    json!({
                        "text": text,
                        "href": href
                    })
                })
                .collect();

            let meta: Value = s
                .meta_tags()
                .into_iter()
                .map(|(k, v)| (k, json!(v)))
                .collect();

            (headings, links, meta)
        }
        None => (vec![], vec![], json!({})),
    };

    Ok(Some(json!({
        "url": url,
        "title": title,
        "headings": headings,
        "links": links,
        "meta": meta,
        "linkCount": links.len(),
        "headingCount": headings.len(),
    })))
}

/// OXI.getAccessibilityTree — return semantic tree of page content.
///
/// Shows what a user (or screen reader) would perceive:
/// roles, labels, visibility, interactivity, approximate positions.
async fn get_accessibility_tree(ctx: &DispatchContext) -> DomainResult {
    let guard = ctx.session.read().await;
    let snapshot = guard
        .page()
        .map(|p| oxibrowser_core::js::dom_snapshot::DomSnapshot::from_frame(p.root_frame()));

    let tree = match snapshot {
        Some(s) => oxibrowser_core::css::render_accessibility_tree(&s),
        None => "(no page loaded)".into(),
    };

    Ok(Some(json!({ "tree": tree })))
}

/// OXI.getBoxModelScreenshot — PNG with colored boxes for each element.
///
/// Uses LayoutEngine to estimate positions and draws:
/// - Background-colored rectangles for each visible element
/// - Text content inside boxes
/// - Element borders
async fn get_box_model_screenshot(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let viewport_width = params
        .get("viewportWidth")
        .and_then(|v| v.as_u64())
        .unwrap_or(1280) as u32;

    let guard = ctx.session.read().await;
    let snapshot = guard
        .page()
        .map(|p| oxibrowser_core::js::dom_snapshot::DomSnapshot::from_frame(p.root_frame()));

    let png_bytes = match snapshot {
        Some(s) => oxibrowser_core::css::render_box_model_png(&s, viewport_width)
            .unwrap_or_default(),
        None => Vec::new(),
    };

    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

    Ok(Some(json!({
        "data": data,
        "metadata": {
            "pageScaleFactor": 1,
            "deviceWidth": viewport_width,
        }
    })))
}
