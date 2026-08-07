//! CDP DOM domain handler.
//!
//! Handles DOM.getDocument, DOM.querySelector, DOM.querySelectorAll,
//! DOM.getOuterHTML, DOM.describeNode, DOM.resolveNode.
//!
//! All reads go through the live (post-JS) [`DomSnapshot`] serialized from the
//! `RenderDocument` on the JS thread (`Session::dom_snapshot`), so DOM reads
//! reflect JS-driven mutations rather than a stale navigate-time copy.

use crate::domains::{DispatchContext, DomainResult};
use crate::protocol::CdpError;
use oxibrowser_core::js::dom_snapshot::DomSnapshot;
use serde_json::{Value, json};

/// Dispatch DOM domain methods.
pub async fn handle(method: &str, params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    match method {
        "getDocument" => get_document(ctx).await,
        "querySelector" => query_selector(params, ctx).await,
        "querySelectorAll" => query_selector_all(params, ctx).await,
        "getOuterHTML" => get_outer_html(ctx).await,
        "describeNode" => describe_node(params, ctx).await,
        "resolveNode" => resolve_node(params),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Method not found: DOM.{method}"),
        }),
    }
}

/// DOM.getDocument — returns the root DOM node from the live document.
async fn get_document(ctx: &DispatchContext) -> DomainResult {
    let mut guard = ctx.session.write().await;
    let snap = guard.dom_snapshot().await?;
    match snap {
        Some(s) => Ok(Some(json!({ "root": build_cdp_node(&s, s.root_id, 0) }))),
        None => Ok(Some(json!({
            "root": {
                "nodeId": 0,
                "backendNodeId": 0,
                "nodeType": 9,
                "nodeName": "#document",
                "localName": "",
                "nodeValue": "",
                "childNodeCount": 0
            }
        }))),
    }
}

/// DOM.querySelector — finds a single node matching a CSS selector.
async fn query_selector(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let selector = params
        .get("selector")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut guard = ctx.session.write().await;
    let snap = guard.dom_snapshot().await?;
    let node_id = snap
        .as_ref()
        .and_then(|s| s.query_selector(selector))
        .unwrap_or(0);
    Ok(Some(json!({ "nodeId": node_id })))
}

/// DOM.querySelectorAll — finds all nodes matching a CSS selector.
async fn query_selector_all(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let selector = params
        .get("selector")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut guard = ctx.session.write().await;
    let snap = guard.dom_snapshot().await?;
    let node_ids: Vec<u64> = snap
        .as_ref()
        .map(|s| {
            s.query_selector_all(selector)
                .iter()
                .map(|id| *id as u64)
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(json!({ "nodeIds": node_ids })))
}

/// DOM.getOuterHTML — returns the page's HTML.
async fn get_outer_html(ctx: &DispatchContext) -> DomainResult {
    let guard = ctx.session.read().await;
    let html = match guard.page() {
        Some(page) => page.content().to_string(),
        None => "<html><head></head><body></body></html>".to_string(),
    };
    Ok(Some(json!({ "outerHTML": html })))
}

/// DOM.describeNode — describes a DOM node with real data from the live tree.
async fn describe_node(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let node_id = params
        .get("nodeId")
        .and_then(|v| v.as_u64())
        .or_else(|| params.get("backendNodeId").and_then(|v| v.as_u64()))
        .unwrap_or(0) as u32;

    let mut guard = ctx.session.write().await;
    let snap = guard.dom_snapshot().await?;
    let Some(snap) = snap else {
        return Err(CdpError {
            code: -32000,
            message: "No active page".to_string(),
        });
    };
    let Some(node) = snap.nodes.get(&node_id) else {
        return Err(CdpError {
            code: -32000,
            message: format!("Node not found: {node_id}"),
        });
    };

    let (node_type_num, node_name, local_name, node_value) = match node.node_type {
        9 => (9, "#document".to_string(), String::new(), String::new()),
        1 => (1, node.tag.to_uppercase(), node.tag.clone(), String::new()),
        3 => (
            3,
            "#text".to_string(),
            String::new(),
            node.text_content.clone(),
        ),
        8 => (
            8,
            "#comment".to_string(),
            String::new(),
            node.text_content.clone(),
        ),
        _ => (1, node.tag.to_uppercase(), node.tag.clone(), String::new()),
    };

    let child_count = node.children.len();
    let attributes: Vec<Value> = if node.node_type == 1 {
        node.attributes
            .iter()
            .flat_map(|(k, v)| [json!(k), json!(v)])
            .collect()
    } else {
        Vec::new()
    };

    Ok(Some(json!({
        "node": {
            "nodeId": node_id,
            "backendNodeId": node_id,
            "nodeType": node_type_num,
            "nodeName": node_name,
            "localName": local_name,
            "nodeValue": node_value,
            "childNodeCount": child_count,
            "attributes": attributes,
        }
    })))
}

/// DOM.resolveNode — resolves a DOM node to a JS remote object.
///
/// Uses deterministic objectId format "oxi-node-{nodeId}" so that
/// Runtime.callFunctionOn can look up the node by its objectId.
fn resolve_node(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0);
    let object_id = format!("oxi-node-{node_id}");

    Ok(Some(json!({
        "object": {
            "type": "object",
            "subtype": "node",
            "className": "HTMLElement",
            "description": format!("node#{node_id}"),
            "objectId": object_id
        }
    })))
}

// ---------------------------------------------------------------------------
// CDP node tree builder
// ---------------------------------------------------------------------------

/// Maximum depth for the CDP node tree (avoids huge outputs).
const MAX_CDP_TREE_DEPTH: usize = 10;

/// Build a CDP-compatible JSON node from the live [`DomSnapshot`] tree.
fn build_cdp_node(snapshot: &DomSnapshot, node_id: u32, depth: usize) -> Value {
    let Some(node) = snapshot.nodes.get(&node_id) else {
        return json!({});
    };

    let (node_type_num, node_name, local_name, node_value) = match node.node_type {
        9 => (9, "#document".to_string(), String::new(), String::new()),
        1 => (1, node.tag.to_uppercase(), node.tag.clone(), String::new()),
        3 => (
            3,
            "#text".to_string(),
            String::new(),
            node.text_content.clone(),
        ),
        8 => (
            8,
            "#comment".to_string(),
            String::new(),
            node.text_content.clone(),
        ),
        _ => (1, node.tag.to_uppercase(), node.tag.clone(), String::new()),
    };

    // Attribute pairs [name1, value1, name2, value2, ...]
    let attributes: Vec<Value> = if node.node_type == 1 {
        node.attributes
            .iter()
            .flat_map(|(k, v)| [json!(k), json!(v)])
            .collect()
    } else {
        Vec::new()
    };

    let children: Vec<Value> = if depth < MAX_CDP_TREE_DEPTH {
        node.children
            .iter()
            .filter(|&&child_id| {
                if let Some(child_node) = snapshot.nodes.get(&child_id)
                    && child_node.node_type == 3
                {
                    return !child_node.text_content.trim().is_empty();
                }
                true
            })
            .map(|&child_id| build_cdp_node(snapshot, child_id, depth + 1))
            .collect()
    } else {
        Vec::new()
    };

    json!({
        "nodeId": node_id,
        "backendNodeId": node_id,
        "nodeType": node_type_num,
        "nodeName": node_name,
        "localName": local_name,
        "nodeValue": node_value,
        "childNodeCount": children.len(),
        "children": children,
        "attributes": attributes,
    })
}
